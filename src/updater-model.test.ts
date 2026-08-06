import { describe, expect, it } from "vitest";
import {
  INITIAL_UPDATE_STATE,
  type UpdateState,
  applyCheckResult,
  applyError,
  applyProgress,
  describeUpdateState,
  formatBytes,
  markReady,
  startCheck,
  startDownload,
  updateProgressPercent,
} from "./updater-model";

const running = (overrides: Partial<UpdateState> = {}): UpdateState => ({
  ...INITIAL_UPDATE_STATE,
  currentVersion: "0.3.0",
  ...overrides,
});

describe("updater model", () => {
  it("starts from an idle state that has downloaded nothing", () => {
    expect(INITIAL_UPDATE_STATE).toEqual({
      status: "idle",
      currentVersion: "",
      availableVersion: null,
      notes: null,
      downloadedBytes: 0,
      totalBytes: null,
      error: null,
      failure: null,
    });
  });

  it("drops the previous result when a new check starts", () => {
    const stale = running({
      status: "error",
      availableVersion: "0.4.0",
      notes: "Old notes",
      downloadedBytes: 512,
      totalBytes: 2_048,
      error: "Network unreachable",
    });

    // Re-checking must not leave "0.4.0 is available" on screen: the server is
    // free to answer differently, and the old numbers would be a lie.
    expect(startCheck(stale)).toEqual(running({ status: "checking" }));
  });

  it("treats a null check result as up to date", () => {
    const state = applyCheckResult(startCheck(running()), null);
    expect(state).toEqual(running({ status: "up-to-date" }));
  });

  it("records the offered version, its notes, and the reported current version", () => {
    const state = applyCheckResult(startCheck(running()), {
      version: "0.4.0",
      currentVersion: "0.3.1",
      body: "  Faster store search.  ",
    });

    expect(state).toEqual(
      running({
        status: "available",
        currentVersion: "0.3.1",
        availableVersion: "0.4.0",
        notes: "Faster store search.",
      }),
    );
  });

  it("keeps the known current version and blanks empty notes", () => {
    const state = applyCheckResult(startCheck(running()), { version: "0.4.0", body: "   " });
    expect(state.currentVersion).toBe("0.3.0");
    expect(state.notes).toBeNull();
  });

  it("clears a previous error when an update is offered", () => {
    const state = applyCheckResult(running({ status: "error", error: "Timed out" }), {
      version: "0.4.0",
    });
    expect(state.status).toBe("available");
    expect(state.error).toBeNull();
  });

  it("accumulates chunks against the announced content length", () => {
    let state = startDownload(running({ status: "available", availableVersion: "0.4.0" }));
    expect(state).toEqual(
      running({ status: "downloading", availableVersion: "0.4.0", notes: null }),
    );

    state = applyProgress(state, 0, 4_000);
    state = applyProgress(state, 1_000);
    state = applyProgress(state, 1_000);

    expect(state.downloadedBytes).toBe(2_000);
    expect(state.totalBytes).toBe(4_000);
    expect(updateProgressPercent(state)).toBe(50);
  });

  it("keeps the total unknown when the server reports no content length", () => {
    let state = applyProgress(startDownload(running()), 0, null);
    state = applyProgress(state, 4_096);

    expect(state.totalBytes).toBeNull();
    expect(state.downloadedBytes).toBe(4_096);
    expect(updateProgressPercent(state)).toBeNull();
    expect(describeUpdateState(state)).toEqual({
      label: "Downloading update…",
      detail: "4.0 KB downloaded",
      buttonLabel: "Downloading…",
      buttonDisabled: true,
    });
  });

  it("ignores content lengths and chunk sizes that cannot be counted", () => {
    let state = applyProgress(startDownload(running()), 0, 0);
    expect(state.totalBytes).toBeNull();

    state = applyProgress(state, 0, -1);
    state = applyProgress(state, 0, Number.NaN);
    expect(state.totalBytes).toBeNull();

    state = applyProgress(state, -500);
    state = applyProgress(state, Number.POSITIVE_INFINITY);
    state = applyProgress(state, 10.7);
    expect(state.downloadedBytes).toBe(10);

    // A later, valid content length is still adopted.
    state = applyProgress(state, 0, 1_000.9);
    expect(state.totalBytes).toBe(1_000);
  });

  it("clamps the percentage when a stream overshoots the announced length", () => {
    const state = applyProgress(applyProgress(startDownload(running()), 0, 1_000), 5_000);
    expect(updateProgressPercent(state)).toBe(100);
    expect(describeUpdateState(state).label).toBe("Downloading update — 100%");
  });

  it("snaps the counter to the total when the download finishes", () => {
    const downloading = applyProgress(applyProgress(startDownload(running()), 0, 4_000), 3_990);
    const ready = markReady(downloading);

    expect(ready.status).toBe("ready");
    expect(ready.downloadedBytes).toBe(4_000);
    expect(updateProgressPercent(ready)).toBe(100);
  });

  it("leaves the counter alone when the download finishes with no known total", () => {
    const ready = markReady(applyProgress(startDownload(running()), 2_048));
    expect(ready.downloadedBytes).toBe(2_048);
    expect(ready.totalBytes).toBeNull();
  });

  it("coerces every kind of failure into a readable message", () => {
    expect(applyError(running(), new Error("Signature mismatch")).error).toBe("Signature mismatch");
    expect(applyError(running(), "  endpoint returned 503  ").error).toBe("endpoint returned 503");
    expect(applyError(running(), { message: "no updater endpoint" }).error).toBe(
      "no updater endpoint",
    );
    expect(applyError(running(), new Error("   ")).error).toBe(
      "The update could not be completed.",
    );
    expect(applyError(running(), null).error).toBe("The update could not be completed.");
    expect(applyError(running(), 42).error).toBe("The update could not be completed.");
    expect(applyError(running(), { message: 7 }).error).toBe("The update could not be completed.");
  });

  it("keeps the offered version visible after a failed download", () => {
    const failed = applyError(
      startDownload(running({ status: "available", availableVersion: "0.4.0" })),
      new Error("Connection reset"),
    );
    expect(failed.status).toBe("error");
    expect(failed.availableVersion).toBe("0.4.0");
  });

  it("attributes a failure to the step it happened in", () => {
    expect(applyError(startCheck(running()), new Error("503")).failure).toBe("check");
    expect(applyError(startDownload(running()), new Error("disk full")).failure).toBe("download");
    // A retried check clears the attribution along with the message.
    expect(startCheck(applyError(startDownload(running()), new Error("x"))).failure).toBeNull();
  });

  it("describes the idle state", () => {
    expect(describeUpdateState(running())).toEqual({
      label: "Orivo has not checked for updates yet.",
      detail: "You are running version 0.3.0.",
      buttonLabel: "Check for updates",
      buttonDisabled: false,
    });
    expect(describeUpdateState(INITIAL_UPDATE_STATE).detail).toBe(
      "Check whenever you like — nothing is downloaded until you ask.",
    );
  });

  it("describes the checking state", () => {
    expect(describeUpdateState(startCheck(running()))).toEqual({
      label: "Checking for updates…",
      detail: "Asking the Orivo release feed for a newer version.",
      buttonLabel: "Checking…",
      buttonDisabled: true,
    });
  });

  it("describes an available update, preferring its release notes", () => {
    const withNotes = applyCheckResult(running(), {
      version: "0.4.0",
      body: "Adds Wine profiles.",
    });
    expect(describeUpdateState(withNotes)).toEqual({
      label: "Version 0.4.0 is available",
      detail: "Adds Wine profiles.",
      buttonLabel: "Download and install",
      buttonDisabled: false,
    });

    const withoutNotes = applyCheckResult(running(), { version: "0.4.0" });
    expect(describeUpdateState(withoutNotes).detail).toBe(
      "Download it now. Orivo installs the update and restarts once to finish.",
    );

    // A malformed manifest must not render "Version null is available".
    expect(describeUpdateState(running({ status: "available" })).label).toBe(
      "Version unknown is available",
    );
  });

  it("describes the up-to-date state", () => {
    expect(describeUpdateState(applyCheckResult(running(), null))).toEqual({
      label: "You're on the latest version.",
      detail: "Orivo 0.3.0 is the newest release.",
      buttonLabel: "Check again",
      buttonDisabled: false,
    });
    expect(describeUpdateState(applyCheckResult(INITIAL_UPDATE_STATE, null)).detail).toBe(
      "There is nothing newer to install.",
    );
  });

  it("describes a download with a known size", () => {
    const state = applyProgress(applyProgress(startDownload(running()), 0, 2_097_152), 1_048_576);
    expect(describeUpdateState(state)).toEqual({
      label: "Downloading update — 50%",
      detail: "1.0 MB of 2.0 MB",
      buttonLabel: "Downloading…",
      buttonDisabled: true,
    });
  });

  it("describes the ready state", () => {
    const ready = markReady(startDownload(applyCheckResult(running(), { version: "0.4.0" })));
    expect(describeUpdateState(ready)).toEqual({
      label: "Update ready to install",
      detail: "Restart Orivo to finish updating to version 0.4.0.",
      buttonLabel: "Restart to update",
      buttonDisabled: false,
    });
    expect(describeUpdateState(markReady(running())).detail).toBe(
      "Restart Orivo to finish the update.",
    );
  });

  it("describes the error state, naming the step that failed", () => {
    expect(describeUpdateState(applyError(startCheck(running()), new Error("Signature mismatch")))).toEqual({
      label: "The update check failed",
      detail: "Signature mismatch",
      buttonLabel: "Try again",
      buttonDisabled: false,
    });
    // A download that dies must not claim the *check* failed: the user has
    // already been told a version is waiting, and the two are fixed differently.
    expect(describeUpdateState(applyError(startDownload(running()), new Error("Disk full")))).toEqual({
      label: "The update could not be installed",
      detail: "Disk full",
      buttonLabel: "Try again",
      buttonDisabled: false,
    });
    expect(describeUpdateState(running({ status: "error" })).detail).toBe(
      "Something went wrong. Try again in a moment.",
    );
  });

  it("formats byte counters the way the rest of Settings does", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(2_048)).toBe("2.0 KB");
    expect(formatBytes(1_048_576)).toBe("1.0 MB");
    expect(formatBytes(1_073_741_824)).toBe("1.0 GB");
    expect(formatBytes(Number.NaN)).toBe("0 B");
  });

  it("never mutates the state it is handed", () => {
    const state = running({ status: "downloading", downloadedBytes: 10, totalBytes: 100 });
    const snapshot = { ...state };
    startCheck(state);
    applyCheckResult(state, { version: "0.4.0" });
    applyCheckResult(state, null);
    startDownload(state);
    applyProgress(state, 5, 200);
    markReady(state);
    applyError(state, new Error("boom"));
    expect(state).toEqual(snapshot);
  });
});
