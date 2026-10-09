//! Downloads a catalog model into the models folder, printing progress.
//! usage: download_model <catalog file name>   (e.g. parakeet-tdt-0.6b-v3-int8)

use audian_common::catalog;
use audian_common::download::{self, State};

fn main() {
    let name = std::env::args().nth(1).expect("usage: download_model <catalog file name>");
    let model = catalog::find(&name).expect("not in the catalog");
    let dl = download::start(model);
    let t = std::time::Instant::now();
    loop {
        let st = dl.state();
        println!("{:>6} ms  {:>5.1}%  {:?}", t.elapsed().as_millis(), dl.fraction() * 100.0, st);
        if !matches!(st, State::Downloading | State::Verifying) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    println!("installed: {}", catalog::is_installed(model));
}
