import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LaunchWeave } from "./launch-weave";
import {
  LAUNCH_SPLASH_TIMING,
  createLaunchSplash,
  readLaunchHandoff,
  streamDuration,
  windowForegroundSignals,
  type ForegroundSignal,
  type LaunchSplash,
  type LaunchSplashOptions,
} from "./launch-splash";

const { holdMs, deadlineMs, windowDeadlineMs, fadeMs, settleMs } = LAUNCH_SPLASH_TIMING;

const hades = {
  id: "steam:1145350",
  title: "Hades II",
  artUrl: "https://cdn.example/hades-hero.jpg",
  logoUrl: "https://cdn.example/hades-logo.png",
};

/** A window whose focus and visibility the test decides. */
function foreground() {
  const listeners = new Set<(signal: ForegroundSignal) => void>();
  return {
    signals: {
      watch(listener: (signal: ForegroundSignal) => void) {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    },
    send(signal: ForegroundSignal) {
      for (const listener of [...listeners]) listener(signal);
    },
    get watching() {
      return listeners.size;
    },
  };
}

describe("starting a game", () => {
  let host: HTMLElement;
  let shell: HTMLElement;
  let play: HTMLButtonElement;
  let window_: ReturnType<typeof foreground>;
  let stepAside: ReturnType<typeof vi.fn<() => Promise<void>>>;
  let splash: LaunchSplash;

  const mount = (options: Partial<LaunchSplashOptions> = {}): LaunchSplash => {
    splash = createLaunchSplash({
      host,
      covers: [shell],
      shouldStepAside: () => true,
      stepAside,
      signals: window_.signals,
      reducedMotion: () => false,
      now: () => Date.now(),
      ...options,
    });
    return splash;
  };

  beforeEach(() => {
    vi.useFakeTimers({ now: 0 });
    document.body.replaceChildren();
    shell = document.createElement("div");
    play = document.createElement("button");
    play.textContent = "Play";
    shell.append(play);
    host = document.createElement("div");
    host.hidden = true;
    document.body.append(shell, host);
    window_ = foreground();
    stepAside = vi.fn(() => Promise.resolve());
  });

  afterEach(() => {
    splash?.dispose();
    vi.useRealTimers();
  });

  it("covers the shell with the game's own art and wordmark, and takes it out of reach", () => {
    mount().open(hades);

    expect(host.hidden).toBe(false);
    expect(host.classList.contains("is-open")).toBe(true);
    expect(host.querySelector<HTMLImageElement>(".launch-splash__image")!.src).toBe(hades.artUrl);
    const logo = host.querySelector<HTMLImageElement>(".launch-splash__logo")!;
    expect(logo.src).toBe(hades.logoUrl);
    // The wordmark carries the name; the title waits in the markup behind it.
    expect(logo.alt).toBe("Hades II");
    expect(host.querySelector(".launch-splash__title")!.textContent).toBe("Hades II");
    expect(host.querySelector(".launch-splash__eyebrow")!.textContent).toBe("Starting");
    expect(shell.inert).toBe(true);
    expect(splash.isOpen()).toBe(true);
  });

  it("gives the title back when the wordmark does not load", () => {
    mount().open(hades);
    const content = host.querySelector(".launch-splash__content")!;
    expect(content.classList.contains("has-logo")).toBe(true);

    host.querySelector(".launch-splash__logo")!.dispatchEvent(new Event("error"));

    expect(host.querySelector(".launch-splash__logo")).toBeNull();
    expect(content.classList.contains("has-logo")).toBe(false);
  });

  it("shows the title alone for a game no store published a wordmark for", () => {
    mount().open({ ...hades, logoUrl: "" });

    expect(host.querySelector(".launch-splash__logo")).toBeNull();
    expect(host.querySelector(".launch-splash__content")!.classList.contains("has-logo")).toBe(false);
  });

  it("steps aside once a game Orivo started takes the foreground, after its beat", async () => {
    mount().open(hades);
    splash.launched(hades.id, "process");
    await vi.advanceTimersByTimeAsync(300);
    window_.send("blur");

    // The game is up, but the splash has not had its beat yet.
    await vi.advanceTimersByTimeAsync(holdMs - 400);
    expect(stepAside).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(100);
    expect(stepAside).toHaveBeenCalledTimes(1);
    // Nothing more to answer once Orivo has asked to leave.
    expect(splash.isOpen()).toBe(false);
    expect(window_.watching).toBe(0);

    // The splash outlives the request long enough for the window to animate
    // away with it, and is gone before the player can come back to it.
    await vi.advanceTimersByTimeAsync(settleMs);
    expect(host.hidden).toBe(true);
    expect(host.childElementCount).toBe(0);
    expect(shell.inert).toBe(false);
    // The deadline was cancelled with everything else: one request, ever.
    await vi.advanceTimersByTimeAsync(deadlineMs);
    expect(stepAside).toHaveBeenCalledTimes(1);
  });

  it("counts a foreground lost before the host has even answered", async () => {
    mount().open(hades);
    window_.send("blur");
    await vi.advanceTimersByTimeAsync(holdMs + 200);
    expect(stepAside).not.toHaveBeenCalled();

    // The launch was accepted after the game came up: there is no beat left.
    splash.launched(hades.id, "process");
    expect(stepAside).toHaveBeenCalledTimes(1);
  });

  it("does not take a store client coming forward for the game", async () => {
    mount().open(hades);
    splash.launched(hades.id, "client");
    // `open` activates Steam at once; the game is nowhere yet.
    window_.send("blur");

    await vi.advanceTimersByTimeAsync(deadlineMs - 1);
    expect(stepAside).not.toHaveBeenCalled();
    expect(splash.isOpen()).toBe(true);
  });

  it("steps aside the moment the game covers Orivo, whoever started it", async () => {
    mount().open(hades);
    splash.launched(hades.id, "client");
    await vi.advanceTimersByTimeAsync(200);

    window_.send("hidden");

    // Nobody can see the splash any more, so there is no beat to wait out.
    expect(stepAside).toHaveBeenCalledTimes(1);
  });

  it("steps aside after the deadline when nothing ever says the game is up", async () => {
    mount().open(hades);
    splash.launched(hades.id, "client");

    await vi.advanceTimersByTimeAsync(deadlineMs);

    // A game that opened behind Orivo is not left there.
    expect(stepAside).toHaveBeenCalledTimes(1);
  });

  it("waits for a watched game's window, however long it loads, and steps aside when it is up", async () => {
    mount().open(hades);
    splash.launched(hades.id, "window");
    // Losing focus is not the game: the player may be clicking elsewhere.
    window_.send("blur");
    await vi.advanceTimersByTimeAsync(deadlineMs * 2);
    expect(stepAside).not.toHaveBeenCalled();
    expect(splash.isOpen()).toBe(true);

    splash.gameWindow(hades.id, true);
    expect(stepAside).toHaveBeenCalledTimes(1);
  });

  it("goes back to Orivo when a watched game ends before its window", async () => {
    mount().open(hades);
    splash.launched(hades.id, "window");
    splash.gameWindow("steam:1", false);
    expect(splash.isOpen()).toBe(true);

    splash.gameWindow(hades.id, false);
    expect(splash.isOpen()).toBe(false);
    await vi.advanceTimersByTimeAsync(windowDeadlineMs);
    expect(stepAside).not.toHaveBeenCalled();
  });

  it("still steps aside for a watched game whose host never says", async () => {
    mount().open(hades);
    splash.launched(hades.id, "window");
    await vi.advanceTimersByTimeAsync(windowDeadlineMs);
    expect(stepAside).toHaveBeenCalledTimes(1);
  });

  it("stays when the player asked Orivo to, and fades back to the library after its beat", async () => {
    mount({ shouldStepAside: () => false }).open(hades);
    splash.launched(hades.id, "process");
    window_.send("blur");

    await vi.advanceTimersByTimeAsync(holdMs - 1);
    expect(splash.isOpen()).toBe(true);
    await vi.advanceTimersByTimeAsync(1);
    expect(splash.isOpen()).toBe(false);
    // The shell is live again as the splash fades, not after.
    expect(shell.inert).toBe(false);
    expect(host.classList.contains("is-open")).toBe(false);
    expect(host.hidden).toBe(false);

    await vi.advanceTimersByTimeAsync(fadeMs);
    expect(host.hidden).toBe(true);
    await vi.advanceTimersByTimeAsync(deadlineMs);
    expect(stepAside).not.toHaveBeenCalled();
  });

  it("stays when the player comes back to Orivo while the game starts", async () => {
    mount().open(hades);
    splash.launched(hades.id, "client");
    window_.send("blur");
    await vi.advanceTimersByTimeAsync(2_000);

    window_.send("focus");

    expect(splash.isOpen()).toBe(false);
    await vi.advanceTimersByTimeAsync(deadlineMs);
    expect(stepAside).not.toHaveBeenCalled();
  });

  it("stays when the player came back before the host had even answered", async () => {
    mount().open(hades);
    window_.send("blur");
    window_.send("focus");
    splash.launched(hades.id, "process");

    await vi.advanceTimersByTimeAsync(deadlineMs);

    expect(stepAside).not.toHaveBeenCalled();
    expect(splash.isOpen()).toBe(false);
  });

  it("gets out of the way of a launch that failed, and never steps aside for it", async () => {
    mount().open(hades);
    splash.failed(hades.id);

    expect(splash.isOpen()).toBe(false);
    expect(shell.inert).toBe(false);
    await vi.advanceTimersByTimeAsync(fadeMs);
    expect(host.hidden).toBe(true);
    await vi.advanceTimersByTimeAsync(deadlineMs);
    expect(stepAside).not.toHaveBeenCalled();
  });

  it("stops waiting when the game fails after the launch was accepted", async () => {
    mount().open(hades);
    splash.launched(hades.id, "process");
    await vi.advanceTimersByTimeAsync(4_000);

    // Wine gave up inside its startup window.
    splash.failed(hades.id);

    expect(splash.isOpen()).toBe(false);
    await vi.advanceTimersByTimeAsync(deadlineMs);
    expect(stepAside).not.toHaveBeenCalled();
  });

  it("answers only for the game it is starting", () => {
    mount().open(hades);

    splash.failed("steam:620");
    splash.launched("steam:620", "process");
    window_.send("blur");

    expect(splash.isOpen()).toBe(true);
    expect(stepAside).not.toHaveBeenCalled();
  });

  it("lets Escape take the player back while the launch carries on", async () => {
    mount().open(hades);
    splash.dismiss();
    expect(splash.isOpen()).toBe(false);

    // The host answers after the player left the splash: nothing reopens and
    // nothing steps aside behind their back.
    splash.launched(hades.id, "process");
    window_.send("blur");
    await vi.advanceTimersByTimeAsync(deadlineMs);
    expect(stepAside).not.toHaveBeenCalled();
    expect(host.hidden).toBe(true);
  });

  it("puts focus back on the control that started the game", async () => {
    play.focus();
    mount().open(hades);
    splash.dismiss();

    expect(document.activeElement).toBe(play);
  });

  it("does not fade under reduced motion: it is there, then it is not", () => {
    mount({ reducedMotion: () => true }).open(hades);

    splash.failed(hades.id);

    expect(host.hidden).toBe(true);
    expect(host.childElementCount).toBe(0);
  });

  it("leaves Orivo where it was when the host cannot step aside", async () => {
    stepAside.mockRejectedValueOnce(new Error("no window"));
    mount().open(hades);
    splash.launched(hades.id, "process");
    window_.send("hidden");

    await vi.advanceTimersByTimeAsync(0);
    expect(shell.inert).toBe(false);
    await vi.advanceTimersByTimeAsync(fadeMs);
    expect(host.hidden).toBe(true);
  });

  it("starts afresh when a second game is started over the first", () => {
    mount().open(hades);
    splash.open({ ...hades, id: "steam:620", title: "Portal 2", logoUrl: "" });

    expect(host.querySelectorAll(".launch-splash__content")).toHaveLength(1);
    expect(host.querySelector(".launch-splash__title")!.textContent).toBe("Portal 2");
    expect(window_.watching).toBe(1);
  });

  it("lets the shell go when it is torn down mid-launch", () => {
    mount().open(hades);
    splash.dispose();

    expect(shell.inert).toBe(false);
    expect(host.hidden).toBe(true);
    expect(window_.watching).toBe(0);
  });
});

/** Strings whose way in and way out the test finishes when it chooses. */
function fakeWeave() {
  let endDive = (): void => undefined;
  let endUnfold = (): void => undefined;
  const weave = {
    start: vi.fn(),
    stage: vi.fn(),
    dive: vi.fn(() => new Promise<void>((resolve) => (endDive = resolve))),
    unfold: vi.fn(() => new Promise<void>((resolve) => (endUnfold = resolve))),
    stop: vi.fn(),
  } satisfies LaunchWeave;
  return { weave, endDive: () => endDive(), endUnfold: () => endUnfold() };
}

describe("streaming a game", () => {
  let host: HTMLElement;
  let shell: HTMLElement;
  let window_: ReturnType<typeof foreground>;
  let stepAside: ReturnType<typeof vi.fn<() => Promise<void>>>;
  let streamControl: ReturnType<typeof vi.fn<(gameId: string, action: string) => void>>;
  let strings: ReturnType<typeof fakeWeave>;
  let splash: LaunchSplash;

  const mount = (options: Partial<LaunchSplashOptions> = {}): LaunchSplash => {
    splash = createLaunchSplash({
      host,
      covers: [shell],
      shouldStepAside: () => true,
      stepAside,
      signals: window_.signals,
      reducedMotion: () => false,
      now: () => Date.now(),
      streamControl,
      createWeave: () => strings.weave,
      ...options,
    });
    return splash;
  };
  const status = (): string => host.querySelector(".launch-splash__status")?.textContent ?? "";
  const button = (label: string): HTMLButtonElement =>
    [...host.querySelectorAll<HTMLButtonElement>(".launch-splash__answer")].find(
      (candidate) => candidate.textContent === label,
    )!;

  beforeEach(() => {
    vi.useFakeTimers({ now: 0 });
    document.body.replaceChildren();
    shell = document.createElement("div");
    host = document.createElement("div");
    host.hidden = true;
    document.body.append(shell, host);
    window_ = foreground();
    stepAside = vi.fn(() => Promise.resolve());
    streamControl = vi.fn();
    strings = fakeWeave();
  });

  afterEach(() => {
    splash?.dispose();
    vi.useRealTimers();
  });

  it("stands in front of Moonlight until the first picture, saying each step", async () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    expect(strings.weave.start).toHaveBeenCalledTimes(1);
    expect(status()).toBe("Reaching your PC");

    // Moonlight taking the foreground and covering Orivo says nothing here.
    window_.send("blur");
    window_.send("hidden");
    await vi.advanceTimersByTimeAsync(deadlineMs * 10);
    expect(stepAside).not.toHaveBeenCalled();
    expect(splash.isOpen()).toBe(true);

    splash.stream(hades.id, { stage: "launching", resumed: true });
    expect(status()).toBe("Resuming on your PC");
    splash.stream(hades.id, { stage: "handshake" });
    expect(status()).toBe("Opening the stream");
    expect(strings.weave.stage).toHaveBeenLastCalledWith("handshake");
  });

  it("opens onto the stream when its first picture is up, then steps aside", async () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "visible" });

    expect(strings.weave.dive).toHaveBeenCalledTimes(1);
    // The seam is still opening: Orivo has not gone, and nothing else starts.
    expect(stepAside).not.toHaveBeenCalled();
    expect(splash.isOpen()).toBe(true);

    strings.endDive();
    await vi.advanceTimersByTimeAsync(0);
    expect(stepAside).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(settleMs);
    expect(host.hidden).toBe(true);
    expect(shell.inert).toBe(false);
  });

  it("lets the stream have the screen when the player asked Orivo to stay", async () => {
    mount({ shouldStepAside: () => false }).open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "visible" });
    strings.endDive();
    await vi.advanceTimersByTimeAsync(0);

    expect(stepAside).not.toHaveBeenCalled();
    expect(streamControl).toHaveBeenCalledWith(hades.id, "uncover");
    expect(host.hidden).toBe(true);
  });

  it("asks in its own words when the PC is running another game, and closes it on the player's word", () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "host-busy" });

    expect(host.querySelector(".launch-splash__question-text")!.textContent).toBe(
      "Another game is open on your PC",
    );
    expect(document.activeElement).toBe(button("Close it and play"));
    button("Close it and play").click();

    expect(streamControl).toHaveBeenCalledWith(hades.id, "replace");
    expect(host.querySelector(".launch-splash__question")).toBeNull();
    expect(status()).toBe("Closing the other game");
    expect(splash.isOpen()).toBe(true);
  });

  it("drops the question when Moonlight moves on by itself", () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "host-busy" });
    splash.stream(hades.id, { stage: "connecting" });

    expect(host.querySelector(".launch-splash__question")).toBeNull();
    expect(status()).toBe("Reaching your PC");
  });

  it("stops the stream and gives the library back on Cancel, or on Escape while it asks", () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "host-busy" });
    button("Cancel").click();
    expect(streamControl).toHaveBeenCalledWith(hades.id, "cancel");
    expect(splash.isOpen()).toBe(false);

    streamControl.mockClear();
    splash.open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "host-busy" });
    splash.dismiss();
    expect(streamControl).toHaveBeenCalledWith(hades.id, "cancel");
  });

  it("lets Moonlight in front when the player leaves a stream that is only loading", () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    splash.dismiss();

    expect(streamControl).toHaveBeenCalledWith(hades.id, "uncover");
    expect(splash.isOpen()).toBe(false);
  });

  it("gets out of Moonlight's way when it stalls, and out of the way of a failure", async () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "stalled" });
    expect(stepAside).toHaveBeenCalledTimes(1);

    splash.open(hades);
    splash.launched(hades.id, "stream");
    splash.stream(hades.id, { stage: "failed" });
    expect(splash.isOpen()).toBe(false);
    expect(stepAside).toHaveBeenCalledTimes(1);
  });

  it("takes steps for the stream it started only", () => {
    mount().open(hades);
    splash.launched(hades.id, "stream");
    splash.stream("steam:1", { stage: "visible" });

    expect(strings.weave.dive).not.toHaveBeenCalled();
  });

  it("comes back from a stream the way it went in, then gives the library back", async () => {
    mount();
    splash.welcomeBack(hades, 2_520);

    expect(host.hidden).toBe(false);
    expect(host.classList.contains("is-return")).toBe(true);
    expect(host.querySelector(".launch-splash__eyebrow")!.textContent).toBe("Stream ended");
    expect(status()).toBe("42 min");
    expect(strings.weave.unfold).toHaveBeenCalledTimes(1);
    expect(shell.inert).toBe(true);

    strings.endUnfold();
    await vi.advanceTimersByTimeAsync(900 + fadeMs);
    expect(host.hidden).toBe(true);
    expect(shell.inert).toBe(false);
  });
});

describe("how long a stream lasted", () => {
  it("reads in minutes, then in hours and minutes", () => {
    expect(streamDuration(20)).toBe("1 min");
    expect(streamDuration(2_520)).toBe("42 min");
    expect(streamDuration(3_900)).toBe("1 h 05 min");
  });
});

describe("the host's launch answer", () => {
  it("reads who puts the game on screen, and assumes Orivo did when it does not say", () => {
    expect(readLaunchHandoff({ handoff: "stream" })).toBe("stream");
    expect(readLaunchHandoff({ status: "Launching Spacewar", handoff: "client" })).toBe("client");
    expect(readLaunchHandoff({ status: "Launching Spacewar", handoff: "process" })).toBe("process");
    expect(readLaunchHandoff({ status: "Launching Spacewar", handoff: "window" })).toBe("window");
    expect(readLaunchHandoff({ status: "Launching Spacewar" })).toBe("process");
    expect(readLaunchHandoff(undefined)).toBe("process");
    expect(readLaunchHandoff({ handoff: "steam" })).toBe("process");
  });
});

describe("what a WebView is told about the foreground", () => {
  it("passes on focus and visibility until it is told to stop", () => {
    const heard: ForegroundSignal[] = [];
    const stop = windowForegroundSignals().watch((signal) => heard.push(signal));

    window.dispatchEvent(new Event("blur"));
    window.dispatchEvent(new Event("focus"));
    document.dispatchEvent(new Event("visibilitychange"));
    stop();
    window.dispatchEvent(new Event("blur"));

    expect(heard).toEqual(["blur", "focus", document.visibilityState === "hidden" ? "hidden" : "visible"]);
  });
});
