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

/**
 * Sources with no wordmark of their own. Their lockups were drawn here (a
 * glyph beside the name set in an image), which read as neither official nor
 * part of the interface; they are plain segments with a standard glyph now
 * (`SOURCE_GLYPHS` in app.ts).
 */
const NOT_BRANDS = new Set(["local", "wine", "moonlight"]);

export function brandLogoFor(mode: BrowseMode, segmentId: string): BrandLogo | null {
  // A genre called "Steam" is a genre, not the store.
  if (mode !== "source" && mode !== "platform") return null;
  if (NOT_BRANDS.has(segmentId)) return null;
  return Object.prototype.hasOwnProperty.call(BRAND_LOGOS, segmentId)
    ? BRAND_LOGOS[segmentId as BrandLogoId]
    : null;
}
