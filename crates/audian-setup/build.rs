//! Embeds the application binaries (deflate-compressed) into the installer.
//! Build the app first (`scripts/package.ps1` does this), or set AUDIAN_PAYLOAD_DIR.

use std::path::PathBuf;

const FILES: &[&str] = &["audian.exe", "audian-stt.exe", "audian-llm.exe"];

fn main() {
    audian_build::embed("audian-setup.exe", "Audian Setup");
    println!("cargo::rustc-check-cfg=cfg(no_payload)");
    println!("cargo:rerun-if-env-changed=AUDIAN_PAYLOAD_DIR");
    let dir = std::env::var_os("AUDIAN_PAYLOAD_DIR").map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("target").join("release")
    });
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut complete = true;
    for f in FILES {
        let src = dir.join(f);
        println!("cargo:rerun-if-changed={}", src.display());
        let data = std::fs::read(&src).unwrap_or_else(|_| {
            complete = false;
            Vec::new()
        });
        let packed = miniz_oxide::deflate::compress_to_vec(&data, 9);
        std::fs::write(out.join(format!("{f}.z")), packed).unwrap();
    }
    if !complete {
        println!("cargo:warning=Audian binaries not found in {}; building an installer without payload", dir.display());
        println!("cargo:rustc-cfg=no_payload");
    }
}
