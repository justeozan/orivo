//! Run one pipeline step at a time, for manual testing.
//!
//!   cargo run -p orivo-plugin-quiky-host --example quiky-step -- search "Dredge"
//!   cargo run -p orivo-plugin-quiky-host --example quiky-step -- scrape <page-url>
//!   cargo run -p orivo-plugin-quiky-host --example quiky-step -- download <url> <dest>
//!   cargo run -p orivo-plugin-quiky-host --example quiky-step -- aria2 <url> <dir>
//!   cargo run -p orivo-plugin-quiky-host --example quiky-step -- extract <archive> <dest>
//!   cargo run -p orivo-plugin-quiky-host --example quiky-step -- fitgirl <repack-dir> <install-dir>
//!   cargo run -p orivo-plugin-quiky-host --example quiky-step -- inspect <game-dir>

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use quiky_host::{download, extract, fitgirl, scrape, search};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let step = args.first().map(String::as_str).unwrap_or("help");
    let result: Result<(), String> = match step {
        "search" => search_step(&args[1..]),
        "scrape" => scrape_step(&args[1..]),
        "download" => download_step(&args[1..]),
        "aria2" => aria2_step(&args[1..]),
        "extract" => extract_step(&args[1..]),
        "fitgirl" => fitgirl_step(&args[1..]),
        "inspect" => inspect_step(&args[1..]),
        _ => {
            eprintln!(
                "usage: quiky-step <search|scrape|download|aria2|extract|fitgirl|inspect> [args...]"
            );
            std::process::exit(2);
        }
    };
    if let Err(error) = result {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(feature = "scraping")]
fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("quiky/0.1")
        .build()
        .map_err(|e| e.to_string())
}

#[cfg(feature = "scraping")]
fn search_step(args: &[String]) -> Result<(), String> {
    let title = args.first().ok_or("search needs a title")?;
    let hub = search::Hub::default();
    let found = search::resolve_game_url(&client()?, &hub, title).map_err(|e| e.to_string())?;
    println!("{found:?}");
    Ok(())
}

#[cfg(not(feature = "scraping"))]
fn search_step(_args: &[String]) -> Result<(), String> {
    Err("built without the scraping feature".into())
}

#[cfg(feature = "scraping")]
fn scrape_step(args: &[String]) -> Result<(), String> {
    let url = args.first().ok_or("scrape needs a page url")?;
    let links = scrape::scrape_game_page(&client()?, url)?;
    println!("magnet:  {:?}", links.magnet);
    println!("torrent: {:?}", links.torrent);
    println!("direct:  {:?}", links.direct);
    Ok(())
}

#[cfg(not(feature = "scraping"))]
fn scrape_step(_args: &[String]) -> Result<(), String> {
    Err("built without the scraping feature".into())
}

#[cfg(feature = "scraping")]
fn download_step(args: &[String]) -> Result<(), String> {
    let url = args.first().ok_or("download needs a url")?;
    let dest = args.get(1).ok_or("download needs a destination path")?;
    let cancel = AtomicBool::new(false);
    let written = download::download_http(
        &client()?,
        url,
        Path::new(dest),
        Some(&cancel),
        |done, total| {
            if total > 0 {
                eprint!("\r{} / {} B", done, total);
            } else {
                eprint!("\r{} B", done);
            }
        },
    )?;
    eprintln!("\nwrote {written} B to {dest}");
    Ok(())
}

#[cfg(not(feature = "scraping"))]
fn download_step(_args: &[String]) -> Result<(), String> {
    Err("built without the scraping feature".into())
}

#[cfg(feature = "aria2")]
fn aria2_step(args: &[String]) -> Result<(), String> {
    let url = args.first().ok_or("aria2 needs a url")?;
    let dir = args.get(1).ok_or("aria2 needs a destination directory")?;
    let aria2 = download::find_aria2(None).ok_or("aria2c not found on PATH")?;
    let cancel = AtomicBool::new(false);
    let ok = download::run_aria2(&aria2, url, Path::new(dir), Some(&cancel))?;
    if ok {
        println!("aria2 ok: {url} -> {dir}");
        Ok(())
    } else {
        Err("aria2 exited non-zero".into())
    }
}

#[cfg(not(feature = "aria2"))]
fn aria2_step(_args: &[String]) -> Result<(), String> {
    Err("built without the aria2 feature".into())
}

fn extract_step(args: &[String]) -> Result<(), String> {
    let archive = args.first().ok_or("extract needs an archive")?;
    let dest = args.get(1).ok_or("extract needs a destination")?;
    extract::extract_archive(Path::new(archive), Path::new(dest))
}

fn fitgirl_step(args: &[String]) -> Result<(), String> {
    let repack = args
        .first()
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("QUIKY_TEST_GAME_DIR").map(PathBuf::from))
        .ok_or("fitgirl needs a repack directory")?;
    let install = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("quiky-install"));
    let work = std::env::temp_dir().join("quiky-fitgirl-step");
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
    .map_err(|error| error.to_string())?;
    println!("{report:?}");
    Ok(())
}

fn inspect_step(args: &[String]) -> Result<(), String> {
    let dir = args.first().ok_or("inspect needs a directory")?;
    let setup =
        extract::find_setup_exe(Path::new(dir)).ok_or_else(|| format!("no setup*.exe in {dir}"))?;
    println!("setup:    {}", setup.display());
    println!(
        "kind:     {:?}",
        extract::detect_kind(&setup.to_string_lossy())
    );
    println!("7z:       {:?}", extract::find_7z());
    println!("fitgirl:  {}", fitgirl::is_fitgirl_repack(Path::new(dir)));
    println!("archives: {:?}", fitgirl::find_bins(Path::new(dir)));
    Ok(())
}
