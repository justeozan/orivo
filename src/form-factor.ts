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

/**
 * A finger is the only pointer: a phone or a tablet. A desktop window never
 * answers yes, however short it is dragged — a laptop with a touch screen
 * included, since its primary pointer is still the trackpad.
 */
export const TOUCH_QUERY = "(hover: none) and (pointer: coarse)";

export function isCompactHeight(height: number): boolean {
  return height <= COMPACT_MAX_HEIGHT;
}

/**
 * A phone, to the stylesheet: a compact screen that only a finger points at.
 * It gets the compact layout and then a quieter one on top (the phone block at
 * the end of styles.css), while a desktop window dragged just as short keeps
 * its navigation, its search and its arrows.
 */
export function isPhone(compact: boolean, touch: boolean): boolean {
  return compact && touch;
}

/**
 * Mirror the current form factor onto `<html>` and keep it there. The listener
 * is not optional: Android hands the app a new viewport on rotation and on
 * split-screen resize, and a stale attribute is worse than none. A mouse
 * plugged into a phone changes the pointer just as a rotation changes the
 * height, so both queries are listened to.
 */
export function applyFormFactor(): void {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return;

  const compact = window.matchMedia(COMPACT_QUERY);
  const touch = window.matchMedia(TOUCH_QUERY);
  const root = document.documentElement;
  const reflect = () => {
    if (compact.matches) root.dataset.formFactor = "compact";
    else delete root.dataset.formFactor;
    if (isPhone(compact.matches, touch.matches)) root.dataset.device = "phone";
    else delete root.dataset.device;
  };

  reflect();
  compact.addEventListener?.("change", reflect);
  touch.addEventListener?.("change", reflect);
}
