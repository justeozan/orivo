# Changelog

## [Unreleased]

### Added

- Connect six more game libraries from Settings › Libraries: Epic Games, GOG, Ubisoft Connect, Xbox, Microsoft Store and Instant Gaming. Each signs in through that store's own window, and the games you own appear in your library with their store artwork. Every connected store also appears in the top-left Sources menu, where selecting it syncs it.
- Epic, GOG and Microsoft keep an encrypted connection in the system keychain and sync in the background afterwards. Ubisoft Connect and Instant Gaming publish no account API, so their sign-in window stays signed in and each sync runs inside it — no long-lived credential for those two ever leaves the window.
- Xbox and Microsoft Store share one Microsoft sign-in: connecting either connects both, Xbox lists what you have played on a console and Microsoft Store lists the PC side. Settings says so under the pair.
- Epic, GOG, Ubisoft Connect and Microsoft Store games launch through that store's own client when it is installed on this machine; when it is not, the game still stays in your library and the Play button says which app is missing rather than failing. Xbox console entitlements and Instant Gaming keys are records of what you own and never pretend to launch.
- Disconnecting a store asks whether to keep the games it already imported.
- Games synced from a store that publishes no usable artwork — Xbox and Microsoft Store in particular — now get a real portrait cover, landscape cover and background, resolved from Steam's official artwork by title during the sync.
- "Reset the covers" (in a game's ⋯ menu, replacing "Search cover & images") refills all three formats at once from a reliable high-resolution source, instead of downloading one image and stretching it across the lot. It names any format it could not find rather than reporting a clean result.
- Optional SteamGridDB API key in Settings › Plugins. With a key, a cover reset pulls 4K artwork for all three formats; without one it uses Steam's official art (1200×1800 portrait, 1920×620 hero).
- Full controller and keyboard navigation on every page. The arrow keys or the d-pad move between whatever is on screen; `a` (A on a pad) opens a game's page and Enter (X on a pad) launches it straight away. B or Escape goes back, Y jumps to the search field, and the shoulder buttons walk the top-level pages. Holding a direction repeats.
- The "Me" page can now be reached and read without a mouse: its metric cards and profile stats take focus, and the page remembers where you were when you come back to it.
- New "Me" page (`#/me`) with a cognitive scan: engagement, regularity, genre diversity, intensity, and balance metrics computed from the library, plus a generated player profile summary.
- "Most Played" row at the top of the library, sorted by play time (games without play time excluded).
- Instant Gaming price display on store cards: price badge, strikethrough original price, and discount pill; games without pricing show nothing.
- Larger real source logo (Steam or Local) on the game detail page.

### Changed

- Opening Settings › Libraries no longer asks for the macOS keychain password. Which stores are connected is now recorded outside the keychain, so only an operation that genuinely needs a token — a library sync — ever opens it, and then at most once per launch.
- "Provider status" and "Other game libraries" are one card: each store shows its connection and its price-data health on the same row, and shops with no library to connect are listed separately beneath.
- Each store is shown in its own brand colours in Settings and as a white mark in the library, the hero badge and the game page. The Epic mark is an outlined shield with a solid "E" in white, where the filled version turned into a blob at badge size.
- Settings › Libraries reads more calmly: the tinted plates behind the store logos are gone, the logos are larger, every row's text starts at the same place, and a store's optional price-feed state is a small dot beside its name instead of a red "Unavailable" pill on every row. A connected store shows its account instead of repeating the pitch for connecting it, and the GOG mark is a legible "G" where the full wordmark collapsed into an unreadable "20".
- The top-left library menu now shows a "Sources" section listing connected sources (Steam, Local) with an "Add a new source" entry, replacing the two duplicate import/connect buttons.
- Game detail no longer shows "wine-staging", "incompatible", or "installed" badges (the Play button already conveys this), and the bookmark button is gone.
- Genre pills never wrap; long game titles clamp cleanly instead of breaking mid-word (detail page and store cards).
- Play time is hidden entirely when a game has none.

### Fixed

- Games showed another game's cover — usually Elden Ring's — and only reverted to their own after being opened. Cached artwork arrives as an opaque token that cannot be resolved on the spot, and the library filled the gap from the first bundled fixture. A game now never borrows another's artwork, the cache is resolved before the first paint, and every rendered card is hydrated instead of only the first sixteen.
- The Ubisoft Connect window opened the marketing site instead of the sign-in form.
- The Instant Gaming window opened a page that no longer exists (404). Its order history is now discovered from the account itself rather than assumed, and Orivo refuses to read any page it cannot confirm is an order history — the shop front is wall-to-wall product links and would otherwise be imported as purchases.
- Locally added games no longer fall back to Elden Ring artwork; media is searched by title with a neutral placeholder fallback.
- Hozy Playtest now has a cover.
- Store cards no longer print an empty price frame when no shop has quoted a price, which read as "free".
- The cheapest offer is now picked consistently: a stale quote could beat a freshly verified one, and the winner could change depending on the order the shops answered in.
- Store filters apply again outside the desktop app, where browsing previously returned the whole catalogue whatever was selected.
- Games tagged "Stories" land in the "Récits forts" category again.
- The search field on the Store says "Search the store…" instead of the library's wording.

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
