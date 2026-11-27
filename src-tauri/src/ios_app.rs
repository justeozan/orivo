//! A game's own iPhone version, started from its card.
//!
//! iOS tells no app which other apps are installed, and starts one only
//! through a link it declares: a URL scheme, or a universal link on a domain
//! it owns. Which link opens which game is nothing the App Store publishes, so
//! the few known ones are written down here, by App Store id.
//!
//! "Install" on a Steam game whose iOS version has a link tries that link
//! first. iOS answers whether an app took it, without falling back to Safari
//! for a universal link, so the answer is also the only "it is installed"
//! there is: it is remembered, and from then on the card offers Play, which
//! opens the game straight away. A link that stops working means the game was
//! deleted; the card forgets it and offers Install again.

use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

/// The launch option id the iPhone version is offered under, beside the
/// card's own targets (which on an iPhone start nothing).
pub const PLAY_OPTION: &str = "ios-app";
const REMEMBERED_FILE: &str = "ios-apps.json";

/// App Store id → the link that opens that app. A universal link must be one
/// the domain's `apple-app-site-association` gives the app for every path.
const LINKS: &[(u64, &str)] = &[
    // Thronefall - A Little Kingdom: "6R86MUXYMA.com.doghowlgames.thronefall", "/": "*".
    (6_744_029_320, "https://thronefallmobile.com/"),
];

pub fn link_of(track_id: u64) -> Option<&'static str> {
    LINKS.iter().find(|(id, _)| *id == track_id).map(|(_, link)| *link)
}

#[derive(Default)]
struct Service {
    data_dir: Option<PathBuf>,
    /// Game id → the App Store id of its iPhone version that opened.
    opened: Option<BTreeMap<String, u64>>,
}

fn service() -> &'static Mutex<Service> {
    static SERVICE: OnceLock<Mutex<Service>> = OnceLock::new();
    SERVICE.get_or_init(|| Mutex::new(Service::default()))
}

pub fn configure(data_dir: PathBuf) {
    if let Ok(mut service) = service().lock() {
        service.data_dir = Some(data_dir);
        service.opened = None;
    }
}

/// Read once, then answered from memory: every card asks while the library
/// view is built.
fn with_opened<T>(change: impl FnOnce(&mut BTreeMap<String, u64>) -> T) -> Option<(T, bool)> {
    let mut service = service().lock().ok()?;
    let data_dir = service.data_dir.clone();
    let opened = service.opened.get_or_insert_with(|| {
        data_dir
            .as_ref()
            .and_then(|dir| fs::read(dir.join(REMEMBERED_FILE)).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    });
    let before = opened.clone();
    let answer = change(opened);
    let changed = *opened != before;
    if changed {
        if let (Some(dir), Ok(bytes)) = (data_dir, serde_json::to_vec(&*opened)) {
            let _ = fs::write(dir.join(REMEMBERED_FILE), bytes);
        }
    }
    Some((answer, changed))
}

/// The link of this game's iPhone version, if it opened before and still has
/// a link to open it by.
pub fn remembered(game_id: &str) -> Option<&'static str> {
    with_opened(|opened| opened.get(game_id).copied())
        .and_then(|(track_id, _)| track_id)
        .and_then(link_of)
}

pub fn remember(game_id: &str, track_id: u64) {
    with_opened(|opened| opened.insert(game_id.to_string(), track_id));
}

/// Whether there was anything to forget, which is whether the card changes.
pub fn forget(game_id: &str) -> bool {
    with_opened(|opened| opened.remove(game_id).is_some()).is_some_and(|(removed, _)| removed)
}

/// Open `link` and say whether an app took it. A universal link is opened
/// only by its app: never by Safari, which would be a web page, not the game.
pub async fn open(app: &tauri::AppHandle, link: &'static str) -> bool {
    #[cfg(target_os = "ios")]
    {
        let (answer, answered) = tokio::sync::oneshot::channel();
        let asked = app.run_on_main_thread(move || ios::open(link, answer));
        if asked.is_err() {
            return false;
        }
        answered.await.unwrap_or(false)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = (app, link);
        false
    }
}

#[cfg(target_os = "ios")]
mod ios {
    use std::sync::Mutex;

    use block2::RcBlock;
    use objc2::{runtime::AnyObject, runtime::Bool, MainThreadMarker};
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};
    use objc2_ui_kit::{UIApplication, UIApplicationOpenURLOptionUniversalLinksOnly};

    pub fn open(link: &str, answer: tokio::sync::oneshot::Sender<bool>) {
        let answer = Mutex::new(Some(answer));
        let reply = move |opened: bool| {
            if let Some(answer) = answer.lock().ok().and_then(|mut answer| answer.take()) {
                let _ = answer.send(opened);
            }
        };
        let (Some(main_thread), Some(url)) =
            (MainThreadMarker::new(), NSURL::URLWithString(&NSString::from_str(link)))
        else {
            return reply(false);
        };
        let flag = NSNumber::new_bool(link.starts_with("https://"));
        let value: &AnyObject = &flag;
        // SAFETY: a UIKit constant, defined for the life of the process.
        let key: &NSString = unsafe { UIApplicationOpenURLOptionUniversalLinksOnly };
        let options = NSDictionary::<NSString, AnyObject>::from_slices(&[key], &[value]);
        let completion = RcBlock::new(move |opened: Bool| reply(opened.as_bool()));
        // SAFETY: the one key is UIKit's own and its value the NSNumber that
        // `openURL:options:completionHandler:` documents for it.
        unsafe {
            UIApplication::sharedApplication(main_thread).openURL_options_completionHandler(
                &url,
                &options,
                Some(&completion),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_link_is_a_universal_link_or_a_scheme_and_named_once() {
        let mut seen = std::collections::HashSet::new();
        for (track_id, link) in LINKS {
            assert!(seen.insert(track_id), "{track_id} is listed twice");
            assert!(link.contains("://"), "{link} is not a link");
            assert!(!link.starts_with("http://"), "{link}: iOS opens no app for plain http");
        }
        assert_eq!(link_of(6_744_029_320), Some("https://thronefallmobile.com/"));
        assert_eq!(link_of(1), None);
    }

    #[test]
    fn remembers_what_opened_across_a_restart_and_forgets_it_once() {
        let dir = std::env::temp_dir().join(format!("orivo-ios-app-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        configure(dir.clone());
        assert_eq!(remembered("steam:2239150"), None);
        remember("steam:2239150", 6_744_029_320);
        // An id with no link left to open it by is as good as not remembered.
        remember("steam:1", 1);
        assert_eq!(remembered("steam:1"), None);

        configure(dir.clone()); // as a restart would: read again from disk
        assert_eq!(remembered("steam:2239150"), Some("https://thronefallmobile.com/"));
        assert!(forget("steam:2239150"));
        assert!(!forget("steam:2239150"));
        configure(dir.clone());
        assert_eq!(remembered("steam:2239150"), None);
        let _ = fs::remove_dir_all(dir);
    }
}
