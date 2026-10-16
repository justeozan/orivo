/**
 * The store and platform logos the browse bar wears in its Source and Platform
 * modes: white, the mark and the name side by side, all the same height. They
 * are built from `assets/brands` by `scripts/build-brand-logos.mjs` and shipped
 * with the app, never fetched — the bar is the first thing on screen and works
 * offline like the rest of the Library.
 *
 * A segment's id is its brand's id, so the lookup is the whole contract. A
 * segment whose brand has no logo here keeps its word.
 */
import { BRAND_LOGOS, type BrandLogoId } from "./brand-logos.generated";
import type { BrowseMode } from "./library-browse";

export interface BrandLogo {
  src: string;
  width: number;
  height: number;
}

export function brandLogoFor(mode: BrowseMode, segmentId: string): BrandLogo | null {
  // A genre called "Steam" is a genre, not the store.
  if (mode !== "source" && mode !== "platform") return null;
  return Object.prototype.hasOwnProperty.call(BRAND_LOGOS, segmentId)
    ? BRAND_LOGOS[segmentId as BrandLogoId]
    : null;
}
