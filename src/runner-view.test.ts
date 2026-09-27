import { beforeEach, describe, expect, it } from "vitest";
import { mountRunnerPanel } from "./runner-view";
import type {
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
    importFor: (profileId) => jobs[profileId] ?? null,
    async createProfile(pluginId, displayName) {
      calls.push(`createProfile:${pluginId}:${displayName}`);
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

    expect(controller.calls).toContain("createProfile:com.orivo.dolphin:Dolphin");
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
