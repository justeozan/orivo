/**
 * The single guard behind every animation in Orivo, JS and CSS alike.
 *
 * The Appearance → Motion preference wins over the operating system:
 *   - `"full"`    → always animate, even when the OS asks for reduced motion.
 *                  Windows flips `prefers-reduced-motion` to `reduce` as soon as
 *                  "Animation effects" is off under Accessibility/Performance —
 *                  usually a speed choice, not an accessibility one, and this is
 *                  how such users get Orivo's motion back.
 *   - `"reduced"` → never animate, regardless of the OS.
 *   - `"system"`  → follow `prefers-reduced-motion`.
 *
 * The CSS guards that cut animations under reduced motion are scoped to
 * `html:not([data-motion="full"])`, and `applyMotionPreference` mirrors the
 * value onto `document.documentElement` so both stay in sync.
 */
export function prefersReducedMotion(): boolean {
  if (document.querySelector('[data-motion="full"]')) return false;
  if (document.querySelector('[data-motion="reduced"]')) return true;
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}