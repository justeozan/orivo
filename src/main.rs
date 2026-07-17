#[allow(dead_code)]
mod catalog;
#[allow(dead_code)]
mod launcher;

slint::include_modules!();

use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
};

use image::{DynamicImage, GenericImageView, Rgba, RgbaImage};

// The glass samples this fixed logical scene at one quarter resolution. Scaling
// the pre-filtered buffer back to the viewport produces a strong backdrop blur
// while keeping each cached game under 400 KiB.
const GLASS_BACKDROP_WIDTH: u32 = 384;
const GLASS_BACKDROP_HEIGHT: u32 = 256;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    slint::BackendSelector::new()
        .require_wgpu_29(slint::wgpu_29::WGPUConfiguration::default())
        .select()?;

    let selector = OrivoSelector::new()?;
    let avatar_path = bundled_asset("assets/profile/steam-avatar.png");
    set_image_if_available(
        &selector,
        Some(&avatar_path),
        OrivoSelector::set_avatar_image,
    );

    let catalog_path = catalog::default_path();
    let stored_catalog = match catalog::Catalog::load(&catalog_path) {
        Ok(catalog) => catalog,
        Err(catalog::CatalogError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            catalog::Catalog::default()
        }
        Err(error) => {
            eprintln!("Could not load catalog: {error}");
            catalog::Catalog::default()
        }
    };
    // Keep the persisted library separate from the visual rail. The supplied
    // reference is a curated ten-game carousel; a local catalog should enrich
    // it, not silently replace it with its (often much shorter) contents.
    let catalog = Rc::new(RefCell::new(stored_catalog));
    let selector_catalog = Rc::new(RefCell::new(presentation_catalog(&catalog.borrow())));
    let media_cache = Rc::new(RefCell::new(MediaCache::default()));
    let selected_index = Rc::new(RefCell::new(0usize));

    {
        let games = selector_catalog.borrow().games.clone();
        let mut media = media_cache.borrow_mut();
        // Keep hero and cover handles alive before interaction begins. Selecting
        // a card then only swaps cached Slint images; it performs no software
        // image conversion or blur work on the UI thread.
        media.preload_games(&games);
        set_rail_media(&selector, &games, &mut media);
        if let Some(game) = games.first() {
            present_game(&selector, game, 0, games.len(), &mut media);
        }
    }

    let import_catalog = Rc::clone(&catalog);
    let import_selector_catalog = Rc::clone(&selector_catalog);
    let import_media_cache = Rc::clone(&media_cache);
    let import_index = Rc::clone(&selected_index);
    let import_path = catalog_path.clone();
    let import_selector = selector.as_weak();
    selector.on_import_game(move || {
        let Some(executable) = rfd::FileDialog::new()
            .set_title("Import a local game executable")
            .pick_file()
        else {
            return;
        };

        let result = catalog::Game::from_executable(executable).and_then(|game| {
            let game_id = game.id.clone();
            import_catalog.borrow_mut().add(game)?;
            import_catalog.borrow().save_atomically(&import_path)?;
            Ok(game_id)
        });

        if let Some(selector) = import_selector.upgrade() {
            match result {
                Ok(imported_id) => {
                    let games = {
                        let saved_catalog = import_catalog.borrow();
                        *import_selector_catalog.borrow_mut() =
                            presentation_catalog(&saved_catalog);
                        import_selector_catalog.borrow().games.clone()
                    };
                    if let Some(index) = games.iter().position(|game| game.id == imported_id) {
                        *import_index.borrow_mut() = index;
                        let mut media = import_media_cache.borrow_mut();
                        media.preload_game(&games[index]);
                        set_rail_media(&selector, &games, &mut media);
                        present_game(&selector, &games[index], index, games.len(), &mut media);
                    }
                }
                Err(error) => selector.set_status_text(format!("Import failed: {error}").into()),
            }
        }
    });

    let play_catalog = Rc::clone(&selector_catalog);
    let play_selector = selector.as_weak();
    let previous_catalog = Rc::clone(&selector_catalog);
    let previous_media_cache = Rc::clone(&media_cache);
    let previous_index = Rc::clone(&selected_index);
    let previous_selector = selector.as_weak();
    selector.on_previous_game(move || {
        let games = previous_catalog.borrow();
        if games.games.is_empty() {
            return;
        }
        let mut index = previous_index.borrow_mut();
        *index = if *index == 0 {
            games.games.len() - 1
        } else {
            *index - 1
        };
        if let Some(selector) = previous_selector.upgrade() {
            present_game(
                &selector,
                &games.games[*index],
                *index,
                games.games.len(),
                &mut previous_media_cache.borrow_mut(),
            );
        }
    });

    let next_catalog = Rc::clone(&selector_catalog);
    let next_media_cache = Rc::clone(&media_cache);
    let next_index = Rc::clone(&selected_index);
    let next_selector = selector.as_weak();
    selector.on_next_game(move || {
        let games = next_catalog.borrow();
        if games.games.is_empty() {
            return;
        }
        let mut index = next_index.borrow_mut();
        *index = (*index + 1) % games.games.len();
        if let Some(selector) = next_selector.upgrade() {
            present_game(
                &selector,
                &games.games[*index],
                *index,
                games.games.len(),
                &mut next_media_cache.borrow_mut(),
            );
        }
    });

    let select_catalog = Rc::clone(&selector_catalog);
    let select_media_cache = Rc::clone(&media_cache);
    let select_index = Rc::clone(&selected_index);
    let select_selector = selector.as_weak();
    selector.on_select_game(move |requested_index| {
        let games = select_catalog.borrow();
        let Some(index) = usize::try_from(requested_index)
            .ok()
            .filter(|index| *index < games.games.len())
        else {
            return;
        };
        *select_index.borrow_mut() = index;
        if let Some(selector) = select_selector.upgrade() {
            present_game(
                &selector,
                &games.games[index],
                index,
                games.games.len(),
                &mut select_media_cache.borrow_mut(),
            );
        }
    });

    let play_index = Rc::clone(&selected_index);
    selector.on_play_game(move || {
        let games = play_catalog.borrow();
        let Some(game) = games.games.get(*play_index.borrow()).cloned() else {
            if let Some(selector) = play_selector.upgrade() {
                selector.set_status_text("Import a game first".into());
            }
            return;
        };

        if game.id.starts_with("showcase-") {
            if let Some(selector) = play_selector.upgrade() {
                selector.set_status_text("Visual showcase — import a local game to play".into());
            }
            return;
        }

        let status = match launcher::launch(&game) {
            Ok(_) => "Launching game".to_string(),
            Err(error) => format!("Launch failed: {error}"),
        };
        if let Some(selector) = play_selector.upgrade() {
            selector.set_status_text(status.into());
        }
    });

    selector.window().set_fullscreen(true);
    selector.run()?;

    Ok(())
}

fn bundled_asset(relative_path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative_path)
}

/// The reference rail deliberately contains these ten IGDB-backed games. They
/// remain available as a visual layer even when the user has a smaller local
/// catalog, while matching local entries retain their executable metadata.
fn showcase_catalog() -> catalog::Catalog {
    catalog::Catalog {
        schema_version: catalog::CURRENT_SCHEMA_VERSION,
        games: vec![
            showcase_game(
                "elden-ring",
                "Elden Ring",
                "A vast world full of mystery and peril. What will you discover?",
                "Achievements 67/82",
                "2 days ago",
                128,
                "elden-ring.jpg",
                Some("elden-ring.jpg"),
            ),
            showcase_game(
                "cyberpunk-2077",
                "Cyberpunk 2077",
                "Night City is yours for the taking. Choose your legend.",
                "Achievements 41/57",
                "5 days ago",
                85,
                "cyberpunk-2077.jpg",
                Some("cyberpunk-2077.webp"),
            ),
            showcase_game(
                "baldurs-gate-3",
                "Baldur's Gate 3",
                "Gather your party and return to the Forgotten Realms.",
                "Achievements 39/54",
                "1 week ago",
                97,
                "baldurs-gate-3.jpg",
                Some("baldurs-gate-3.jpg"),
            ),
            showcase_game(
                "hades-2",
                "Hades II",
                "Defy the Titan of Time beneath a moonlit underworld.",
                "Achievements 26/50",
                "1 week ago",
                51,
                "hades-2.jpg",
                Some("hades-2.jpg"),
            ),
            showcase_game(
                "red-dead-redemption-2",
                "Red Dead Redemption 2",
                "Outlaws for life in a fading American frontier.",
                "Achievements 35/51",
                "2 weeks ago",
                110,
                "red-dead-redemption-2.jpg",
                Some("red-dead-redemption-2.jpg"),
            ),
            showcase_game(
                "the-witcher-3",
                "The Witcher 3",
                "Track monsters and chase the Wild Hunt across the Continent.",
                "Achievements 56/78",
                "3 weeks ago",
                200,
                "the-witcher-3-wild-hunt.jpg",
                Some("the-witcher-3-wild-hunt.jpg"),
            ),
            showcase_game(
                "horizon-forbidden-west",
                "Horizon Forbidden West",
                "Explore a vibrant frontier ruled by colossal machines.",
                "Achievements 33/59",
                "1 month ago",
                68,
                "horizon-forbidden-west.jpg",
                Some("horizon-forbidden-west.jpg"),
            ),
            showcase_game(
                "god-of-war",
                "God of War",
                "A deeply personal journey through the Norse realms.",
                "Achievements 28/37",
                "1 month ago",
                120,
                "god-of-war.jpg",
                Some("god-of-war.jpg"),
            ),
            // The supplied Selector crop visibly shows eight unique games. The
            // last two complete the demo library with games already present in
            // this project; they are not represented as visible in that crop.
            showcase_game(
                "unrailed",
                "Unrailed!",
                "Build a railway together before the runaway train reaches the end.",
                "Local co-op • Railway 18",
                "2 months ago",
                24,
                "unrailed.jpg",
                Some("unrailed.jpg"),
            ),
            showcase_game(
                "astro-duel-2",
                "Astro Duel 2",
                "A high-speed space battle where pilots jump between ship and body.",
                "Versus • Campaign",
                "2 months ago",
                12,
                "astro-duel-2.jpg",
                Some("astro-duel-2.jpg"),
            ),
        ],
        extra: BTreeMap::new(),
    }
}

/// Build the visible carousel without ever persisting bundled sample records.
/// A matching local game keeps its id and launch configuration, but takes the
/// curated IGDB artwork used by the reference composition.
fn presentation_catalog(stored_catalog: &catalog::Catalog) -> catalog::Catalog {
    let mut presentation = showcase_catalog();

    for local_game in &stored_catalog.games {
        if let Some(showcase_game) = presentation
            .games
            .iter_mut()
            .find(|showcase_game| game_key(&showcase_game.title) == game_key(&local_game.title))
        {
            let mut merged = local_game.clone();
            merged.description = local_game
                .description
                .clone()
                .or_else(|| showcase_game.description.clone());
            merged.metadata = local_game
                .metadata
                .clone()
                .or_else(|| showcase_game.metadata.clone());
            merged.artwork_path = showcase_game.artwork_path.clone();
            merged.cover_path = showcase_game.cover_path.clone();
            if merged.last_played_at.is_none() {
                merged.last_played_at = showcase_game.last_played_at.clone();
            }
            if merged.play_time_seconds == 0 {
                merged.play_time_seconds = showcase_game.play_time_seconds;
            }
            *showcase_game = merged;
        } else {
            presentation.games.push(local_game.clone());
        }
    }

    presentation
}

fn game_key(title: &str) -> String {
    title
        .chars()
        .filter(|character| character.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn showcase_game(
    id: &str,
    title: &str,
    description: &str,
    metadata: &str,
    last_played_at: &str,
    hours: u64,
    cover_file: &str,
    hero_file: Option<&str>,
) -> catalog::Game {
    let cover_path = bundled_asset("assets/igdb/covers").join(cover_file);
    let artwork_path = hero_file
        .map(|file| bundled_asset("assets/igdb/heroes").join(file))
        .or_else(|| Some(cover_path.clone()));

    catalog::Game {
        id: format!("showcase-{id}"),
        title: title.into(),
        executable_path: PathBuf::from(format!("showcase://{id}")),
        working_directory: None,
        arguments: Vec::new(),
        description: Some(description.into()),
        metadata: Some(metadata.into()),
        artwork_path,
        cover_path: Some(cover_path),
        logo_path: None,
        hero_video_path: None,
        last_played_at: Some(last_played_at.into()),
        play_time_seconds: hours * 3_600,
        extra: BTreeMap::new(),
    }
}

fn present_game(
    selector: &OrivoSelector,
    game: &catalog::Game,
    index: usize,
    total: usize,
    media_cache: &mut MediaCache,
) {
    selector.set_selected_title(game.title.clone().into());
    selector.set_selected_description(
        game.description
            .clone()
            .unwrap_or_else(|| "Ready for your next session.".into())
            .into(),
    );
    selector.set_status_text("Ready to play".into());
    selector.set_selected_play_time(format_play_time(game.play_time_seconds).into());
    selector.set_selected_last_played(
        game.last_played_at
            .as_deref()
            .map(|last_played| format!("Last played {last_played}"))
            .unwrap_or_else(|| "Not played yet".into())
            .into(),
    );
    selector.set_selected_progress(
        game.metadata
            .clone()
            .unwrap_or_else(|| "Ready to play".into())
            .into(),
    );
    selector.set_rail_position(format!("{} / {}", index + 1, total).into());
    selector.set_selected_index(index as i32);
    // Keep a five-card look-ahead so the active card never drifts beyond the
    // visible rail when a longer showcase or a real library is navigated.
    selector.set_rail_first_index(index.saturating_sub(5) as i32);
    set_game_media(selector, game, media_cache);
}

fn format_play_time(play_time_seconds: u64) -> String {
    let hours = play_time_seconds / 3_600;
    if hours > 0 {
        format!("{hours}h played")
    } else {
        let minutes = play_time_seconds / 60;
        if minutes > 0 {
            format!("{minutes}m played")
        } else {
            "Ready to play".into()
        }
    }
}

fn set_rail_media(selector: &OrivoSelector, games: &[catalog::Game], media_cache: &mut MediaCache) {
    let covers = games
        .iter()
        .map(|game| {
            game.cover_path
                .as_deref()
                .or(game.artwork_path.as_deref())
                .and_then(|path| media_cache.image_for(path).ok())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    let titles = games
        .iter()
        .map(|game| slint::SharedString::from(game.title.as_str()))
        .collect::<Vec<_>>();
    let metadata = games
        .iter()
        .map(|game| slint::SharedString::from(format_play_time(game.play_time_seconds)))
        .collect::<Vec<_>>();
    selector.set_rail_covers(slint::ModelRc::new(slint::VecModel::from(covers)));
    selector.set_rail_titles(slint::ModelRc::new(slint::VecModel::from(titles)));
    selector.set_rail_metadata(slint::ModelRc::new(slint::VecModel::from(metadata)));
}

fn set_game_media(selector: &OrivoSelector, game: &catalog::Game, media_cache: &mut MediaCache) {
    let Some(path) = game.artwork_path.as_deref().or(game.cover_path.as_deref()) else {
        selector.set_hero_image(slint::Image::default());
        selector.set_glass_backdrop_image(slint::Image::default());
        return;
    };

    match media_cache.image_for(path) {
        Ok(image) => selector.set_hero_image(image),
        Err(error) => {
            eprintln!("Could not load hero artwork {}: {error}", path.display());
            selector.set_hero_image(slint::Image::default());
        }
    }

    match media_cache.backdrop_for(path) {
        Ok(backdrop) => selector.set_glass_backdrop_image(backdrop),
        Err(error) => {
            eprintln!("Could not build glass backdrop {}: {error}", path.display());
            selector.set_glass_backdrop_image(slint::Image::default());
        }
    }
}

/// Own the media handles and low-resolution pre-composited backdrops for the
/// selector's lifetime. The backdrop cache is what makes every glass surface
/// sample the same blurred scene without recomputing anything on a card click.
#[derive(Default)]
struct MediaCache {
    images: HashMap<PathBuf, slint::Image>,
    backdrops: HashMap<PathBuf, slint::Image>,
}

impl MediaCache {
    fn preload_games(&mut self, games: &[catalog::Game]) {
        for game in games {
            self.preload_game(game);
        }
    }

    fn preload_game(&mut self, game: &catalog::Game) {
        if let Some(path) = game.artwork_path.as_deref().or(game.cover_path.as_deref())
            && let Err(error) = self.image_for(path)
        {
            eprintln!("Could not preload hero artwork {}: {error}", path.display());
        }
        if let Some(path) = game.artwork_path.as_deref().or(game.cover_path.as_deref())
            && let Err(error) = self.backdrop_for(path)
        {
            eprintln!(
                "Could not preload glass backdrop {}: {error}",
                path.display()
            );
        }
        if let Some(path) = game.cover_path.as_deref().or(game.artwork_path.as_deref())
            && let Err(error) = self.image_for(path)
        {
            eprintln!(
                "Could not preload cover artwork {}: {error}",
                path.display()
            );
        }
    }

    fn image_for(&mut self, path: &Path) -> Result<slint::Image, Box<dyn std::error::Error>> {
        if let Some(image) = self.images.get(path) {
            return Ok(image.clone());
        }

        let image = load_artwork(path)?;
        self.images.insert(path.to_path_buf(), image.clone());
        Ok(image)
    }

    fn backdrop_for(&mut self, path: &Path) -> Result<slint::Image, Box<dyn std::error::Error>> {
        if let Some(backdrop) = self.backdrops.get(path) {
            return Ok(backdrop.clone());
        }

        let backdrop = load_glass_backdrop(path)?;
        self.backdrops.insert(path.to_path_buf(), backdrop.clone());
        Ok(backdrop)
    }
}

fn set_image_if_available(
    selector: &OrivoSelector,
    path: Option<&std::path::PathBuf>,
    setter: fn(&OrivoSelector, slint::Image),
) {
    let Some(path) = path else {
        setter(selector, slint::Image::default());
        return;
    };
    match load_artwork(path) {
        Ok(image) => setter(selector, image),
        Err(error) => {
            eprintln!("Could not load artwork {}: {error}", path.display());
            setter(selector, slint::Image::default());
        }
    }
}

fn load_artwork(path: &Path) -> Result<slint::Image, Box<dyn std::error::Error>> {
    Ok(slint::Image::load_from_path(path)
        .map_err(|_| std::io::Error::other(format!("Slint could not decode {}", path.display())))?)
}

/// Render the hero to the same aspect ratio as the Slint Window, blur it once,
/// and retain it at a quarter-resolution. FrostedGlass then uses the exact
/// viewport offset of each control, which is the key difference from a
/// re-centered thumbnail in every button.
fn load_glass_backdrop(path: &Path) -> Result<slint::Image, Box<dyn std::error::Error>> {
    let artwork = image::open(path)?;
    let viewport = cover_artwork(&artwork, GLASS_BACKDROP_WIDTH, GLASS_BACKDROP_HEIGHT);
    let mut blurred = DynamicImage::ImageRgba8(viewport).blur(3.6).to_rgba8();
    apply_scene_tones(&mut blurred);
    Ok(slint_image_from_rgba(blurred))
}

fn cover_artwork(artwork: &DynamicImage, width: u32, height: u32) -> RgbaImage {
    let (source_width, source_height) = artwork.dimensions();
    let scale = (width as f64 / source_width as f64).max(height as f64 / source_height as f64);
    let scaled_width = ((source_width as f64 * scale).ceil() as u32).max(width);
    let scaled_height = ((source_height as f64 * scale).ceil() as u32).max(height);
    let scaled = artwork.resize_exact(
        scaled_width,
        scaled_height,
        image::imageops::FilterType::Lanczos3,
    );
    let rgba = scaled.to_rgba8();
    image::imageops::crop_imm(
        &rgba,
        (scaled_width - width) / 2,
        (scaled_height - height) / 2,
        width,
        height,
    )
    .to_image()
}

/// Match the selector's broad hero readability overlays before the buffer is
/// displayed under a glass surface. The gradients remain smooth at this scale,
/// while the artwork itself is strongly defocused.
fn apply_scene_tones(backdrop: &mut RgbaImage) {
    let width = backdrop.width().saturating_sub(1).max(1) as f32;
    let height = backdrop.height().saturating_sub(1).max(1) as f32;

    for (x, y, pixel) in backdrop.enumerate_pixels_mut() {
        let x = x as f32 / width;
        let y = y as f32 / height;

        // The hero itself is shown at 84% opacity over the selector base.
        blend_pixel(pixel, [11, 11, 13], 0.16);
        // Left readability falloff: opaque on the title side, transparent at 74%.
        blend_pixel(pixel, [9, 10, 14], (1.0 - x / 0.74).clamp(0.0, 1.0) * 0.91);
        // The broad bottom fade from the reference, with a very light top fade.
        blend_pixel(pixel, [8, 9, 13], vertical_overlay_alpha(y));
        blend_pixel(pixel, [9, 10, 14], (1.0 - y / 0.115).clamp(0.0, 1.0) * 0.55);
    }
}

fn vertical_overlay_alpha(y: f32) -> f32 {
    if y <= 0.51 {
        0.27 * (1.0 - y / 0.51)
    } else if y <= 0.76 {
        0.42 * (y - 0.51) / 0.25
    } else {
        0.42 + 0.49 * (y - 0.76) / 0.24
    }
}

fn blend_pixel(pixel: &mut Rgba<u8>, overlay: [u8; 3], alpha: f32) {
    let alpha = alpha.clamp(0.0, 1.0);
    for (channel, overlay_channel) in pixel.0[..3].iter_mut().zip(overlay) {
        *channel = (*channel as f32 * (1.0 - alpha) + overlay_channel as f32 * alpha).round() as u8;
    }
}

fn slint_image_from_rgba(rgba: RgbaImage) -> slint::Image {
    let (width, height) = rgba.dimensions();
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
    pixels.make_mut_bytes().copy_from_slice(rgba.as_raw());
    slint::Image::from_rgba8(pixels)
}

#[cfg(test)]
mod visual_snapshot_tests {
    use super::*;

    #[test]
    fn bundled_showcase_contains_ten_loadable_igdb_games() {
        let showcase = showcase_catalog();
        assert_eq!(showcase.games.len(), 10);

        let mut media_cache = MediaCache::default();
        media_cache.preload_games(&showcase.games);
        assert_eq!(media_cache.images.len(), 20);
        assert_eq!(media_cache.backdrops.len(), 10);
    }

    #[test]
    fn presentation_catalog_keeps_the_ten_game_reference_rail() {
        let mut local_catalog = catalog::Catalog::default();
        let mut local_unrailed = showcase_catalog().games[8].clone();
        local_unrailed.id = "local-unrailed".into();
        local_unrailed.executable_path = PathBuf::from("/Applications/Unrailed.app");
        local_catalog.games.push(local_unrailed);

        let presentation = presentation_catalog(&local_catalog);
        assert_eq!(presentation.games.len(), 10);
        assert_eq!(presentation.games[8].id, "local-unrailed");
        assert!(
            presentation.games[8]
                .artwork_path
                .as_ref()
                .is_some_and(|path| path.ends_with("assets/igdb/heroes/unrailed.jpg"))
        );
    }

    #[test]
    fn renders_selector_snapshot_for_visual_review() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                ..Default::default()
            },
        )))
        .expect("testing backend should initialize before any window exists");

        let selector = OrivoSelector::new().expect("selector should render headlessly");
        let avatar_path = bundled_asset("assets/profile/steam-avatar.png");
        set_image_if_available(
            &selector,
            Some(&avatar_path),
            OrivoSelector::set_avatar_image,
        );
        let showcase = showcase_catalog();
        assert_eq!(showcase.games.len(), 10);
        let rail_games = showcase.games;
        let mut media_cache = MediaCache::default();
        media_cache.preload_games(&rail_games);
        set_rail_media(&selector, &rail_games, &mut media_cache);
        present_game(
            &selector,
            &rail_games[0],
            0,
            rail_games.len(),
            &mut media_cache,
        );
        selector
            .window()
            .set_size(slint::PhysicalSize::new(1536, 1024));

        let snapshot = selector
            .window()
            .take_snapshot()
            .expect("headless renderer should produce a snapshot");
        let rgba = image::RgbaImage::from_raw(
            snapshot.width(),
            snapshot.height(),
            snapshot.as_bytes().to_vec(),
        )
        .expect("snapshot should have a valid RGBA buffer");
        rgba.save(".context/orivo-selector-snapshot.png")
            .expect("snapshot should be writable for visual review");

        present_game(
            &selector,
            rail_games.last().expect("showcase should not be empty"),
            rail_games.len() - 1,
            rail_games.len(),
            &mut media_cache,
        );
        assert_eq!(selector.get_rail_first_index(), 4);
    }
}
