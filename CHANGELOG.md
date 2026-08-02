# Changelog

## [Unreleased]

### Added

- New "Me" page (`#/me`) with a cognitive scan: engagement, regularity, genre diversity, intensity, and balance metrics computed from the library, plus a generated player profile summary.
- "Most Played" row at the top of the library, sorted by play time (games without play time excluded).
- Instant Gaming price display on store cards: price badge, strikethrough original price, and discount pill; games without pricing show nothing.
- Larger real source logo (Steam or Local) on the game detail page.

### Changed

- The top-left library menu now shows a "Sources" section listing connected sources (Steam, Local) with an "Add a new source" entry, replacing the two duplicate import/connect buttons.
- Game detail no longer shows "wine-staging", "incompatible", or "installed" badges (the Play button already conveys this), and the bookmark button is gone.
- Genre pills never wrap; long game titles clamp cleanly instead of breaking mid-word (detail page and store cards).
- Play time is hidden entirely when a game has none.

### Fixed

- Locally added games no longer fall back to Elden Ring artwork; media is searched by title with a neutral placeholder fallback.
- Hozy Playtest now has a cover.

### Changed (Wine)

- Run every local Windows `.exe` through Wine-Staging automatically: importing, launching, or reopening the library now associates each `.exe` with a managed default Wine profile, so there is no manual "add a game via Wine" step. The original local record is kept and reappears if the managed profile is removed.
- Default to DXVK-macOS on Apple Silicon Macs so Windows games use the Metal graphics path out of the box, without enabling it on every profile. The pinned DXVK runtime is still downloaded, hash-verified, and copied only into Orivo's private prefix, and Wine 3D remains available as an optional override.

### Removed

- The manual Wine setup wizard and the "attach this game to a Wine profile" flow from the interface; Windows games are handled automatically instead.

## [0.3.0] - 2026-07-19

### Added

- Connect a personal Steam library directly from Orivo, then sync owned games whether or not they are installed locally.
- Show Steam-provided descriptions, genres, native Windows, macOS, and Linux support, plus the match with the current machine.
- Install eligible owned games through Steam from the library view.

### Changed

- Use distinct official Steam artwork for the hero, landscape card, and vertical cover.
- Keep Steam credentials in the macOS Keychain with stable development signing and recover gracefully from legacy inaccessible entries.
