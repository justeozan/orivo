//! Install orchestration: queued -> downloading -> extracting -> installed.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::download;
use crate::extract;
use crate::fitgirl;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Queued,
    Downloading,
    Extracting,
    Installed,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn is_terminal(self) -> bool {
        matches!(self, Phase::Installed | Phase::Failed | Phase::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Downloading => "downloading",
            Phase::Extracting => "extracting",
            Phase::Installed => "installed",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        }
    }
}

pub fn can_transition(from: Phase, to: Phase) -> bool {
    use Phase::*;
    matches!(
        (from, to),
        (Queued, Downloading)
            | (Queued, Cancelled)
            | (Queued, Failed)
            | (Downloading, Extracting)
            | (Downloading, Failed)
            | (Downloading, Cancelled)
            | (Extracting, Installed)
            | (Extracting, Failed)
            | (Extracting, Cancelled)
    )
}

#[derive(Debug, Clone)]
pub enum DownloadSource {
    Url(String),
    Magnet(String),
    Torrent(String),
    Existing(PathBuf),
}

#[derive(Debug, Clone)]
pub struct InstallRequest {
    pub slug: String,
    pub title: String,
    pub source: DownloadSource,
    /// Where download artefacts live.
    pub work_dir: PathBuf,
    /// Final install directory.
    pub install_dir: PathBuf,
    /// Optional expected SHA-256 of a direct download.
    pub expected_sha256: Option<String>,
    /// Optional explicit aria2 path.
    pub aria2_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct InstallEvent {
    pub phase: Phase,
    pub percent: u8,
    pub message: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("cancelled")]
    Cancelled,
    #[error("download failed: {0}")]
    Download(String),
    #[error("hash mismatch for {0}")]
    HashMismatch(String),
    #[error("setup not found in {0}")]
    SetupNotFound(String),
    #[error("installation failed: {0}")]
    Install(String),
    #[error("invalid transition {0:?} -> {1:?}")]
    Transition(Phase, Phase),
}

/// Progress sink used by the host frontend.
pub trait Reporter {
    fn report(&mut self, event: InstallEvent);
}

impl<F> Reporter for F
where
    F: FnMut(InstallEvent),
{
    fn report(&mut self, event: InstallEvent) {
        self(event);
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), InstallError> {
    if cancel.load(Ordering::Relaxed) {
        Err(InstallError::Cancelled)
    } else {
        Ok(())
    }
}

/// Run a full install pipeline. The caller owns cancellation.
pub fn run_install<R: Reporter>(
    request: &InstallRequest,
    cancel: &AtomicBool,
    reporter: &mut R,
) -> Result<Phase, InstallError> {
    let mut phase = Phase::Queued;
    reporter.report(InstallEvent {
        phase,
        percent: 0,
        message: "queued".into(),
    });

    let result = run_inner(request, cancel, reporter, &mut phase);

    let final_phase = match &result {
        Ok(()) => Phase::Installed,
        Err(InstallError::Cancelled) => Phase::Cancelled,
        Err(_) => Phase::Failed,
    };
    if can_transition(phase, final_phase) {
        phase = final_phase;
    }
    reporter.report(InstallEvent {
        phase,
        percent: if phase == Phase::Installed { 100 } else { 0 },
        message: match &result {
            Ok(()) => "installed".to_string(),
            Err(e) => e.to_string(),
        },
    });
    match result {
        Ok(()) => Ok(Phase::Installed),
        Err(InstallError::Cancelled) => Ok(Phase::Cancelled),
        Err(e) => Err(e),
    }
}

fn transition(
    current: &mut Phase,
    next: Phase,
    _reporter: &mut impl Reporter,
) -> Result<(), InstallError> {
    if !can_transition(*current, next) && *current != next {
        return Err(InstallError::Transition(*current, next));
    }
    *current = next;
    Ok(())
}

fn run_inner<R: Reporter>(
    request: &InstallRequest,
    cancel: &AtomicBool,
    reporter: &mut R,
    phase: &mut Phase,
) -> Result<(), InstallError> {
    check_cancel(cancel)?;
    std::fs::create_dir_all(&request.work_dir)
        .map_err(|e| InstallError::Download(e.to_string()))?;

    transition(phase, Phase::Downloading, reporter)?;
    reporter.report(InstallEvent {
        phase: *phase,
        percent: 5,
        message: "downloading".into(),
    });

    match &request.source {
        DownloadSource::Existing(path) => {
            if !path.exists() {
                return Err(InstallError::Download(format!(
                    "missing existing source {}",
                    path.display()
                )));
            }
        }
        DownloadSource::Url(url) => {
            download_direct(request, url, cancel, reporter)?;
        }
        DownloadSource::Magnet(source) | DownloadSource::Torrent(source) => {
            download_aria2(request, source, cancel, reporter)?;
        }
    }

    check_cancel(cancel)?;
    transition(phase, Phase::Extracting, reporter)?;
    reporter.report(InstallEvent {
        phase: *phase,
        percent: 90,
        message: "extracting".into(),
    });

    let candidate = match &request.source {
        DownloadSource::Existing(path) if path.is_dir() => path.clone(),
        _ => request.work_dir.clone(),
    };
    let repack_dir = fitgirl::find_repack_dir(&candidate).unwrap_or(candidate);

    if fitgirl::is_fitgirl_repack(&repack_dir) {
        let work = std::env::temp_dir().join(format!("quiky-fitgirl-{}", std::process::id()));
        let options = fitgirl::FitGirlOptions::from_env(work);
        let report = {
            let mut progress = |step: &str, percent: u8, message: String| {
                let scaled = 90 + ((percent as u32 * 10 / 100).min(9) as u8);
                reporter.report(InstallEvent {
                    phase: *phase,
                    percent: scaled,
                    message: format!("fitgirl {step}: {message}"),
                });
            };
            fitgirl::install_fitgirl(
                &repack_dir,
                &request.install_dir,
                &options,
                cancel,
                &mut progress,
            )
            .map_err(|error| match error {
                fitgirl::FitGirlError::Cancelled => InstallError::Cancelled,
                other => InstallError::Install(other.to_string()),
            })?
        };
        reporter.report(InstallEvent {
            phase: *phase,
            percent: 99,
            message: format!(
                "fitgirl installed: {} archives, {} audio banks, {} resources",
                report.archives, report.fgpacks, report.resource_files
            ),
        });
        return Ok(());
    }

    let setup = extract::find_setup_exe(&request.work_dir)
        .ok_or_else(|| InstallError::SetupNotFound(request.work_dir.display().to_string()))?;
    let ok = extract::run_setup(&setup, &request.install_dir).map_err(InstallError::Install)?;
    if !ok {
        return Err(InstallError::Install(format!(
            "setup exited non-zero: {}",
            setup.display()
        )));
    }
    Ok(())
}

#[cfg(feature = "scraping")]
fn download_direct<R: Reporter>(
    request: &InstallRequest,
    url: &str,
    cancel: &AtomicBool,
    reporter: &mut R,
) -> Result<(), InstallError> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| InstallError::Download(e.to_string()))?;
    let file_name = url
        .rsplit('/')
        .next()
        .filter(|n| !n.is_empty())
        .unwrap_or(&request.slug)
        .to_string();
    let dest = request.work_dir.join(file_name);
    let mut last_report = 0u8;
    download::download_http(&client, url, &dest, Some(cancel), |written, total| {
        let percent = if total > 0 {
            ((written * 80) / total) as u8
        } else {
            10
        };
        if percent >= last_report.saturating_add(2) {
            last_report = percent;
            reporter.report(InstallEvent {
                phase: Phase::Downloading,
                percent,
                message: format!("downloaded {written} B"),
            });
        }
    })
    .map_err(InstallError::Download)?;

    if let Some(expected) = &request.expected_sha256 {
        if !download::verify_sha256(&dest, expected).map_err(InstallError::Download)? {
            return Err(InstallError::HashMismatch(dest.display().to_string()));
        }
    }
    Ok(())
}

#[cfg(not(feature = "scraping"))]
fn download_direct<R: Reporter>(
    _request: &InstallRequest,
    _url: &str,
    _cancel: &AtomicBool,
    _reporter: &mut R,
) -> Result<(), InstallError> {
    Err(InstallError::Download("scraping feature disabled".into()))
}

#[cfg(feature = "aria2")]
fn download_aria2<R: Reporter>(
    request: &InstallRequest,
    source: &str,
    cancel: &AtomicBool,
    reporter: &mut R,
) -> Result<(), InstallError> {
    let aria2 = download::find_aria2(request.aria2_path.as_deref())
        .ok_or_else(|| InstallError::Download("aria2 not installed".into()))?;
    reporter.report(InstallEvent {
        phase: Phase::Downloading,
        percent: 10,
        message: format!("aria2: {}", aria2.display()),
    });
    let ok = download::run_aria2(&aria2, source, &request.work_dir, Some(cancel))
        .map_err(InstallError::Download)?;
    if !ok {
        if cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        return Err(InstallError::Download("aria2 exited non-zero".into()));
    }
    Ok(())
}

#[cfg(not(feature = "aria2"))]
fn download_aria2<R: Reporter>(
    _request: &InstallRequest,
    _source: &str,
    _cancel: &AtomicBool,
    _reporter: &mut R,
) -> Result<(), InstallError> {
    Err(InstallError::Download("aria2 feature disabled".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_transition() {
        assert!(can_transition(Phase::Queued, Phase::Downloading));
        assert!(!can_transition(Phase::Downloading, Phase::Installed));
    }

    #[test]
    fn terminal() {
        assert!(Phase::Installed.is_terminal());
        assert!(!Phase::Downloading.is_terminal());
    }

    #[test]
    fn cancelled_before_start() {
        let dir = tempfile::tempdir().unwrap();
        let request = InstallRequest {
            slug: "x".into(),
            title: "X".into(),
            source: DownloadSource::Existing(dir.path().join("missing")),
            work_dir: dir.path().to_path_buf(),
            install_dir: dir.path().join("out"),
            expected_sha256: None,
            aria2_path: None,
        };
        let cancel = AtomicBool::new(true);
        let mut events = Vec::new();
        let result = run_install(&request, &cancel, &mut |e| events.push(e));
        assert!(matches!(result, Ok(Phase::Cancelled)));
        assert_eq!(events.last().unwrap().phase, Phase::Cancelled);
    }

    #[test]
    fn existing_without_setup_fails() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("archive.bin");
        std::fs::write(&source, b"x").unwrap();
        let request = InstallRequest {
            slug: "x".into(),
            title: "X".into(),
            source: DownloadSource::Existing(source),
            work_dir: dir.path().to_path_buf(),
            install_dir: dir.path().join("out"),
            expected_sha256: None,
            aria2_path: None,
        };
        let cancel = AtomicBool::new(false);
        let result = run_install(&request, &cancel, &mut |_| {});
        assert!(matches!(result, Err(InstallError::SetupNotFound(_))));
    }
}
