import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  LAUNCH_SPLASH_TIMING,
  createLaunchSplash,
  readLaunchHandoff,
  windowForegroundSignals,
  type ForegroundSignal,
  type LaunchSplash,
  type LaunchSplashOptions,
} from "./launch-splash";

const { holdMs, deadlineMs, fadeMs, settleMs } = LAUNCH_SPLASH_TIMING;

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

describe("the host's launch answer", () => {
  it("reads who puts the game on screen, and assumes Orivo did when it does not say", () => {
    expect(readLaunchHandoff({ status: "Launching Spacewar", handoff: "client" })).toBe("client");
    expect(readLaunchHandoff({ status: "Launching Spacewar", handoff: "process" })).toBe("process");
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
