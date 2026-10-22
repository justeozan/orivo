//! Debug helpers: structured tracing, dumps and replay.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// One traced step of the pipeline.
#[derive(Debug, Clone)]
pub struct Trace {
    pub step: String,
    pub duration: Duration,
    pub outcome: String,
}

impl Trace {
    pub fn line(&self) -> String {
        format!(
            "step={} duration_ms={} outcome={}",
            self.step,
            self.duration.as_millis(),
            self.outcome
        )
    }
}

/// Collects traces and writes dumps to a debug directory.
#[derive(Debug)]
pub struct DebugSink {
    dir: Option<PathBuf>,
    traces: Vec<Trace>,
}

impl DebugSink {
    pub fn new(dir: Option<PathBuf>) -> Self {
        Self {
            dir,
            traces: Vec::new(),
        }
    }

    pub fn record(&mut self, step: &str, started: Instant, outcome: impl Into<String>) {
        let trace = Trace {
            step: step.to_string(),
            duration: started.elapsed(),
            outcome: outcome.into(),
        };
        if std::env::var_os("QUIKY_DEBUG").is_some() {
            eprintln!("[quiky] {}", trace.line());
        }
        self.traces.push(trace);
    }

    pub fn traces(&self) -> &[Trace] {
        &self.traces
    }

    pub fn dump(&self, name: &str, content: &str) -> std::io::Result<Option<String>> {
        match &self.dir {
            Some(dir) => Ok(Some(dump(name, content, dir)?)),
            None => Ok(None),
        }
    }

    pub fn dump_traces(&self) -> std::io::Result<Option<String>> {
        let text = self
            .traces
            .iter()
            .map(Trace::line)
            .collect::<Vec<_>>()
            .join("\n");
        self.dump("traces", &text)
    }
}

pub fn dump(name: &str, content: &str, dir: impl AsRef<Path>) -> std::io::Result<String> {
    let dir = dir.as_ref();
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.dump", name));
    fs::write(&path, content)?;
    Ok(path.display().to_string())
}

pub fn read_dump(path: impl AsRef<Path>) -> std::io::Result<String> {
    fs::read_to_string(path)
}

pub struct Replay {
    pub step: String,
    pub payload: String,
}

impl Replay {
    pub fn from_file(path: impl AsRef<Path>, step: &str) -> std::io::Result<Self> {
        Ok(Self {
            step: step.to_string(),
            payload: read_dump(path)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env::temp_dir;

    #[test]
    fn dump_and_read() {
        let dir = temp_dir().join("quiky-debug-test");
        let p = dump("sample", "hello", &dir).unwrap();
        assert!(Path::new(&p).exists());
        assert_eq!(read_dump(&p).unwrap(), "hello");
    }

    #[test]
    fn traces_recorded() {
        let mut sink = DebugSink::new(None);
        let started = Instant::now();
        sink.record("search", started, "ok");
        assert_eq!(sink.traces().len(), 1);
        assert!(sink.traces()[0].line().contains("step=search"));
    }

    #[test]
    fn replay_reads_dump() {
        let dir = temp_dir().join("quiky-replay-test");
        let p = dump("page", "<html/>", &dir).unwrap();
        let replay = Replay::from_file(&p, "scrape").unwrap();
        assert_eq!(replay.step, "scrape");
        assert_eq!(replay.payload, "<html/>");
    }
}
