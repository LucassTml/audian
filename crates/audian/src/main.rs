//! Audian: press a shortcut, speak, and polished text appears wherever you were typing.
//!
//! One executable, three roles:
//! * default: the background daemon (tray icon, hotkey, recording overlay, pipeline)
//! * `--settings`: the settings window, run as a separate short-lived process so the daemon
//!   never carries a GUI toolkit in memory
//! * diagnostics such as `--transcribe <wav>` (see `cli.rs`)

#![windows_subsystem = "windows"]

mod app;
mod audio;
mod autostart;
mod cli;
mod endpoint;
mod helper;
mod hotkey;
mod inject;
mod overlay;
mod pipeline;
mod settings;
mod sound;
mod stt;
mod text;
mod tray;
mod win;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = match args.get(1).map(String::as_str) {
        Some("--settings") => {
            audian_common::logging::init("settings");
            report(settings::run(args.get(2).cloned()))
        }
        Some("--transcribe") => {
            audian_common::logging::init("cli");
            report(cli::transcribe(&args[2..]))
        }
        Some("--list-devices") => report(cli::list_devices()),
        Some("--export-art") => report(cli::export_art(&args[2..])),
        Some("--reload") => report(cli::reload()),
        Some("--rewrite-file") => {
            audian_common::logging::init("cli");
            report(cli::rewrite_file(&args[2..]))
        }
        Some("--version") => {
            println!("{} {}", audian_common::APP_NAME, audian_common::APP_VERSION);
            0
        }
        // No argument or `--background` (used by autostart).
        _ => {
            audian_common::logging::init("audian");
            app::run()
        }
    };
    std::process::exit(code);
}

fn report(result: anyhow::Result<()>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            log::error!("{e:#}");
            1
        }
    }
}
