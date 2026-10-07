//! Shared build-script logic: every Audian executable embeds the application manifest
//! (per-monitor DPI awareness, common controls v6, asInvoker), the logo icon and a
//! VERSIONINFO block (shown by Explorer, Task Manager and Windows' "Installed apps").

use std::path::Path;

/// Call from a binary crate's `build.rs`.
pub fn embed(original_filename: &str, description: &str) {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR");
    let out = Path::new(&out);
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts: Vec<u16> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(4, 0);
    let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);

    let ico = out.join("audian.ico");
    std::fs::write(&ico, audian_art::logo_ico()).expect("write ico");
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources").join("app.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());

    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    let rc = format!(
        r#"1 ICON "{ico}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Audian"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "{original_filename}"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "{original_filename}"
      VALUE "ProductName", "Audian"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        ico = esc(&ico),
        manifest = esc(&manifest),
    );
    let rc_path = out.join("audian.rc");
    std::fs::write(&rc_path, rc).expect("write rc");
    embed_resource::compile(&rc_path, embed_resource::NONE).manifest_required().unwrap();
}
