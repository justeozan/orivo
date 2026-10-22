//! Opt-in test against a real FitGirl repack on disk.
//!
//! Run with:
//!   QUIKY_TEST_GAME_DIR="/path/to/Repack" cargo test -p orivo-plugin-quiky-host \
//!     --test local_repack -- --ignored
//!
//! It never launches the installer; it only validates the layout the host
//! pipeline expects (setup.exe detection, kind detection, folder matching).

use quiky_host::download::detect_game_folder;
use quiky_host::extract::{detect_kind, find_setup_exe, ExtractKind};
use quiky_host::fitgirl;
use std::path::PathBuf;

fn game_dir() -> PathBuf {
    std::env::var("QUIKY_TEST_GAME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from("/Users/spectre/Games/Stick It to the Stickman [FitGirl Repack]")
        })
}

#[test]
#[ignore]
fn local_repack_layout() {
    let dir = game_dir();
    assert!(dir.is_dir(), "missing game dir: {}", dir.display());

    let setup = find_setup_exe(&dir).expect("setup*.exe not found");
    assert_eq!(detect_kind(&setup.to_string_lossy()), ExtractKind::SevenZ);

    let file_name = dir.file_name().unwrap().to_string_lossy().to_string();
    let detected = detect_game_folder(&[file_name.clone()], "Stick It to the Stickman");
    assert_eq!(detected.as_deref(), Some(file_name.as_str()));

    let bin_count = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "bin").unwrap_or(false))
        .count();
    assert!(bin_count > 0, "no .bin payload found");
    assert!(
        fitgirl::is_fitgirl_repack(&dir),
        "FitGirl layout not detected"
    );
    let bins = fitgirl::find_bins(&dir);
    assert_eq!(bins.len(), bin_count);
    assert_eq!(bins[0].file_name().unwrap(), "fg-01.bin");
    println!(
        "repack ok: {} ({} .bin files, setup: {})",
        dir.display(),
        bin_count,
        setup.file_name().unwrap().to_string_lossy()
    );
}
