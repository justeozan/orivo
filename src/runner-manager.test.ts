import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createDefaultRunnerManagerClient,
  createRunnerManagerController,
  isRunnerImportRunning,
  readInstalledRunners,
  runnerErrorMessage,
  runnerImportSummary,
  runnerProfileStatusLabel,
  type InstalledRunnerView,
  type RunnerImportJobView,
  type RunnerManagerClient,
  type RunnerProfileView,
} from "./runner-manager";

const liveSignal = (): AbortSignal => new AbortController().signal;

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

function job(overrides: Partial<RunnerImportJobView> = {}): RunnerImportJobView {
  return {
    jobId: "runner-import-1",
    profileId: "runner-1",
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

interface RecordedRunnerManager {
  client: RunnerManagerClient;
  calls: string[];
}

function createFakeRunnerManager(overrides: Partial<RunnerManagerClient> = {}): RecordedRunnerManager {
  const calls: string[] = [];
  const client: RunnerManagerClient = {
    async getInstalledRunners(signal) {
      calls.push("getInstalledRunners");
      return overrides.getInstalledRunners ? overrides.getInstalledRunners(signal) : [runner()];
    },
    async createProfile(pluginId, displayName, signal) {
      calls.push(`createProfile:${pluginId}:${displayName}`);
      return overrides.createProfile
        ? overrides.createProfile(pluginId, displayName, signal)
        : profile({ pluginId, displayName });
    },
    async renameProfile(profileId, displayName, signal) {
      calls.push(`renameProfile:${profileId}:${displayName}`);
      return overrides.renameProfile
        ? overrides.renameProfile(profileId, displayName, signal)
        : profile({ id: profileId, displayName });
    },
    async setProfileEnabled(profileId, enabled, signal) {
      calls.push(`setProfileEnabled:${profileId}:${enabled}`);
      return overrides.setProfileEnabled
        ? overrides.setProfileEnabled(profileId, enabled, signal)
        : profile({ id: profileId, enabled });
    },
    async deleteProfile(profileId, signal) {
      calls.push(`deleteProfile:${profileId}`);
      return overrides.deleteProfile ? overrides.deleteProfile(profileId, signal) : true;
    },
    async grantDirectory(profileId, slot, signal) {
      calls.push(`grantDirectory:${profileId}:${slot ?? "default"}`);
      return overrides.grantDirectory
        ? overrides.grantDirectory(profileId, slot, signal)
        : profile({ id: profileId });
    },
    async revokeDirectory(profileId, directoryId, signal) {
      calls.push(`revokeDirectory:${profileId}:${directoryId}`);
      return overrides.revokeDirectory
        ? overrides.revokeDirectory(profileId, directoryId, signal)
        : profile({ id: profileId, directories: [] });
    },
    async startImport(profileId, signal) {
      calls.push(`startImport:${profileId}`);
      return overrides.startImport ? overrides.startImport(profileId, signal) : job({ profileId });
    },
    async getImportStatus(jobId, signal) {
      calls.push(`getImportStatus:${jobId}`);
      return overrides.getImportStatus
        ? overrides.getImportStatus(jobId, signal)
        : job({ jobId, phase: "ready" });
    },
    async cancelImport(jobId, signal) {
      calls.push(`cancelImport:${jobId}`);
      // The real cancel_import (runner_commands.rs) only sets a flag and
      // returns the job's current view — still "running" until the worker
      // thread notices and stops — which is exactly what the default here
      // mimics, rather than pretending the cancel is instant.
      return overrides.cancelImport
        ? overrides.cancelImport(jobId, signal)
        : job({ jobId, phase: "running" });
    },
  };
  return { client, calls };
}

afterEach(() => {
  vi.useRealTimers();
});

// ---------------------------------------------------------------------------
// Reading the host's answer
// ---------------------------------------------------------------------------

describe("readInstalledRunners", () => {
  it("keeps a well-formed runner with its profiles and drops one with no id", () => {
    const rows = readInstalledRunners([runner(), { name: "no id" }]);
    expect(rows).toEqual([runner()]);
  });

  it("falls back an unknown state to invalid rather than trusting it", () => {
    const rows = readInstalledRunners([{ id: "x", state: "brand-new" }]);
    expect(rows[0]?.state).toBe("invalid");
  });

  it("drops a profile with no plugin id, without dropping the runner", () => {
    const rows = readInstalledRunners([{ ...runner(), profiles: [{ id: "orphan" }] }]);
    expect(rows[0]?.profiles).toEqual([]);
  });

  it("defaults enabled to true and a missing game count to zero", () => {
    const rows = readInstalledRunners([
      { ...runner(), profiles: [{ id: "runner-1", pluginId: "com.orivo.dolphin" }] },
    ]);
    expect(rows[0]?.profiles[0]).toMatchObject({ enabled: true, gameCount: 0, directories: [] });
  });
});

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

describe("runnerProfileStatusLabel", () => {
  it("names every status a profile can carry", () => {
    expect(runnerProfileStatusLabel(profile({ status: "valid" }))).toBe("Ready");
    expect(runnerProfileStatusLabel(profile({ status: "unvalidated" }))).toBe(
      "Checking with the plugin…",
    );
    expect(runnerProfileStatusLabel(profile({ status: "rejected", statusMessage: "too old" }))).toBe(
      "Rejected: too old",
    );
    expect(runnerProfileStatusLabel(profile({ status: "rejected", statusMessage: null }))).toBe(
      "Rejected by the plugin",
    );
  });
});

describe("runnerImportSummary and isRunnerImportRunning", () => {
  it("says nothing about a profile that never ran an import", () => {
    expect(runnerImportSummary(null)).toBe("");
    expect(isRunnerImportRunning(null)).toBe(false);
  });

  it("distinguishes a fresh import from one resuming a cursor", () => {
    expect(runnerImportSummary(job({ phase: "running", imported: 2 }))).toBe(
      "Importing… 2 game(s) so far",
    );
    expect(runnerImportSummary(job({ phase: "running", resumed: true, refreshed: 1 }))).toBe(
      "Continuing… 1 game(s) so far",
    );
    expect(isRunnerImportRunning(job({ phase: "running" }))).toBe(true);
    expect(isRunnerImportRunning(job({ phase: "ready" }))).toBe(false);
  });

  it("falls back to a plain sentence for a finished, cancelled or failed job with no message", () => {
    expect(runnerImportSummary(job({ phase: "ready", message: "", imported: 4, refreshed: 1 }))).toBe(
      "Imported 4 game(s), refreshed 1.",
    );
    expect(runnerImportSummary(job({ phase: "cancelled", message: "" }))).toBe(
      "Import stopped. Orivo kept its place.",
    );
    expect(runnerImportSummary(job({ phase: "failed", message: "" }))).toBe("This import did not finish.");
  });
});

describe("runnerErrorMessage", () => {
  it("prefers a host string, then an Error message, then a default", () => {
    expect(runnerErrorMessage("No folder was chosen.")).toBe("No folder was chosen.");
    expect(runnerErrorMessage(new Error("boom"))).toBe("boom");
    expect(runnerErrorMessage(undefined)).toBe("This did not work. Try again.");
  });
});

// ---------------------------------------------------------------------------
// The controller
// ---------------------------------------------------------------------------

describe("createRunnerManagerController", () => {
  it("loads the installed runners with their profiles", async () => {
    const fake = createFakeRunnerManager();
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());
    expect(controller.runners()).toEqual([runner()]);
  });

  it("reloads the list after creating a profile", async () => {
    const fake = createFakeRunnerManager();
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    const created = await controller.createProfile("com.orivo.dolphin", "My Dolphin");
    expect(created.displayName).toBe("My Dolphin");
    expect(fake.calls).toContain("getInstalledRunners");
    expect(fake.calls.filter((call) => call === "getInstalledRunners")).toHaveLength(2);
  });

  it("rejects with the host's message and changes nothing when the picker is cancelled", async () => {
    const fake = createFakeRunnerManager({
      createProfile: async () => {
        throw "No emulator was chosen.";
      },
    });
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    await expect(controller.createProfile("com.orivo.dolphin", "My Dolphin")).rejects.toBe(
      "No emulator was chosen.",
    );
    expect(fake.calls.filter((call) => call === "getInstalledRunners")).toHaveLength(1);
  });

  it("renames, enables, grants and revokes, reloading after each", async () => {
    const fake = createFakeRunnerManager();
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    await controller.renameProfile("runner-1", "New name");
    await controller.setEnabled("runner-1", false);
    await controller.grantDirectory("runner-1");
    await controller.revokeDirectory("runner-1", "games");

    expect(fake.calls).toEqual([
      "getInstalledRunners",
      "renameProfile:runner-1:New name",
      "getInstalledRunners",
      "setProfileEnabled:runner-1:false",
      "getInstalledRunners",
      "grantDirectory:runner-1:default",
      "getInstalledRunners",
      "revokeDirectory:runner-1:games",
      "getInstalledRunners",
    ]);
  });

  it("deleting a profile forgets any import job it had and reloads", async () => {
    vi.useFakeTimers();
    const fake = createFakeRunnerManager();
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    await controller.startImport("runner-1");
    expect(controller.importFor("runner-1")?.phase).toBe("running");

    await controller.deleteProfile("runner-1");
    expect(controller.importFor("runner-1")).toBeNull();

    // The job's poll must not still be ticking after the profile is gone.
    await vi.advanceTimersByTimeAsync(5_000);
    expect(fake.calls.filter((call) => call.startsWith("getImportStatus"))).toHaveLength(0);
  });

  it("polls a running import to completion and then reloads the list", async () => {
    vi.useFakeTimers();
    let polls = 0;
    const fake = createFakeRunnerManager({
      getImportStatus: async (jobId) => {
        polls += 1;
        return polls < 3
          ? job({ jobId, phase: "running", imported: polls })
          : job({ jobId, phase: "ready", imported: 3, message: "Imported 3 game(s), refreshed 0." });
      },
    });
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    await controller.startImport("runner-1");
    expect(controller.importFor("runner-1")?.phase).toBe("running");

    await vi.advanceTimersByTimeAsync(500);
    expect(controller.importFor("runner-1")?.phase).toBe("running");
    expect(controller.importFor("runner-1")?.imported).toBe(1);

    await vi.advanceTimersByTimeAsync(1_000);
    expect(controller.importFor("runner-1")?.phase).toBe("ready");
    // The list is re-read once the job settles, since only the host knows the
    // profile's new game count.
    expect(fake.calls.filter((call) => call === "getInstalledRunners")).toHaveLength(2);
  });

  it("keeps polling after cancel until the host actually stops the job", async () => {
    vi.useFakeTimers();
    const fake = createFakeRunnerManager({
      getImportStatus: async (jobId) => job({ jobId, phase: "cancelled" }),
    });
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    await controller.startImport("runner-1");
    await controller.cancelImport("runner-1");

    // cancel_import only flags the job; the answer to the cancel call itself
    // is still "running" until the worker thread notices and stops, so the
    // panel must not read this as settled yet.
    expect(controller.importFor("runner-1")?.phase).toBe("running");
    expect(fake.calls).toContain("cancelImport:runner-import-1");

    await vi.advanceTimersByTimeAsync(500);
    expect(controller.importFor("runner-1")?.phase).toBe("cancelled");
  });

  it("stops polling once the host confirms the cancellation went through", async () => {
    vi.useFakeTimers();
    const fake = createFakeRunnerManager({
      getImportStatus: async (jobId) => job({ jobId, phase: "cancelled" }),
    });
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    await controller.startImport("runner-1");
    await controller.cancelImport("runner-1");
    await vi.advanceTimersByTimeAsync(500);
    expect(controller.importFor("runner-1")?.phase).toBe("cancelled");

    const pollsAtSettling = fake.calls.filter((call) => call.startsWith("getImportStatus")).length;
    await vi.advanceTimersByTimeAsync(5_000);
    expect(fake.calls.filter((call) => call.startsWith("getImportStatus"))).toHaveLength(
      pollsAtSettling,
    );
  });

  it("turns a refused start into a failed job rather than throwing", async () => {
    const fake = createFakeRunnerManager({
      startImport: async () => {
        throw "This runner profile is no longer available.";
      },
    });
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());

    await controller.startImport("runner-1");
    const failed = controller.importFor("runner-1");
    expect(failed?.phase).toBe("failed");
    expect(failed?.message).toBe("This runner profile is no longer available.");
  });

  it("stops polling once disposed", async () => {
    vi.useFakeTimers();
    const fake = createFakeRunnerManager();
    const controller = createRunnerManagerController(fake.client);
    await controller.load(liveSignal());
    await controller.startImport("runner-1");

    controller.dispose();
    await vi.advanceTimersByTimeAsync(5_000);

    expect(fake.calls.filter((call) => call.startsWith("getImportStatus"))).toHaveLength(0);
    expect(controller.runners()).toEqual([]);
  });

  it("notifies listeners on load and on every import tick", async () => {
    vi.useFakeTimers();
    const fake = createFakeRunnerManager();
    const controller = createRunnerManagerController(fake.client);
    let changes = 0;
    controller.onChange(() => {
      changes += 1;
    });

    await controller.load(liveSignal());
    expect(changes).toBe(1);

    await controller.startImport("runner-1");
    expect(changes).toBe(2);

    await vi.advanceTimersByTimeAsync(500);
    // One tick for the poll result, one for the reload it triggers on completion.
    expect(changes).toBe(4);
  });
});

// ---------------------------------------------------------------------------
// A host that cannot answer
// ---------------------------------------------------------------------------

describe("the default client outside the desktop shell", () => {
  it("reads an empty list rather than throwing", async () => {
    const client = createDefaultRunnerManagerClient();
    await expect(client.getInstalledRunners(liveSignal())).resolves.toEqual([]);
  });

  it("says every mutation needs the desktop app", async () => {
    const client = createDefaultRunnerManagerClient();
    await expect(client.createProfile("com.orivo.dolphin", "Dolphin", liveSignal())).rejects.toThrow(
      "Orivo desktop app",
    );
    await expect(client.grantDirectory("runner-1", null, liveSignal())).rejects.toThrow(
      "Orivo desktop app",
    );
    await expect(client.startImport("runner-1", liveSignal())).rejects.toThrow("Orivo desktop app");
  });
});
