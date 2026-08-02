# Changelog

## [Unreleased]

### Changed

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
