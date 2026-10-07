//! Audian speech-to-text helper.
//!
//! Runs whisper.cpp in its own process so that (a) the always-running daemon stays small,
//! (b) the model's memory is returned to the OS when this process exits after being idle,
//! and (c) a crash in native inference code cannot take the daemon down.
//!
//! Protocol: framed JSON requests on stdin, responses on stdout (see `audian_common::ipc`).
//! Also supports `audian-stt --bench <model> <wav> [threads]` for measuring performance.

#![windows_subsystem = "windows"]

use std::io::{self, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use audian_common::ipc::{self, SttRequest, SttResponse};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState};

/// whisper.cpp ignores input shorter than one second, so short clips are padded with silence.
const MIN_SAMPLES: usize = (audian_common::SAMPLE_RATE as usize * 11) / 10;

struct Engine {
    model_path: String,
    // `state` borrows nothing from `ctx` in whisper-rs (both hold an Arc to the inner
    // context), but keep the context alive alongside it for clarity.
    _ctx: WhisperContext,
    state: WhisperState,
    threads: i32,
}

impl Engine {
    fn load(model_path: &str, threads: u32) -> anyhow::Result<Engine> {
        if !Path::new(model_path).is_file() {
            anyhow::bail!("model file not found: {model_path}");
        }
        let mut params = WhisperContextParameters::default();
        // Measured no benefit on CPU for small models; overridable for experiments.
        params.flash_attn = std::env::var("WF_FLASH").map(|v| v == "1").unwrap_or(false);
        let ctx = WhisperContext::new_with_params(model_path, params)
            .map_err(|e| anyhow::anyhow!("failed to load whisper model: {e}"))?;
        let state = ctx.create_state().map_err(|e| anyhow::anyhow!("failed to create state: {e}"))?;
        let threads = if threads == 0 { audian_common::default_threads() } else { threads };
        let mut engine = Engine { model_path: model_path.to_string(), _ctx: ctx, state, threads: threads as i32 };
        // The first inference allocates buffers, and language detection runs at the full 30 s
        // window size until a previous call has set a smaller audio context. A tiny silent
        // run here (while the user is still speaking) removes ~1.5 s from the first dictation.
        let t = Instant::now();
        let _ = engine.transcribe(&vec![0.0; MIN_SAMPLES], "en", &[], "");
        log::info!("warm-up inference took {} ms", t.elapsed().as_millis());
        Ok(engine)
    }

    /// Picks the most likely language among `candidates` (e.g. a bilingual user's two
    /// languages), which avoids misdetections such as Portuguese -> Galician on short clips.
    fn detect_among(&mut self, samples: &[f32], candidates: &[String]) -> Option<&'static str> {
        let threads = self.threads as usize;
        self.state.pcm_to_mel(samples, threads).ok()?;
        let (_, probs) = self.state.lang_detect(0, threads).ok()?;
        candidates
            .iter()
            .filter_map(|code| {
                let id = whisper_rs::get_lang_id(code.trim())?;
                Some((whisper_rs::get_lang_str(id)?, *probs.get(id as usize)?))
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(code, _)| code)
    }

    /// Returns (text, detected language, highest per-segment no-speech probability).
    fn transcribe(&mut self, samples: &[f32], language: &str, candidates: &[String], prompt: &str) -> anyhow::Result<(String, String, f32)> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(self.threads);
        params.set_translate(false);
        let mut language = if language.trim().is_empty() { "auto" } else { language.trim() };
        let detected;
        if language == "auto" && !candidates.is_empty() {
            let clip = &samples[..samples.len().min(audian_common::SAMPLE_RATE as usize * 10)];
            let padded: Vec<f32>;
            let clip = if clip.len() < MIN_SAMPLES {
                padded = clip.iter().copied().chain(std::iter::repeat(0.0)).take(MIN_SAMPLES).collect();
                &padded[..]
            } else {
                clip
            };
            if let Some(code) = self.detect_among(clip, candidates) {
                detected = code;
                language = detected;
            }
        }
        params.set_language(Some(language));
        params.set_no_context(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        let prompt = prompt.replace('\0', "");
        if !prompt.trim().is_empty() {
            params.set_initial_prompt(prompt.trim());
        }

        // Whisper always encodes a 30 s window (1500 frames). For short dictations, limiting the
        // encoder context to the actual audio length (plus margin) is several times faster.
        if std::env::var("WF_AUDIO_CTX").map(|v| v != "0").unwrap_or(true) {
            let secs = samples.len().max(MIN_SAMPLES) as f32 / audian_common::SAMPLE_RATE as f32;
            let frames = ((secs * 50.0).ceil() as i32 + 128).clamp(384, 1500);
            if frames < 1500 {
                params.set_audio_ctx(frames);
            }
        }

        let padded;
        let audio = if samples.len() < MIN_SAMPLES {
            padded = {
                let mut v = samples.to_vec();
                v.resize(MIN_SAMPLES, 0.0);
                v
            };
            &padded[..]
        } else {
            samples
        };

        self.state.full(params, audio).map_err(|e| anyhow::anyhow!("transcription failed: {e}"))?;

        let mut text = String::new();
        let mut no_speech: f32 = 0.0;
        for segment in self.state.as_iter() {
            if let Ok(s) = segment.to_str_lossy() {
                text.push_str(&s);
            }
            no_speech = no_speech.max(segment.no_speech_probability());
        }
        let lang = whisper_rs::get_lang_str(self.state.full_lang_id_from_state()).unwrap_or("").to_string();
        Ok((text.trim().to_string(), lang, no_speech))
    }
}

fn main() {
    audian_common::logging::init("audian-stt");
    whisper_rs::install_logging_hooks();

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
                if engine.as_ref().is_some_and(|e| e.model_path == model_path) {
                    SttResponse::Loaded { model: model_path, load_ms: 0 }
                } else {
                    engine = None; // free the previous model before loading another
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
            SttRequest::Transcribe { id, language, candidates, initial_prompt } => {
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
                        match engine.transcribe(&samples, &language, &candidates, &initial_prompt) {
                            Ok((text, language, no_speech_prob)) => {
                                let elapsed_ms = t.elapsed().as_millis() as u64;
                                log::info!(
                                    "transcribed {:.1}s of audio in {elapsed_ms} ms (lang {language}, no_speech {no_speech_prob:.2}); {}",
                                    samples.len() as f32 / audian_common::SAMPLE_RATE as f32,
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

/// `audian-stt --bench <model> <wav> [threads] [language]`: loads the model, transcribes the file
/// three times and prints timings, for choosing a model on a given machine.
fn bench(args: &[String]) -> anyhow::Result<()> {
    let model = args.first().ok_or_else(|| anyhow::anyhow!("usage: --bench <model> <wav> [threads] [lang]"))?;
    let wav = args.get(1).ok_or_else(|| anyhow::anyhow!("missing wav path"))?;
    let threads: u32 = args.get(2).and_then(|t| t.parse().ok()).unwrap_or(0);
    let language = args.get(3).cloned().unwrap_or_else(|| "auto".into());
    let candidates: Vec<String> = args.get(4).map(|c| c.split(',').map(str::to_string).collect()).unwrap_or_default();
    let (samples, rate) = audian_common::wav::read_mono(Path::new(wav))?;
    anyhow::ensure!(rate == audian_common::SAMPLE_RATE, "bench input must be 16 kHz (got {rate})");

    let mut out = io::stdout().lock();
    let t = Instant::now();
    let mut engine = Engine::load(&audian_common::paths::resolve_model(model).to_string_lossy(), threads)?;
    writeln!(out, "model: {model}\nthreads: {}\nload: {} ms", engine.threads, t.elapsed().as_millis())?;
    writeln!(out, "audio: {:.2} s", samples.len() as f32 / rate as f32)?;
    for run in 1..=3 {
        let t = Instant::now();
        let (text, lang, ns) = engine.transcribe(&samples, &language, &candidates, "")?;
        writeln!(out, "run {run}: {} ms  lang={lang} no_speech={ns:.2}\n  {text}", t.elapsed().as_millis())?;
    }
    Ok(())
}
