import { beforeEach, describe, expect, it } from "vitest";
import { mountRunnerPanel } from "./runner-view";
import type {
  DetectedClientView,
  InstalledRunnerView,
  RunnerImportJobView,
  RunnerManagerController,
  RunnerProfileView,
} from "./runner-manager";

function profile(overrides: Partial<RunnerProfileView> = {}): RunnerProfileView {
  return {
    id: "runner-1",
    pluginId: "com.orivo.dolphin",
    displayName: "Dolphin",
    status: "valid",
    statusMessage: null,
    enabled: true,
    applicationLabel: "Dolphin.app",
    launchMode: "default",
    directories: [{ id: "games", label: "My ROMs", granted: true }],
    gameCount: 3,
    importComplete: true,
    importResumable: false,
    lastImportedAt: 1_700_000_000_000,
    ...overrides,
  };
}

function runner(overrides: Partial<InstalledRunnerView> = {}): InstalledRunnerView {
  return {
    id: "com.orivo.dolphin",
    name: "Dolphin",
    version: "5.0.1",
    state: "ready",
    message: "",
    profiles: [profile()],
    ...overrides,
  };
}

/** A hand-rolled controller: this suite is about the DOM, not the polling logic runner-manager.test.ts already covers. */
function createFakeController(
  runners: InstalledRunnerView[],
  jobs: Record<string, RunnerImportJobView> = {},
  clients: DetectedClientView[] = [],
  findingArtwork: Set<string> = new Set(),
): RunnerManagerController & { calls: string[]; emit(): void } {
  const calls: string[] = [];
  const listeners = new Set<() => void>();
  return {
    calls,
    emit() {
      for (const listener of [...listeners]) listener();
    },
    async load() {
      calls.push("load");
    },
    runners: () => runners,
    isFindingArtwork: (profileId) => findingArtwork.has(profileId),
    importFor: (profileId) => jobs[profileId] ?? null,
    async createProfile(pluginId, displayName, clientId) {
      calls.push(`createProfile:${pluginId}:${displayName}:${clientId ?? "picker"}`);
      return profile();
    },
    async renameProfile(profileId, displayName) {
      calls.push(`renameProfile:${profileId}:${displayName}`);
      return profile();
    },
    async setEnabled(profileId, enabled) {
      calls.push(`setEnabled:${profileId}:${enabled}`);
      return profile();
    },
    async setLaunchMode(profileId, launchMode) {
      calls.push(`setLaunchMode:${profileId}:${launchMode}`);
      return profile({ launchMode });
    },
    async beginPairing(profileId) {
      calls.push(`beginPairing:${profileId}`);
      return { state: "started" as const, pin: "0417", host: "astra.local" };
    },
    streamClients: () => clients,
    async deleteProfile(profileId) {
      calls.push(`deleteProfile:${profileId}`);
      return true;
    },
    async grantDirectory(profileId) {
      calls.push(`grantDirectory:${profileId}`);
      return profile();
    },
    async revokeDirectory(profileId, directoryId) {
      calls.push(`revokeDirectory:${profileId}:${directoryId}`);
      return profile();
    },
    async startImport(profileId) {
      calls.push(`startImport:${profileId}`);
    },
    async cancelImport(profileId) {
      calls.push(`cancelImport:${profileId}`);
    },
    onChange(callback) {
      listeners.add(callback);
      return () => listeners.delete(callback);
    },
    dispose() {
      listeners.clear();
    },
  };
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

describe("mountRunnerPanel", () => {
  let root: HTMLElement;
  let toasts: string[];

  beforeEach(() => {
    root = document.createElement("div");
    toasts = [];
  });

  it("shows an empty-state message when no runner plugin is installed", () => {
    mountRunnerPanel(root, createFakeController([]), { showToast: (message) => toasts.push(message) });
    expect(root.textContent).toContain("Install a runner plugin");
  });

  it("renders one group per installed runner and one card per profile", () => {
    mountRunnerPanel(root, createFakeController([runner()]), {
      showToast: (message) => toasts.push(message),
    });
    expect(root.querySelectorAll("[data-runner-plugin]")).toHaveLength(1);
    expect(root.querySelectorAll("[data-runner-profile]")).toHaveLength(1);
    expect(root.textContent).toContain("Dolphin");
    expect(root.textContent).toContain("Ready");
    expect(root.textContent).toContain("My ROMs");
  });

  it("disables adding a profile for a runner the host could not use", () => {
    mountRunnerPanel(root, createFakeController([runner({ state: "invalid", message: "bad component" })]), {
      showToast: (message) => toasts.push(message),
    });
    const button = [...root.querySelectorAll("button")].find((el) => el.textContent?.includes("Add an emulator"));
    expect(button?.disabled).toBe(true);
    expect(root.textContent).toContain("bad component");
  });

  it("creates a profile from the runner's own name when the button is clicked", async () => {
    const controller = createFakeController([runner({ profiles: [] })]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    const button = [...root.querySelectorAll("button")].find((el) => el.textContent?.includes("Add an emulator"))!;
    button.click();
    await flush();

    expect(controller.calls).toContain("createProfile:com.orivo.dolphin:Dolphin:picker");
  });

  it("surfaces a rejected native picker as a toast rather than throwing", async () => {
    const controller = createFakeController([runner({ profiles: [] })]);
    controller.createProfile = async () => {
      throw "No emulator was chosen.";
    };
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    const button = [...root.querySelectorAll("button")].find((el) => el.textContent?.includes("Add an emulator"))!;
    button.click();
    await flush();

    expect(toasts).toEqual(["No emulator was chosen."]);
  });

  it("revokes one folder without touching the others", async () => {
    const controller = createFakeController([
      runner({
        profiles: [
          profile({
            directories: [
              { id: "games", label: "ROMs", granted: true },
              { id: "saves", label: "Saves", granted: true },
            ],
          }),
        ],
      }),
    ]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    const [revokeFirst] = [...root.querySelectorAll<HTMLButtonElement>("button")].filter(
      (el) => el.textContent === "Remove access",
    );
    revokeFirst!.click();
    await flush();

    expect(controller.calls).toEqual(["revokeDirectory:runner-1:games"]);
  });

  it("starts an import when games can be found and disables the button without a granted folder", () => {
    const controller = createFakeController([
      runner({
        profiles: [
          profile({ gameCount: 0, directories: [{ id: "games", label: "ROMs", granted: false }] }),
        ],
      }),
    ]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });
    const importButton = [...root.querySelectorAll<HTMLButtonElement>("button")].find(
      (el) => el.textContent === "Import games",
    )!;
    expect(importButton.disabled).toBe(true);
    expect(root.textContent).toContain("Add a game folder before importing");
  });

  it("shows cancel while an import is running and calls it on click", () => {
    const controller = createFakeController(
      [runner()],
      {
        "runner-1": {
          jobId: "job-1",
          profileId: "runner-1",
          phase: "running",
          imported: 2,
          refreshed: 0,
          skipped: 0,
          pages: 1,
          resumed: false,
          complete: false,
          message: "",
        },
      },
    );
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });
    const cancelButton = [...root.querySelectorAll<HTMLButtonElement>("button")].find(
      (el) => el.textContent === "Cancel import",
    )!;
    expect(root.textContent).toContain("Importing… 2 game(s) so far");
    cancelButton.click();
    expect(controller.calls).toContain("cancelImport:runner-1");
  });

  // The launch shape is the user's own permission on their own profile, so
  // the card both shows it and is where it changes.
  it("shows the profile's launch shape and changes it on the user's own choice", async () => {
    const controller = createFakeController([runner()]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    const mode = root.querySelector<HTMLSelectElement>("[data-runner-launch-mode='runner-1']")!;
    expect(mode).not.toBeNull();
    expect(mode.value).toBe("default");

    mode.value = "stream";
    mode.dispatchEvent(new Event("change"));
    await flush();

    expect(controller.calls).toEqual(["setLaunchMode:runner-1:stream"]);
  });

  it("renders a streaming profile as the kind of folder it actually has", () => {
    const controller = createFakeController([
      runner({
        profiles: [
          profile({
            launchMode: "stream",
            gameCount: 0,
            directories: [{ id: "games", label: "Streams", granted: true }],
          }),
        ],
      }),
    ]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    expect(root.querySelector<HTMLSelectElement>("[data-runner-launch-mode='runner-1']")!.value).toBe(
      "stream",
    );
    expect(root.textContent).toContain("Importing asks the streaming host what it can stream");
    expect(root.textContent).toContain("Add a folder for its stream cards…");
  });

  // Pairing reads no credential, so it is offered even when the feed is being
  // refused — which is exactly when a user goes looking for it.
  it("offers pairing on a streaming profile and shows the PIN it was given", async () => {
    const controller = createFakeController([
      runner({ profiles: [profile({ launchMode: "stream" })] }),
    ]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    const pair = [...root.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
      button.textContent?.includes("Pair with this host"),
    )!;
    expect(pair).not.toBeUndefined();
    pair.click();
    await flush();

    expect(controller.calls).toEqual(["beginPairing:runner-1"]);
    const panel = root.querySelector<HTMLElement>("[data-runner-pairing='runner-1']")!;
    expect(panel.querySelector(".runner-pairing__pin")!.textContent).toBe("0417");
    expect(panel.textContent).toContain("astra.local");
  });

  // Not on a timer: a PIN that vanished while the user was walking to the other
  // machine would be the one failure mode worth avoiding.
  it("keeps the PIN until the user says the attempt is done", async () => {
    const controller = createFakeController([
      runner({ profiles: [profile({ launchMode: "stream" })] }),
    ]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    [...root.querySelectorAll<HTMLButtonElement>("button")]
      .find((button) => button.textContent?.includes("Pair with this host"))!
      .click();
    await flush();

    // A re-render for any other reason must not take it away.
    controller.emit();
    expect(root.querySelector("[data-runner-pairing='runner-1']")).not.toBeNull();

    [...root.querySelectorAll<HTMLButtonElement>("button")]
      .find((button) => button.textContent === "Done")!
      .click();
    await flush();
    expect(root.querySelector("[data-runner-pairing='runner-1']")).toBeNull();
  });

  // The defect this shape exists to prevent: a client that is already paired
  // refuses to start a handshake, so a PIN shown here could only be rejected.
  it("says a machine is already paired instead of showing a PIN for it", async () => {
    const controller = createFakeController([
      runner({ profiles: [profile({ launchMode: "stream" })] }),
    ]);
    controller.beginPairing = async () => ({
      state: "alreadyPaired" as const,
      host: "astra.local",
    });
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    [...root.querySelectorAll<HTMLButtonElement>("button")]
      .find((button) => button.textContent?.includes("Pair with this host"))!
      .click();
    await flush();

    const panel = root.querySelector<HTMLElement>("[data-runner-pairing='runner-1']")!;
    expect(panel.querySelector(".runner-pairing__pin")).toBeNull();
    expect(panel.textContent).toContain("astra.local is already paired");
  });

  // A user who streams has the client installed. Making them walk a file
  // dialog to a bundle Orivo can already see is asking them to do its work.
  it("offers an application it found before offering the file picker", async () => {
    const controller = createFakeController([runner({ profiles: [] })], {}, [
      { id: "client-abc", label: "Moonlight" },
    ]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    const labels = [...root.querySelectorAll<HTMLButtonElement>(".runner-group__adders button")].map(
      (button) => button.textContent,
    );
    expect(labels).toEqual(["+ Add Moonlight", "+ Add an emulator"]);

    root.querySelector<HTMLButtonElement>(".runner-group__adders button")!.click();
    await flush();

    // The handle is what crosses, never a path, and the application's own name
    // is what the profile is called.
    expect(controller.calls).toEqual(["createProfile:com.orivo.dolphin:Moonlight:client-abc"]);
  });

  it("falls back to the picker alone when nothing was found", () => {
    const controller = createFakeController([runner({ profiles: [] })]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });
    expect(
      [...root.querySelectorAll(".runner-group__adders button")].map((button) => button.textContent),
    ).toEqual(["+ Add an emulator"]);
  });

  it("never offers pairing on a profile that launches game files", () => {
    const controller = createFakeController([runner()]);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });
    expect(root.textContent).not.toContain("Pair with this host");
  });

  it("surfaces a refused pairing as a toast and keeps the button", async () => {
    const controller = createFakeController([
      runner({ profiles: [profile({ launchMode: "stream" })] }),
    ]);
    controller.beginPairing = async () => {
      throw "No GameStream host is configured yet.";
    };
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    [...root.querySelectorAll<HTMLButtonElement>("button")]
      .find((button) => button.textContent?.includes("Pair with this host"))!
      .click();
    await flush();

    expect(toasts).toEqual(["No GameStream host is configured yet."]);
    expect(root.querySelector("[data-runner-pairing='runner-1']")).toBeNull();
    expect(
      [...root.querySelectorAll<HTMLButtonElement>("button")].some((button) =>
        button.textContent?.includes("Pair with this host"),
      ),
    ).toBe(true);
  });

  // Artwork runs after the games are in and takes a request and up to four
  // downloads per card: a library's worth of cards is minutes of silence
  // unless the panel says what it is doing.
  it("says when it is still looking for artwork, and will not start a second import", () => {
    const controller = createFakeController([runner()], {}, [], new Set(["runner-1"]));
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });

    expect(root.textContent).toContain("Finding artwork…");
    expect(root.querySelector(".plugin-progress")).not.toBeNull();
    const importButton = [...root.querySelectorAll<HTMLButtonElement>("button")].find((button) =>
      button.textContent?.includes("Import games"),
    )!;
    expect(importButton.disabled).toBe(true);
  });

  it("re-renders itself whenever the controller announces a change", () => {
    const runners = [runner({ profiles: [] })];
    const controller = createFakeController(runners);
    mountRunnerPanel(root, controller, { showToast: (message) => toasts.push(message) });
    expect(root.querySelectorAll("[data-runner-profile]")).toHaveLength(0);

    runners[0]!.profiles.push(profile());
    controller.emit();

    expect(root.querySelectorAll("[data-runner-profile]")).toHaveLength(1);
  });
});
