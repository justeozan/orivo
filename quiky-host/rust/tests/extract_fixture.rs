use quiky_host::extract::{extract_archive, find_setup_exe};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

#[test]
fn extracts_zip_and_finds_setup() {
    let dir = tempfile::tempdir().unwrap();
    extract_archive(&fixture("sample.zip"), dir.path()).expect("extract");
    let setup = find_setup_exe(dir.path()).expect("setup");
    assert_eq!(setup.file_name().unwrap(), "Setup.EXE");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("readme.txt")).unwrap(),
        "quiky fixture"
    );
}
