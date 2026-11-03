fn main() {
    build_silk();
    tauri_build::build()
}

/// Compile the vendored Skype SILK v3 reference decoder used for WeChat voice messages.
///
/// The sources live in `vendor/silk` (BSD-style licence, kept in the file headers). They are
/// plain C with no external dependencies, so the same `cc` build works for macOS and Windows.
fn build_silk() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor/silk");
    let wrapper = dir.join("wechat_silk.c");
    if !wrapper.exists() {
        println!("cargo:warning=src-tauri/vendor/silk is missing; voice decoding will be unavailable");
        return;
    }

    let mut build = cc::Build::new();
    build.include(&dir);
    build.warnings(false);
    build.flag_if_supported("-w");

    let mut sources = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|ext| ext == "c").unwrap_or(false) {
                build.file(&path);
                println!("cargo:rerun-if-changed={}", path.display());
                sources += 1;
            }
        }
    }
    println!("cargo:rerun-if-changed={}", dir.display());
    println!("cargo:rerun-if-changed={}", wrapper.display());
    if sources == 0 {
        println!("cargo:warning=no SILK C sources found; voice decoding will be unavailable");
        return;
    }
    build.compile("wechat_silk");
}
