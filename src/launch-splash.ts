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

   A streamed game is told apart (a `stream` handoff). Moonlight opens pages
   of its own before the picture — a question, a grey "Starting…" page, a
   black window — and the host keeps Orivo's window above them while it reads
   Moonlight's log (`stream_session.rs`). The splash says each step, draws the
   link between this machine and the PC (`launch-weave.ts`), asks in its own
   words when the PC is already running another game, and opens onto the
   stream when its first picture arrives. When the stream ends the host brings
   Orivo back, and the splash plays the way in backwards before the library.
   Focus and visibility say nothing here: Moonlight takes the foreground the
   moment it starts.
   --------------------------------------------------------------------------- */

import { applyLogoFit, prepareLogo } from "./logo-fit";
import { t, type MessageKey } from "./i18n";
import { createLaunchWeave, type LaunchWeave, type WeaveStage } from "./launch-weave";
import { prefersReducedMotion } from "./motion";

/**
 * Who puts the game on screen, as the host reports it with the launch.
 * `window`: a game Orivo started and whose window the host watches for; it
 * says when that window is up (`gameWindow`), and Orivo steps aside then.
 */
export type LaunchHandoff = "process" | "client" | "window" | "stream";

/** A stream's steps, as the host reads them from Moonlight (`stream_session.rs`). */
export type StreamStage =
  | "connecting"
  | "host-busy"
  | "stalled"
  | "closing"
  | "launching"
  | "handshake"
  | "visible"
  | "failed"
  | "ended";

export interface StreamUpdate {
  stage: StreamStage;
  /** The game is resumed rather than started on the PC. */
  resumed?: boolean;
}

/** What the player can say to a stream that is waiting. */
export type StreamAction = "replace" | "cancel" | "uncover";

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
  /**
   * The same, for a game whose window the host watches for: it says when the
   * window is up, so this only covers a host that never says.
   */
  windowDeadlineMs: number;
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
  windowDeadlineMs: 120_000,
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
  /**
   * The player's answer to a waiting stream: quit what the PC runs and play
   * (`replace`), stop (`cancel`), or let Moonlight show itself (`uncover`).
   */
  streamControl?: (gameId: string, action: StreamAction) => void;
  /** The strings' drawing; replaced in tests. */
  createWeave?: (strings: HTMLCanvasElement, seam: HTMLCanvasElement) => LaunchWeave;
}

export interface LaunchSplash {
  open(game: LaunchSplashGame): void;
  /** The host accepted the launch of `gameId`. */
  launched(gameId: string, handoff: LaunchHandoff): void;
  /** The host watched for `gameId`'s window: it is on screen (`true`), or the game ended first. */
  gameWindow(gameId: string, shown: boolean): void;
  /** Starting `gameId` failed, before or after the host accepted it. */
  failed(gameId: string): void;
  /** A step of the stream started for `gameId`. */
  stream(gameId: string, update: StreamUpdate): void;
  /** The host brought Orivo back from a stream that lasted `seconds`. */
  welcomeBack(game: LaunchSplashGame, seconds: number | null): void;
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
  const handoff = (result as { handoff?: unknown } | null | undefined)?.handoff;
  return handoff === "client" || handoff === "window" || handoff === "stream" ? handoff : "process";
}

type Phase = "closed" | "starting" | "waiting" | "diving" | "leaving" | "returning";

/** How long the library waits behind the way back before it is given back. */
const RETURN_BEAT_MS = 900;

/** The line under the progress bar for each step a stream announces. */
const STREAM_STATUS: Partial<Record<StreamStage, MessageKey>> = {
  connecting: "misc.splash.stream.connecting",
  closing: "misc.splash.stream.closing",
  launching: "misc.splash.stream.launching",
  handshake: "misc.splash.stream.handshake",
};

const WEAVE_STAGE: Partial<Record<StreamStage, WeaveStage>> = {
  connecting: "connecting",
  "host-busy": "busy",
  closing: "closing",
  launching: "launching",
  handshake: "handshake",
  visible: "visible",
};

/** "42 min" or "1 h 05 min", for the way back from a stream. */
export function streamDuration(seconds: number): string {
  const minutes = Math.max(1, Math.round(seconds / 60));
  if (minutes < 60) return t("misc.splash.stream.minutes", { minutes });
  return t("misc.splash.stream.hours", {
    hours: Math.floor(minutes / 60),
    minutes: String(minutes % 60).padStart(2, "0"),
  });
}

export function createLaunchSplash(options: LaunchSplashOptions): LaunchSplash {
  const { host, covers } = options;
  const timing: LaunchSplashTiming = { ...LAUNCH_SPLASH_TIMING, ...options.timing };
  const signals = options.signals ?? windowForegroundSignals();
  const reducedMotion = options.reducedMotion ?? prefersReducedMotion;
  const now = options.now ?? (() => performance.now());
  const makeWeave =
    options.createWeave ??
    ((strings: HTMLCanvasElement, seam: HTMLCanvasElement) => createLaunchWeave({ strings, seam, reducedMotion }));

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
  /** The host saw the game's window on screen (a `window` handoff). */
  let windowShown = false;
  let beatPending = false;
  let returnFocusTo: HTMLElement | null = null;
  let stopWatching: (() => void) | null = null;
  let weave: LaunchWeave | null = null;
  let streamStage: StreamStage | null = null;
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
    eyebrow.textContent = t("misc.splash.starting");
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
    // What a stream is doing, said once per step; empty for any other game.
    const status = document.createElement("p");
    status.className = "launch-splash__status";
    status.setAttribute("role", "status");
    content.append(title, progress, status);

    const mark = document.createElement("img");
    mark.className = "launch-splash__mark";
    mark.src = "/media/orivo-ring-icon.png";
    mark.alt = "";
    mark.setAttribute("aria-hidden", "true");

    const strings = document.createElement("canvas");
    strings.className = "launch-splash__strings";
    strings.setAttribute("aria-hidden", "true");
    const seam = document.createElement("canvas");
    seam.className = "launch-splash__seam";
    seam.setAttribute("aria-hidden", "true");

    host.replaceChildren(art, veil, strings, content, mark, seam);
  };

  const part = <T extends HTMLElement>(selector: string): T | null => host.querySelector<T>(selector);

  /** The strings, made the first time a stream needs them. */
  const strings = (): LaunchWeave | null => {
    if (weave) return weave;
    const canvas = part<HTMLCanvasElement>(".launch-splash__strings");
    const seam = part<HTMLCanvasElement>(".launch-splash__seam");
    if (!canvas || !seam) return null;
    weave = makeWeave(canvas, seam);
    weave.start();
    return weave;
  };

  const say = (key: MessageKey | null): void => {
    const status = part(".launch-splash__status");
    if (status) status.textContent = key ? t(key) : "";
  };

  const dropQuestion = (): void => {
    part(".launch-splash__question")?.remove();
    host.classList.remove("is-asking");
  };

  /** Asked in the splash, over the game's art: Moonlight's own dialog stays behind it. */
  const ask = (id: string): void => {
    if (part(".launch-splash__question")) return;
    const question = document.createElement("div");
    question.className = "launch-splash__question";
    question.setAttribute("role", "alertdialog");
    question.setAttribute("aria-label", t("misc.splash.stream.busy"));
    const text = document.createElement("p");
    text.className = "launch-splash__question-text";
    text.textContent = t("misc.splash.stream.busy");
    const actions = document.createElement("div");
    actions.className = "launch-splash__question-actions";
    const replace = document.createElement("button");
    replace.type = "button";
    replace.className = "launch-splash__answer launch-splash__answer--primary";
    replace.textContent = t("misc.splash.stream.replace");
    replace.addEventListener("click", () => {
      dropQuestion();
      options.streamControl?.(id, "replace");
      streamStep({ stage: "closing" });
    });
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.className = "launch-splash__answer";
    cancel.textContent = t("misc.splash.stream.cancel");
    cancel.addEventListener("click", () => {
      options.streamControl?.(id, "cancel");
      close(true);
    });
    actions.append(replace, cancel);
    question.append(text, actions);
    part(".launch-splash__content")?.append(question);
    host.classList.add("is-asking");
    replace.focus({ preventScroll: true });
  };

  /** Take everything down at once. The shell is already live again by now. */
  const finish = (): void => {
    clearTimers();
    stopWatching?.();
    stopWatching = null;
    weave?.stop();
    weave = null;
    streamStage = null;
    phase = "closed";
    gameId = null;
    host.classList.remove("is-open", "is-asking", "is-return");
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
    if (phase !== "waiting" || handoff === "stream") return;
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
    // A watched window is the one signal that counts: losing focus could be
    // the player clicking elsewhere while the game is still loading.
    const gameIsUp = handoff === "window" ? windowShown : lostFocus && handoff === "process";
    if (!gameIsUp) return; // The deadline will.
    if (beat > 0) afterBeat(beat);
    else stepAside();
  };

  const onSignal = (signal: ForegroundSignal): void => {
    if (phase !== "starting" && phase !== "waiting") return;
    // Moonlight comes forward the moment it starts, behind the splash; none
    // of this says anything about the stream.
    if (handoff === "stream") return;
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

  /** The picture is up: close the strings into the seam and open it onto the stream. */
  const openOntoStream = (): void => {
    const id = gameId;
    // Still a launch in progress until the seam is open: nothing else starts.
    phase = "diving";
    clearTimers();
    stopWatching?.();
    stopWatching = null;
    dropQuestion();
    say(null);
    const leave = (): void => {
      phase = "leaving";
      if (stepsAside) {
        options.stepAside().then(
          () => schedule(() => close(false), timing.settleMs),
          () => {
            if (id) options.streamControl?.(id, "uncover");
            phase = "waiting";
            close(false);
          },
        );
      } else {
        // Orivo stays: it stops standing above the stream's window, which
        // then has the screen, and the library is there behind it.
        if (id) options.streamControl?.(id, "uncover");
        phase = "waiting";
        close(false);
      }
    };
    const current = strings();
    if (!current) {
      leave();
      return;
    }
    current.stage("visible");
    void current.dive().then(() => {
      if (phase === "diving" && gameId === id) leave();
    });
  };

  function streamStep(update: StreamUpdate): void {
    if (phase !== "starting" && phase !== "waiting") return;
    streamStage = update.stage;
    const weaveStage = WEAVE_STAGE[update.stage];
    if (weaveStage && update.stage !== "visible") strings()?.stage(weaveStage);
    switch (update.stage) {
      case "host-busy":
        say(null);
        if (gameId) ask(gameId);
        return;
      case "visible":
        openOntoStream();
        return;
      case "stalled":
        // Moonlight is waiting on something Orivo cannot read: it gets the
        // screen, the way any other game's window does.
        dropQuestion();
        if (phase === "waiting" && stepsAside) {
          weave?.stop();
          stepAside();
        } else close(true);
        return;
      case "failed":
      case "ended":
        close(true);
        return;
      default:
        dropQuestion();
        say(
          update.stage === "launching" && update.resumed
            ? "misc.splash.stream.resuming"
            : (STREAM_STATUS[update.stage] ?? null),
        );
    }
  }

  return {
    open(game) {
      if (phase !== "closed") finish();
      gameId = game.id;
      openedAt = now();
      handoff = "process";
      streamStage = null;
      lostFocus = false;
      covered = false;
      returned = false;
      windowShown = false;
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
      if (kind === "stream") {
        // The host says when the picture is up, or that it never will be:
        // no deadline here, and the first step may already have been said.
        strings();
        if (!streamStage) streamStep({ stage: "connecting" });
        return;
      }
      if (stepsAside) schedule(stepAside, kind === "window" ? timing.windowDeadlineMs : timing.deadlineMs);
      evaluate();
    },

    gameWindow(id, shown) {
      if ((phase !== "starting" && phase !== "waiting") || id !== gameId) return;
      if (!shown) {
        close(true);
        return;
      }
      windowShown = true;
      evaluate();
    },

    failed(id) {
      if ((phase === "starting" || phase === "waiting") && id === gameId) close(true);
    },

    stream(id, update) {
      if (id !== gameId) return;
      // Steps can arrive before the launch call has answered.
      if (phase === "starting") handoff = "stream";
      streamStep(update);
    },

    welcomeBack(game, seconds) {
      if (phase !== "closed") finish();
      gameId = game.id;
      openedAt = now();
      handoff = "stream";
      phase = "returning";
      const active = document.activeElement;
      returnFocusTo = active instanceof HTMLElement && active !== document.body ? active : null;
      host.hidden = false;
      render(game);
      const eyebrow = part(".launch-splash__eyebrow");
      if (eyebrow) eyebrow.textContent = t("misc.splash.stream.ended");
      const status = part(".launch-splash__status");
      if (status && seconds && seconds > 0) status.textContent = streamDuration(seconds);
      for (const element of covers) element.inert = true;
      // Straight in, no fade: the way back starts on the stream's black.
      host.classList.add("is-open", "is-return");
      const current = strings();
      const giveBack = (): void => {
        if (phase === "returning" && gameId === game.id) schedule(() => close(true), RETURN_BEAT_MS);
      };
      if (current) void current.unfold().then(giveBack);
      else giveBack();
    },

    dismiss() {
      if (phase === "returning") {
        close(true);
        return;
      }
      if (phase !== "starting" && phase !== "waiting") return;
      if (handoff === "stream" && gameId) {
        // Waiting on the player's answer, Escape is "no"; otherwise Orivo
        // stays and Moonlight is let in front when the stream comes.
        options.streamControl?.(gameId, streamStage === "host-busy" ? "cancel" : "uncover");
      }
      close(true);
    },

    isOpen() {
      return phase === "starting" || phase === "waiting" || phase === "diving";
    },

    dispose() {
      if (phase === "closed") return;
      for (const element of covers) element.inert = false;
      returnFocusTo = null;
      finish();
    },
  };
}
