//! Full opt-in install against the real FitGirl repack.
//!
//! This extracts ~5 GiB, converts four 128 MiB OGG chunks to FSB5, applies an
//! HDiffPatch and extracts the resulting FreeArc archive. It is intentionally
//! ignored by default.
//!
//! ```sh
//! QUIKY_TEST_FITGIRL=1 \
//! QUIKY_TEST_GAME_DIR="/path/to/Repack" \
//! QUIKY_WINEPREFIX="$HOME/.quiky-wineprefix" \
//! cargo test -p orivo-plugin-quiky-host --test local_fitgirl -- --ignored --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use quiky_host::fitgirl;

fn enabled() -> bool {
    std::env::var_os("QUIKY_TEST_FITGIRL").is_some()
}

fn game_dir() -> PathBuf {
    std::env::var_os("QUIKY_TEST_GAME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from("/Users/spectre/Games/Stick It to the Stickman [FitGirl Repack]")
        })
}

fn install_dir() -> PathBuf {
    std::env::var_os("QUIKY_TEST_INSTALL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("quiky-fitgirl-install"))
}

fn exists_case_insensitive(dir: &Path, name: &str) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case(name)
            })
        })
        .unwrap_or(false)
}

#[test]
#[ignore]
fn local_fitgirl_install() {
    if !enabled() {
        eprintln!("skipped: set QUIKY_TEST_FITGIRL=1");
        return;
    }
    let repack = game_dir();
    assert!(
        fitgirl::is_fitgirl_repack(&repack),
        "missing repack: {}",
        repack.display()
    );

    let install = install_dir();
    let work = std::env::temp_dir().join("quiky-fitgirl-test-work");
    let options = fitgirl::FitGirlOptions::from_env(work);
    let cancel = AtomicBool::new(false);

    let report = fitgirl::install_fitgirl(
        &repack,
        &install,
        &options,
        &cancel,
        &mut |step, percent, message| {
            eprintln!("[{percent:3}%] {step}: {message}");
        },
    )
    .expect("FitGirl install failed");

    println!("{report:?}");
    assert!(report.archives >= 4);
    assert!(report.fgpacks >= 4);
    assert!(report.resource_files > 0);
    assert!(report.installed_bytes > 1_000_000_000);

    assert!(exists_case_insensitive(
        &install,
        "Stick It To The Stickman.exe"
    ));
    let data = install.join("Stick It To The Stickman_Data");
    assert!(data.join("resources.assets.resS").is_file());
    assert!(data.join("resources.resource").is_file());
    assert!(data.join("sharedassets0.resource").is_file());
}
