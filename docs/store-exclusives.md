# Store exclusives

The Store's brand tabs (PlayStation, Xbox, Nintendo, Steam, Epic Games) and
"Tout" show only games sold on one brand's store. Which games those are is a
curated, sourced list, never something inferred from a feed. Issue #109.

## The rule

- **Console brands** (`playstation`, `xbox`, `nintendo`): the game is sold on
  that maker's consoles and on no other console maker's console. A PC version
  is allowed. Ghost of Yōtei (PS5 only) qualifies; Marvel's Spider-Man 2
  (PS5 and PC) qualifies; Forza Horizon 5 (Xbox, PC and PS5 since 2025) does
  not.
- **PC brands** (`steam`, `epic`): on PC, the game is sold through that
  storefront and no other PC storefront: GOG, Epic or Steam, the Microsoft
  Store / PC Game Pass, a publisher's launcher. A reseller's key for that
  storefront counts as that storefront. Console versions are allowed: Elden
  Ring is a Steam exclusive because on PC only Steam sells it.
- A game is exclusive to **one brand at most**.
- What counts is what is released, as of the date the list was checked. A
  port that is announced but not out does not end an exclusive; a released
  one does. A timed exclusive that ends within weeks is left out rather than
  left to go wrong (Forza Horizon 6, on PS5 from 2027-01-26).
- **When unsure, leave it out.** Death Stranding 2 is out of the list for that
  reason: an Xbox port is rumoured but unconfirmed.

## Where it lives

| File | What it is |
| --- | --- |
| `scripts/store-exclusives.json` | The list. One row per game: brand, sources, why it qualifies, the date it was checked, French copy, price, critic score, artwork URLs. Edit this. |
| `scripts/build-store-exclusives.mjs` | `pnpm store:exclusives` (also run by `pnpm store:build`). Validates the list and writes the three files below. Offline, from committed files only. |
| `scripts/fetch-store-exclusives-media.mjs` | Downloads a row's artwork into `public/media/store/<slug>/` (`capsule.jpg` with the wordmark, `hero.jpg`, `cover.jpg`, `scene-N.jpg`). macOS only (`sips`, `cjpeg`). |
| `src/generated/store-exclusives.ts` | `STORE_EXCLUSIVES`, for the WebView. `src/store-catalog-all.ts` merges it with the catalogue as `BUNDLED_STORE_GAMES`. |
| `src-tauri/resources/store-exclusives.json` | The same rows, embedded by `store.rs`. A test fails if the two differ. |
| `src-tauri/resources/store-exclusive-pages.json` | Offer id → the brand's product page. Read by the host only: the WebView asks to open an offer by id, as for every other offer. |

To add a game: add its row, run `node scripts/fetch-store-exclusives-media.mjs
<slug>`, then `pnpm store:exclusives`, then `pnpm check` (the tests pin the
count, so update `EXCLUSIVES_SIZE` in `store.rs`). To drop one, delete its row
and its media folder and do the same.

## How a row was checked

- **Exclusivity**: the brand's own product page, plus the platform list on
  Metacritic and, where a port was rumoured or released, a news or publisher
  source. PC rows were checked against GOG's catalogue, Epic, the Microsoft
  Store and PCGamingWiki's availability table. The `sources` and `exclusivity`
  fields say which.
- **Price**: the brand's own French store, read the day the row was checked,
  through what that store's page itself uses: the PlayStation Store page, the
  Microsoft Store catalogue (`displaycatalog`), Nintendo's eShop price API,
  Steam's `appdetails` with `cc=fr`. `price.source` names it. A sale price is
  recorded as quoted (the list was checked on Black Friday 2026). The Epic
  Games Store refuses automated reads, so its paid games carry `price: null`;
  the free ones say so on their own listing.
- **Critic score**: the Metacritic metascore on the game's page (the main
  console's for console games, PC for PC games), or `null` when there is none.
- **Copy**: the French short description from the store listing, trimmed.

The host marks every bundled price as unverified a day after it was checked;
Steam exclusives are priced live by the regular Steam refresh, and the other
stores have no feed, so their prices stay as recorded until the list is
re-checked.

## Re-checking

Ports land often (Microsoft brought much of its catalogue to PS5 in
2025–2026). Before a release that ships this list, re-check each row's
sources and the news since `checkedAsOf`, and update `verifiedAt`. Left out
on 2026-11-26, and why:

- Starfield, Avowed, South of Midnight, Grounded 2, Keeper, Forza Horizon 5,
  Gears of War: Reloaded, Hellblade II, Indiana Jones, Microsoft Flight
  Simulator 2024, Halo: Campaign Evolved: out on PS5.
- Forza Horizon 6: timed, PS5 on 2027-01-26.
- Final Fantasy VII Rebirth, Stellar Blade: out on Switch 2 (and Xbox).
- Helldivers 2: out on Xbox.
- Fortnite: on the Microsoft Store for PC since November 2025.
- Death Stranding 2: an Xbox port is rumoured.
