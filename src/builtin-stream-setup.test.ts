import { describe, expect, it } from "vitest";
import {
  builtinComputerName,
  builtinSetupStage,
  builtinStepStatuses,
  createBuiltinStreamSetup,
  deviceNameFor,
  mountBuiltinStreamSetup,
  readPairingStatus,
  type BuiltinFacts,
  type BuiltinHostStatus,
  type BuiltinPairingStatus,
  type BuiltinStreamClient,
} from "./builtin-stream-setup";
import { SETUP_COPY } from "./gamestream-setup";

const NOTHING: BuiltinFacts = {
  hostSaved: false,
  hostReachable: false,
  paired: false,
  gamesImported: false,
};

describe("builtinSetupStage", () => {
  it("has no client step: it starts on the PC and moves one step at a time", () => {
    expect(builtinSetupStage(NOTHING)).toBe("host");
    expect(builtinSetupStage({ ...NOTHING, hostSaved: true })).toBe("host");
    expect(builtinSetupStage({ ...NOTHING, hostSaved: true, hostReachable: true })).toBe("pair");
    expect(
      builtinSetupStage({ hostSaved: true, hostReachable: true, paired: true, gamesImported: false }),
    ).toBe("games");
    expect(
      builtinSetupStage({ hostSaved: true, hostReachable: true, paired: true, gamesImported: true }),
    ).toBe("done");
  });

  it("counts being paired as the PC having answered", () => {
    expect(builtinSetupStage({ ...NOTHING, hostSaved: true, paired: true })).toBe("games");
  });

  it("marks steps done, active and todo around the current one", () => {
    expect(builtinStepStatuses({ ...NOTHING, hostSaved: true, hostReachable: true })).toEqual({
      host: "done",
      pair: "active",
      games: "todo",
    });
  });
});

describe("readPairingStatus", () => {
  it("reads the host's tagged answer and falls back to idle", () => {
    expect(readPairingStatus({ phase: "waiting", pin: "0420" })).toEqual({ phase: "waiting", pin: "0420" });
    expect(readPairingStatus({ phase: "paired" })).toEqual({ phase: "paired" });
    expect(readPairingStatus({ phase: "failed", message: "Nope." })).toEqual({
      phase: "failed",
      message: "Nope.",
    });
    expect(readPairingStatus({ phase: "failed" })).toEqual({
      phase: "failed",
      message: SETUP_COPY.pairingFailed,
    });
    expect(readPairingStatus(null)).toEqual({ phase: "idle" });
  });
});

describe("builtinComputerName", () => {
  it("does not call an iPhone a Mac, though its user agent says Mac OS X", () => {
    expect(builtinComputerName("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)")).toBe("this iPhone");
    expect(builtinComputerName("Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X)")).toBe("this iPad");
    expect(builtinComputerName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("this Mac");
  });
});

describe("deviceNameFor", () => {
  it("names the device the way Sunshine's client list will show it", () => {
    expect(deviceNameFor("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)")).toBe("Orivo (iPhone)");
    expect(deviceNameFor("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("Orivo (Mac)");
    expect(deviceNameFor("Mozilla/5.0 (Linux; Android 15)")).toBe("Orivo (Android)");
    expect(deviceNameFor("")).toBe("Orivo");
  });
});

/* ---------------------------------------------------------------------------
   The flow, against a recorded host
   --------------------------------------------------------------------------- */

interface FakeHost {
  client: BuiltinStreamClient;
  calls: string[];
  saved: { host: string; paired: boolean };
  probeAnswer: BuiltinHostStatus | Error;
  pairing: BuiltinPairingStatus[];
  games: { games: number; changed: boolean } | Error;
}

function fakeHost(): FakeHost {
  const host: FakeHost = {
    calls: [],
    saved: { host: "", paired: false },
    probeAnswer: { state: "notPaired", host: "astra.local", games: 0 },
    pairing: [],
    games: { games: 3, changed: true },
    client: null as unknown as BuiltinStreamClient,
  };
  host.client = {
    async settings() {
      host.calls.push("settings");
      return host.saved;
    },
    async probe(typed) {
      host.calls.push(`probe:${typed}`);
      if (host.probeAnswer instanceof Error) throw host.probeAnswer;
      if (host.probeAnswer.state !== "unreachable") host.saved.host = host.probeAnswer.host;
      return host.probeAnswer;
    },
    async beginPairing(name) {
      host.calls.push(`begin:${name}`);
      return { pin: "4821", host: host.saved.host };
    },
    async pairingStatus() {
      host.calls.push("status");
      return host.pairing.shift() ?? { phase: "waiting", pin: "4821" };
    },
    async cancelPairing() {
      host.calls.push("cancel");
    },
    async forget() {
      host.calls.push("forget");
      host.saved = { host: "", paired: false };
    },
    async syncGames() {
      host.calls.push("sync");
      if (host.games instanceof Error) throw host.games;
      return host.games;
    },
  };
  return host;
}

/** A clock and a timer queue the test advances by hand. */
function manualTimers() {
  let clock = 0;
  const queue: Array<{ at: number; run: () => void }> = [];
  return {
    setTimer: (run: () => void, ms: number) => {
      const entry = { at: clock + ms, run };
      queue.push(entry);
      return entry;
    },
    clearTimer: (handle: unknown) => {
      const index = queue.indexOf(handle as (typeof queue)[number]);
      if (index >= 0) queue.splice(index, 1);
    },
    now: () => clock,
    async tick(ms: number) {
      clock += ms;
      const due = queue.filter((entry) => entry.at <= clock);
      for (const entry of due) {
        queue.splice(queue.indexOf(entry), 1);
        entry.run();
      }
      for (let i = 0; i < 10; i += 1) await Promise.resolve();
    },
    pending: () => queue.length,
  };
}

function setupWith(host: FakeHost, timers = manualTimers()) {
  let refreshed = 0;
  let libraryOpened = 0;
  const setup = createBuiltinStreamSetup({
    client: host.client,
    refreshLibrary: () => {
      refreshed += 1;
    },
    openLibrary: () => {
      libraryOpened += 1;
    },
    computerName: "this Mac",
    deviceName: "Orivo (Mac)",
    setTimer: timers.setTimer,
    clearTimer: timers.clearTimer,
    now: timers.now,
    pairingPollMs: 1_000,
    pairingTimeoutMs: 10_000,
  });
  return {
    setup,
    timers,
    refreshed: () => refreshed,
    libraryOpened: () => libraryOpened,
  };
}

describe("the built-in setup flow", () => {
  it("asks for an address before asking the PC anything", async () => {
    const host = fakeHost();
    const { setup } = setupWith(host);
    await setup.open();
    await setup.connect();
    expect(setup.snapshot().host.error).toBe(SETUP_COPY.needHost);
    expect(host.calls).toEqual(["settings"]);
  });

  it("keeps a PC that does not answer off the record and says what to check", async () => {
    const host = fakeHost();
    host.probeAnswer = { state: "unreachable", host: "astra.local", games: 0 };
    const { setup } = setupWith(host);
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    const view = setup.snapshot();
    expect(view.stage).toBe("host");
    expect(view.host.saved).toBe("");
    expect(view.host.error).toBe(SETUP_COPY.unreachable);
  });

  it("tidies a pasted address, then goes straight to a PIN for a PC that does not know Orivo", async () => {
    const host = fakeHost();
    const { setup } = setupWith(host);
    await setup.open();
    setup.setHostInput("https://astra.local:47990/pin");
    await setup.connect();
    const view = setup.snapshot();
    expect(host.calls).toEqual(["settings", "probe:astra.local", "begin:Orivo (Mac)"]);
    expect(view.stage).toBe("pair");
    expect(view.host.saved).toBe("astra.local");
    expect(view.pairing).toMatchObject({ phase: "waiting", pin: "4821" });
  });

  it("watches the pairing, then imports the games and refreshes the Library", async () => {
    const host = fakeHost();
    host.pairing = [{ phase: "waiting", pin: "4821" }, { phase: "paired" }];
    const harness = setupWith(host);
    const { setup, timers } = harness;
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    await timers.tick(1_000);
    expect(setup.snapshot().stage).toBe("pair");
    await timers.tick(1_000);
    const view = setup.snapshot();
    expect(view.stage).toBe("done");
    expect(view.games).toMatchObject({ phase: "ready", count: 3 });
    expect(harness.refreshed()).toBe(1);
    expect(timers.pending()).toBe(0);
  });

  it("does not refresh the Library when the import changed nothing", async () => {
    const host = fakeHost();
    host.probeAnswer = { state: "paired", host: "astra.local", games: 3 };
    host.games = { games: 3, changed: false };
    const harness = setupWith(host);
    await harness.setup.open();
    harness.setup.setHostInput("astra.local");
    await harness.setup.connect();
    expect(harness.setup.snapshot().stage).toBe("done");
    expect(harness.refreshed()).toBe(0);
  });

  it("shows the host's own sentence when pairing fails, and offers a new PIN", async () => {
    const host = fakeHost();
    host.pairing = [{ phase: "failed", message: "Your gaming PC refused the PIN. Try again with the new one." }];
    const { setup, timers } = setupWith(host);
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    await timers.tick(1_000);
    expect(setup.snapshot().pairing).toMatchObject({
      phase: "failed",
      message: "Your gaming PC refused the PIN. Try again with the new one.",
    });
    expect(timers.pending()).toBe(0);
    await setup.startPairing();
    expect(setup.snapshot().pairing.phase).toBe("waiting");
  });

  it("gives up after the wait and tells the host to stop listening", async () => {
    const host = fakeHost();
    const { setup, timers } = setupWith(host);
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    for (let i = 0; i < 12; i += 1) await timers.tick(1_000);
    expect(setup.snapshot().pairing.phase).toBe("failed");
    expect(host.calls).toContain("cancel");
    expect(timers.pending()).toBe(0);
  });

  it("drops the attempt when the view is closed", async () => {
    const host = fakeHost();
    const { setup, timers } = setupWith(host);
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    setup.close();
    expect(host.calls).toContain("cancel");
    expect(timers.pending()).toBe(0);
    expect(setup.snapshot().pairing.phase).toBe("idle");
  });

  it("shows a returning person how things stand now and lists the games again", async () => {
    const host = fakeHost();
    host.saved = { host: "astra.local", paired: true };
    host.probeAnswer = { state: "paired", host: "astra.local", games: 3 };
    host.games = { games: 3, changed: false };
    const { setup } = setupWith(host);
    await setup.open();
    expect(host.calls).toEqual(["settings", "probe:", "sync"]);
    expect(setup.snapshot().stage).toBe("done");
  });

  it("puts a returning person on pairing when the PC no longer knows Orivo", async () => {
    const host = fakeHost();
    host.saved = { host: "astra.local", paired: true };
    const { setup } = setupWith(host);
    await setup.open();
    expect(setup.snapshot().stage).toBe("pair");
    expect(host.calls).not.toContain("sync");
  });

  it("says a PC with no games has none, instead of calling it a success", async () => {
    const host = fakeHost();
    host.probeAnswer = { state: "paired", host: "astra.local", games: 0 };
    host.games = { games: 0, changed: false };
    const { setup } = setupWith(host);
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    expect(setup.snapshot().games.phase).toBe("empty");
  });

  it("reports a failed import with the host's words and lets the person retry", async () => {
    const host = fakeHost();
    host.probeAnswer = { state: "paired", host: "astra.local", games: 0 };
    host.games = new Error("Your gaming PC did not answer.");
    const { setup } = setupWith(host);
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    expect(setup.snapshot().games).toMatchObject({
      phase: "failed",
      message: "Your gaming PC did not answer.",
    });
    host.games = { games: 2, changed: true };
    await setup.getGames();
    expect(setup.snapshot().games).toMatchObject({ phase: "ready", count: 2 });
  });

  it("forgets the PC and starts over", async () => {
    const host = fakeHost();
    host.saved = { host: "astra.local", paired: true };
    host.probeAnswer = { state: "paired", host: "astra.local", games: 3 };
    const { setup } = setupWith(host);
    await setup.open();
    await setup.forget();
    const view = setup.snapshot();
    expect(host.calls).toContain("forget");
    expect(view.stage).toBe("host");
    expect(view.host.saved).toBe("");
  });
});

describe("the built-in setup view", () => {
  it("draws three steps, the PIN large, and a quiet way to forget the PC", async () => {
    const host = fakeHost();
    const { setup } = setupWith(host);
    const root = document.createElement("div");
    mountBuiltinStreamSetup(root, setup);
    expect(root.childElementCount).toBe(0);
    await setup.open();
    setup.setHostInput("astra.local");
    await setup.connect();
    expect([...root.querySelectorAll("[data-bs-step]")].map((step) => step.getAttribute("data-bs-step"))).toEqual([
      "host",
      "pair",
      "games",
    ]);
    expect(root.querySelector(".gs-pin")?.textContent).toBe("4821");
    expect(root.querySelector("[data-bs-action='forget']")).not.toBeNull();
    expect(root.textContent).not.toMatch(/Moonlight/);
  });

  it("ends on a count and an Open Library button", async () => {
    const host = fakeHost();
    host.probeAnswer = { state: "paired", host: "astra.local", games: 3 };
    const harness = setupWith(host);
    const root = document.createElement("div");
    mountBuiltinStreamSetup(root, harness.setup);
    await harness.setup.open();
    harness.setup.setHostInput("astra.local");
    await harness.setup.connect();
    root.querySelector<HTMLButtonElement>("[data-bs-action='open-library']")?.click();
    expect(harness.libraryOpened()).toBe(1);
  });
});
