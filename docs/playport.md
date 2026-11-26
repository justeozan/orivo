# Playport on iPhone and iPad

[Playport](https://playport.dev) runs Windows games on a stock, non-jailbroken
iPhone: Wine, the FEX x86 translator and DXMT inside one app, on the phone's own
CPU and GPU, with JIT from a debugger it pairs with itself. It installs games
from Steam, GOG and Epic Games — the same accounts Orivo reads libraries from —
or from a folder in Files. It is sideloaded: never on the App Store, signed by
the player's own Apple ID, Developer Mode on.

An iOS build of Orivo runs no PC game (`src-tauri/src/platform.rs`), so on a
device that has Playport, Playport is how Install leads to playing the copy the
player already owns. Like Winlator on Android it is a separate application, so
Orivo's part is a **hand-off**, and a narrow one.

The host side is [`src-tauri/src/playport.rs`](../src-tauri/src/playport.rs);
the menu it joins is [`store_choices.rs`](../src-tauri/src/store_choices.rs);
the WebView's part is [`src/playport.ts`](../src/playport.ts).

## What Playport exposes to another app

Read from Playport 0.4.0's own source (commit `412de5f`, 2026-10-13), not from
a guide, and checked again against commit `06e83ca` (2026-11-13): the URL
handler and decision 0012 are unchanged, and the list's statuses are now
`runs` and `broken`.

- **A URL scheme, `playport`.** `app/Info.plist` registers it for LocalDevVPN's
  way back after it turns the VPN on, and `PlayportApp.swift` handles it with
  `.onOpenURL { _ in }`: opening any `playport://` URL brings Playport to the
  front and reads nothing from it. There is no URL that opens a game, its page
  or an install. That is deliberate — its decision 0012, "The UI is the only
  entry point", retired every way into the app that is not its own interface.
- **A public compatibility list**, at
  <https://playport.dev/compatibility/games.json>: `{ name, store, id?, status }`
  per game, `status` either `runs` ("boots to a menu or further on the
  phone") or `broken` ("does not reach a menu yet"). Until November 2026 it
  split `runs` into `works` and `issues`; its page still reads those two as
  `runs`, and so does Orivo. Steam entries carry the app id and GOG ones the
  product id; Epic entries carry only a name.

Nothing else another app can reach: its entitlements hold only Increased
Memory Limit, so no App Group, and its `Info.plist` declares no document or
activity types.

## What Orivo does with it

- **It asks whether Playport is installed** with `canOpenURL:` on
  `playport://`. iOS answers that only for schemes the asking app declares, so
  `src-tauri/Info.ios.plist` lists `playport` in `LSApplicationQueriesSchemes`;
  without it the answer is always `false`. It is asked when a menu is built,
  again when its row is picked, and by the WebView at start and each time
  Orivo comes back to the front — the moments an app can have come or gone.
  iOS 27 deprecates `canOpenURL:` without removing it, and caps an app linked
  against its SDK at 25 declared schemes (50 before): `playport` is the only
  one Orivo declares.
- **It offers the games Playport can take**: a Steam, GOG or Epic Games game
  whose store published a Windows build, or no platforms at all
  (`playport::game_for`, and `playportTakes` for the button).
- **It puts one row first in Install's menu**: *PC version*, with Playport's
  verdict underneath — *Runs*, *Reported broken*, or *Not on its
  compatibility list yet*. A Steam game keeps its other answers
  after it (its iOS version, its Steam page). A GOG or Epic game keeps its iOS
  version, when the App Store sells one, after it; neither store publishes a
  page the catalog can name, so there is no page row.
- **It opens `playport://`**, a constant, through the same opener the store
  pages use, after checking once more that Playport is still there.
- **It reads the list** only when a row is about to be offered: at most once
  every six hours, retried five minutes after a failure, and never waited on
  for more than four seconds. A list that cannot be read costs the row its
  verdict and nothing else. Steam and GOG games are found by store and id,
  Epic games by title.

## What it does not do

- **It does not name the game to Playport.** It cannot: see above. Playport
  opens on its own screen, and the game is in its Library under the account
  that owns it.
- **It does not mention Playport where it is not installed** — no row, no link,
  no hint. Playport is sideloaded and needs Developer Mode, which is not a
  route an App Store app should be sending people down; offering it to the
  people who already chose it is a different thing.
- **It carries none of Playport's code.** Playport is GPL-3.0, which
  [CONTRIBUTING.md](../CONTRIBUTING.md) rules out, and guideline 2.5.2 refuses
  an App Store app that downloads or runs code it did not ship with.

## If Playport ever opens games by URL

Playport already identifies every copy by its store and that store's id (its
decision 0057: Steam app id, GOG product id, Epic app name), and its game page
can be opened by that key (`GameRef.store`). A URL such as
`playport://game/<store>/<id>` that opened the game's page — not started it —
would keep its UI the only entry point, and Orivo has all three ids: the Epic
app name is the third part of an Epic launch reference. That is a change for
Playport to want, not one Orivo can make; until then, `playport://` is all
there is.

## Trying it on the simulator

Playport's IPA only runs on a device, so the simulator needs a stand-in: any
app whose `Info.plist` claims the `playport` scheme. Build one with `swiftc`
for `arm64-apple-ios17.0-simulator`, sign it ad hoc (`codesign -s -`), install
it with `xcrun simctl install`, and Orivo's menu grows the row; uninstall it and
the row goes. A stand-in that records the URL it was opened with shows exactly
what Orivo sends: `playport://`, and nothing more.
