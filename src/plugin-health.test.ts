import { describe, expect, it } from "vitest";
import {
  createDefaultPluginHealthClient,
  phraseJournalEntry,
  pluginHealthErrorMessage,
  pluginHealthSummary,
  readHealthReport,
  readJournal,
  type PluginHealthView,
  type PluginJournalEntryView,
} from "./plugin-health";

const liveSignal = (): AbortSignal => new AbortController().signal;

function health(overrides: Partial<PluginHealthView> = {}): PluginHealthView {
  return {
    pluginId: "com.orivo.dolphin",
    queued: 0,
    running: 0,
    consecutiveFailures: 0,
    degraded: false,
    ...overrides,
  };
}

function entry(overrides: Partial<PluginJournalEntryView> = {}): PluginJournalEntryView {
  return {
    pluginId: "com.orivo.dolphin",
    decision: "scope-refused",
    detail: "",
    repeats: 1,
    ...overrides,
  };
}

describe("readHealthReport", () => {
  it("keeps well-formed rows and drops one with no id", () => {
    const rows = readHealthReport([
      { pluginId: "com.orivo.dolphin", queued: 2, running: 1, consecutiveFailures: 3, degraded: true },
      { queued: 1 },
      "not an object",
    ]);
    expect(rows).toEqual([health({ queued: 2, running: 1, consecutiveFailures: 3, degraded: true })]);
  });

  it("reads a negative or missing count as zero rather than trusting it", () => {
    const rows = readHealthReport([{ pluginId: "com.orivo.dolphin", queued: -5 }]);
    expect(rows).toEqual([health()]);
  });

  it("is not confused by a non-array payload", () => {
    expect(readHealthReport(null)).toEqual([]);
    expect(readHealthReport({ pluginId: "x" })).toEqual([]);
  });
});

describe("readJournal", () => {
  it("keeps a well-formed entry and drops one with no decision", () => {
    const rows = readJournal([
      { pluginId: "com.orivo.dolphin", decision: "trap", detail: "fuel exhausted", repeats: 3 },
      { pluginId: "com.orivo.dolphin", detail: "missing decision" },
    ]);
    expect(rows).toEqual([entry({ decision: "trap", detail: "fuel exhausted", repeats: 3 })]);
  });

  it("floors a fractional repeat count and never reads below one", () => {
    const rows = readJournal([
      { pluginId: "com.orivo.dolphin", decision: "trap", repeats: 2.9 },
      { pluginId: "com.orivo.dolphin", decision: "trap", repeats: 0 },
    ]);
    expect(rows[0]?.repeats).toBe(2);
    expect(rows[1]?.repeats).toBe(1);
  });
});

describe("phraseJournalEntry", () => {
  it("renders every decision the host is known to record", () => {
    const cases: Array<[string, string]> = [
      ["job-panicked", "A background job crashed unexpectedly. Orivo kept running."],
      ["cancelled", "A queued request was cancelled before it ran."],
      ["degraded", "Orivo paused this plugin after repeated failures."],
      ["resumed", "This plugin was resumed."],
      ["scope-refused", "This plugin asked for a folder it was not granted."],
      ["identity-mismatch", "This plugin's component does not match what it announced."],
    ];
    for (const [decision, expected] of cases) {
      expect(phraseJournalEntry(entry({ decision, detail: "" }))).toBe(expected);
    }
  });

  it("folds the host's own detail into decisions that carry one", () => {
    expect(phraseJournalEntry(entry({ decision: "trap", detail: "fuel exhausted" }))).toBe(
      "This plugin's request failed: fuel exhausted",
    );
    expect(phraseJournalEntry(entry({ decision: "prepare-launch", detail: "refused: no grant" }))).toBe(
      "Preparing a launch failed: refused: no grant",
    );
  });

  it("still renders a decision it does not recognise, humanised rather than dropped", () => {
    expect(phraseJournalEntry(entry({ decision: "future-refusal-kind", detail: "" }))).toBe(
      "Future refusal kind.",
    );
    expect(phraseJournalEntry(entry({ decision: "future-refusal-kind", detail: "because" }))).toBe(
      "Future refusal kind: because",
    );
  });

  it("appends the repeat count only when it repeated", () => {
    expect(phraseJournalEntry(entry({ decision: "resumed", repeats: 1 }))).not.toMatch(/×/);
    expect(phraseJournalEntry(entry({ decision: "scope-refused", repeats: 5 }))).toBe(
      "This plugin asked for a folder it was not granted. (×5)",
    );
  });
});

describe("pluginHealthSummary", () => {
  it("says nothing about a plugin with nothing to report", () => {
    expect(pluginHealthSummary(null)).toBe("");
    expect(pluginHealthSummary(health())).toBe("");
  });

  it("leads with degraded over queue depth", () => {
    expect(pluginHealthSummary(health({ degraded: true, consecutiveFailures: 1 }))).toBe(
      "Degraded · 1 failure",
    );
    expect(pluginHealthSummary(health({ degraded: true, consecutiveFailures: 4, running: 2 }))).toBe(
      "Degraded · 4 failures",
    );
  });

  it("reports what is actually moving when the plugin is not degraded", () => {
    expect(pluginHealthSummary(health({ running: 1 }))).toBe("Running 1 job");
    expect(pluginHealthSummary(health({ running: 3 }))).toBe("Running 3 jobs");
    expect(pluginHealthSummary(health({ queued: 1 }))).toBe("1 job queued");
    expect(pluginHealthSummary(health({ queued: 4 }))).toBe("4 jobs queued");
  });
});

describe("the default client outside the desktop shell", () => {
  it("answers every call safely with no Tauri runtime present", async () => {
    const client = createDefaultPluginHealthClient();
    await expect(client.getHealthReport(["com.orivo.dolphin"], liveSignal())).resolves.toEqual([]);
    await expect(client.getJournal("com.orivo.dolphin", liveSignal())).resolves.toEqual([]);
    await expect(client.resume("com.orivo.dolphin", liveSignal())).rejects.toThrow(
      "réservé à l'application Orivo",
    );
  });

  it("returns no report at all for an empty id list, without a round trip", async () => {
    const client = createDefaultPluginHealthClient();
    await expect(client.getHealthReport([], liveSignal())).resolves.toEqual([]);
  });
});

describe("pluginHealthErrorMessage", () => {
  it("prefers a host string, then an Error message, then a default", () => {
    expect(pluginHealthErrorMessage("Ce plugin n'existe plus.")).toBe("Ce plugin n'existe plus.");
    expect(pluginHealthErrorMessage(new Error("boom"))).toBe("boom");
    expect(pluginHealthErrorMessage(undefined)).toBe("Orivo could not read this plugin's health.");
  });
});
