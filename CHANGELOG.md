# Changelog

## [Unreleased]

### Added

- The Store shows what has just come out. Every refresh reads Steam's own ranked new-release listing — `filter=popularnew`, ordered by the audience a release found rather than by the hour it was published, because the unranked feed for any given week opens on three asset flips — and adds the games the catalogue does not already carry. A game Orivo has written about is never pulled in twice and never has its own copy overwritten by what a storefront says about it. Announcements, downloadable content, and listings their own publisher flagged as adult sexual content are left where they are. What arrives brings its price, its genres, how it is played and the platforms it runs on, and the chips file it from those alone — it has no editorial entry, so it shows no scores, no verdict and no reason: the card leaves those slots empty rather than filling them. A listing Steam has itself mis-sorted cannot slip through either: a release the shelf can date and that came out more than ninety days ago is refused, while a date in a form Orivo cannot read — a year on its own, a quarter — leaves the listing's own ordering to speak for the game rather than failing it on a parse. Its artwork is the 616×353 capsule the card is cut for when the listing is old enough to have one, and the listing's own header — the same picture, 18% narrower — when it is not. The block underneath still takes its colour from whichever arrived, read through a second request in CORS mode so a host that declines costs the block its colour and never the card its picture.

### Removed

- Importing installed Steam games by walking local manifests. The scan itself stays, because it is what tells a synced library which owned games are actually on disk, but nothing offers to walk it any more: connecting a Steam library, syncing it, and importing a single game from disk are the three ways a game gets in now, and the preview that showed what the scan had found went with them.

### Changed

- The Store's shelf is what the shops have just released. The forty-seven games Orivo wrote by hand were written to stand in for a catalogue that did not exist yet; with one in place they were forty-seven cards standing in front of everything that actually came out this week. They step aside as soon as there is anything live to show, and *Appearance › Beta & debug* puts them back. With nothing live — a refresh that has not run, or one that never reached Steam — they are the shelf again, because an empty Store is worse than a dated one. The cards are all one height again as a result: a game a shop sent has no scores to print, and a shelf of them is a shelf of the shorter card.
- The arrow keys and the controller read every page as rows. Up and down go from row to row — on the Library that is the topbar, Play, the games and the browse bar — and land where the row already points: the game on screen, the page you are on in the topbar, the open section's tab, the control you last left there. Left and right move along the row focus is on, and on the Library they change the game, from the rail and from Play alike; Play keeps focus while the game changes under it. The Settings column is walked with up and down and climbs out of its top to the topbar, a search field, a switch or a select no longer swallows the arrows, and B closes an open menu or the notifications before it goes back a page. The scene's previous and next arrows stay for the mouse; the keys already are left and right, so they walk past them.
- No focus ring anywhere. A control the keys or a controller land on takes the look the pointer gives it on hover instead — Play rises and glows, a topbar entry lights its plate, a browse segment gets a pill of light — and none of it ever shows for the mouse.
- The settings entrance is a breath. The section on screen lifts 4px instead of 6 and settles over 620ms on one decelerating curve, with the title and description arriving alongside it, and every block a panel renders now takes the same path — the choices, search field and forms that used to be sitting on screen before their own labels had finished arriving no longer are.
- Settings › Libraries' buttons are a single measure wide with 6px corners, the sidebar's active plate takes the same 6px, and a connected Steam account whose name outgrows the button ellipsizes instead of pushing the row wider. A store already signed in states it at rest: its button carries the account name in the blue the platform chips use, so the connected rows can be told from the ones still offering to connect without reading a word. Red under the pointer belongs to the one button that disconnects — that same blue one; offering to connect a store, or to refresh every library, lights up in white like every other control, and a button that has only one label keeps it rather than emptying itself the moment the pointer arrives.
- A store whose prices Orivo cannot read yet says "Soon" rather than "Unavailable" or "Not Configured". The two states differ only in why — one has no authorised feed configured, the other has no public feed to configure — and neither is a fault, nor anything the reader can act on, so a column of red down half the list was reporting errors that were not there. The feed's own sentence is still one pointer away on the row.
- The Store is the shelf and the photograph behind it. Three pieces of chrome are gone: the mindful reminder across the foot of the page, the "Moins de bruit. Plus de sens." label above the shelf, and the two-line standfirst under the headline — the title, the button and the panel of reasons already carry all three. What they were taking goes to the games. The card is wider, it is as tall as what it actually says rather than stretched to the height of the shelf and padded with nothing, and it sits at the foot of the band it is given, next to the chip bar, so the room that comes back opens above the shelf. The veil over the bottom of the page is what fills that room: it used to be solid black across the lower two fifths of the window — a banner, a label and a row of chips all had to be read off it — and two of those are gone while the chips carry their own glass. The photograph runs behind the shelf now. The five-dot scores come back with it: they were hidden on any window under 900px tall and the one-line verdict on any under 780px, which is most laptops open on a dock — both now hold down to a window barely half that, and what yields first, when it finally has to, is unchanged. The hero's button is Orivo's glass now, the same recipe as the topbar's search field, rather than a hairline with the photograph showing straight through it.
- A Store card has no edge between its picture and its facts any more. The block under the artwork carries on with the artwork: the same file, mirrored so the first row below the cut is the last row above it, out of focus, and gone again within forty pixels. Because both sides of the cut are the same pixels there is nothing to match and nothing to tune — measured over the shelf's first six capsules, the break in brightness at the join comes to under two levels of 255, against forty-one for the edge it replaces. The veil that darkens the foot of the artwork is carried into the reflection at the same depth and the same strength, because a card whose artwork is not a capsule wears a heavy one — without that, the reflection came back unveiled and laid a bright ridge along the very join it exists to remove. A card whose artwork never loaded has nothing to continue and shows none of this.
- The block of facts itself is the hero's panel of reasons: the same lifted blue-grey behind the same blur, so the shelf and the panel beside it read as one material rather than two, and the photograph carries on behind both. The pane is the whole card rather than the block alone: a pane that begins at the block has to arrive from nothing at the cut, and between it arriving and the reflection leaving there is a band a few pixels down where neither covers much — 46% of the page showing through at the worst row, which draws a bright line across every card once the page behind has a colour. Behind the artwork it costs nothing, the artwork being opaque. Its colour is the artwork's own. Each capsule is read once as the card is built — at 48px in a canvas, its colours sorted into coarse buckets and each bucket weighed by how much of the picture it is *and* how colourful it is, so a grey sky over half the frame cannot outvote the one thing in the picture that has a colour. Three numbers come back. The hue says which game it is. The saturation drops almost to nothing for artwork that has none of its own, because a black-and-white cover handed the panel's blue-grey reads cold against a warm page. And the lightness follows the foot of the picture — the part the block is printed directly under — over a narrow range, because a near-white cover standing on a dark slab is a hole cut in the card rather than a card. Reading the whole shelf costs about a millisecond. Against a pane that much lighter than the near-black it replaces, the genre, the scores and the verdict are all set brighter.
- Store cards show the game's own capsule — the art it was sold with, carrying its wordmark — instead of a screenshot of it being played. The card is cut to that art's exact shape, 616×353, so none of it is lost to a crop, and the width now rides the same viewport-height axis as every other step on the page: a shorter window gets a narrower card rather than a picture sliced to fit one that stayed wide. Under the art, the five-dot scores give way first on a short window and the one-line verdict second; the genre and the two chips never do. The card prints no title of its own any more, because the art already says the name — which is a rule the page always had, for the four games that happened to have this art before.
- The mood switch is out of the Library's browse bar: the segments and the Activity control are what it carries now.

### Fixed

- Signing in to a store with Google, Apple or Facebook did nothing at all. Those buttons open a popup, and the sign-in window refused every one of them, so the window simply never appeared and an account with no password of its own could not be reached. HTTPS popups are allowed now: a popup there belongs to the identity provider, and capabilities are granted by window label to `main` alone, so it can no more reach Orivo than the page that opened it can.
- Connecting Instant Gaming opened a 404, and signing in anyway imported nothing. The site has no sign-in page any more — `/en/login/` is gone, and signing in happens in a box on whatever page you are on — so the connector asks for the account itself, which is what opens that box when signed out and is where a re-sync wants to land when signed in. Its order history had moved too, to `/en/my-orders/`, which the guard that decides whether a page may be read was rejecting: `orders` sits behind a hyphen there, and the guard was looking for the bare word between slashes, so the real page was thrown away every time. It matches whole segments now, which is what keeps a product slug that merely contains the word from talking its way in. When no order history is found at all, the addresses that were tried are logged, because the store has moved this page more than once.
- The arrow keys only ever changed the game. With nothing focused — which is how the app starts, and where a click leaves it on a Mac, since WebKit never focuses a clicked button — every arrow was a shortcut for the previous or next game, so the keys never reached Play, the topbar or anything else on the page. With nothing focused they now start from the game on screen instead: left and right still change it, up reaches Play.
- Walking the rail waited until the selected game hit the edge of the shelf before scrolling, so the next game came into view only once you were already on the last one you could see. The shelf now moves a game earlier, in both directions and on both rails: whichever game you are on, the one you are heading for is already whole. It also used to stop short when stepping quickly, because the scroll was measured while the selected card was still growing; it settles first now. Scroll snapping is the hand's and no longer pulls the shelf back over that next game, until a wheel or a finger scrolls the rail again.
- Play lost its left and bottom edges when the keys lifted it: the hero clipped its overflow, and its box ends flush with the button. Nothing is clipped there now — the one thing that can overflow, a title too tall for the window, is already clipped a level in.
- A key held down left the rail behind. Every press started a fresh eased scroll, faster than one could run, so the shelf only ever played the first and slowest sliver of it: it crawled a few pixels a frame while the selection ran thousands of pixels ahead, and the walk looked frozen until the key was let go. The rail is followed now rather than launched — one animation that is re-aimed at whatever is focused, closing a share of the distance each frame, quicker the further behind it is. This is also what made the rendered window teleport: the shelf being that far behind put the card the window was pinned to outside the window it moved to.
- Walking a library longer than the rail renders looked like being stuck against an edge. The rail draws a 48-card window of a long list, and that window was centred on the selection, so every step slid it by one and handed all 48 cards the next game along: the selection stayed at the same card in the same place on a shelf that never scrolled, while the artwork churned underneath. The window holds still now and only moves when the selection comes within a dozen cards of its edge — further than a screenful, so the cards being swapped are always off screen — and the rail is scrolled back by as much as the window moved, which leaves the games on screen exactly where they were. Turning around mid-library walks the selection back across still cards, as it always did near the ends.

## [0.3.6] - 2026-08-28

### Changed

- The hero under a game's wordmark is one line instead of three: the genre, then the studio. The store badge is gone — the rail, the game's page and the Sources menu already say where a game came from — and so are the play time and last session, which belong to the game's own page. The pill carries the genre and nothing else: "Library" is what the backend returns when no store published one, so a game without a genre shows no pill rather than a word that says nothing.
- The studio has its own field now instead of riding in `metadata`, which is a mixed bag — a store fills it with the developer, Steam with install state, Wine with the runner name, the bundled demo with an achievement count. The hero used to print whichever of those arrived.
- "Windows only" is no longer a chip beside the hero. A game with no macOS build says so in the Play button, which greys out: the row used to announce the problem while the button underneath still offered Play or Install as though there were none. Only a store that actually answered decides this, and only on a Mac. The button names what the game does run on rather than assuming Windows, because GOG sells Linux-only titles and those have no macOS build either.

### Fixed

- The Play button moved on every selection. The hero stacked downwards from a fixed top edge, so the one control the whole scene exists for landed in a different place depending on what was above it — 64px between a one- and a two-line title, and far more when a game had a wordmark instead. The scene now reads upwards from the button: it holds still and the wordmark grows into whatever room is left, losing its top on a short window rather than pushing the button out of the frame. The launch status moved above the button, where the block above has room to give it, so a message appearing neither displaces the button that was just pressed nor paints over the rail. The whole block also sits higher: Play cleared the rail beneath it by 18px, and by 10px on a short window, which read as part of the shelf rather than as the scene's own action.
- A wordmark taller than the room the window leaves now scales down instead of losing its top.
- Browsing by `Platform` offered nothing but "All Games" for an Epic or GOG library. Every one of those games already knew what it ran on — it is the same answer that greys out the Play button — and the mode ignored it. GOG's platform matrix is now read from the product record the sync already fetches, Epic's Windows and Mac entitlement lists both reach the library, and the segment reads "Mac" rather than "Apple".
- The Play button's label never changed. It was written into the progress-bar overlay rather than the label beside the icon, so a button that should have read `Install`, `Downloading 37%` or `Unavailable` said `Play` in every state.
- A game with no genre no longer files itself under a "Library" shelf in the genre row, beside real genres, and the "Moi" page no longer reports "Library" as your dominant genre or counts it toward how eclectic your library is.
- A window between 801 and 900 pixels tall cropped the game's title: it fell between the tall-desktop layout and the first short-window step, so the whole shortfall was taken out of the title. The short-window step now starts at 900.

## [0.3.5] - 2026-08-23

### Added

- A welcome screen for a library that holds nothing yet. It stands in front of the same wallpaper the Library uses, introduces Orivo in three steps, and puts the only thing it is asking for on the right: connect a library, or import a game from this Mac. Choosing a library lists every store Orivo can sign into — Steam, Epic Games, GOG, Ubisoft Connect, Xbox, Microsoft Store and Instant Gaming — and each one gets a page that says what connecting brings, how its sign-in works, and whether Orivo can launch what it finds, before anything is started. It runs the same connectors Settings does, so a sign-in begun here reports back here, and the screen dissolves into the library the moment the first game lands.
- A notification bell beside your profile picture. It carries advice that is worth giving once and never worth interrupting for: where to add an artwork key so covers come back sharper, and where to see which store price feeds can actually answer. Nothing fires on arrival, nothing is said twice, nothing is said that has stopped being true, and a notice dismissed is gone for good.

### Changed

- An empty library now says so. It used to be filled in with the ten bundled showcase games, which made a fresh install look like a library of titles nobody owns — and left the one screen whose whole job is to ask for a connection with nothing to ask for. The demo games are still one toggle away in Settings › Appearance.

## [0.3.4] - 2026-08-21

### Added

- The bottom bar is now the only control the library needs. One button cycles through four ways of reading your collection — `Activity`, `Genre`, `Source`, `Platform` — and the segments for the current mode sit in the middle of the bar. `Activity` offers `Recently Played`, `Most Played`, `Play Next`, `Resume` and `Never Played`; the first two only sort, so the library never opens on an empty rail, and the others appear only when they hold something. `Genre`, `Source` and `Platform` list the values actually present in your library rather than a fixed list, and a library with nothing to divide by shows `All Games`. The two dropdowns above the rail, which did nothing, are gone.
- A mood switch on the left of the bar, between `Orivo` and `Rage`. It filters nothing: the brand becomes the spiral and the accent colour changes.
- Epic games now know whether they are installed on this machine. Orivo reads the Epic Games Launcher's own install manifests and nothing else — it never asks Epic's servers what is on your disk, and it never writes into the launcher's data.
- An in-app feedback button beside your profile picture, and crash reports, both through Sentry. Neither exists unless a DSN is configured: a build from source initialises nothing and makes no network call.
- README, LICENSE (PolyForm Noncommercial 1.0.0), CONTRIBUTING and `.env.example`.

### Changed

- The Selector hero draws the game's own wordmark where the title used to be, at the same place and weight, and falls back to the text immediately if the image is missing or fails to decode. Hero artwork is now chosen unbranded where possible, so a game never shows two logos.
- The hero no longer carries a synopsis — that belongs to the game's page — and `Play` is its only action. The last session is written in plain language (`2 days ago`) instead of whatever a connector returned.
- Rail covers are bare: no title, no play time, no gradient over the artwork. That information lives in the hero.
- The game detail page and the model behind it were rewritten.
- Wallpaper search reworked, and SteamGridDB's CDN added to the content security policy so its artwork can load in the packaged app.

## [0.3.3] - 2026-08-20

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
