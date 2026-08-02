import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GamepadBridge } from "./gamepad";
import type { NavDirection } from "./spatial-nav";
import { createGamepadBridge } from "./gamepad";

const BUTTON_COUNT = 17;

const pad = (options: { buttons?: number[]; axes?: number[] } = {}): Gamepad => {
  const pressed = new Set(options.buttons ?? []);
  return {
    buttons: Array.from({ length: BUTTON_COUNT }, (_, index) => ({
      pressed: pressed.has(index),
      touched: pressed.has(index),
      value: pressed.has(index) ? 1 : 0,
    })),
    axes: options.axes ?? [0, 0, 0, 0],
    connected: true,
    id: "test pad (STANDARD GAMEPAD)",
    index: 0,
    mapping: "standard",
    timestamp: 0,
  } as unknown as Gamepad;
};

describe("gamepad bridge", () => {
  let actions: {
    move: ReturnType<typeof vi.fn<(direction: NavDirection) => void>>;
    activate: ReturnType<typeof vi.fn<() => void>>;
    back: ReturnType<typeof vi.fn<() => void>>;
    launch: ReturnType<typeof vi.fn<() => void>>;
    focusSearch: ReturnType<typeof vi.fn<() => void>>;
    cycleNav: ReturnType<typeof vi.fn<(delta: number) => void>>;
    scroll: ReturnType<typeof vi.fn<(delta: number) => void>>;
    onActivity: ReturnType<typeof vi.fn<() => void>>;
  };
  let bridge: GamepadBridge;
  let frame: ((now: number) => void) | null;
  let pads: (Gamepad | null)[];

  /** Drive exactly one poll, so button edges are deterministic. */
  const tick = (now: number): void => {
    const next = frame;
    frame = null;
    next?.(now);
  };

  beforeEach(() => {
    pads = [];
    frame = null;
    vi.stubGlobal("requestAnimationFrame", (callback: (now: number) => void): number => {
      frame = callback;
      return 1;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {
      frame = null;
    });
    navigator.getGamepads = () => pads;

    actions = {
      move: vi.fn<(direction: NavDirection) => void>(),
      activate: vi.fn<() => void>(),
      back: vi.fn<() => void>(),
      launch: vi.fn<() => void>(),
      focusSearch: vi.fn<() => void>(),
      cycleNav: vi.fn<(delta: number) => void>(),
      scroll: vi.fn<(delta: number) => void>(),
      onActivity: vi.fn<() => void>(),
    };
    bridge = createGamepadBridge(actions);
    bridge.start();
  });

  afterEach(() => {
    bridge.destroy();
    vi.unstubAllGlobals();
  });

  it("flags the body while a pad is connected", () => {
    expect(document.body.dataset.gamepad).toBe("connected");
    bridge.stop();
    expect(document.body.dataset.gamepad).toBeUndefined();
  });

  it("maps the d-pad to the four directions", () => {
    pads = [pad({ buttons: [15] })];
    tick(0);
    expect(actions.move).toHaveBeenCalledWith("right");

    pads = [pad({ buttons: [13] })];
    tick(16);
    expect(actions.move).toHaveBeenLastCalledWith("down");
  });

  it("fires a held direction once, then repeats after the delay", () => {
    pads = [pad({ buttons: [15] })];
    tick(0);
    expect(actions.move).toHaveBeenCalledTimes(1);

    tick(200);
    tick(400);
    expect(actions.move).toHaveBeenCalledTimes(1);

    tick(500);
    expect(actions.move).toHaveBeenCalledTimes(2);

    tick(560);
    expect(actions.move).toHaveBeenCalledTimes(2);

    tick(640);
    expect(actions.move).toHaveBeenCalledTimes(3);
  });

  it("ignores stick movement inside the dead zone", () => {
    pads = [pad({ axes: [0.4, 0, 0, 0] })];
    tick(0);
    expect(actions.move).not.toHaveBeenCalled();

    pads = [pad({ axes: [0.9, 0, 0, 0] })];
    tick(16);
    expect(actions.move).toHaveBeenCalledWith("right");
  });

  it("resolves a diagonal stick push to its dominant axis", () => {
    pads = [pad({ axes: [0.6, 0.95, 0, 0] })];
    tick(0);
    expect(actions.move).toHaveBeenCalledExactlyOnceWith("down");
  });

  it("fires face buttons once per press, not once per frame", () => {
    pads = [pad({ buttons: [0] })];
    tick(0);
    tick(16);
    tick(32);
    expect(actions.activate).toHaveBeenCalledTimes(1);

    pads = [pad()];
    tick(48);
    pads = [pad({ buttons: [0] })];
    tick(64);
    expect(actions.activate).toHaveBeenCalledTimes(2);
  });

  it("maps B to back, X to launch and Y to search", () => {
    pads = [pad({ buttons: [1] })];
    tick(0);
    pads = [pad({ buttons: [2] })];
    tick(16);
    pads = [pad({ buttons: [3] })];
    tick(32);

    expect(actions.back).toHaveBeenCalledTimes(1);
    expect(actions.launch).toHaveBeenCalledTimes(1);
    expect(actions.focusSearch).toHaveBeenCalledTimes(1);
  });

  it("walks the top-level pages with the shoulder buttons", () => {
    pads = [pad({ buttons: [4] })];
    tick(0);
    pads = [pad({ buttons: [5] })];
    tick(16);

    expect(actions.cycleNav).toHaveBeenNthCalledWith(1, -1);
    expect(actions.cycleNav).toHaveBeenNthCalledWith(2, 1);
  });

  it("scrolls with the right stick", () => {
    pads = [pad({ axes: [0, 0, 0, 0.8] })];
    tick(0);
    expect(actions.scroll).toHaveBeenCalledOnce();
    expect(actions.scroll.mock.calls[0]?.[0]).toBeGreaterThan(0);
  });

  it("reports activity so the focus ring can switch to controller mode", () => {
    pads = [pad({ buttons: [0] })];
    tick(0);
    expect(actions.onActivity).toHaveBeenCalled();
  });

  it("does nothing at all with no pad attached", () => {
    pads = [null];
    tick(0);
    tick(16);
    expect(actions.move).not.toHaveBeenCalled();
    expect(actions.activate).not.toHaveBeenCalled();
    expect(actions.onActivity).not.toHaveBeenCalled();
  });
});
