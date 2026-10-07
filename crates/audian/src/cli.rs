//! Command-line diagnostics. The app is a GUI-subsystem executable, so output only appears
//! when stdout is redirected (e.g. `audian.exe --transcribe a.wav | more`).
//!
//! `--transcribe <wav> [--mode <mode>] [--provider <provider>] [--repeat N]`
//!     Runs the full speech-to-text + rewriting pipeline on a WAV file and prints the result.

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use audian_common::config::{Config, ProcessingMode, RewriteProvider};

use crate::audio::Recording;
use crate::pipeline::{self, Engines, Job};

fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> anyhow::Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| anyhow::anyhow!("invalid value: {s}"))
}

/// `--rewrite-file <txt> [--mode m] [--provider p]`: runs only the text-processing stage on
/// each line of a file (one transcript per line) — a quick evaluation loop for prompts.
pub fn rewrite_file(args: &[String]) -> anyhow::Result<()> {
    let path = args.first().ok_or_else(|| anyhow::anyhow!("usage: --rewrite-file <txt> [--mode m] [--provider p]"))?;
    let mut cfg = Config::load();
    let mut i = 1;
    while i + 1 < args.len() {
        match args[i].as_str() {
            "--mode" => cfg.processing.mode = parse_enum(&args[i + 1])?,
            "--provider" => cfg.processing.provider = parse_enum(&args[i + 1])?,
            "--output" => cfg.processing.output_language = args[i + 1].clone(),
            other => anyhow::bail!("unknown option {other}"),
        }
        i += 2;
    }
    let mut processor = crate::text::create(&cfg);
    processor.warm_up(cfg.processing.mode, &cfg.processing.custom_instructions, &cfg.processing.output_language);
    let mut out = std::io::stdout().lock();
    for line in std::fs::read_to_string(path)?.lines().filter(|l| !l.trim().is_empty()) {
        // Optional "xx|" prefix gives the transcript language, as speech recognition would.
        let (language, line) = match line.split_once('|') {
            Some((code, rest)) if code.len() == 2 => (code, rest),
            _ => ("en", line),
        };
        let t = Instant::now();
        let request = crate::text::RewriteRequest {
            text: line.trim(),
            language,
            output_language: &cfg.processing.output_language,
            mode: cfg.processing.mode,
            custom_instructions: &cfg.processing.custom_instructions,
        };
        match processor.process(&request) {
            Ok(text) => writeln!(out, "IN : {}\nOUT: {}  ({} ms)\n", line.trim(), text, t.elapsed().as_millis())?,
            Err(e) => writeln!(out, "IN : {}\nERR: {e}\n", line.trim())?,
        }
    }
    Ok(())
}

/// `--reload`: asks the running daemon to re-read the configuration file (after manual edits).
pub fn reload() -> anyhow::Result<()> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, PostMessageW, RegisterWindowMessageW};
    unsafe {
        let hwnd = FindWindowW(crate::app::WINDOW_CLASS, windows::core::PCWSTR::null())
            .map_err(|_| anyhow::anyhow!("Audian is not running"))?;
        let name = crate::win::WideStr::new(crate::app::CONFIG_CHANGED_MESSAGE);
        PostMessageW(Some(hwnd), RegisterWindowMessageW(name.pcwstr()), WPARAM(0), LPARAM(0))?;
    }
    println!("reload requested");
    Ok(())
}

/// `--list-devices`: prints input devices with the ids used in the config file.
pub fn list_devices() -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    for d in crate::audio::list_input_devices() {
        writeln!(out, "{}{}\n    id: {}", d.name, if d.is_default { "  (default)" } else { "" }, d.id)?;
    }
    Ok(())
}

pub fn transcribe(args: &[String]) -> anyhow::Result<()> {
    let wav_path = args.first().ok_or_else(|| anyhow::anyhow!("usage: --transcribe <wav> [--mode m] [--provider p]"))?;
    let mut cfg = Config::load();
    let mut repeat = 1;
    let mut i = 1;
    while i < args.len() {
        let value = args.get(i + 1).map(String::as_str).unwrap_or_default();
        match args[i].as_str() {
            "--mode" => cfg.processing.mode = parse_enum::<ProcessingMode>(value)?,
            "--provider" => cfg.processing.provider = parse_enum::<RewriteProvider>(value)?,
            "--model" => cfg.transcription.whisper.model = value.to_string(),
            "--language" => cfg.transcription.language = value.to_string(),
            "--repeat" => repeat = value.parse()?,
            "--output" => cfg.processing.output_language = value.to_string(),
            "--no-rewrite" => {
                cfg.processing.enabled = false;
                i -= 1;
            }
            other => anyhow::bail!("unknown option {other}"),
        }
        i += 2;
    }
    let (samples, sample_rate) = audian_common::wav::read_mono(Path::new(wav_path))?;
    let mut out = std::io::stdout().lock();
    writeln!(out, "file: {wav_path} ({:.1} s)", samples.len() as f32 / sample_rate as f32)?;
    writeln!(out, "mode: {:?}, provider: {:?}", cfg.processing.mode, cfg.processing.provider)?;

    let mut engines = Engines::new(&cfg);
    let t = Instant::now();
    engines.warm_up(&cfg, 1);
    writeln!(out, "warm-up dispatched in {} ms", t.elapsed().as_millis())?;
    for run in 1..=repeat {
        let t = Instant::now();
        let job = Job {
            recording: Recording { samples: samples.clone(), sample_rate, peak_rms: 1.0, vad: Default::default() },
            cfg: cfg.clone(),
            target_is_terminal: false,
            precomputed: None,
        };
        match pipeline::run(&mut engines, job, &|stage| log::debug!("stage {stage:?}")) {
            Ok(o) => {
                writeln!(out, "run {run}: {} ms\n  => {}", t.elapsed().as_millis(), o.text)?;
                if let Some(n) = o.notice {
                    writeln!(out, "  notice: {n}")?;
                }
            }
            Err(e) => writeln!(out, "run {run}: error after {} ms: {e}", t.elapsed().as_millis())?,
        }
    }
    Ok(())
}

/// `--export-art <dir>`: writes the logo and the recording-indicator previews (every theme
/// and style) as PNG files, for documentation.
pub fn export_art(args: &[String]) -> anyhow::Result<()> {
    use audian_common::config::{IndicatorStyle, ThemeId};
    use crate::overlay::{self, Phase};

    let dir = Path::new(args.first().ok_or_else(|| anyhow::anyhow!("usage: --export-art <dir>"))?);
    std::fs::create_dir_all(dir)?;
    audian_art::draw_logo(256).save_png(dir.join("logo.png"))?;
    for theme in ThemeId::ALL {
        for style in [IndicatorStyle::Dark, IndicatorStyle::Light] {
            for (phase, countdown, name) in [
                (Phase::Recording, false, "recording"),
                (Phase::Recording, true, "countdown"),
                (Phase::Processing, false, "processing"),
                (Phase::Done, false, "done"),
            ] {
                if let Some(pm) = overlay::preview(theme, style, true, phase, countdown, 2.0) {
                    let file = format!("indicator-{}-{}-{name}.png", theme.label().to_lowercase(), style.label().to_lowercase());
                    pm.save_png(dir.join(file))?;
                }
            }
        }
    }
    println!("wrote artwork to {}", dir.display());
    Ok(())
}
