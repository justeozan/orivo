//! Following a streamed game from Play to the end of the stream.
//!
//! Moonlight is an app of its own, with windows of its own: a dialog asking
//! whether to quit what the PC is already running, a grey "Starting…" page, a
//! black window, then the picture. Left alone, all of it shows between Orivo's
//! splash and the game, and when the stream ends nothing brings Orivo back.
//! This module lets Orivo stand in front of Moonlight until the picture is
//! there, answer the "another game is running" question in its own words, and
//! come back when the stream is over.
//!
//! What it reads is Moonlight's own log. Moonlight names the file on stderr the
//! moment it starts (`Redirecting log output to /tmp/Moonlight-….log`) and
//! writes a line at each step: the PC found, the game launched or resumed, the
//! RTSP handshake, the first video packet, the quit key combo, any error it is
//! about to put in a dialog. Those lines are Moonlight 6's wording, not a
//! contract, so every reading here has a way out: a log that stops making
//! progress uncovers Moonlight after a deadline (`Stalled`), and the end of a
//! stream is still the process ending.
//!
//! "Another game is running" is the one step Moonlight does not log: it asks,
//! and waits. Its log goes quiet right after the PC answered, where a launch
//! would otherwise open its stream page at once, and that silence is what is
//! read as `HostBusy`. Nothing is decided on it: Orivo asks, and the player's
//! "close it and play" is carried out with Moonlight's own `quit <host>` and a
//! fresh launch, because the dialog cannot be answered from outside. If
//! Moonlight moves on by itself after all (a slow app list), the question goes
//! away.

use crate::runner_host::PreparedRunnerLaunch;
use serde::Serialize;
use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::Child,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

/// Every step of a stream, sent to the main window.
pub const STREAM_SESSION_EVENT: &str = "stream-session";

/// How long Moonlight may stay quiet after the PC answered before Orivo reads
/// it as waiting on "quit what is running?". A launch with nothing to quit
/// opens its stream page within the same second.
const HOST_BUSY_AFTER: Duration = Duration::from_millis(2_500);
/// How long a stream may go without a step before Orivo stops standing in
/// front of Moonlight and lets it show whatever it is waiting on.
const STALL_AFTER: Duration = Duration::from_secs(40);
/// How long Moonlight has to name its log before Orivo stops waiting for it.
const LOG_NAME_DEADLINE: Duration = Duration::from_secs(5);
/// How long `quit <host>` may take. On failure it opens a dialog and waits, so
/// past this it is ended rather than left behind Orivo.
const QUIT_DEADLINE: Duration = Duration::from_secs(20);
/// A stream Orivo could not follow, which then ran this long, was played.
const UNFOLLOWED_PLAY: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(80);
/// How long the page has to draw the way back before Orivo is shown again.
const RETURN_PAINT: Duration = Duration::from_millis(150);
/// Bounds on what is read from the log: it is another program's file.
const MAX_READ: u64 = 1 << 20;
const MAX_LINE: usize = 2_048;
const MAX_MESSAGE: usize = 280;

/// Where a stream is, as the main window is told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamStage {
    /// Moonlight is looking for the PC and its games.
    Connecting,
    /// The PC is most likely running another game, and Moonlight is asking
    /// whether to quit it.
    HostBusy,
    /// Moonlight stopped making progress; Orivo has stepped out of its way.
    Stalled,
    /// What the PC was running is being quit.
    Closing,
    /// The game is being started, or resumed, on the PC.
    Launching,
    /// The stream itself is being set up.
    Handshake,
    /// The first picture has arrived.
    Visible,
    /// Moonlight said why it could not stream, or closed before it did.
    Failed,
    /// The stream is over, and Orivo is back.
    Ended,
}

impl StreamStage {
    /// Steps only move forward; the two "not moving" states sit just above
    /// the start, so any real step leaves them.
    fn rank(self) -> u8 {
        match self {
            Self::Connecting => 0,
            Self::HostBusy | Self::Stalled => 1,
            Self::Closing => 2,
            Self::Launching => 3,
            Self::Handshake => 4,
            Self::Visible => 5,
            Self::Failed | Self::Ended => 9,
        }
    }
}

/// One line of Moonlight's log that says something about the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LogLine {
    /// `"Astra" is now online at "100.72.203.120:47989"`
    Online,
    /// A stream or quit page opened: Moonlight is past the PC and app lookup.
    Page,
    /// `Executing request: "https://…/cancel?…"`
    Cancel,
    /// `Executing request: "https://…/launch?…"` or `/resume?`
    Launch { resumed: bool },
    /// `Starting RTSP handshake...`
    Handshake,
    /// `Received first video packet after 200 ms`
    FirstFrame,
    /// `Detected quit key combo`
    QuitCombo,
    /// `Qt Critical: …` — Moonlight logs each error it shows in a dialog.
    Critical(String),
}

fn read_line(line: &str) -> Option<LogLine> {
    if line.contains(" is now online at ") {
        Some(LogLine::Online)
    } else if line.contains("Found \"ModeSeven.ttf\"") {
        Some(LogLine::Page)
    } else if line.contains("Executing request:") && line.contains("/cancel?") {
        Some(LogLine::Cancel)
    } else if line.contains("Executing request:") && line.contains("/launch?") {
        Some(LogLine::Launch { resumed: false })
    } else if line.contains("Executing request:") && line.contains("/resume?") {
        Some(LogLine::Launch { resumed: true })
    } else if line.contains("Starting RTSP handshake") {
        Some(LogLine::Handshake)
    } else if line.contains("Received first video packet") {
        Some(LogLine::FirstFrame)
    } else if line.contains("Detected quit key combo") {
        Some(LogLine::QuitCombo)
    } else {
        let (_, message) = line.split_once("Qt Critical: ")?;
        Some(LogLine::Critical(plain(message)))
    }
}

/// Text from the log made fit to show: one line, no control characters,
/// bounded, and never empty.
fn plain(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    let trimmed = cleaned.trim();
    let mut message: String = trimmed.chars().take(MAX_MESSAGE).collect();
    if trimmed.chars().count() > MAX_MESSAGE {
        message.push('…');
    }
    message
}

/// The log file Moonlight announced on stderr. Only a file named the way
/// Moonlight names its logs is taken: whatever else stderr says is ignored.
fn announced_log(line: &str) -> Option<PathBuf> {
    let path = Path::new(line.trim().strip_prefix("Redirecting log output to ")?);
    let stamp = path
        .file_name()?
        .to_str()?
        .strip_prefix("Moonlight-")?
        .strip_suffix(".log")?;
    (path.is_absolute() && !stamp.is_empty() && stamp.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| path.to_path_buf())
}

/// How a stream ended, for the main window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEnd {
    pub stage: StreamStage,
    pub message: Option<String>,
    pub seconds: Option<u64>,
}

/// The reading of one Moonlight run, line by line. Pure: the caller gives it
/// the lines and the time, and announces the stages it returns.
#[derive(Debug)]
pub struct StreamWatch {
    host: String,
    stage: StreamStage,
    last_step: Instant,
    online_at: Option<Instant>,
    /// Moonlight went past looking for the PC and its games.
    moved_on: bool,
    resumed: bool,
    visible_at: Option<Instant>,
    failure: Option<String>,
    quit_by_player: bool,
}

impl StreamWatch {
    pub fn new(host: &str, now: Instant) -> Self {
        Self {
            host: host.to_owned(),
            stage: StreamStage::Connecting,
            last_step: now,
            online_at: None,
            moved_on: false,
            resumed: false,
            visible_at: None,
            failure: None,
            quit_by_player: false,
        }
    }

    pub fn stage(&self) -> StreamStage {
        self.stage
    }

    pub fn resumed(&self) -> bool {
        self.resumed
    }

    pub fn visible_at(&self) -> Option<Instant> {
        self.visible_at
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub fn quit_by_player(&self) -> bool {
        self.quit_by_player
    }

    fn set(&mut self, stage: StreamStage, now: Instant) -> Option<StreamStage> {
        if stage == self.stage {
            return None;
        }
        self.stage = stage;
        self.last_step = now;
        Some(stage)
    }

    fn advance(&mut self, stage: StreamStage, now: Instant) -> Option<StreamStage> {
        if stage.rank() > self.stage.rank() {
            self.set(stage, now)
        } else {
            self.last_step = now;
            None
        }
    }

    /// One line of the log. Returns the stage to announce, when it changed.
    pub fn read(&mut self, line: &str, now: Instant) -> Option<StreamStage> {
        if self.stage.rank() >= StreamStage::Failed.rank() {
            return None;
        }
        match read_line(line)? {
            // Moonlight watches every PC it knows; only this one counts.
            LogLine::Online => {
                if self.online_at.is_none() && line.contains(self.host.as_str()) {
                    self.online_at = Some(now);
                    self.last_step = now;
                }
                None
            }
            LogLine::Page => {
                self.moved_on = true;
                self.last_step = now;
                // The question Orivo thought Moonlight was asking was not
                // being asked after all: back to waiting, without it.
                if self.stage == StreamStage::HostBusy {
                    self.set(StreamStage::Connecting, now)
                } else {
                    None
                }
            }
            LogLine::Cancel => {
                self.moved_on = true;
                self.advance(StreamStage::Closing, now)
            }
            LogLine::Launch { resumed } => {
                self.moved_on = true;
                self.resumed = resumed;
                self.advance(StreamStage::Launching, now)
            }
            LogLine::Handshake => self.advance(StreamStage::Handshake, now),
            LogLine::FirstFrame => {
                self.visible_at.get_or_insert(now);
                self.advance(StreamStage::Visible, now)
            }
            LogLine::QuitCombo => {
                self.quit_by_player = true;
                None
            }
            LogLine::Critical(message) => {
                self.failure.get_or_insert(message);
                // Before the picture, Moonlight is about to sit on an error
                // dialog nobody can see: that is the end of this launch. After
                // it, the stream is over and the reason goes with the return.
                if self.visible_at.is_none() {
                    self.set(StreamStage::Failed, now)
                } else {
                    None
                }
            }
        }
    }

    /// The passing of time with no line: what silence means at this step.
    pub fn tick(&mut self, now: Instant) -> Option<StreamStage> {
        match self.stage {
            StreamStage::Connecting if !self.moved_on => {
                if let Some(online) = self.online_at
                    && now.duration_since(online) >= HOST_BUSY_AFTER
                {
                    return self.set(StreamStage::HostBusy, now);
                }
            }
            // A question waits for the player as long as it takes.
            StreamStage::HostBusy => return None,
            _ => {}
        }
        if self.stage.rank() < StreamStage::Visible.rank()
            && self.stage != StreamStage::Stalled
            && now.duration_since(self.last_step) >= STALL_AFTER
        {
            return self.set(StreamStage::Stalled, now);
        }
        None
    }

    /// Moonlight has exited. `started` is when this run began; `cancelled`
    /// says Orivo ended it on the player's word, which needs no announcement.
    pub fn finish(&self, now: Instant, started: Instant, cancelled: bool) -> Option<StreamEnd> {
        if cancelled {
            return None;
        }
        if let Some(visible) = self.visible_at {
            return Some(StreamEnd {
                stage: StreamStage::Ended,
                message: self.failure.clone(),
                seconds: Some(now.duration_since(visible).as_secs()),
            });
        }
        // Already announced, with Moonlight's own words.
        if self.stage == StreamStage::Failed {
            return None;
        }
        // Orivo stepped out of the way of a stream it could not follow, and
        // the stream went on: it was played, it just was not read.
        if self.stage == StreamStage::Stalled && now.duration_since(started) >= UNFOLLOWED_PLAY {
            return Some(StreamEnd {
                stage: StreamStage::Ended,
                message: None,
                seconds: None,
            });
        }
        Some(StreamEnd {
            stage: StreamStage::Failed,
            message: self.failure.clone(),
            seconds: None,
        })
    }
}

/// The end of the log already read, and the line being written.
struct LogTail {
    path: PathBuf,
    file: Option<File>,
    partial: Vec<u8>,
}

impl LogTail {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            partial: Vec::new(),
        }
    }

    /// Every whole line written since the last call.
    fn read_new(&mut self) -> Vec<String> {
        if self.file.is_none() {
            self.file = File::open(&self.path).ok();
        }
        let Some(file) = self.file.as_mut() else {
            return Vec::new();
        };
        let mut chunk = Vec::new();
        if file.take(MAX_READ).read_to_end(&mut chunk).is_err() || chunk.is_empty() {
            return Vec::new();
        }
        self.partial.extend_from_slice(&chunk);
        let mut lines = Vec::new();
        while let Some(end) = self.partial.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            let line = &line[..line.len() - 1];
            lines.push(String::from_utf8_lossy(&line[..line.len().min(MAX_LINE)]).into_owned());
        }
        // A line that never ends is not a line Moonlight writes.
        if self.partial.len() > MAX_LINE {
            self.partial.clear();
        }
        lines
    }
}

/// Where the log lines come from while Moonlight runs.
enum LogSource {
    /// Waiting for Moonlight to name its log on stderr.
    Naming {
        names: mpsc::Receiver<PathBuf>,
        since: Instant,
    },
    Reading(LogTail),
    /// It never did: this run is followed by its process alone.
    Unread,
}

/// One Moonlight process and what its log has said.
struct Run {
    child: Child,
    log: LogSource,
    watch: StreamWatch,
    started: Instant,
}

impl Run {
    fn start(mut child: Child, host: &str) -> Self {
        let now = Instant::now();
        let log = match child.stderr.take() {
            Some(stderr) => {
                let (name, names) = mpsc::channel();
                // stderr is drained to the end, whatever it says, so Moonlight
                // never blocks on a full pipe.
                thread::spawn(move || {
                    let mut named = false;
                    for line in BufReader::new(stderr).lines() {
                        let Ok(line) = line else { break };
                        if !named && let Some(path) = announced_log(&line) {
                            named = name.send(path).is_ok();
                        }
                    }
                });
                LogSource::Naming { names, since: now }
            }
            None => LogSource::Unread,
        };
        Self {
            child,
            log,
            watch: StreamWatch::new(host, now),
            started: now,
        }
    }

    fn new_lines(&mut self) -> Vec<String> {
        let named = match &self.log {
            LogSource::Naming { names, since } => match names.try_recv() {
                Ok(path) => Some(LogSource::Reading(LogTail::new(path))),
                Err(mpsc::TryRecvError::Empty) if since.elapsed() < LOG_NAME_DEADLINE => None,
                Err(_) => Some(LogSource::Unread),
            },
            _ => None,
        };
        if let Some(log) = named {
            self.log = log;
        }
        match &mut self.log {
            LogSource::Reading(tail) => tail.read_new(),
            _ => Vec::new(),
        }
    }

    fn unread(&self) -> bool {
        matches!(self.log, LogSource::Unread)
    }

    fn end(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }
}

/// What the main window can ask of the stream it is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    /// Stop: the player does not want this stream after all.
    Cancel,
    /// Quit what the PC is running and start this game in its place.
    Replace,
}

struct ActiveStream {
    run_id: u64,
    game_id: String,
    control: mpsc::Sender<Control>,
}

static ACTIVE: Mutex<Option<ActiveStream>> = Mutex::new(None);
static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

/// Start the same game again, through the whole authorisation a launch goes
/// through: the stream process and its `quit`.
pub type Relaunch = Box<dyn Fn() -> Result<(Child, PreparedRunnerLaunch), String> + Send>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamSessionMessage<'a> {
    game_id: &'a str,
    stage: StreamStage,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    resumed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    seconds: Option<u64>,
    by_player: bool,
}

fn announce(app: &AppHandle, game_id: &str, watch: &StreamWatch, stage: StreamStage) {
    let _ = app.emit_to(
        crate::MAIN_WINDOW_LABEL,
        STREAM_SESSION_EVENT,
        StreamSessionMessage {
            game_id,
            stage,
            message: (stage == StreamStage::Failed)
                .then(|| watch.failure())
                .flatten(),
            resumed: watch.resumed(),
            seconds: None,
            by_player: false,
        },
    );
}

/// Keep Orivo's window above every other app's, so Moonlight's own pages
/// open behind the splash rather than over it.
pub fn cover(app: &AppHandle, on: bool) {
    #[cfg(desktop)]
    {
        use tauri::Manager;
        if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW_LABEL) {
            let _ = window.set_always_on_top(on);
        }
    }
    #[cfg(mobile)]
    let _ = (app, on);
}

/// Bring Orivo back where it was, in front.
fn come_back(app: &AppHandle) {
    cover(app, false);
    #[cfg(target_os = "macos")]
    let _ = app.show();
    #[cfg(desktop)]
    {
        use tauri::Manager;
        if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW_LABEL) {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

/// `quit <host>`, bounded: it ends with Moonlight's own answer or is ended.
fn quit_host(quit: &PreparedRunnerLaunch) {
    let Ok(mut child) = quit.command().spawn() else {
        return;
    };
    let deadline = Instant::now() + QUIT_DEADLINE;
    while matches!(child.try_wait(), Ok(None)) {
        if Instant::now() >= deadline {
            let _ = child.kill();
            break;
        }
        thread::sleep(POLL);
    }
    let _ = child.wait();
}

/// Follow a stream Moonlight has just started for `game_id`, until it ends.
///
/// `quit` is the same client's `quit <host>`; `relaunch` starts the game
/// again; `record` counts the time the picture was up as play time.
pub fn watch(
    app: AppHandle,
    game_id: String,
    child: Child,
    quit: PreparedRunnerLaunch,
    relaunch: Relaunch,
    record: fn(&AppHandle, &str, Instant),
) {
    let run_id = NEXT_RUN.fetch_add(1, Ordering::Relaxed);
    let (control, controls) = mpsc::channel();
    if let Ok(mut active) = ACTIVE.lock() {
        *active = Some(ActiveStream {
            run_id,
            game_id: game_id.clone(),
            control,
        });
    }
    cover(&app, true);
    thread::spawn(move || {
        follow(&app, &game_id, child, quit, relaunch, record, &controls);
        if let Ok(mut active) = ACTIVE.lock()
            && active
                .as_ref()
                .is_some_and(|stream| stream.run_id == run_id)
        {
            *active = None;
        }
    });
}

fn follow(
    app: &AppHandle,
    game_id: &str,
    child: Child,
    mut quit: PreparedRunnerLaunch,
    relaunch: Relaunch,
    record: fn(&AppHandle, &str, Instant),
    controls: &mpsc::Receiver<Control>,
) {
    let host = quit.machine().unwrap_or_default();
    let mut run = Run::start(child, &host);
    let mut cancelled = false;
    loop {
        while let Ok(control) = controls.try_recv() {
            match control {
                Control::Cancel => {
                    cancelled = true;
                    run.end();
                }
                Control::Replace => {
                    run.end();
                    announce(app, game_id, &run.watch, StreamStage::Closing);
                    quit_host(&quit);
                    match relaunch() {
                        Ok((child, next_quit)) => {
                            quit = next_quit;
                            run = Run::start(child, &host);
                            announce(app, game_id, &run.watch, StreamStage::Connecting);
                        }
                        Err(message) => {
                            cover(app, false);
                            let _ = app.emit_to(
                                crate::MAIN_WINDOW_LABEL,
                                STREAM_SESSION_EVENT,
                                StreamSessionMessage {
                                    game_id,
                                    stage: StreamStage::Failed,
                                    message: Some(&message),
                                    resumed: false,
                                    seconds: None,
                                    by_player: false,
                                },
                            );
                            return;
                        }
                    }
                }
            }
        }
        let now = Instant::now();
        let mut stages: Vec<StreamStage> = run
            .new_lines()
            .iter()
            .filter_map(|line| run.watch.read(line, now))
            .collect();
        // A log Moonlight never named cannot be followed: Orivo gets out of
        // the way at once rather than after the stall deadline.
        if run.unread() && run.watch.stage() == StreamStage::Connecting {
            stages.extend(run.watch.set(StreamStage::Stalled, now));
        }
        stages.extend(run.watch.tick(now));
        for stage in stages {
            announce(app, game_id, &run.watch, stage);
            match stage {
                StreamStage::Stalled => cover(app, false),
                StreamStage::Failed => run.end(),
                _ => {}
            }
        }
        // An error after the picture ends the stream: Moonlight would only
        // wait on a dialog, so Orivo takes the player back with the reason.
        if run.watch.visible_at().is_some() && run.watch.failure().is_some() {
            run.end();
        }
        if run.exited() {
            break;
        }
        thread::sleep(POLL);
    }
    let now = Instant::now();
    for line in run.new_lines() {
        run.watch.read(&line, now);
    }
    if let Some(visible) = run.watch.visible_at() {
        record(app, game_id, visible);
    }
    // Whatever happened, Orivo is the one in front afterwards: either it never
    // left, or it stepped aside for a stream that is now over.
    let Some(end) = run.watch.finish(now, run.started, cancelled) else {
        come_back(app);
        return;
    };
    let _ = app.emit_to(
        crate::MAIN_WINDOW_LABEL,
        STREAM_SESSION_EVENT,
        StreamSessionMessage {
            game_id,
            stage: end.stage,
            message: end.message.as_deref(),
            resumed: run.watch.resumed(),
            seconds: end.seconds,
            by_player: run.watch.quit_by_player(),
        },
    );
    if end.stage == StreamStage::Ended {
        // The way back starts on black: the page gets a moment to draw it
        // before the window is shown, so the library never flashes first.
        thread::sleep(RETURN_PAINT);
    }
    come_back(app);
}

/// The main window's answer to a stream it is waiting on: `cancel` stops it,
/// `replace` quits what the PC is running and starts this game instead, and
/// `uncover` stops standing in front of Moonlight.
#[tauri::command]
pub fn stream_session_control(
    app: AppHandle,
    game_id: String,
    action: String,
) -> Result<(), String> {
    let control = match action.as_str() {
        "uncover" => {
            cover(&app, false);
            return Ok(());
        }
        "cancel" => Control::Cancel,
        "replace" => Control::Replace,
        _ => return Err("Orivo doesn't know that action.".into()),
    };
    let active = ACTIVE
        .lock()
        .map_err(|_| "This stream has already ended.".to_string())?;
    match active.as_ref() {
        Some(stream) if stream.game_id == game_id => stream
            .control
            .send(control)
            .map_err(|_| "This stream has already ended.".to_string()),
        _ => Err("This stream has already ended.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "100.72.203.120";

    /// Lines as Moonlight 6.2 writes them, ids replaced.
    const ONLINE: &str = "00:00:00 - Qt Info: \"Astra\" is now online at \"100.72.203.120:47989\"";
    const OTHER_ONLINE: &str = "00:00:00 - Qt Info: \"Laptop\" is now online at \"10.0.0.7:47989\"";
    const PAGE: &str = "00:00:00 - Qt Info: Found \"ModeSeven.ttf\" at \":/data/ModeSeven.ttf\"";
    const CANCEL: &str = "00:00:02 - Qt Info: Executing request: \"https://100.72.203.120:47984/cancel?uniqueid=0123456789abcdef&uuid=00000000000000000000000000000000\"";
    const LAUNCH: &str = "00:00:07 - Qt Info: Executing request: \"https://100.72.203.120:47984/launch?uniqueid=0123456789abcdef&uuid=0&appid=1&mode=3024x1964x60&rikey=REDACTED\"";
    const RESUME: &str = "00:00:05 - Qt Info: Executing request: \"https://100.72.203.120:47984/resume?uniqueid=0123456789abcdef&uuid=0&appid=1\"";
    const HANDSHAKE: &str = "00:00:08 - SDL Info (0): Starting RTSP handshake...";
    const FIRST_FRAME: &str = "00:00:09 - SDL Info (0): Received first video packet after 200 ms";
    const QUIT_COMBO: &str = "00:00:52 - SDL Info (0): Detected quit key combo";
    const CRITICAL: &str = "00:00:20 - Qt Critical: Failed to connect to Astra";

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn moonlight_names_its_log_on_stderr_and_nothing_else_is_taken_for_one() {
        assert_eq!(
            announced_log("Redirecting log output to /tmp/Moonlight-1795707438.log"),
            Some(PathBuf::from("/tmp/Moonlight-1795707438.log"))
        );
        assert_eq!(
            announced_log("Redirecting log output to /tmp/other.log"),
            None
        );
        assert_eq!(
            announced_log("Redirecting log output to Moonlight-1.log"),
            None
        );
        assert_eq!(
            announced_log("Redirecting log output to /tmp/Moonlight-.log"),
            None
        );
        assert_eq!(announced_log("Moonlight 6.2.0"), None);
    }

    #[test]
    fn a_launch_with_nothing_to_quit_goes_straight_to_the_picture() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        assert_eq!(watch.read(ONLINE, at(start, 100)), None);
        assert_eq!(watch.read(PAGE, at(start, 120)), None);
        // Moonlight can show its warnings for seconds before it launches:
        // that is not the question.
        assert_eq!(watch.tick(at(start, 4_000)), None);
        assert_eq!(
            watch.read(RESUME, at(start, 5_000)),
            Some(StreamStage::Launching)
        );
        assert!(watch.resumed());
        assert_eq!(
            watch.read(HANDSHAKE, at(start, 6_000)),
            Some(StreamStage::Handshake)
        );
        assert_eq!(
            watch.read(FIRST_FRAME, at(start, 6_500)),
            Some(StreamStage::Visible)
        );
        assert_eq!(watch.read(QUIT_COMBO, at(start, 60_000)), None);
        assert!(watch.quit_by_player());
        let end = watch.finish(at(start, 61_500), start, false).unwrap();
        assert_eq!(end.stage, StreamStage::Ended);
        assert_eq!(end.seconds, Some(55));
    }

    #[test]
    fn silence_after_the_pc_answered_is_read_as_another_game_running() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        watch.read(ONLINE, at(start, 100));
        assert_eq!(watch.tick(at(start, 1_000)), None);
        assert_eq!(watch.tick(at(start, 2_700)), Some(StreamStage::HostBusy));
        // It waits for the player, however long they take.
        assert_eq!(watch.tick(at(start, 120_000)), None);
        // The player let Moonlight quit it: the stream goes on.
        assert_eq!(
            watch.read(PAGE, at(start, 121_000)),
            Some(StreamStage::Connecting)
        );
        assert_eq!(
            watch.read(CANCEL, at(start, 121_100)),
            Some(StreamStage::Closing)
        );
        assert_eq!(
            watch.read(LAUNCH, at(start, 126_000)),
            Some(StreamStage::Launching)
        );
        assert!(!watch.resumed());
    }

    #[test]
    fn another_pc_coming_online_says_nothing_about_this_one() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        watch.read(OTHER_ONLINE, at(start, 100));
        assert_eq!(watch.tick(at(start, 5_000)), None);
    }

    #[test]
    fn a_log_that_stops_moving_lets_moonlight_show_itself() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        watch.read(ONLINE, at(start, 100));
        watch.read(PAGE, at(start, 150));
        watch.read(LAUNCH, at(start, 1_000));
        assert_eq!(watch.tick(at(start, 30_000)), None);
        assert_eq!(watch.tick(at(start, 41_100)), Some(StreamStage::Stalled));
        assert_eq!(watch.tick(at(start, 90_000)), None);
        // It may still get there; the stages carry on from where they were.
        assert_eq!(
            watch.read(HANDSHAKE, at(start, 91_000)),
            Some(StreamStage::Handshake)
        );
    }

    #[test]
    fn an_error_before_the_picture_fails_the_launch_with_moonlights_words() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        assert_eq!(
            watch.read(CRITICAL, at(start, 20_000)),
            Some(StreamStage::Failed)
        );
        assert_eq!(watch.failure(), Some("Failed to connect to Astra"));
        // Announced once, at the time: the exit adds nothing.
        assert_eq!(watch.finish(at(start, 20_500), start, false), None);
        // And nothing after it moves the stage again.
        assert_eq!(watch.read(FIRST_FRAME, at(start, 21_000)), None);
    }

    #[test]
    fn an_error_after_the_picture_comes_back_with_the_end() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        watch.read(FIRST_FRAME, at(start, 9_000));
        assert_eq!(watch.read(CRITICAL, at(start, 30_000)), None);
        let end = watch.finish(at(start, 31_000), start, false).unwrap();
        assert_eq!(end.stage, StreamStage::Ended);
        assert_eq!(end.message.as_deref(), Some("Failed to connect to Astra"));
    }

    #[test]
    fn moonlight_closing_before_the_picture_is_a_failure_unless_orivo_closed_it() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        watch.read(ONLINE, at(start, 100));
        let end = watch.finish(at(start, 3_000), start, false).unwrap();
        assert_eq!(end.stage, StreamStage::Failed);
        assert_eq!(end.message, None);
        assert_eq!(watch.finish(at(start, 3_000), start, true), None);
    }

    #[test]
    fn a_stream_orivo_could_not_follow_that_went_on_was_played() {
        let start = Instant::now();
        let mut watch = StreamWatch::new(HOST, start);
        watch.set(StreamStage::Stalled, start);
        let end = watch.finish(at(start, 600_000), start, false).unwrap();
        assert_eq!(end.stage, StreamStage::Ended);
        assert_eq!(end.seconds, None);
    }

    #[test]
    fn errors_are_shown_as_one_bounded_line() {
        assert_eq!(
            plain("  Échec\tde la\nconnexion  "),
            "Échec de la connexion"
        );
        let long = "x".repeat(MAX_MESSAGE + 10);
        assert_eq!(plain(&long).chars().count(), MAX_MESSAGE + 1);
    }

    #[test]
    fn the_tail_reads_whole_lines_as_they_are_written() {
        let directory =
            std::env::temp_dir().join(format!("orivo-stream-tail-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("Moonlight-1.log");
        let mut tail = LogTail::new(path.clone());
        // Not written yet: nothing, and no error.
        assert!(tail.read_new().is_empty());
        std::fs::write(&path, format!("{ONLINE}\n{PAGE}\npartial")).unwrap();
        assert_eq!(tail.read_new(), vec![ONLINE.to_owned(), PAGE.to_owned()]);
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, b" line\n").unwrap();
        assert_eq!(tail.read_new(), vec!["partial line".to_owned()]);
        assert!(tail.read_new().is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
