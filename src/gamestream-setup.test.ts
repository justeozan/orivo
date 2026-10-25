import { beforeEach, describe, expect, it } from "vitest";
import {
  GAMESTREAM_PLUGIN_ID,
  SETUP_COPY,
  createGameStreamSetup,
  mountGameStreamSetup,
  setupStage,
  setupStepStatuses,
  thisComputerName,
  tidyHostInput,
  type GameStreamSetup,
  type SetupFacts,
} from "./gamestream-setup";
import type { GameStreamClient, GameStreamLink } from "./gamestream-settings";
import type {
  DetectedClientView,
  GameStreamPairing,
  HostStatusView,
  InstalledRunnerView,
  PairingState,
  RunnerImportJobView,
  RunnerManagerController,
  RunnerProfileView,
} from "./runner-manager";

/* ---------------------------------------------------------------------------
   The pure part: which step is the current one
   --------------------------------------------------------------------------- */

const NOTHING: SetupFacts = {
  clientFound: false,
  hostSaved: false,
  hostReachable: false,
  paired: false,
  gamesImported: false,
};

describe("setupStage", () => {
  it("starts on the client and moves one step at a time as facts arrive", () => {
    expect(setupStage(NOTHING)).toBe("client");
    expect(setupStage({ ...NOTHING, clientFound: true })).toBe("host");
    expect(setupStage({ ...NOTHING, clientFound: true, hostSaved: true })).toBe("host");
    expect(
      setupStage({ ...NOTHING, clientFound: true, hostSaved: true, hostReachable: true }),
    ).toBe("pair");
    expect(
      setupStage({
        clientFound: true,
        hostSaved: true,
        hostReachable: true,
        paired: true,
        gamesImported: false,
      }),
    ).toBe("games");
    expect(
      setupStage({
        clientFound: true,
        hostSaved: true,
        hostReachable: true,
        paired: true,
        gamesImported: true,
      }),
    ).toBe("done");
  });

  it("does not ask for an address that was never saved, however reachable it looked", () => {
    expect(setupStage({ ...NOTHING, clientFound: true, hostReachable: true })).toBe("host");
  });

  it("counts being paired as the PC having answered", () => {
    expect(
      setupStage({ ...NOTHING, clientFound: true, hostSaved: true, paired: true }),
    ).toBe("games");
  });

  it("sends a person back to the earliest step that is not true, whatever follows", () => {
    // Moonlight was uninstalled after everything else worked.
    expect(
      setupStage({
        clientFound: false,
        hostSaved: true,
        hostReachable: true,
        paired: true,
        gamesImported: true,
      }),
    ).toBe("client");
    // Games without a saved PC (a stale library) still start from the PC.
    expect(
      setupStage({ ...NOTHING, clientFound: true, gamesImported: true }),
    ).toBe("host");
  });

  it("marks steps done before the current one, active on it and todo after", () => {
    expect(setupStepStatuses({ ...NOTHING, clientFound: true })).toEqual({
      client: "done",
      host: "active",
      pair: "todo",
      games: "todo",
    });
    expect(setupStepStatuses(NOTHING)).toEqual({
      client: "active",
      host: "todo",
      pair: "todo",
      games: "todo",
    });
    expect(
      setupStepStatuses({
        clientFound: true,
        hostSaved: true,
        hostReachable: true,
        paired: true,
        gamesImported: true,
      }),
    ).toEqual({ client: "done", host: "done", pair: "done", games: "done" });
  });
});

describe("tidyHostInput", () => {
  it("reduces a pasted web address to the machine", () => {
    for (const [input, expected] of [
      ["my-pc.local", "my-pc.local"],
      ["  my-pc.local ", "my-pc.local"],
      ["https://my-pc.local:47990/pin", "my-pc.local"],
      ["HTTP://my-pc.local", "my-pc.local"],
      ["192.168.1.20:47990", "192.168.1.20"],
      ["https://admin:secret@my-pc.local:47990/?x=1#top", "my-pc.local"],
      ["https://[fd00::1]:47990/", "[fd00::1]"],
      ["[fd00::1]", "[fd00::1]"],
      ["", ""],
    ] as const) {
      expect(tidyHostInput(input), input).toBe(expected);
    }
  });

  it("leaves a bare IPv6 address for the host to refuse rather than mangling it", () => {
    expect(tidyHostInput("fd00::1")).toBe("fd00::1");
  });
});

describe("thisComputerName", () => {
  it("names what the person is sitting at", () => {
    expect(thisComputerName("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)")).toBe("this Mac");
    expect(thisComputerName("Mozilla/5.0 (Windows NT 10.0; Win64; x64)")).toBe("this PC");
    expect(thisComputerName("Mozilla/5.0 (X11; Linux x86_64)")).toBe("this computer");
  });
});

/* ---------------------------------------------------------------------------
   The flow, against a fake host
   --------------------------------------------------------------------------- */

const flush = async (): Promise<void> => {
  for (let tick = 0; tick < 6; tick += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

function profile(overrides: Partial<RunnerProfileView> = {}): RunnerProfileView {
  return {
    id: "stream-1",
    pluginId: GAMESTREAM_PLUGIN_ID,
    displayName: "Game streaming",
    status: "valid",
    statusMessage: null,
    enabled: true,
    applicationLabel: "Moonlight.app",
    applicationAvailable: true,
    launchMode: "stream",
    directories: [],
    gameCount: 0,
    importComplete: false,
    importResumable: false,
    lastImportedAt: null,
    ...overrides,
  };
}

function job(overrides: Partial<RunnerImportJobView> = {}): RunnerImportJobView {
  return {
    jobId: "job-1",
    profileId: "stream-1",
    phase: "running",
    imported: 0,
    refreshed: 0,
    skipped: 0,
    pages: 0,
    resumed: false,
    complete: false,
    message: "",
    ...overrides,
  };
}

interface FakeHost {
  runners: RunnerManagerController;
  stream: GameStreamClient;
  calls: string[];
  /** What the PC answers when asked. */
  probe: HostStatusView | Error;
  /** What the next `begin_gamestream_pairing` answers. */
  pairing: GameStreamPairing | Error;
  /** What each pairing poll answers, in order; the last repeats. */
  pairingStates: PairingState[];
  clients: DetectedClientView[];
  saved: string;
  openedLinks: GameStreamLink[];
  /** Replace the stored profile, as the host would after a command. */
  setProfile(next: RunnerProfileView | null): void;
  finishImport(games: number): void;
  progressImport(found: number): void;
}

function createFakeHost(initial: { profile?: RunnerProfileView | null; saved?: string } = {}): FakeHost {
  const calls: string[] = [];
  const listeners = new Set<() => void>();
  let current: RunnerProfileView | null = initial.profile ?? null;
  let importJob: RunnerImportJobView | null = null;
  const notify = (): void => {
    for (const listener of [...listeners]) listener();
  };
  const host: FakeHost = {
    calls,
    probe: { state: "paired", host: "my-pc.local", games: 3 },
    pairing: { state: "started", host: "my-pc.local", pin: "1234" },
    pairingStates: ["waiting"],
    clients: [{ id: "client-1", label: "Moonlight" }],
    saved: initial.saved ?? "",
    openedLinks: [],
    setProfile(next) {
      current = next;
      notify();
    },
    finishImport(games) {
      importJob = job({ phase: "ready", imported: games, complete: true });
      if (current) current = { ...current, gameCount: games };
      notify();
    },
    progressImport(found) {
      importJob = job({ phase: "running", imported: found });
      notify();
    },
    runners: undefined as unknown as RunnerManagerController,
    stream: undefined as unknown as GameStreamClient,
  };
  let pollIndex = 0;
  const runners: Partial<RunnerManagerController> = {
    async load() {
      calls.push("load");
    },
    runners(): InstalledRunnerView[] {
      return [
        {
          id: GAMESTREAM_PLUGIN_ID,
          name: "Game streaming",
          version: "0.2.0",
          state: "ready",
          message: "",
          profiles: current ? [current] : [],
        },
      ];
    },
    streamClients: () => host.clients,
    async refreshStreamClients() {
      calls.push("refreshStreamClients");
      return host.clients;
    },
    importFor: () => importJob,
    isFindingArtwork: () => false,
    async createStreamProfile(clientId) {
      calls.push(`createStreamProfile:${clientId ?? "picker"}`);
      current = profile({ gameCount: current?.gameCount ?? 0 });
      notify();
      return current;
    },
    async probeHost(profileId, typed) {
      calls.push(`probeHost:${profileId}:${typed}`);
      if (host.probe instanceof Error) throw host.probe;
      return host.probe;
    },
    async beginPairing(profileId) {
      calls.push(`beginPairing:${profileId}`);
      if (host.pairing instanceof Error) throw host.pairing;
      return host.pairing;
    },
    async pairingStatus(profileId) {
      calls.push(`pairingStatus:${profileId}`);
      const state = host.pairingStates[Math.min(pollIndex, host.pairingStates.length - 1)] ?? "waiting";
      pollIndex += 1;
      return state;
    },
    async cancelPairing(profileId) {
      calls.push(`cancelPairing:${profileId}`);
    },
    async prepareLibrary(profileId) {
      calls.push(`prepareLibrary:${profileId}`);
      return current ?? profile();
    },
    async startImport(profileId) {
      calls.push(`startImport:${profileId}`);
      importJob = job({ phase: "running" });
      notify();
    },
    onChange(callback) {
      listeners.add(callback);
      return () => listeners.delete(callback);
    },
  };
  host.runners = runners as RunnerManagerController;
  host.stream = {
    async load() {
      return { host: host.saved };
    },
    async save(update) {
      calls.push(`save:${update.host}`);
      host.saved = update.host ?? host.saved;
      return { host: host.saved };
    },
    async openLink(link) {
      host.openedLinks.push(link);
    },
  };
  return host;
}

/** A clock and a timer queue the test drives by hand. */
function createClock() {
  let time = 1_000_000;
  const pending: Array<{ id: number; callback: () => void }> = [];
  let nextId = 1;
  return {
    now: () => time,
    advance(ms: number) {
      time += ms;
    },
    setTimer(callback: () => void) {
      const id = nextId++;
      pending.push({ id, callback });
      return id;
    },
    clearTimer(handle: unknown) {
      const index = pending.findIndex((entry) => entry.id === handle);
      if (index >= 0) pending.splice(index, 1);
    },
    get queued() {
      return pending.length;
    },
    async tick() {
      const next = pending.shift();
      next?.callback();
      await flush();
    },
  };
}

function build(host: FakeHost, clock = createClock()) {
  const toasts: string[] = [];
  let libraryOpened = 0;
  const setup = createGameStreamSetup({
    runners: host.runners,
    stream: host.stream,
    showToast: (message) => toasts.push(message),
    openLibrary: () => {
      libraryOpened += 1;
    },
    computerName: "this Mac",
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
    now: clock.now,
    pairingPollMs: 2_000,
    pairingTimeoutMs: 180_000,
  });
  return { setup, clock, toasts, libraryOpened: () => libraryOpened };
}

describe("the setup flow", () => {
  it("starts on Moonlight when none is installed, and moves on once it is found", async () => {
    const host = createFakeHost();
    host.clients = [];
    const { setup } = build(host);
    await setup.open();

    let view = setup.snapshot();
    expect(view.stage).toBe("client");
    expect(view.client.clients).toEqual([]);

    // The person installed it and pressed "Check again".
    host.clients = [{ id: "client-1", label: "Moonlight" }];
    await setup.checkAgain();
    view = setup.snapshot();
    expect(view.facts.clientFound).toBe(true);
    expect(view.stage).toBe("host");
    expect(view.client.label).toBe("Moonlight");
  });

  it("says so when checking again still finds nothing", async () => {
    const host = createFakeHost();
    host.clients = [];
    const { setup } = build(host);
    await setup.open();
    await setup.checkAgain();
    expect(setup.snapshot().client.stillMissing).toBe(true);
    expect(setup.snapshot().stage).toBe("client");
  });

  it("opens Moonlight's website through the host, not by address", async () => {
    const host = createFakeHost();
    const { setup } = build(host);
    await setup.open();
    await setup.installMoonlight();
    expect(host.openedLinks).toEqual(["moonlight"]);
  });

  it("keeps a PC that does not answer out of the settings, and says what to check", async () => {
    const host = createFakeHost();
    host.probe = { state: "unreachable", host: "my-pc.local", games: 0 };
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();

    const view = setup.snapshot();
    expect(view.host.error).toBe(SETUP_COPY.unreachable);
    expect(view.host.canSaveAnyway).toBe(true);
    expect(view.stage).toBe("host");
    expect(host.saved).toBe("");
    expect(host.calls.some((call) => call.startsWith("save:"))).toBe(false);
  });

  it("can keep a PC that is switched off, and stays on the same step", async () => {
    const host = createFakeHost();
    host.probe = { state: "unreachable", host: "my-pc.local", games: 0 };
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("https://my-pc.local:47990/pin");
    await setup.connect();
    await setup.saveAnyway();

    const view = setup.snapshot();
    expect(host.saved).toBe("my-pc.local");
    expect(view.host.notice).toBe(SETUP_COPY.savedAnyway);
    expect(view.stage).toBe("host");
    expect(view.pairing.phase).toBe("idle");
  });

  it("asks for an address before asking the host anything", async () => {
    const host = createFakeHost();
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("   ");
    await setup.connect();
    expect(setup.snapshot().host.error).toBe(SETUP_COPY.needHost);
    expect(host.calls.filter((call) => call.startsWith("probeHost"))).toEqual([]);
  });

  it("shows the host's own sentence when the address is refused", async () => {
    const host = createFakeHost();
    host.probe = new Error("That doesn't look like a PC name or address.");
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("not a host");
    await setup.connect();
    expect(setup.snapshot().host.error).toBe("That doesn't look like a PC name or address.");
    expect(host.saved).toBe("");
  });

  it("goes straight to the PIN for a PC that has not paired, saving the tidy address", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("https://my-pc.local:47990/");
    await setup.connect();

    const view = setup.snapshot();
    // The profile is made when the person commits to a PC, from the detected client.
    expect(host.calls).toContain("createStreamProfile:client-1");
    expect(host.calls).toContain("probeHost:stream-1:my-pc.local");
    expect(host.saved).toBe("my-pc.local");
    expect(view.stage).toBe("pair");
    expect(view.pairing.phase).toBe("waiting");
    expect(view.pairing.pin).toBe("1234");
  });

  it("skips pairing for a PC that is already paired and lists its games", async () => {
    const host = createFakeHost();
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await flush();

    expect(host.calls).not.toContain("beginPairing:stream-1");
    expect(host.calls).toContain("prepareLibrary:stream-1");
    expect(host.calls).toContain("startImport:stream-1");
    expect(setup.snapshot().stage).toBe("games");
  });

  it("watches for the pairing itself and moves on to the games when it works", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    host.pairingStates = ["waiting", "waiting", "paired"];
    const { setup, clock } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    expect(setup.snapshot().pairing.phase).toBe("waiting");

    await clock.tick();
    expect(setup.snapshot().pairing.phase).toBe("waiting");
    await clock.tick();
    expect(host.calls).not.toContain("startImport:stream-1");
    await clock.tick();

    const view = setup.snapshot();
    expect(view.pairing.phase).toBe("paired");
    expect(view.facts.paired).toBe(true);
    expect(host.calls).toContain("prepareLibrary:stream-1");
    expect(host.calls).toContain("startImport:stream-1");
    expect(view.stage).toBe("games");
    // Nothing is left polling once it worked.
    expect(clock.queued).toBe(0);
  });

  it("reports real progress and then how many games were added", async () => {
    const host = createFakeHost();
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await flush();

    expect(setup.snapshot().games.phase).toBe("importing");
    host.progressImport(5);
    expect(setup.snapshot().games).toMatchObject({ phase: "importing", found: 5 });

    host.finishImport(12);
    const view = setup.snapshot();
    expect(view.games).toMatchObject({ phase: "ready", count: 12 });
    expect(view.stage).toBe("done");
  });

  it("says a PC with no games listed has none, instead of calling it a success", async () => {
    const host = createFakeHost();
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await flush();
    host.finishImport(0);

    expect(setup.snapshot().games.phase).toBe("empty");
    expect(setup.snapshot().stage).toBe("games");
  });

  it("gives up after the wait, ends the attempt and offers a new PIN", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    const { setup, clock } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();

    clock.advance(180_001);
    await clock.tick();

    const view = setup.snapshot();
    expect(view.pairing.phase).toBe("timedOut");
    expect(view.pairing.message).toBe(SETUP_COPY.pairingFailed);
    expect(host.calls).toContain("cancelPairing:stream-1");
    expect(clock.queued).toBe(0);

    // "Try again" is a new attempt with a new PIN.
    host.pairing = { state: "started", host: "my-pc.local", pin: "5678" };
    await setup.startPairing();
    expect(setup.snapshot().pairing).toMatchObject({ phase: "waiting", pin: "5678" });
    expect(host.calls.filter((call) => call === "beginPairing:stream-1")).toHaveLength(2);
  });

  it("stops waiting at once when the attempt ended without pairing", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    host.pairingStates = ["failed"];
    const { setup, clock } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await clock.tick();

    expect(setup.snapshot().pairing.phase).toBe("failed");
    expect(clock.queued).toBe(0);
  });

  it("notes a PC that went quiet without giving up on it", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    host.pairingStates = ["unreachable", "waiting"];
    const { setup, clock } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await clock.tick();

    expect(setup.snapshot().pairing).toMatchObject({ phase: "waiting", lostContact: true });
    expect(clock.queued).toBe(1);
    await clock.tick();
    expect(setup.snapshot().pairing.lostContact).toBe(false);
  });

  it("treats a PC that turns out to be paired already as done", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    host.pairing = { state: "alreadyPaired", host: "my-pc.local" };
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await flush();

    expect(setup.snapshot().pairing.phase).toBe("paired");
    expect(host.calls).toContain("startImport:stream-1");
  });

  it("drops the attempt when the view is closed", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    const { setup, clock } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    expect(clock.queued).toBe(1);

    setup.close();
    await flush();
    expect(clock.queued).toBe(0);
    expect(host.calls).toContain("cancelPairing:stream-1");
  });

  it("sends the person back to Moonlight when the host says it is gone", async () => {
    const host = createFakeHost();
    host.probe = { state: "clientMissing", host: "", games: 0 };
    host.clients = [];
    const { setup } = build(host);
    await setup.open();
    host.clients = [];
    setup.setHostInput("my-pc.local");
    // No client was detected, so there is nothing to connect with yet.
    await setup.connect();
    const view = setup.snapshot();
    expect(view.stage).toBe("client");
    expect(host.saved).toBe("");
  });

  it("shows how things stand now when someone comes back to a finished setup", async () => {
    const host = createFakeHost({
      profile: profile({ gameCount: 12 }),
      saved: "my-pc.local",
    });
    const { setup } = build(host);
    await setup.open();

    const view = setup.snapshot();
    expect(view.stage).toBe("done");
    expect(view.games).toMatchObject({ phase: "ready", count: 12 });
    // Asked, not trusted; and no import is started on arrival.
    expect(host.calls).toContain("probeHost:stream-1:my-pc.local");
    expect(host.calls).not.toContain("startImport:stream-1");
  });

  it("puts a returning person on the pairing step when the PC no longer knows them", async () => {
    const host = createFakeHost({ profile: profile({ gameCount: 12 }), saved: "my-pc.local" });
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    const { setup } = build(host);
    await setup.open();

    const view = setup.snapshot();
    expect(view.stage).toBe("pair");
    // The PIN is not drawn until they ask for it.
    expect(view.pairing.phase).toBe("idle");
    expect(host.calls).not.toContain("beginPairing:stream-1");
  });

  it("reports a failed import with the host's words and lets the person retry", async () => {
    const host = createFakeHost();
    const { setup } = build(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await flush();
    host.runners.importFor = () =>
      job({ phase: "failed", message: "Can't reach my-pc.local. Check it's on." });
    const view = setup.snapshot();
    expect(view.games).toMatchObject({
      phase: "failed",
      message: "Can't reach my-pc.local. Check it's on.",
    });
  });

  it("opens the Library and the PIN page on request", async () => {
    const host = createFakeHost();
    const { setup, libraryOpened } = build(host);
    await setup.open();
    setup.openLibrary();
    await setup.openPinPage();
    expect(libraryOpened()).toBe(1);
    expect(host.openedLinks).toEqual(["sunshine-pin"]);
  });
});

/* ---------------------------------------------------------------------------
   The view
   --------------------------------------------------------------------------- */

describe("the setup view", () => {
  let root: HTMLElement;

  beforeEach(() => {
    root = document.createElement("div");
    document.body.replaceChildren(root);
  });

  const text = (): string => root.textContent ?? "";
  const button = (name: string): HTMLButtonElement =>
    root.querySelector<HTMLButtonElement>(`[data-gs-action='${name}']`)!;
  const mountFor = (host: FakeHost): { setup: GameStreamSetup; clock: ReturnType<typeof createClock> } => {
    const { setup, clock } = build(host);
    mountGameStreamSetup(root, setup);
    return { setup, clock };
  };

  it("draws nothing until it is opened", () => {
    mountFor(createFakeHost());
    expect(root.children).toHaveLength(0);
  });

  it("offers the install, the check and the picker when Moonlight is missing", async () => {
    const host = createFakeHost();
    host.clients = [];
    const { setup } = mountFor(host);
    await setup.open();

    expect(root.querySelector("[data-gs-step='client']")!.classList.contains("is-active")).toBe(true);
    expect(text()).toContain("Moonlight on this Mac");
    expect(button("install-moonlight")).not.toBeNull();
    expect(button("check-again")).not.toBeNull();
    expect(button("choose-client")).not.toBeNull();

    button("install-moonlight").click();
    await flush();
    expect(host.openedLinks).toEqual(["moonlight"]);
  });

  it("collapses a finished step to a line and expands only the active one", async () => {
    const host = createFakeHost();
    const { setup } = mountFor(host);
    await setup.open();

    const client = root.querySelector("[data-gs-step='client']")!;
    expect(client.classList.contains("is-done")).toBe(true);
    expect(client.querySelector(".gs-step__body")).toBeNull();
    expect(client.textContent).toContain("Moonlight found (Moonlight)");
    const pcStep = root.querySelector("[data-gs-step='host']")!;
    expect(pcStep.classList.contains("is-active")).toBe(true);
    expect(pcStep.getAttribute("aria-current")).toBe("step");
    expect(pcStep.querySelector("input")!.placeholder).toBe("my-pc.local or 192.168.1.20");
    expect(root.querySelector("[data-gs-step='pair'] .gs-step__body")).toBeNull();
  });

  it("offers a small choice when several clients are installed", async () => {
    const host = createFakeHost();
    host.clients = [
      { id: "client-1", label: "Moonlight" },
      { id: "client-2", label: "Moonlight Qt" },
    ];
    const { setup } = mountFor(host);
    await setup.open();
    setup.edit("client");

    const select = root.querySelector<HTMLSelectElement>("#gs-client-select")!;
    expect([...select.options].map((option) => option.textContent)).toEqual([
      "Moonlight",
      "Moonlight Qt",
    ]);
    select.value = "client-2";
    select.dispatchEvent(new Event("change"));
    expect(setup.snapshot().client.selectedId).toBe("client-2");
  });

  it("tidies a pasted address in the box and connects with it", async () => {
    const host = createFakeHost();
    const { setup } = mountFor(host);
    await setup.open();

    const input = root.querySelector<HTMLInputElement>("#gs-host")!;
    input.value = "https://my-pc.local:47990/pin";
    input.dispatchEvent(new Event("input"));
    input.dispatchEvent(new Event("blur"));
    expect(input.value).toBe("my-pc.local");

    button("connect").click();
    await flush();
    expect(host.calls).toContain("probeHost:stream-1:my-pc.local");
  });

  it("shows the PIN large with the exact instructions, the PIN page button and the certificate note", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    const { setup } = mountFor(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();

    expect(root.querySelector(".gs-pin")!.textContent).toBe("1234");
    expect(text()).toContain("On your PC, open Sunshine (or Apollo) › PIN, type 1234 and a name.");
    expect(text()).toContain("Your browser may warn about the certificate");
    expect(text()).toContain("Waiting for your PC…");

    button("open-pin-page").click();
    await flush();
    expect(host.openedLinks).toEqual(["sunshine-pin"]);
  });

  it("offers a retry that draws a new PIN after a failed attempt", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    host.pairingStates = ["failed"];
    const { setup, clock } = mountFor(host);
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await clock.tick();

    expect(text()).toContain("Pairing didn't finish. Try again.");
    host.pairing = { state: "started", host: "my-pc.local", pin: "9876" };
    button("start-pairing").click();
    await flush();
    expect(root.querySelector(".gs-pin")!.textContent).toBe("9876");
  });

  it("reports progress and ends on a count and an Open Library button", async () => {
    const host = createFakeHost();
    const { setup, libraryOpened } = (() => {
      const built = build(host);
      mountGameStreamSetup(root, built.setup);
      return built;
    })();
    await setup.open();
    setup.setHostInput("my-pc.local");
    await setup.connect();
    await flush();

    expect(text()).toContain("Looking for games on your PC…");
    host.progressImport(7);
    expect(text()).toContain("Found 7 games…");
    host.finishImport(12);
    expect(text()).toContain("12 games added to your Library.");
    expect(root.querySelector("[data-gs-step='games']")!.classList.contains("is-done")).toBe(true);

    button("open-library").click();
    expect(libraryOpened()).toBe(1);
    expect(button("refresh-games").textContent).toBe("Refresh games");
  });

  it("keeps the cursor in the address box through a repaint", async () => {
    const host = createFakeHost();
    const { setup } = mountFor(host);
    await setup.open();

    const input = root.querySelector<HTMLInputElement>("#gs-host")!;
    input.value = "my-p";
    input.dispatchEvent(new Event("input"));
    input.focus();
    input.setSelectionRange(2, 3);

    // Something else changes while they type.
    host.setProfile(profile({ displayName: "Renamed" }));

    const again = root.querySelector<HTMLInputElement>("#gs-host")!;
    expect(again.value).toBe("my-p");
    expect(document.activeElement).toBe(again);
    expect([again.selectionStart, again.selectionEnd]).toEqual([2, 3]);
  });

  it("never says emulator, runner, profile, granted folder, stream cards or placeholder", async () => {
    const host = createFakeHost();
    host.probe = { state: "notPaired", host: "my-pc.local", games: 0 };
    const { setup } = mountFor(host);
    const seen: string[] = [];
    await setup.open();
    seen.push(text());
    setup.setHostInput("my-pc.local");
    await setup.connect();
    seen.push(text());
    host.pairingStates = ["paired"];
    await setup.startPairing();
    host.finishImport(0);
    seen.push(text());
    for (const screen of seen) {
      expect(screen).not.toMatch(/emulator|runner|profile|granted|stream cards?|placeholder/i);
    }
  });
});
