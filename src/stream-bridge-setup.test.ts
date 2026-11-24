// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import {
  bridgeStatuses,
  createStreamBridgeSetup,
  mountStreamBridgeSetup,
  readBridgeView,
  readHostStatus,
  type StreamBridgeDeps,
} from "./stream-bridge-setup";

const NONE = { available: true, installed: false, moonlight: null };
const READY = { available: true, installed: true, moonlight: true };

function fakeHost(answers: Record<string, unknown>) {
  const calls: { command: string; args?: Record<string, unknown> }[] = [];
  const timers: (() => void)[] = [];
  const deps: StreamBridgeDeps = {
    call: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push({ command, args });
      const answer = answers[command];
      if (answer instanceof Error) throw answer;
      return (typeof answer === "function" ? (answer as (a?: unknown) => unknown)(args) : answer) as T;
    },
    openLink: vi.fn(async () => {}),
    showToast: vi.fn(),
    onImported: vi.fn(),
    openLibrary: vi.fn(),
    setTimer: (callback) => {
      timers.push(callback);
      return timers.length;
    },
    clearTimer: () => {},
  };
  return { deps, calls, timers };
}

describe("reading the host's answers", () => {
  it("keeps only what it understands", () => {
    expect(readBridgeView(null)).toEqual({ available: false, installed: false, moonlight: null });
    expect(readBridgeView({ available: true, installed: true, moonlight: "yes" })).toEqual({
      available: true,
      installed: true,
      moonlight: null,
    });
    expect(readHostStatus({ state: "paired", host: "astra", games: 14.7 })).toEqual({
      state: "paired",
      host: "astra",
      games: 14,
    });
    expect(readHostStatus({ state: "asleep" })).toBeNull();
  });
});

describe("which step is open", () => {
  it("asks for the permission before anything else", () => {
    expect(bridgeStatuses(NONE, null, "", false)).toEqual({
      moonlight: "active",
      host: "todo",
      pair: "todo",
      games: "todo",
    });
  });

  it("asks for Moonlight when the script found none", () => {
    expect(bridgeStatuses({ ...READY, moonlight: false }, null, "astra", false).moonlight).toBe("active");
  });

  it("goes to the PC, then pairing, then the games", () => {
    expect(bridgeStatuses(READY, null, "", false).host).toBe("active");
    const notPaired = { state: "notPaired" as const, host: "astra", games: 0 };
    expect(bridgeStatuses(READY, notPaired, "astra", false)).toMatchObject({ host: "done", pair: "active" });
    const paired = { state: "paired" as const, host: "astra", games: 3 };
    expect(bridgeStatuses(READY, paired, "astra", false)).toMatchObject({ pair: "done", games: "active" });
    expect(bridgeStatuses(READY, paired, "astra", true).games).toBe("done");
  });

  it("keeps a PC that does not answer on the PC step", () => {
    const off = { state: "unreachable" as const, host: "astra", games: 0 };
    expect(bridgeStatuses(READY, off, "astra", false).host).toBe("active");
  });
});

describe("the setup, end to end against a fake host", () => {
  it("allows, finds the PC, pairs with a PIN and imports the games", async () => {
    let installed = false;
    let paired = false;
    const { deps, calls, timers } = fakeHost({
      get_moonlight_bridge: () => (installed ? READY : NONE),
      get_gamestream_settings: { host: "" },
      install_moonlight_bridge: () => {
        installed = true;
      },
      check_streaming_pc: (args: { host: string }) => ({
        state: paired ? "paired" : "notPaired",
        host: args.host,
        games: paired ? 3 : 0,
      }),
      pair_streaming_pc: { host: "astra", pin: "0427" },
      import_streaming_games: 3,
    });
    const setup = createStreamBridgeSetup(deps);
    await setup.open();
    expect(setup.snapshot().statuses.moonlight).toBe("active");

    await setup.allow();
    expect(setup.snapshot().statuses).toMatchObject({ moonlight: "done", host: "active" });

    setup.setHostInput(" astra ");
    await setup.connect();
    expect(calls.find((call) => call.command === "check_streaming_pc")?.args).toEqual({ host: "astra" });
    expect(setup.snapshot().statuses).toMatchObject({ host: "done", pair: "active" });

    await setup.startPairing();
    expect(setup.snapshot().pin).toBe("0427");
    paired = true;
    timers.shift()?.();
    await vi.waitFor(() => expect(setup.snapshot().pin).toBeNull());
    expect(setup.snapshot().statuses).toMatchObject({ pair: "done", games: "active" });

    await setup.getGames();
    expect(setup.snapshot().games).toEqual({ phase: "ready", count: 3 });
    expect(deps.onImported).toHaveBeenCalledOnce();
    expect(setup.snapshot().statuses.games).toBe("done");
  });

  it("goes straight to the games when Moonlight already knows the saved PC", async () => {
    const { deps } = fakeHost({
      get_moonlight_bridge: READY,
      get_gamestream_settings: { host: "astra" },
      check_streaming_pc: { state: "paired", host: "astra", games: 12 },
    });
    const setup = createStreamBridgeSetup(deps);
    await setup.open();
    expect(setup.snapshot().statuses).toMatchObject({
      moonlight: "done",
      host: "done",
      pair: "done",
      games: "active",
    });
  });

  it("says why when the player closes the panel without allowing", async () => {
    const { deps } = fakeHost({
      get_moonlight_bridge: NONE,
      get_gamestream_settings: { host: "" },
      install_moonlight_bridge: new Error("Nothing was installed."),
    });
    const setup = createStreamBridgeSetup(deps);
    await setup.open();
    await setup.allow();
    expect(setup.snapshot().error).toBe("Nothing was installed.");
    expect(setup.snapshot().statuses.moonlight).toBe("active");
  });

  it("names a PC that does not answer", async () => {
    const { deps } = fakeHost({
      get_moonlight_bridge: READY,
      get_gamestream_settings: { host: "" },
      check_streaming_pc: { state: "unreachable", host: "astra", games: 0 },
    });
    const setup = createStreamBridgeSetup(deps);
    await setup.open();
    setup.setHostInput("astra");
    await setup.connect();
    expect(setup.snapshot().error).toMatch(/reach|injoignable/i);
    expect(setup.snapshot().statuses.host).toBe("active");
  });
});

describe("the view", () => {
  it("offers Allow first, and nothing else until it is done", async () => {
    const { deps } = fakeHost({ get_moonlight_bridge: NONE, get_gamestream_settings: { host: "" } });
    const setup = createStreamBridgeSetup(deps);
    const root = document.createElement("div");
    mountStreamBridgeSetup(root, setup);
    await setup.open();
    const steps = [...root.querySelectorAll<HTMLElement>("[data-gs-step]")];
    expect(steps.map((step) => step.dataset.gsStep)).toEqual(["moonlight", "host", "pair", "games"]);
    expect(steps[0].getAttribute("aria-current")).toBe("step");
    expect(root.querySelector('[data-gs-action="allow-bridge"]')).not.toBeNull();
    expect(root.querySelector('[data-gs-action="connect"]')).toBeNull();
  });

  it("shows the PIN in large while waiting for the PC", async () => {
    const { deps } = fakeHost({
      get_moonlight_bridge: READY,
      get_gamestream_settings: { host: "astra" },
      check_streaming_pc: { state: "notPaired", host: "astra", games: 0 },
      pair_streaming_pc: { host: "astra", pin: "9031" },
    });
    const setup = createStreamBridgeSetup(deps);
    const root = document.createElement("div");
    mountStreamBridgeSetup(root, setup);
    await setup.open();
    await setup.startPairing();
    expect(root.querySelector(".gs-pin")?.textContent).toBe("9031");
    expect(root.querySelector('[data-gs-action="open-pin-page"]')).not.toBeNull();
  });
});
