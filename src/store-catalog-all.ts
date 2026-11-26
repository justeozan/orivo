import type { GameSummary, StoreBrand } from "./contracts";
import { STORE_CATALOG } from "./generated/store-catalog";
import { STORE_EXCLUSIVES } from "./generated/store-exclusives";

/**
 * Every game the app ships with: the brands' exclusives first, then the
 * curated catalogue. The same two files `store.rs` embeds, so the WebView
 * outside the desktop shell shows the shelf the host would have served.
 *
 * The two lists never share a game: an exclusive is built from its own
 * sourced list (`scripts/store-exclusives.json`), the catalogue from Steam.
 */
export const BUNDLED_STORE_GAMES: GameSummary[] = [...STORE_EXCLUSIVES, ...STORE_CATALOG];

/** One brand's exclusives, in the order the list ranks them. */
export function bundledExclusives(brand: StoreBrand): GameSummary[] {
  return STORE_EXCLUSIVES.filter((game) => game.exclusiveTo === brand);
}
