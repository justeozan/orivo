/* ---------------------------------------------------------------------------
   The startup animation.

   The splash is plain markup in index.html, outside #app, and its stylesheet
   is linked from there too: it is the first frame the window paints, before
   this bundle has even been parsed. This module only decides when it leaves.

   It leaves once the library has answered and the scene's artwork has had a
   moment to decode, so what it uncovers is the real scene rather than an empty
   backdrop. On the first start of a session it first lets the mark finish its
   arrival; it never stays longer than a hard cap, whatever the library does.
   Under reduced motion it has no arrival to wait for and no exit to play: it
   is simply removed the moment the library is ready.

   While it stands, <html> carries `data-booting`, so a staged arrival behind it
   — the welcome screen's — can wait for the dark to lift instead of playing
   out unseen beneath it.

   It belongs to an app starting. index.html holds it back (`hidden`) and the
   bundle reveals it before mounting, in the desktop and mobile apps; the
   browser preview — `pnpm dev`, and the end-to-end suite driving it — reloads
   all day, and an arrival on every load cost the suite a fifth of its time in
   repaints. There it plays only when asked for, with `?boot`.
   --------------------------------------------------------------------------- */

import { prefersReducedMotion } from "./motion";

/**
 * How long, from the moment the page started, the mark holds the screen: its
 * arrival in boot-splash.css is over by then, with a beat to spare.
 */
export const BOOT_HOLD_MS = 1_400;
/** The longest the splash may stand in front of a library that has not answered. */
export const BOOT_CAP_MS = 6_000;
/** How long the reveal waits for the scene's artwork to decode before going anyway. */
export const BOOT_ARTWORK_GRACE_MS = 700;
/** The exit transition in boot-splash.css, and a little more, in case it never reports. */
const LEAVE_FALLBACK_MS = 640;
/**
 * Set once the full arrival has played. A reload within the same session — a
 * developer's, or the WebView recovering — gets out of the way instead of
 * performing the whole entrance again.
 */
const SESSION_KEY = "orivo.boot-splash-played";

type SessionMemory = Pick<Storage, "getItem" | "setItem">;

/** Whether this page load is an app starting, rather than a preview reloading. */
export function startsAnApp(
  tauri: boolean = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window,
  search: string = typeof window === "undefined" ? "" : window.location.search,
): boolean {
  if (tauri) return true;
  try {
    return new URLSearchParams(search).has("boot");
  } catch {
    return false;
  }
}

/** Shows the splash index.html holds back, or drops it for good. Runs before mounting. */
export function revealBootSplash(
  element: HTMLElement | null = document.getElementById("boot-splash"),
  wanted: boolean = startsAnApp(),
): void {
  if (!element) return;
  if (wanted) element.hidden = false;
  else element.remove();
}

export interface BootSplash {
  /**
   * The library has answered. `artwork` settles once the scene behind the
   * splash has its picture; it is waited for, but only briefly.
   */
  release(artwork?: Promise<unknown>): void;
}

export interface BootSplashOptions {
  /** Defaults to `#boot-splash`. A document without one gets a splash that does nothing. */
  element?: HTMLElement | null;
  /** Milliseconds since the page started. */
  now?: () => number;
  reducedMotion?: () => boolean;
  /** Defaults to `sessionStorage`. */
  memory?: SessionMemory | null;
  holdMs?: number;
  capMs?: number;
}

function sessionMemory(): SessionMemory | null {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

function playedThisSession(memory: SessionMemory | null): boolean {
  try {
    const played = memory?.getItem(SESSION_KEY) === "1";
    memory?.setItem(SESSION_KEY, "1");
    return played;
  } catch {
    return false;
  }
}

const settleWithin = (promise: Promise<unknown>, ms: number): Promise<void> =>
  new Promise<void>((resolve) => {
    const timer = window.setTimeout(resolve, ms);
    const done = (): void => {
      window.clearTimeout(timer);
      resolve();
    };
    promise.then(done, done);
  });

export function createBootSplash(options: BootSplashOptions = {}): BootSplash {
  const element =
    options.element === undefined ? document.getElementById("boot-splash") : options.element;
  if (!element) return { release() {} };

  const now = options.now ?? (() => performance.now());
  const reducedMotion = options.reducedMotion ?? prefersReducedMotion;
  const memory = options.memory === undefined ? sessionMemory() : options.memory;
  const holdMs = playedThisSession(memory) ? 0 : (options.holdMs ?? BOOT_HOLD_MS);
  const capMs = options.capMs ?? BOOT_CAP_MS;

  let leaving = false;
  let released = false;
  const root = document.documentElement;
  root.dataset.booting = "";

  const leave = (): void => {
    if (leaving) return;
    leaving = true;
    window.clearTimeout(capTimer);
    delete root.dataset.booting;
    if (reducedMotion()) {
      element.remove();
      return;
    }
    const remove = (): void => element.remove();
    element.addEventListener("transitionend", (event) => {
      if (event.target === element) remove();
    });
    window.setTimeout(remove, LEAVE_FALLBACK_MS);
    element.classList.add("is-leaving");
  };

  const capTimer = window.setTimeout(leave, Math.max(0, capMs - now()));

  return {
    release(artwork) {
      if (released || leaving) return;
      released = true;
      const ready = artwork ? settleWithin(artwork, BOOT_ARTWORK_GRACE_MS) : Promise.resolve();
      void ready.then(() => {
        if (leaving) return;
        const wait = reducedMotion() ? 0 : Math.max(0, holdMs - now());
        if (wait === 0) leave();
        else window.setTimeout(leave, wait);
      });
    },
  };
}
