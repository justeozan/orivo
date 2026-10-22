//! Download helpers: aria2 invocation, direct HTTP with resume, hashing.

use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const ARIA2_FLAGS: &[&str] = &[
    "--max-connection-per-server=8",
    "--min-split-size=1M",
    "--split=8",
    "--continue=true",
    "--timeout=120",
    "--retry-wait=5",
    "--max-tries=10",
    "--file-allocation=none",
    "--console-log-level=notice",
    "--summary-interval=5",
    "--allow-overwrite=true",
    "--bt-save-metadata=true",
    "--bt-enable-lpd=true",
    "--bt-max-peers=100",
    "--seed-time=0",
];

pub fn aria2_args(source: &str, dir: &str) -> Vec<String> {
    let mut args = vec![format!("--dir={}", dir)];
    for f in ARIA2_FLAGS {
        args.push((*f).to_string());
    }
    args.push(source.to_string());
    args
}

/// Detect the folder created by a torrent download, mirroring QuikyTest priorities.
pub fn detect_game_folder(folders: &[String], game_name: &str) -> Option<String> {
    let game_lower = game_name.to_lowercase();
    for f in folders {
        let fl = f.to_lowercase();
        if fl.starts_with(&format!("{} [", game_lower))
            || fl.starts_with(&format!("{}[", game_lower))
        {
            return Some(f.clone());
        }
    }
    for f in folders {
        if f.to_lowercase() == game_lower {
            return Some(f.clone());
        }
    }
    let candidates: Vec<&String> = folders
        .iter()
        .filter(|f| {
            let fl = f.to_lowercase();
            fl.contains(&game_lower) && (f.contains("FitGirl") || f.contains("Repack"))
        })
        .collect();
    if let Some(found) = candidates.first() {
        return Some((*found).clone());
    }
    folders.last().cloned()
}

/// Find `aria2c` on PATH (or accept an explicit path).
pub fn find_aria2(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
        return None;
    }
    if let Some(path) = std::env::var_os("QUIKY_ARIA2").map(PathBuf::from) {
        if path.is_file() {
            return Some(path);
        }
    }
    let names = ["aria2c", "aria2c.exe"];
    for name in names {
        if let Some(path) = crate::extract::which(name) {
            return Some(path);
        }
    }
    None
}

/// Run aria2 and wait for completion, honouring a cooperative cancel flag.
#[cfg(feature = "aria2")]
pub fn run_aria2(
    aria2: &Path,
    source: &str,
    dir: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<bool, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut child: Child = Command::new(aria2)
        .args(aria2_args(source, &dir.to_string_lossy()))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| format!("failed to spawn aria2: {e}"))?;

    loop {
        if let Some(flag) = cancel {
            if flag.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Run aria2 while parsing its periodic summary lines.
///
/// aria2 prints `[#gid 512MiB/1.4GiB(37%) CN:...]` every few seconds. That is
/// the only progress signal available for a magnet before the file exists, so
/// it is parsed here rather than polling a directory whose final size is not
/// known yet. `progress` receives `(percent, downloaded, total)`, with zero
/// sizes when aria2 has not printed a size pair.
#[cfg(feature = "aria2")]
pub fn run_aria2_with_progress<F>(
    aria2: &Path,
    source: &str,
    dir: &Path,
    cancel: Option<&AtomicBool>,
    mut progress: F,
) -> Result<bool, String>
where
    F: FnMut(u8, u64, u64),
{
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut child: Child = Command::new(aria2)
        .args(aria2_args(source, &dir.to_string_lossy()))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to spawn aria2: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "aria2 produced no progress stream".to_string())?;
    let (sender, receiver) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if sender.send(line).is_err() {
                break;
            }
        }
    });

    let summary = regex::Regex::new(
        r"(?x)([0-9]+(?:[.,][0-9]+)?)\s*([KMGTP]i?B)\s*/\s*([0-9]+(?:[.,][0-9]+)?)\s*([KMGTP]i?B)\s*\(\s*([0-9]{1,3})%\s*\)",
    )
    .map_err(|e| e.to_string())?;
    loop {
        if let Some(flag) = cancel {
            if flag.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(false);
            }
        }
        while let Ok(line) = receiver.try_recv() {
            if let Some(caps) = summary.captures(&line) {
                let percent = caps
                    .get(5)
                    .and_then(|value| value.as_str().parse::<u8>().ok())
                    .unwrap_or(0)
                    .min(100);
                let downloaded = parse_size(caps.get(1), caps.get(2));
                let total = parse_size(caps.get(3), caps.get(4));
                progress(percent, downloaded, total);
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status.success()),
            Ok(None) => std::thread::sleep(Duration::from_millis(200)),
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn parse_size(
    value: Option<regex::Match<'_>>,
    unit: Option<regex::Match<'_>>,
) -> u64 {
    let Some(value) = value else { return 0 };
    let Ok(number) = value.as_str().replace(',', ".").parse::<f64>() else {
        return 0;
    };
    let multiplier = match unit.map(|unit| unit.as_str().to_ascii_lowercase()) {
        Some(unit) if unit.starts_with('k') => 1024_u64,
        Some(unit) if unit.starts_with('m') => 1024_u64.pow(2),
        Some(unit) if unit.starts_with('g') => 1024_u64.pow(3),
        Some(unit) if unit.starts_with('t') => 1024_u64.pow(4),
        Some(unit) if unit.starts_with('p') => 1024_u64.pow(5),
        _ => 1,
    };
    (number * multiplier as f64).round() as u64
}

/// Download a URL over HTTP with Range-based resume and progress reporting.
#[cfg(feature = "scraping")]
pub fn download_http<F>(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    cancel: Option<&AtomicBool>,
    mut progress: F,
) -> Result<u64, String>
where
    F: FnMut(u64, u64),
{
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let existing = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);

    let mut request = client.get(url);
    if existing > 0 {
        request = request.header("Range", format!("bytes={existing}-"));
    }
    let mut response = request.send().map_err(|e| e.to_string())?;
    if !response.status().is_success() && response.status().as_u16() != 206 {
        return Err(format!("HTTP {} for {url}", response.status()));
    }

    let resuming = response.status().as_u16() == 206;
    let total = if resuming {
        response.content_length().map(|n| n + existing).unwrap_or(0)
    } else {
        response.content_length().unwrap_or(0)
    };

    let mut file = if resuming {
        File::options()
            .append(true)
            .open(dest)
            .map_err(|e| e.to_string())?
    } else {
        File::create(dest).map_err(|e| e.to_string())?
    };

    let mut written = if resuming { existing } else { 0 };
    let mut buffer = [0u8; 64 * 1024];
    loop {
        if let Some(flag) = cancel {
            if flag.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
        }
        let n = response.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        written += n as u64;
        progress(written, total);
    }
    file.flush().map_err(|e| e.to_string())?;
    Ok(written)
}

/// SHA-256 of a file as lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Verify a file against an expected lowercase hex digest.
pub fn verify_sha256(path: &Path, expected: &str) -> Result<bool, String> {
    let actual = sha256_file(path).map_err(|e| e.to_string())?;
    Ok(actual.eq_ignore_ascii_case(expected.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aria2_args_include_flags() {
        let args = aria2_args("magnet:?x", "/dl");
        assert!(args[0] == "--dir=/dl");
        assert!(args.contains(&"--split=8".to_string()));
        assert_eq!(args.last().unwrap(), "magnet:?x");
    }

    #[test]
    fn detect_priority_fitgirl_bracket() {
        let folders = vec!["Dredge".into(), "Dredge [FitGirl Repack]".into()];
        assert_eq!(
            detect_game_folder(&folders, "Dredge"),
            Some("Dredge [FitGirl Repack]".into())
        );
    }

    #[test]
    fn detect_exact() {
        let folders = vec!["Other".into(), "dredge".into()];
        assert_eq!(
            detect_game_folder(&folders, "Dredge"),
            Some("dredge".into())
        );
    }

    #[test]
    fn sha256_matches_known_vector() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.txt");
        let mut f = File::create(&path).unwrap();
        f.write_all(b"abc").unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(verify_sha256(
            &path,
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"
        )
        .unwrap());
    }
}
