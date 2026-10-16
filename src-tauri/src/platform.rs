//! What this build of Orivo can do on the device it runs on.
//!
//! The WebView cannot tell an iPad from a Mac by its user agent — iPadOS asks
//! for desktop sites by default — and it should not have to: these are facts
//! about the binary, so the binary states them.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformFeatures {
    /// Plugins fetched from the registry or picked from a file. Off on iOS:
    /// App Store Review Guideline 2.5.2 refuses an app that downloads code,
    /// and Cranelift could not compile a component there anyway, since iOS
    /// gives an App Store app no memory it may both write and execute.
    pub downloadable_plugins: bool,
    /// Runners that start a game on or from this device: Wine, Winlator, the
    /// console emulators, game streaming. iOS spawns no process, and none of
    /// the apps they hand a game to has a URL Orivo could open there.
    pub local_runners: bool,
    /// "Install" asks the host where the game can be got on this device, rather
    /// than asking a store's client to download it (see `store_choices`).
    pub store_choices: bool,
    /// Orivo can get out of a started game's way: hide on macOS, minimise on
    /// Windows and Linux (`step_aside_for_game`). A phone switches to the
    /// game's app by itself, so there it is not a choice to offer.
    pub steps_aside: bool,
}

pub const CURRENT: PlatformFeatures = for_target(cfg!(target_os = "ios"), cfg!(mobile));

/// The features of an iOS build, an Android build, or a desktop one. A
/// function of the target so that each answer can be tested on the machine
/// that builds it.
pub const fn for_target(ios: bool, mobile: bool) -> PlatformFeatures {
    PlatformFeatures {
        downloadable_plugins: !ios,
        local_runners: !ios,
        store_choices: ios,
        steps_aside: !mobile,
    }
}

#[tauri::command]
pub fn get_platform_features() -> PlatformFeatures {
    CURRENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ios_build_downloads_no_code_and_runs_nothing_itself() {
        assert_eq!(
            for_target(true, true),
            PlatformFeatures {
                downloadable_plugins: false,
                local_runners: false,
                store_choices: true,
                steps_aside: false,
            }
        );
    }

    #[test]
    fn an_android_build_keeps_plugins_and_runners_and_lets_the_system_switch_apps() {
        assert_eq!(
            for_target(false, true),
            PlatformFeatures {
                downloadable_plugins: true,
                local_runners: true,
                store_choices: false,
                steps_aside: false,
            }
        );
    }

    #[test]
    fn a_desktop_build_keeps_plugins_and_runners_and_steps_aside_for_a_game() {
        assert_eq!(
            for_target(false, false),
            PlatformFeatures {
                downloadable_plugins: true,
                local_runners: true,
                store_choices: false,
                steps_aside: true,
            }
        );
    }

    #[test]
    fn the_webview_reads_camel_case() {
        let json = serde_json::to_value(for_target(true, true)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "downloadablePlugins": false,
                "localRunners": false,
                "storeChoices": true,
                "stepsAside": false,
            })
        );
    }
}
