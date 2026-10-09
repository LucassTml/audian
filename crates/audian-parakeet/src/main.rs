//! Audian speech-to-text helper for NVIDIA Parakeet TDT 0.6B v3 (25 European languages).
//!
//! Runs the ONNX export of the model (istupakov/parakeet-tdt-0.6b-v3-onnx, int8) with ONNX
//! Runtime on the CPU:
//! 1. `nemo128.onnx` turns 16 kHz audio into NeMo's 128-bin log-mel features;
//! 2. the FastConformer encoder turns them into one vector per 80 ms of audio;
//! 3. greedy TDT decoding runs the prediction + joint network frame by frame. Unlike classic
//!    RNN-T, the joint network also predicts how many frames to skip, so silence and long
//!    sounds cost almost nothing.
//!
//! Same process model and protocol as the Whisper helper (`audian-stt`): framed JSON requests
//! on stdin, responses on stdout, exits after being idle. Long recordings are split into
//! ≤30 s chunks at the quietest point, which keeps encoder memory flat.
//!
//! Also supports `audian-parakeet --bench <model-dir> <wav> [threads]`.

#![windows_subsystem = "windows"]

use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use audian_common::SAMPLE_RATE;
use audian_common::ipc::{self, SttRequest, SttResponse};
use ort::session::Session;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::{Tensor, ValueType};

const PREPROCESSOR: &str = "nemo128.onnx";
const ENCODER: &str = "encoder-model.int8.onnx";
const DECODER: &str = "decoder_joint-model.int8.onnx";
const VOCAB: &str = "vocab.txt";

/// Longest piece of audio encoded at once.
const MAX_CHUNK_SECS: usize = 30;
/// Where to look for a quiet split point, before the chunk limit.
const SPLIT_SEARCH_SECS: usize = 6;
/// Very short clips are padded with silence to this length.
const MIN_SAMPLES: usize = SAMPLE_RATE as usize;
/// Safety limit on tokens emitted without advancing a frame.
const MAX_TOKENS_PER_FRAME: usize = 10;

struct Engine {
    model_dir: String,
    preprocessor: Session,
    encoder: Session,
    decoder: Session,
    vocab: Vec<String>,
    blank: usize,
    /// Shape of the prediction network's two LSTM state tensors, e.g. [2, 1, 640].
    state_shape: [usize; 3],
    threads: usize,
}

fn ort_err(e: impl std::fmt::Display) -> anyhow::Error {
    anyhow!("{e}")
}

/// One ONNX Runtime session. The CPU memory arena is off: it kept every intermediate buffer of
/// the largest run alive (~0.3 GB for a 15 s clip), for no measurable speed gain here.
fn session(path: &Path, threads: usize) -> anyhow::Result<Session> {
    Session::builder()
        .map_err(ort_err)?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(ort_err)?
        .with_intra_threads(threads)
        .map_err(ort_err)?
        .with_memory_pattern(false)
        .map_err(ort_err)?
        .with_execution_providers([ort::ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(ort_err)?
        .commit_from_file(path)
        .map_err(|e| anyhow!("cannot load {}: {e}", path.display()))
}

fn require_inputs(s: &Session, file: &str, names: &[&str]) -> anyhow::Result<()> {
    for name in names {
        if !s.inputs().iter().any(|i| i.name() == *name) {
            let found: Vec<&str> = s.inputs().iter().map(|i| i.name()).collect();
            bail!("{file}: unexpected model inputs {found:?} (expected {names:?})");
        }
    }
    Ok(())
}

fn argmax(v: &[f32]) -> usize {
    v.iter().enumerate().fold((0, f32::NEG_INFINITY), |best, (i, &x)| if x > best.1 { (i, x) } else { best }).0
}

impl Engine {
    fn load(model_dir: &str, threads: u32) -> anyhow::Result<Engine> {
        let dir = PathBuf::from(model_dir);
        for f in [PREPROCESSOR, ENCODER, DECODER, VOCAB] {
            if !dir.join(f).is_file() {
                bail!("model file not found: {}", dir.join(f).display());
            }
        }
        let threads = if threads == 0 { audian_common::default_threads() } else { threads } as usize;
        let preprocessor = session(&dir.join(PREPROCESSOR), 1)?;
        let encoder = session(&dir.join(ENCODER), threads)?;
        // The prediction/joint network is tiny and runs once per step: threads only add overhead.
        let decoder = session(&dir.join(DECODER), 1)?;
        require_inputs(&preprocessor, PREPROCESSOR, &["waveforms", "waveforms_lens"])?;
        require_inputs(&encoder, ENCODER, &["audio_signal", "length"])?;
        require_inputs(&decoder, DECODER, &["encoder_outputs", "targets", "target_length", "input_states_1", "input_states_2"])?;

        let state_shape = decoder
            .inputs()
            .iter()
            .find(|i| i.name() == "input_states_1")
            .and_then(|i| match i.dtype() {
                ValueType::Tensor { shape, .. } if shape.len() == 3 => {
                    Some([shape[0].max(1) as usize, shape[1].max(1) as usize, shape[2].max(1) as usize])
                }
                _ => None,
            })
            .unwrap_or([2, 1, 640]);

        let text = std::fs::read_to_string(dir.join(VOCAB)).context("reading vocab.txt")?;
        let mut vocab: Vec<String> = Vec::new();
        for line in text.lines() {
            let Some((token, id)) = line.rsplit_once(' ') else { continue };
            let Ok(id) = id.trim().parse::<usize>() else { continue };
            if vocab.len() <= id {
                vocab.resize(id + 1, String::new());
            }
            vocab[id] = token.to_string();
        }
        anyhow::ensure!(!vocab.is_empty(), "empty vocabulary");
        let blank = vocab.iter().position(|t| t == "<blk>" || t == "<blank>").unwrap_or(vocab.len() - 1);
        Ok(Engine { model_dir: model_dir.into(), preprocessor, encoder, decoder, vocab, blank, state_shape, threads })
    }

    /// Returns (text, detected language, no-speech probability).
    fn transcribe(&mut self, samples: &[f32], language: &str, candidates: &[String]) -> anyhow::Result<(String, String, f32)> {
        let mut text = String::new();
        for chunk in split(samples) {
            let tokens = if chunk.len() < MIN_SAMPLES {
                let mut padded = chunk.to_vec();
                padded.resize(MIN_SAMPLES, 0.0);
                self.decode_chunk(&padded)?
            } else {
                self.decode_chunk(chunk)?
            };
            let piece = self.detokenize(&tokens);
            if !piece.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&piece);
            }
        }
        let lang = match language.trim() {
            "" | "auto" => guess_language(&text, candidates),
            fixed => fixed.to_string(),
        };
        let no_speech = if text.is_empty() { 1.0 } else { 0.0 };
        Ok((text, lang, no_speech))
    }

    fn decode_chunk(&mut self, samples: &[f32]) -> anyhow::Result<Vec<usize>> {
        let n = samples.len();
        // 1. Features [1, 128, frames].
        let out = self
            .preprocessor
            .run(ort::inputs![
                "waveforms" => Tensor::from_array(([1usize, n], samples.to_vec())).map_err(ort_err)?,
                "waveforms_lens" => Tensor::from_array(([1usize], vec![n as i64])).map_err(ort_err)?,
            ])
            .map_err(ort_err)?;
        let (fshape, feats) = out["features"].try_extract_tensor::<f32>().map_err(ort_err)?;
        let fdims = [fshape[0] as usize, fshape[1] as usize, fshape[2] as usize];
        let feats = feats.to_vec();
        let flen = out["features_lens"].try_extract_tensor::<i64>().map_err(ort_err)?.1[0];
        drop(out);

        // 2. Encoder [1, dim, frames].
        let out = self
            .encoder
            .run(ort::inputs![
                "audio_signal" => Tensor::from_array((fdims, feats)).map_err(ort_err)?,
                "length" => Tensor::from_array(([1usize], vec![flen])).map_err(ort_err)?,
            ])
            .map_err(ort_err)?;
        let (eshape, enc) = out["outputs"].try_extract_tensor::<f32>().map_err(ort_err)?;
        let (dim, frames) = (eshape[1] as usize, eshape[2] as usize);
        let enc = enc.to_vec();
        let enc_len = out["encoded_lengths"].try_extract_tensor::<i64>().map_err(ort_err)?.1[0].max(0) as usize;
        drop(out);

        // 3. Greedy TDT decoding.
        let vocab_size = self.vocab.len();
        let state_len = self.state_shape.iter().product::<usize>();
        let (mut h, mut c) = (vec![0f32; state_len], vec![0f32; state_len]);
        let mut last = self.blank as i32;
        let mut tokens = Vec::new();
        let (mut t, mut emitted) = (0usize, 0usize);
        while t < enc_len.min(frames) {
            let frame: Vec<f32> = (0..dim).map(|d| enc[d * frames + t]).collect();
            let out = self
                .decoder
                .run(ort::inputs![
                    "encoder_outputs" => Tensor::from_array(([1usize, dim, 1], frame)).map_err(ort_err)?,
                    "targets" => Tensor::from_array(([1usize, 1], vec![last])).map_err(ort_err)?,
                    "target_length" => Tensor::from_array(([1usize], vec![1i32])).map_err(ort_err)?,
                    "input_states_1" => Tensor::from_array((self.state_shape, h.clone())).map_err(ort_err)?,
                    "input_states_2" => Tensor::from_array((self.state_shape, c.clone())).map_err(ort_err)?,
                ])
                .map_err(ort_err)?;
            let logits = out["outputs"].try_extract_tensor::<f32>().map_err(ort_err)?.1;
            anyhow::ensure!(logits.len() >= vocab_size, "unexpected joint output size {}", logits.len());
            let token = argmax(&logits[..vocab_size]);
            // The remaining outputs score the durations 0..=4 frames.
            let step = if logits.len() > vocab_size { argmax(&logits[vocab_size..]) } else { 0 };
            if token != self.blank {
                h = out["output_states_1"].try_extract_tensor::<f32>().map_err(ort_err)?.1.to_vec();
                c = out["output_states_2"].try_extract_tensor::<f32>().map_err(ort_err)?.1.to_vec();
                tokens.push(token);
                last = token as i32;
                emitted += 1;
            }
            if step > 0 {
                t += step;
                emitted = 0;
            } else if token == self.blank || emitted >= MAX_TOKENS_PER_FRAME {
                t += 1;
                emitted = 0;
            }
        }
        Ok(tokens)
    }

    /// SentencePiece pieces to text ("▁" marks a word start); control tokens are dropped.
    fn detokenize(&self, tokens: &[usize]) -> String {
        let mut s = String::new();
        for &id in tokens {
            let piece = &self.vocab[id];
            if piece.starts_with('<') && piece.ends_with('>') {
                continue;
            }
            s.push_str(piece);
        }
        s.replace('\u{2581}', " ").split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

/// Splits long audio into chunks of at most MAX_CHUNK_SECS, cutting at the quietest 100 ms in
/// the last SPLIT_SEARCH_SECS of each chunk (normally a pause between words).
fn split(samples: &[f32]) -> Vec<&[f32]> {
    let rate = SAMPLE_RATE as usize;
    let (max, search, window) = (MAX_CHUNK_SECS * rate, SPLIT_SEARCH_SECS * rate, rate / 10);
    let mut chunks = Vec::new();
    let mut start = 0;
    while samples.len() - start > max {
        let end = start + max;
        let mut best = (end, f32::INFINITY);
        let mut i = end - search;
        while i + window <= end {
            let energy: f32 = samples[i..i + window].iter().map(|x| x * x).sum();
            if energy < best.1 {
                best = (i + window / 2, energy);
            }
            i += window / 2;
        }
        chunks.push(&samples[start..best.0]);
        start = best.0;
    }
    chunks.push(&samples[start..]);
    chunks
}

/// Parakeet doesn't report the language it heard. Rewriting benefits from knowing it, so pick
/// the language whose most common words appear most often, among the user's languages when
/// they are set. Returns "" when nothing matches.
fn guess_language(text: &str, candidates: &[String]) -> String {
    const COMMON: &[(&str, &[&str])] = &[
        ("en", &["the", "and", "is", "are", "to", "of", "i", "you", "it", "that", "this", "what", "with", "for", "we", "can", "not", "have", "my", "so", "be", "was"]),
        ("pt", &["o", "os", "as", "é", "do", "da", "dos", "das", "não", "um", "uma", "para", "com", "eu", "você", "isso", "isto", "no", "na", "mas", "está", "também", "então", "aqui"]),
        ("es", &["el", "los", "las", "y", "es", "del", "un", "una", "para", "con", "yo", "eso", "esto", "pero", "está", "por", "muy", "también", "entonces", "aquí"]),
        ("fr", &["le", "les", "et", "est", "des", "du", "ne", "pas", "un", "une", "pour", "avec", "je", "ce", "mais", "il", "vous", "nous", "très", "aussi"]),
        ("de", &["der", "die", "das", "und", "ist", "zu", "nicht", "ein", "eine", "für", "mit", "ich", "du", "sie", "wir", "aber", "auf", "den", "auch", "sehr"]),
        ("it", &["il", "lo", "gli", "è", "di", "che", "non", "un", "una", "per", "con", "io", "questo", "ma", "sono", "anche", "molto", "allora"]),
    ];
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect();
    let allowed = |code: &str| candidates.is_empty() || candidates.iter().any(|c| c.eq_ignore_ascii_case(code));
    let best = COMMON
        .iter()
        .filter(|(code, _)| allowed(code))
        .map(|(code, list)| (*code, words.iter().filter(|w| list.contains(&w.as_str())).count()))
        .max_by_key(|(_, score)| *score);
    match best {
        Some((code, score)) if score > 0 => code.to_string(),
        _ => candidates.first().cloned().unwrap_or_default(),
    }
}

fn main() {
    audian_common::logging::init("audian-parakeet");
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--bench") {
        std::process::exit(match bench(&args[2..]) {
            Ok(()) => 0,
            Err(e) => {
                eprintln!("error: {e:#}");
                1
            }
        });
    }

    let idle_secs: u64 = args
        .iter()
        .position(|a| a == "--idle-exit-secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    let started = Instant::now();
    let last_activity = Arc::new(AtomicU64::new(0));
    {
        let last_activity = last_activity.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(5));
            let idle = started.elapsed().as_secs().saturating_sub(last_activity.load(Ordering::Relaxed));
            if idle >= idle_secs {
                log::info!("idle for {idle}s, exiting to free memory");
                log::logger().flush();
                std::process::exit(0);
            }
        });
    }
    let touch = || last_activity.store(started.elapsed().as_secs(), Ordering::Relaxed);

    log::info!("started (pid {}, idle exit after {idle_secs}s)", std::process::id());
    let mut reader = BufReader::new(io::stdin().lock());
    let mut writer = BufWriter::new(io::stdout().lock());
    let mut engine: Option<Engine> = None;

    loop {
        let request: SttRequest = match ipc::recv_json(&mut reader) {
            Ok(r) => r,
            Err(e) => {
                if e.kind() != io::ErrorKind::UnexpectedEof {
                    log::error!("reading request: {e}");
                }
                break;
            }
        };
        touch();
        let response = match request {
            SttRequest::Load { model_path, threads } => {
                if engine.as_ref().is_some_and(|e| e.model_dir == model_path) {
                    SttResponse::Loaded { model: model_path, load_ms: 0 }
                } else {
                    engine = None;
                    let t = Instant::now();
                    match Engine::load(&model_path, threads) {
                        Ok(e) => {
                            let load_ms = t.elapsed().as_millis() as u64;
                            log::info!("loaded {model_path} in {load_ms} ms ({} threads)", e.threads);
                            engine = Some(e);
                            SttResponse::Loaded { model: model_path, load_ms }
                        }
                        Err(e) => {
                            log::error!("{e:#}");
                            SttResponse::Error { id: None, message: format!("{e:#}") }
                        }
                    }
                }
            }
            SttRequest::Transcribe { id, language, candidates, .. } => {
                let samples = match ipc::recv_samples(&mut reader) {
                    Ok(s) => s,
                    Err(e) => {
                        log::error!("reading audio: {e}");
                        break;
                    }
                };
                match engine.as_mut() {
                    None => SttResponse::Error { id: Some(id), message: "no model loaded".into() },
                    Some(engine) => {
                        let t = Instant::now();
                        match engine.transcribe(&samples, &language, &candidates) {
                            Ok((text, language, no_speech_prob)) => {
                                let elapsed_ms = t.elapsed().as_millis() as u64;
                                log::info!(
                                    "transcribed {:.1}s of audio in {elapsed_ms} ms (lang {language}); {}",
                                    samples.len() as f32 / SAMPLE_RATE as f32,
                                    audian_common::memory_summary()
                                );
                                SttResponse::Transcript { id, text, language, elapsed_ms, no_speech_prob }
                            }
                            Err(e) => {
                                log::error!("{e:#}");
                                SttResponse::Error { id: Some(id), message: format!("{e:#}") }
                            }
                        }
                    }
                }
            }
            SttRequest::Shutdown => break,
        };
        if let Err(e) = ipc::send_json(&mut writer, &response) {
            log::error!("writing response: {e}");
            break;
        }
        touch();
    }
    log::info!("exiting");
    log::logger().flush();
}

/// `audian-parakeet --bench <model-dir> <wav> [threads] [en,pt]`: loads the model, transcribes
/// the file three times and prints timings.
fn bench(args: &[String]) -> anyhow::Result<()> {
    let model = args.first().ok_or_else(|| anyhow!("usage: --bench <model-dir> <wav> [threads] [en,pt]"))?;
    let wav = args.get(1).ok_or_else(|| anyhow!("missing wav path"))?;
    let threads: u32 = args.get(2).and_then(|t| t.parse().ok()).unwrap_or(0);
    let candidates: Vec<String> = args.get(3).map(|c| c.split(',').map(str::to_string).collect()).unwrap_or_default();
    let (samples, rate) = audian_common::wav::read_mono(Path::new(wav))?;
    anyhow::ensure!(rate == SAMPLE_RATE, "bench input must be 16 kHz (got {rate})");

    let mut out = io::stdout().lock();
    let t = Instant::now();
    let mut engine = Engine::load(&audian_common::paths::resolve_model(model).to_string_lossy(), threads)?;
    writeln!(out, "model: {model}\nthreads: {}\nload: {} ms", engine.threads, t.elapsed().as_millis())?;
    writeln!(out, "audio: {:.2} s", samples.len() as f32 / rate as f32)?;
    for run in 1..=3 {
        let t = Instant::now();
        let (text, lang, _) = engine.transcribe(&samples, "auto", &candidates)?;
        writeln!(out, "run {run}: {} ms  lang={lang}\n  {text}", t.elapsed().as_millis())?;
    }
    writeln!(out, "{}", audian_common::memory_summary())?;
    Ok(())
}
