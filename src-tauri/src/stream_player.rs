//! The stream player: the game from the person's gaming PC, on screen.
//!
//! Starfire does the network, the decoding (VideoToolbox) and the drawing
//! (Metal). This module owns what only the app can: a window with nothing in it
//! but the picture, the thread that hands each decoded frame to the main thread
//! (Metal and AppKit are main-thread only), and the end of the session when the
//! window closes or the host stops.
//!
//! Mac only for now; the phone follows once the decoder is built for it.
//! Keyboard and mouse go through `stream_input`; sound and controllers are not
//! wired yet.

use std::{
    cell::RefCell,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use starfire_client::{Client, ClientEvent, StarfireConfig};
use starfire_decode::VideoFrame;
use starfire_render::{ActiveRenderer, Renderer, new_for_window};
use tauri::{AppHandle, Manager, Window, WindowEvent, window::WindowBuilder};

use crate::{builtin_stream::StreamTarget, stream_input};

const WINDOW_LABEL: &str = "orivo-stream";
/// The picture the host is asked for. The window scales it.
const STREAM_SIZE: (u32, u32) = (1920, 1080);

/// The session, shared by the thread that feeds the window and the main
/// thread that sends input. `None` once it has stopped.
type Session = Arc<Mutex<Option<Client>>>;

thread_local! {
    /// The Metal renderer, born and used on the main thread only.
    static RENDERER: RefCell<Option<ActiveRenderer>> = const { RefCell::new(None) };
}

/// Open the window and start streaming into it. Returns once the window is up;
/// the stream itself starts in the background.
pub fn play(app: &AppHandle, target: StreamTarget) -> Result<(), String> {
    if app.get_window(WINDOW_LABEL).is_some() {
        return Err("A stream is already playing. Close its window first.".into());
    }

    let window = WindowBuilder::new(app, WINDOW_LABEL)
        .title(&target.title)
        .inner_size(1280.0, 720.0)
        .min_inner_size(640.0, 360.0)
        .build()
        .map_err(|error| format!("The stream window could not open: {error}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    window.on_window_event({
        let stop = Arc::clone(&stop);
        move |event| match event {
            WindowEvent::Resized(size) => {
                RENDERER.with(|renderer| {
                    if let Some(renderer) = renderer.borrow_mut().as_mut() {
                        renderer.resize(size.width, size.height);
                    }
                });
            }
            WindowEvent::CloseRequested { .. } => stop.store(true, Ordering::Release),
            WindowEvent::Destroyed => {
                stop.store(true, Ordering::Release);
                stream_input::uninstall();
                // The next stream gets a renderer of its own window.
                RENDERER.with(|renderer| renderer.borrow_mut().take());
            }
            _ => {}
        }
    });

    let client = Client::connect(StarfireConfig {
        host: target.host,
        pairing: Some(target.pairing),
        device_name: "Orivo".into(),
        app_name: target.title,
        width: STREAM_SIZE.0,
        height: STREAM_SIZE.1,
        audio: false,
        ..Default::default()
    });
    let latest = client.latest();
    let session: Session = Arc::new(Mutex::new(Some(client)));

    // Keyboard and mouse: AppKit is main-thread only, and a raw pointer does
    // not cross threads, so the window travels as its address.
    if let Ok(ns_window) = window.ns_window() {
        let ns_window = ns_window as usize;
        let send: stream_input::Send = {
            let session = Arc::clone(&session);
            Arc::new(move |packet| {
                if let Ok(session) = session.lock()
                    && let Some(client) = session.as_ref()
                {
                    client.send_input(packet);
                }
            })
        };
        let _ = app.run_on_main_thread(move || {
            // SAFETY: the stream window was just built and is open.
            unsafe { stream_input::install(ns_window as *mut std::ffi::c_void, STREAM_SIZE, send) };
        });
    }

    let app = app.clone();
    thread::spawn(move || run(app, window, session, latest, stop));
    Ok(())
}

/// Feed the window until the person closes it or the host ends the stream.
fn run(
    app: AppHandle,
    window: Window,
    session: Session,
    latest: Arc<Mutex<Option<VideoFrame>>>,
    stop: Arc<AtomicBool>,
) {
    // One present in flight at a time: if the main thread is behind, the newest
    // frame in the slot is the one it draws next, and the others are dropped.
    let presenting = Arc::new(AtomicBool::new(false));
    while !stop.load(Ordering::Acquire) {
        let event = session
            .lock()
            .ok()
            .and_then(|session| session.as_ref().and_then(Client::poll_event));
        match event {
            Some(ClientEvent::Frame) => {
                if presenting.swap(true, Ordering::AcqRel) {
                    continue;
                }
                let (window, latest, presenting) =
                    (window.clone(), Arc::clone(&latest), Arc::clone(&presenting));
                let _ = app.run_on_main_thread(move || {
                    present(&window, &latest);
                    presenting.store(false, Ordering::Release);
                });
            }
            Some(ClientEvent::Stopped(why)) => {
                eprintln!("orivo: stream ended: {why}");
                break;
            }
            None => thread::sleep(Duration::from_millis(2)),
        }
    }
    let client = session.lock().ok().and_then(|mut session| session.take());
    if let Some(client) = client {
        client.stop();
    }
    let _ = window.close();
}

/// Draw the newest decoded frame. Main thread only.
fn present(window: &Window, latest: &Mutex<Option<VideoFrame>>) {
    RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let size = window.inner_size().unwrap_or_default();
            match new_for_window(window.clone(), size.width.max(1), size.height.max(1)) {
                Ok(renderer) => *slot = Some(renderer),
                Err(error) => {
                    eprintln!("orivo: the stream window could not draw: {error}");
                    return;
                }
            }
        }
        let Some(renderer) = slot.as_mut() else {
            return;
        };
        // The frame is drawn under the lock: on the Mac it holds a decoder
        // buffer that cannot be moved out.
        if let Ok(frame) = latest.lock()
            && let Some(frame) = frame.as_ref()
            && let Err(error) = renderer.present(frame)
        {
            eprintln!("orivo: present failed: {error}");
        }
    });
}
