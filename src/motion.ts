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

/**
 * The preference as it was last applied, kept by the WebView itself.
 *
 * The host only answers once the shell has asked, and by then the first frame
 * — the startup animation — has long been painted. Remembering the last answer
 * is what lets that frame already honour "Reduced". The host's answer still
 * wins the moment it lands.
 */
const MOTION_MEMORY_KEY = "orivo.motion";
const MOTION_VALUES: readonly string[] = ["full", "system", "reduced"];

type MotionMemory = Pick<Storage, "getItem" | "setItem">;

function localMemory(): MotionMemory | null {
  try {
    return window.localStorage;
  } catch {
    // Storage can be refused outright (a locked-down WebView); the first frame
    // then follows the system, as it did before this existed.
    return null;
  }
}

export function rememberMotionPreference(value: string, memory: MotionMemory | null = localMemory()): void {
  if (!MOTION_VALUES.includes(value)) return;
  try {
    memory?.setItem(MOTION_MEMORY_KEY, value);
  } catch {
    // A full or refused store costs the next first frame its accuracy, nothing else.
  }
}

/** Run before the shell mounts, so the guards above see the choice at once. */
export function applyRememberedMotionPreference(
  memory: MotionMemory | null = localMemory(),
  root: HTMLElement = document.documentElement,
): void {
  let value: string | null = null;
  try {
    value = memory?.getItem(MOTION_MEMORY_KEY) ?? null;
  } catch {
    return;
  }
  if (value && MOTION_VALUES.includes(value)) root.dataset.motion = value;
}
