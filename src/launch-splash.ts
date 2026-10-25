/* ---------------------------------------------------------------------------
   Starting a game.

   Play opens a splash over the whole shell: the game's own artwork, its
   wordmark, the word "Starting" and a line that says Orivo is waiting. It is a
   task, not a place — no route, and nothing behind it is torn down — and it
   ends one of three ways:

   - the launch fails: the splash gets out of the way of the error, which the
     scene reports exactly as it did before there was a splash;
   - the player presses Escape, or B: Orivo stays, the launch carries on;
   - the game reaches the screen: Orivo steps aside — hidden on macOS,
     minimised elsewhere — unless the player asked it to stay.

   "The game reaches the screen" is read from the only signals a WebView gets.
   Being covered (the page went hidden) is the strong one: something now fills
   the screen. Losing focus counts too, but only for a game Orivo started
   itself — a store's client is activated the moment it is asked, long before
   the game it was asked for exists, so for those it says nothing. Either way
   the splash stands for a short beat first, so it reads as a transition rather
   than a flicker, and a launch that shows neither signal still steps aside
   after a deadline: a game that opened behind Orivo is never left there.
   --------------------------------------------------------------------------- */

import { applyLogoFit, prepareLogo } from "./logo-fit";
import { prefersReducedMotion } from "./motion";

/** Who puts the game on screen, as the host reports it with the launch. */
export type LaunchHandoff = "process" | "client";

export interface LaunchSplashGame {
  id: string;
  title: string;
  /** The picture behind everything: the one the Library's hero shows for this game. */
  artUrl: string;
  /** The game's wordmark, when a store published one. Empty when it did not. */
  logoUrl: string;
}

export type ForegroundSignal = "blur" | "focus" | "hidden" | "visible";

/** Where the splash learns that something else has taken the screen. */
export interface ForegroundSignals {
  watch(listener: (signal: ForegroundSignal) => void): () => void;
}

export interface LaunchSplashTiming {
  /** The shortest the splash stands, counted from Play, before Orivo steps aside or fades back. */
  holdMs: number;
  /** Once the host has accepted a launch, the longest Orivo waits for the game before stepping aside anyway. */
  deadlineMs: number;
  /** How long the splash takes to fade when Orivo stays. Matches styles.css. */
  fadeMs: number;
  /**
   * How long the splash stays up after Orivo has asked to step aside: the
   * operating system animates the window away, and that animation should show
   * the splash rather than the library underneath.
   */
  settleMs: number;
}

export const LAUNCH_SPLASH_TIMING: Readonly<LaunchSplashTiming> = Object.freeze({
  holdMs: 1_600,
  deadlineMs: 12_000,
  fadeMs: 260,
  settleMs: 450,
});

export interface LaunchSplashOptions {
  host: HTMLElement;
  /** Everything the splash stands in front of. Inert while it does. */
  covers: readonly HTMLElement[];
  /** Whether this platform can step aside and the player has not asked Orivo to stay. */
  shouldStepAside: () => boolean;
  /** Hide or minimise Orivo. */
  stepAside: () => Promise<void>;
  signals?: ForegroundSignals;
  timing?: Partial<LaunchSplashTiming>;
  reducedMotion?: () => boolean;
  now?: () => number;
}

export interface LaunchSplash {
  open(game: LaunchSplashGame): void;
  /** The host accepted the launch of `gameId`. */
  launched(gameId: string, handoff: LaunchHandoff): void;
  /** Starting `gameId` failed, before or after the host accepted it. */
  failed(gameId: string): void;
  /** Escape or B: back to Orivo, which stays. The launch itself carries on. */
  dismiss(): void;
  isOpen(): boolean;
  dispose(): void;
}

/** Window focus and page visibility, which is everything a WebView is told. */
export function windowForegroundSignals(target: Window = window, page: Document = document): ForegroundSignals {
  return {
    watch(listener) {
      const blur = (): void => listener("blur");
      const focus = (): void => listener("focus");
      const visibility = (): void => listener(page.visibilityState === "hidden" ? "hidden" : "visible");
      target.addEventListener("blur", blur);
      target.addEventListener("focus", focus);
      page.addEventListener("visibilitychange", visibility);
      return () => {
        target.removeEventListener("blur", blur);
        target.removeEventListener("focus", focus);
        page.removeEventListener("visibilitychange", visibility);
      };
    },
  };
}

/** Reads the handoff the host sent with a launch. Anything else is a game Orivo started. */
export function readLaunchHandoff(result: unknown): LaunchHandoff {
  return (result as { handoff?: unknown } | null | undefined)?.handoff === "client" ? "client" : "process";
}

type Phase = "closed" | "starting" | "waiting" | "leaving";

export function createLaunchSplash(options: LaunchSplashOptions): LaunchSplash {
  const { host, covers } = options;
  const timing: LaunchSplashTiming = { ...LAUNCH_SPLASH_TIMING, ...options.timing };
  const signals = options.signals ?? windowForegroundSignals();
  const reducedMotion = options.reducedMotion ?? prefersReducedMotion;
  const now = options.now ?? (() => performance.now());

  let phase: Phase = "closed";
  let gameId: string | null = null;
  let openedAt = 0;
  let handoff: LaunchHandoff = "process";
  let stepsAside = false;
  /** Something else took the foreground since Play. */
  let lostFocus = false;
  /** Something covered Orivo entirely since Play. */
  let covered = false;
  /** The player went somewhere else and came back to Orivo since Play. */
  let returned = false;
  let beatPending = false;
  let returnFocusTo: HTMLElement | null = null;
  let stopWatching: (() => void) | null = null;
  const timers = new Set<number>();

  const schedule = (callback: () => void, ms: number): void => {
    const timer = window.setTimeout(() => {
      timers.delete(timer);
      callback();
    }, Math.max(0, ms));
    timers.add(timer);
  };
  const clearTimers = (): void => {
    for (const timer of timers) window.clearTimeout(timer);
    timers.clear();
    beatPending = false;
  };

  const render = (game: LaunchSplashGame): void => {
    const art = document.createElement("div");
    art.className = "launch-splash__art";
    art.setAttribute("aria-hidden", "true");
    if (game.artUrl) {
      const image = document.createElement("img");
      image.className = "launch-splash__image";
      image.alt = "";
      image.decoding = "async";
      // A picture that will not load leaves the dark surface under it, which
      // is the stated fallback everywhere else a game has no art.
      image.addEventListener("error", () => image.remove(), { once: true });
      image.src = game.artUrl;
      art.append(image);
    }

    const veil = document.createElement("div");
    veil.className = "launch-splash__veil";
    veil.setAttribute("aria-hidden", "true");

    const content = document.createElement("div");
    content.className = "launch-splash__content";
    const eyebrow = document.createElement("p");
    eyebrow.className = "launch-splash__eyebrow";
    eyebrow.textContent = "Starting";
    // The wordmark when the store published one, the title when it did not,
    // as in the Library's hero: the title stays in the markup and comes back
    // the moment the image fails, and the image's alt carries the name.
    const title = document.createElement("p");
    title.className = "launch-splash__title";
    title.textContent = game.title;
    content.append(eyebrow);
    if (game.logoUrl) {
      const logo = document.createElement("img");
      logo.className = "launch-splash__logo";
      logo.alt = game.title;
      logo.addEventListener(
        "error",
        () => {
          logo.remove();
          content.classList.remove("has-logo");
        },
        { once: true },
      );
      logo.src = game.logoUrl;
      // The hero has already measured this mark, so the trimmed copy and its
      // size are normally ready before the splash has finished arriving.
      void prepareLogo(game.logoUrl).then((prepared) => {
        if (!prepared || !logo.parentElement) return;
        applyLogoFit(logo, prepared);
        logo.src = prepared.src;
      });
      content.classList.add("has-logo");
      content.append(logo);
    }
    const progress = document.createElement("div");
    progress.className = "launch-splash__progress";
    progress.setAttribute("aria-hidden", "true");
    progress.append(document.createElement("span"));
    content.append(title, progress);

    const mark = document.createElement("img");
    mark.className = "launch-splash__mark";
    mark.src = "/media/orivo-ring-icon.png";
    mark.alt = "";
    mark.setAttribute("aria-hidden", "true");

    host.replaceChildren(art, veil, content, mark);
  };

  /** Take everything down at once. The shell is already live again by now. */
  const finish = (): void => {
    clearTimers();
    stopWatching?.();
    stopWatching = null;
    phase = "closed";
    gameId = null;
    host.classList.remove("is-open");
    host.hidden = true;
    // The artwork goes with it: a splash that comes back is built afresh.
    host.replaceChildren();
  };

  /** Hand the shell back, then let the splash go — faded, unless it is already out of sight. */
  const close = (fade: boolean): void => {
    if (phase === "closed") return;
    clearTimers();
    stopWatching?.();
    stopWatching = null;
    for (const element of covers) element.inert = false;
    const focusTarget = returnFocusTo;
    returnFocusTo = null;
    if (focusTarget?.isConnected) focusTarget.focus({ preventScroll: true });
    if (!fade || reducedMotion()) {
      finish();
      return;
    }
    phase = "leaving";
    host.classList.remove("is-open");
    schedule(finish, timing.fadeMs);
  };

  const stepAside = (): void => {
    if (phase !== "waiting") return;
    phase = "leaving";
    clearTimers();
    stopWatching?.();
    stopWatching = null;
    options.stepAside().then(
      // Coming back to Orivo finds the library, not a splash for a game that
      // has long been running.
      () => schedule(() => close(false), timing.settleMs),
      // The host could not do it: Orivo stays, and says nothing about a
      // window it was only trying to tidy away.
      () => {
        phase = "waiting";
        close(true);
      },
    );
  };

  /** Comes back to `evaluate` once the splash has stood for its beat. */
  const afterBeat = (beat: number): void => {
    if (beatPending) return;
    beatPending = true;
    schedule(() => {
      beatPending = false;
      evaluate();
    }, beat);
  };

  const evaluate = (): void => {
    if (phase !== "waiting") return;
    const beat = openedAt + timing.holdMs - now();
    if (!stepsAside) {
      // Orivo stays. Covered, there is nobody left to show the splash to;
      // otherwise it has its beat and fades back to the library.
      if (covered) close(false);
      else if (beat > 0) afterBeat(beat);
      else close(true);
      return;
    }
    // Covered is never in doubt, and nobody sees the splash any more anyway.
    if (covered) {
      stepAside();
      return;
    }
    if (!(lostFocus && handoff === "process")) return; // The deadline will.
    if (beat > 0) afterBeat(beat);
    else stepAside();
  };

  const onSignal = (signal: ForegroundSignal): void => {
    if (phase !== "starting" && phase !== "waiting") return;
    if (signal === "blur") lostFocus = true;
    if (signal === "hidden") covered = true;
    if (signal === "focus" && lostFocus) {
      // The player went to something else and came back to Orivo while the
      // game was starting. That is a decision, and Orivo stays for it.
      lostFocus = false;
      returned = true;
      if (phase === "waiting") {
        close(true);
        return;
      }
    }
    evaluate();
  };

  return {
    open(game) {
      if (phase !== "closed") finish();
      gameId = game.id;
      openedAt = now();
      handoff = "process";
      lostFocus = false;
      covered = false;
      returned = false;
      const active = document.activeElement;
      returnFocusTo = active instanceof HTMLElement && active !== document.body ? active : null;
      host.hidden = false;
      render(game);
      for (const element of covers) element.inert = true;
      // Read layout once so the opacity transition starts from the hidden state.
      void host.offsetWidth;
      host.classList.add("is-open");
      phase = "starting";
      stopWatching = signals.watch(onSignal);
    },

    launched(id, kind) {
      if (phase !== "starting" || id !== gameId) return;
      phase = "waiting";
      handoff = kind;
      stepsAside = options.shouldStepAside() && !returned;
      if (stepsAside) schedule(stepAside, timing.deadlineMs);
      evaluate();
    },

    failed(id) {
      if ((phase === "starting" || phase === "waiting") && id === gameId) close(true);
    },

    dismiss() {
      if (phase === "starting" || phase === "waiting") close(true);
    },

    isOpen() {
      return phase === "starting" || phase === "waiting";
    },

    dispose() {
      if (phase === "closed") return;
      for (const element of covers) element.inert = false;
      returnFocusTo = null;
      finish();
    },
  };
}
