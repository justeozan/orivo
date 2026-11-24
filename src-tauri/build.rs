fn main() {
    // `crash_reporting` reads the DSN with `option_env!`, which cargo only
    // re-evaluates when told the variable matters.
    println!("cargo:rerun-if-env-changed=VITE_SENTRY_DSN");
    build_nxnative_fill();
    tauri_build::build()
}

/// The library Orivo loads into a native Switch game it starts on a Mac
/// (`src/nxnative_fill.m`, `nxnative::console_env`). Always arm64: the games
/// are Apple silicon only, whichever architecture Orivo itself is built for.
fn build_nxnative_fill() {
    println!("cargo:rerun-if-changed=src/nxnative_fill.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"))
        .join("nxnative-fill.dylib");
    let status = std::process::Command::new("/usr/bin/xcrun")
        .args(["--sdk", "macosx", "clang"])
        .args([
            "-dynamiclib",
            "-fobjc-arc",
            "-O2",
            "-Wall",
            "-arch",
            "arm64",
        ])
        .args(["-mmacosx-version-min=26.0", "-framework", "Cocoa"])
        .arg("-o")
        .arg(&out)
        .arg("src/nxnative_fill.m")
        .status()
        .expect("xcrun clang could not start");
    assert!(status.success(), "src/nxnative_fill.m did not compile");
}
