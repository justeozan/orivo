//! Games installed on this Android phone as apps of their own.
//!
//! Android files an app as a game when its developer says so
//! (`android:appCategory="game"`, or the older `isGame` flag). Orivo lists those
//! apps (`SafPlugin.listInstalledGames`, through `tauri-plugin-orivo-saf`), adds
//! one card per game, and starts a game through the launch intent its own
//! package declares — exactly what its home-screen icon does.
//!
//! The list is the phone's: a game uninstalled since the last look leaves the
//! library with it, and what the player did with a card in between — its play
//! time, a hidden flag, a wallpaper they chose — is kept for as long as the game
//! is.

use crate::catalog::{Catalog, Game, GameSource, LaunchTarget};
use std::{collections::BTreeMap, path::Path};
use tauri_plugin_orivo_saf::InstalledGame;

pub const PROVIDER: &str = "android";
const ID_PREFIX: &str = "android:";
const MAX_TITLE_CHARS: usize = 120;

/// Apps that call themselves games but are how Orivo starts *other* games, or
/// stores: listing Winlator or Moonlight beside the games they run would be a
/// card that opens a tool. Matched as package prefixes.
const NOT_GAMES: &[&str] = &[
    "com.winlator",
    "com.retroarch",
    "org.ppsspp",
    "com.limelight",
    "com.valvesoftware.steamlink",
    "com.epicgames.portal",
    "com.android.vending",
    "com.google.android.play.games",
    "io.orivo",
    "run.evey.orivo",
];

pub fn game_id(package_name: &str) -> String {
    format!("{ID_PREFIX}{package_name}")
}

/// A package name as Android defines one: dot-separated segments of letters,
/// digits and underscores. Anything else is not a package and is dropped.
pub fn valid_package_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.contains('.')
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

fn excluded(package_name: &str) -> bool {
    NOT_GAMES
        .iter()
        .any(|prefix| package_name == *prefix || package_name.starts_with(&format!("{prefix}.")))
}

/// The cards the phone's games stand for. Pictures are only kept when they are
/// plain file names inside the media cache — the one folder the WebView reads.
pub fn games_from(installed: &[InstalledGame], media_dir: &Path) -> Vec<Game> {
    let picture = |name: &Option<String>| {
        name.as_deref()
            .filter(|name| crate::game_detail::valid_opaque_file_name(name))
            .map(|name| media_dir.join(name))
            .filter(|path| path.is_file())
    };
    let mut games = BTreeMap::new();
    for app in installed {
        let package_name = app.package_name.trim();
        if !valid_package_name(package_name) || excluded(package_name) {
            continue;
        }
        let title: String = app
            .label
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_TITLE_CHARS)
            .collect();
        if title.trim().is_empty() {
            continue;
        }
        let hero = picture(&app.hero);
        let game = Game {
            id: game_id(package_name),
            title: title.trim().to_string(),
            executable_path: None,
            source: GameSource::Android,
            source_id: Some(package_name.to_string()),
            launch_target: LaunchTarget::Provider {
                provider: PROVIDER.to_string(),
                app_ref: package_name.to_string(),
            },
            alternate_launch_targets: Vec::new(),
            installation_path: None,
            working_directory: None,
            arguments: Vec::new(),
            description: None,
            metadata: None,
            artwork_path: hero.clone(),
            artwork_source_path: None,
            cover_path: picture(&app.cover),
            cover_source_path: None,
            home_image_path: None,
            landscape_image_path: hero,
            logo_path: None,
            hidden: false,
            hero_video_path: None,
            last_played_at: None,
            play_time_seconds: 0,
            extra: BTreeMap::new(),
        };
        if game.validate().is_ok() {
            games.insert(game.id.clone(), game);
        }
    }
    games.into_values().collect()
}

/// Bring the catalog's Android cards in line with the phone. Returns whether
/// anything changed.
pub fn apply(catalog: &mut Catalog, games: Vec<Game>) -> bool {
    let mut incoming: BTreeMap<String, Game> =
        games.into_iter().map(|game| (game.id.clone(), game)).collect();
    let mut changed = false;

    catalog.games.retain_mut(|existing| {
        if existing.source != GameSource::Android {
            return true;
        }
        let Some(mut fresh) = incoming.remove(&existing.id) else {
            changed = true;
            return false;
        };
        // What the player made of the card stays; what the phone says about
        // the app is refreshed.
        fresh.hidden = existing.hidden;
        fresh.play_time_seconds = existing.play_time_seconds;
        fresh.last_played_at = existing.last_played_at.clone();
        fresh.home_image_path = existing.home_image_path.clone();
        fresh.description = existing.description.clone();
        fresh.metadata = existing.metadata.clone();
        fresh.extra = existing.extra.clone();
        if existing.logo_path.is_some() {
            fresh.logo_path = existing.logo_path.clone();
        }
        if *existing != fresh {
            *existing = fresh;
            changed = true;
        }
        true
    });

    for (_, game) in incoming {
        catalog.games.push(game);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn installed(package_name: &str, label: &str) -> InstalledGame {
        InstalledGame {
            package_name: package_name.into(),
            label: label.into(),
            icon: Some(format!("android-{package_name}-icon.png")),
            cover: Some(format!("android-{package_name}-cover.jpg")),
            hero: Some(format!("android-{package_name}-hero.jpg")),
            installed_at: 1,
            updated_at: 2,
        }
    }

    fn media(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("orivo-android-games-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_games_and_leaves_out_the_apps_that_start_other_games() {
        let dir = media("list");
        fs::write(dir.join("android-com.mojang.minecraftpe-cover.jpg"), b"jpg").unwrap();
        let games = games_from(
            &[
                installed("com.mojang.minecraftpe", "Minecraft"),
                installed("com.winlator.cmod", "Winlator"),
                installed("com.limelight", "Moonlight"),
                installed("not a package", "Broken"),
                installed("com.example.blank", "   "),
            ],
            &dir,
        );
        assert_eq!(games.len(), 1);
        let game = &games[0];
        assert_eq!(game.id, "android:com.mojang.minecraftpe");
        assert_eq!(game.source, GameSource::Android);
        assert_eq!(
            game.launch_target,
            LaunchTarget::Provider {
                provider: "android".into(),
                app_ref: "com.mojang.minecraftpe".into()
            }
        );
        // Only a picture that exists in the media folder is kept.
        assert_eq!(game.cover_path, Some(dir.join("android-com.mojang.minecraftpe-cover.jpg")));
        assert_eq!(game.artwork_path, None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn follows_the_phone_and_keeps_what_the_player_made_of_a_card() {
        let dir = media("apply");
        let mut catalog = Catalog::default();
        assert!(apply(
            &mut catalog,
            games_from(
                &[installed("com.mojang.minecraftpe", "Minecraft"), installed("com.supercell.clashroyale", "Clash Royale")],
                &dir
            )
        ));
        assert_eq!(catalog.games.len(), 2);
        catalog.games[0].play_time_seconds = 3600;
        catalog.games[0].hidden = true;
        let kept_id = catalog.games[0].id.clone();

        // Nothing moved: nothing to save.
        let again = games_from(
            &[installed("com.mojang.minecraftpe", "Minecraft"), installed("com.supercell.clashroyale", "Clash Royale")],
            &dir,
        );
        assert!(!apply(&mut catalog, again));

        // One game uninstalled, and the other renamed by an update.
        let renamed = games_from(&[installed("com.mojang.minecraftpe", "Minecraft: Bedrock")], &dir);
        let mut kept = catalog.clone();
        assert!(apply(&mut kept, renamed));
        assert_eq!(kept.games.len(), 1);
        assert_eq!(kept.games[0].id, kept_id);
        assert_eq!(kept.games[0].title, "Minecraft: Bedrock");
        assert_eq!(kept.games[0].play_time_seconds, 3600);
        assert!(kept.games[0].hidden);
        kept.validate().unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn knows_a_package_name_when_it_sees_one() {
        assert!(valid_package_name("com.mojang.minecraftpe"));
        assert!(valid_package_name("com.YoStarEN.Arknights_2"));
        for bad in ["", "minecraft", "com..mojang", "com.mojang/../x", "com.moj ang", ".com"] {
            assert!(!valid_package_name(bad), "{bad}");
        }
    }
}
