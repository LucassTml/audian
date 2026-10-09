//! Exercises the model downloader against any URL (used with a local HTTP server in tests).
//! usage: download_test <url> <file-name> <size> <sha256>

use audian_common::catalog::{Engine, ModelInfo, ModelKind};
use audian_common::download::{self, State};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let leak = |s: &str| -> &'static str { Box::leak(s.to_string().into_boxed_str()) };
    let model: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        kind: ModelKind::Speech,
        engine: Engine::Whisper,
        name: "test",
        file: leak(&a[2]),
        url: leak(&a[1]),
        size: a[3].parse().unwrap(),
        sha256: leak(&a[4]),
        parts: &[],
        ram_mb: 0,
        speed: "",
        summary: "",
        recommended: false,
    }));
    let dl = download::start(model);
    let t = std::time::Instant::now();
    loop {
        let st = dl.state();
        println!("{:>6} ms  {:>5.1}%  {:?}", t.elapsed().as_millis(), dl.fraction() * 100.0, st);
        if !matches!(st, State::Downloading | State::Verifying) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
}
