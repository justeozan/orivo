import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  BOOT_ARTWORK_GRACE_MS,
  BOOT_CAP_MS,
  BOOT_HOLD_MS,
  createBootSplash,
  revealBootSplash,
  startsAnApp,
} from "./boot-splash";

/** A session store that starts empty, like a fresh start of the app. */
function memory(initial: Record<string, string> = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
}

describe("the startup animation", () => {
  let element: HTMLElement;

  const leaving = (): boolean => element.classList.contains("is-leaving");
  const gone = (): boolean => !element.isConnected;

  beforeEach(() => {
    vi.useFakeTimers({ now: 0 });
    document.body.replaceChildren();
    element = document.createElement("div");
    element.id = "boot-splash";
    document.body.append(element);
  });

  afterEach(() => {
    vi.useRealTimers();
    delete document.documentElement.dataset.booting;
  });

  const start = (options: Parameters<typeof createBootSplash>[0] = {}) =>
    createBootSplash({
      element,
      now: () => Date.now(),
      reducedMotion: () => false,
      memory: memory(),
      ...options,
    });

  it("holds the mark until its arrival has played, even when the library answers at once", async () => {
    const splash = start();
    await vi.advanceTimersByTimeAsync(200);
    splash.release();

    await vi.advanceTimersByTimeAsync(BOOT_HOLD_MS - 201);
    expect(leaving()).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(leaving()).toBe(true);
  });

  it("leaves as soon as a slow library answers, its arrival long over", async () => {
    const splash = start();
    await vi.advanceTimersByTimeAsync(BOOT_HOLD_MS + 900);
    expect(leaving()).toBe(false);

    splash.release();
    await vi.advanceTimersByTimeAsync(0);
    expect(leaving()).toBe(true);
  });

  it("is removed once its exit has played", async () => {
    const splash = start();
    splash.release();
    await vi.advanceTimersByTimeAsync(BOOT_HOLD_MS);
    expect(gone()).toBe(false);

    const ended = new Event("transitionend");
    element.dispatchEvent(ended);
    expect(gone()).toBe(true);
  });

  it("is removed even when its exit never reports", async () => {
    const splash = start();
    splash.release();
    await vi.advanceTimersByTimeAsync(BOOT_HOLD_MS + 1_000);

    expect(gone()).toBe(true);
  });

  it("does not take a child's transition for its own exit", async () => {
    const child = document.createElement("span");
    element.append(child);
    const splash = start();
    splash.release();
    await vi.advanceTimersByTimeAsync(BOOT_HOLD_MS);

    child.dispatchEvent(new Event("transitionend", { bubbles: true }));

    expect(gone()).toBe(false);
  });

  it("waits for the scene's artwork, so it uncovers the picture", async () => {
    let decoded!: () => void;
    const artwork = new Promise<void>((resolve) => (decoded = resolve));
    const splash = start({ holdMs: 0 });
    splash.release(artwork);

    await vi.advanceTimersByTimeAsync(BOOT_ARTWORK_GRACE_MS - 100);
    expect(leaving()).toBe(false);
    decoded();
    await vi.advanceTimersByTimeAsync(0);
    expect(leaving()).toBe(true);
  });

  it("waits for the artwork only briefly", async () => {
    const splash = start({ holdMs: 0 });
    splash.release(new Promise(() => {}));

    await vi.advanceTimersByTimeAsync(BOOT_ARTWORK_GRACE_MS);

    expect(leaving()).toBe(true);
  });

  it("takes artwork that failed to load as an answer too", async () => {
    const splash = start({ holdMs: 0 });
    splash.release(Promise.reject(new Error("404")));

    await vi.advanceTimersByTimeAsync(0);

    expect(leaving()).toBe(true);
  });

  it("never stands in front of a library that does not answer for longer than its cap", async () => {
    start();

    await vi.advanceTimersByTimeAsync(BOOT_CAP_MS - 1);
    expect(leaving()).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(leaving()).toBe(true);
  });

  it("has nothing to wait for under reduced motion, and no exit to play", async () => {
    const splash = start({ reducedMotion: () => true });
    splash.release();
    await vi.advanceTimersByTimeAsync(0);

    expect(gone()).toBe(true);
  });

  it("plays its whole arrival once a session, and gets out of the way on a reload", async () => {
    const session = memory();
    start({ memory: session });
    document.body.append((element = document.createElement("div")));

    const reloaded = start({ memory: session });
    reloaded.release();
    await vi.advanceTimersByTimeAsync(0);

    expect(leaving()).toBe(true);
  });

  it("holds back what is staged behind it until the dark starts to lift", async () => {
    const splash = start();
    expect(document.documentElement.dataset.booting).toBe("");

    splash.release();
    await vi.advanceTimersByTimeAsync(BOOT_HOLD_MS - 1);
    expect(document.documentElement.dataset.booting).toBe("");
    await vi.advanceTimersByTimeAsync(1);
    // Lifted with the exit, not after it: the arrival plays as the scene appears.
    expect(document.documentElement.dataset.booting).toBeUndefined();
    expect(gone()).toBe(false);
  });

  it("does nothing at all in a document without one", () => {
    element.remove();
    const splash = createBootSplash();
    expect(() => splash.release()).not.toThrow();
    expect(document.documentElement.dataset.booting).toBeUndefined();
  });
});

describe("when the startup animation plays at all", () => {
  it("plays when the app starts, and in a browser only when asked for", () => {
    expect(startsAnApp(true, "")).toBe(true);
    // The preview and the end-to-end suite reload all day.
    expect(startsAnApp(false, "")).toBe(false);
    expect(startsAnApp(false, "?library=empty")).toBe(false);
    expect(startsAnApp(false, "?boot")).toBe(true);
    expect(startsAnApp(false, "?library=empty&boot")).toBe(true);
  });

  it("is revealed for an app starting, and gone for good otherwise", () => {
    document.body.replaceChildren();
    const held = document.createElement("div");
    held.hidden = true;
    document.body.append(held);

    revealBootSplash(held, true);
    expect(held.hidden).toBe(false);

    revealBootSplash(held, false);
    expect(held.isConnected).toBe(false);
    expect(() => revealBootSplash(null, true)).not.toThrow();
  });
});
