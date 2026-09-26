/**
 * Which shape of screen Orivo is running on.
 *
 * The scene was drawn for a desktop window, and it is height that decides
 * whether it fits: the Library hero alone reserves `top: 196px` and
 * `bottom: 443px` before a single line of content, so anything shorter than
 * ~640px cannot lay out at all. A phone held sideways is 914x411 — as wide as a
 * small laptop, which is exactly why the width-only breakpoints all read it as
 * "desktop" and hand it a scene that resolves to a negative height.
 *
 * The answer lands on `<html>` instead of staying a media query because
 * `styles.css` is imported before the four page sheets, so a `@media` block at
 * its end loses to `.store-page` or `.gd-body` on source order. An attribute
 * selector outranks them wherever the rule happens to live.
 */

/** Below this, the desktop scene is arithmetically impossible, not just tight. */
const COMPACT_MAX_HEIGHT = 560;

export const COMPACT_QUERY = `(max-height: ${COMPACT_MAX_HEIGHT}px)`;

export function isCompactHeight(height: number): boolean {
  return height <= COMPACT_MAX_HEIGHT;
}

/**
 * Mirror the current form factor onto `<html>` and keep it there. The listener
 * is not optional: Android hands the app a new viewport on rotation and on
 * split-screen resize, and a stale attribute is worse than none.
 */
export function applyFormFactor(): void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;

  const query = window.matchMedia(COMPACT_QUERY);
  const reflect = (compact: boolean) => {
    if (compact) document.documentElement.dataset.formFactor = "compact";
    else delete document.documentElement.dataset.formFactor;
  };

  reflect(query.matches);
  query.addEventListener?.("change", (event) => reflect(event.matches));
}
