/* ---------------------------------------------------------------------------
   Plugin health and journal — the "why is this plugin unhappy" half of
   Settings › Plugins.

   The host already tracks whether a plugin is `degraded` and keeps a bounded,
   sanitised journal of what it refused and what the plugin logged itself
   (`plugin_health.rs`, reading `plugin_scheduler.rs` and `plugin_runtime.rs`).
   This module turns that into sentences a player can read, the same way
   `plugin-manager.ts` turns a bare `PluginState` into "Installed" or
   "Incompatible". Same contract as the rest of Settings: a host binary built
   before these commands existed, and a browser with no Tauri at all, both
   collapse to "nothing to report" rather than an error.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";

export interface PluginHealthView {
  pluginId: string;
  queued: number;
  running: number;
  consecutiveFailures: number;
  degraded: boolean;
}

export interface PluginJournalEntryView {
  pluginId: string;
  decision: string;
  detail: string;
  /** How many times this exact entry repeated. 1 means it happened once. */
  repeats: number;
}

export interface PluginHealthClient {
  getHealthReport(pluginIds: string[], signal: AbortSignal): Promise<PluginHealthView[]>;
  resume(pluginId: string, signal: AbortSignal): Promise<PluginHealthView>;
  getJournal(pluginId: string, signal: AbortSignal): Promise<PluginJournalEntryView[]>;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("La requête a été annulée.", "AbortError");
}

export function pluginHealthErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return "Orivo could not read this plugin's health.";
}

function readHealthView(value: unknown): PluginHealthView[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<PluginHealthView>;
  if (typeof raw.pluginId !== "string" || !raw.pluginId) return [];
  const count = (n: unknown): number => (typeof n === "number" && Number.isFinite(n) && n >= 0 ? n : 0);
  return [
    {
      pluginId: raw.pluginId,
      queued: count(raw.queued),
      running: count(raw.running),
      consecutiveFailures: count(raw.consecutiveFailures),
      degraded: raw.degraded === true,
    },
  ];
}

export function readHealthReport(value: unknown): PluginHealthView[] {
  return Array.isArray(value) ? value.flatMap(readHealthView) : [];
}

function readJournalEntry(value: unknown): PluginJournalEntryView[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<PluginJournalEntryView>;
  if (typeof raw.pluginId !== "string" || !raw.pluginId) return [];
  if (typeof raw.decision !== "string" || !raw.decision) return [];
  return [
    {
      pluginId: raw.pluginId,
      decision: raw.decision,
      // Already bounded and stripped of control characters by the host
      // (`sanitise_text` in `plugin_runtime.rs`) before it reaches this
      // command, so nothing here re-sanitises it — only defends against a
      // missing field.
      detail: typeof raw.detail === "string" ? raw.detail : "",
      repeats:
        typeof raw.repeats === "number" && Number.isFinite(raw.repeats) && raw.repeats >= 1
          ? Math.floor(raw.repeats)
          : 1,
    },
  ];
}

export function readJournal(value: unknown): PluginJournalEntryView[] {
  return Array.isArray(value) ? value.flatMap(readJournalEntry) : [];
}

export function createDefaultPluginHealthClient(): PluginHealthClient {
  return {
    async getHealthReport(pluginIds, signal) {
      try {
        if (!isTauriRuntime() || pluginIds.length === 0) return [];
        assertActive(signal);
        const report = await invoke<unknown>("get_plugin_health_report", { pluginIds });
        assertActive(signal);
        return readHealthReport(report);
      } catch {
        return [];
      }
    },

    async resume(pluginId, signal) {
      if (!isTauriRuntime()) {
        throw new Error("Reprendre un plugin est réservé à l'application Orivo.");
      }
      assertActive(signal);
      const view = await invoke<unknown>("resume_plugin", { pluginId });
      const [health] = readHealthView(view);
      if (!health) throw new Error("Orivo n'a pas pu reprendre ce plugin.");
      return health;
    },

    async getJournal(pluginId, signal) {
      try {
        if (!isTauriRuntime()) return [];
        assertActive(signal);
        const entries = await invoke<unknown>("get_plugin_journal", { pluginId });
        assertActive(signal);
        return readJournal(entries);
      } catch {
        return [];
      }
    },
  };
}

/* ---------------------------------------------------------------------------
   Copy — turning a host decision code into a sentence
   --------------------------------------------------------------------------- */

/**
 * One phrase per decision the host journal is known to record
 * (`plugin_scheduler.rs`, `plugin_runtime.rs`). An unrecognised code still
 * renders — as its own words with the detail beside it — rather than being
 * dropped, because a plugin's journal outliving this table's memory is a
 * certainty, not an edge case.
 */
const DECISION_PHRASES: Readonly<Record<string, (detail: string) => string>> = {
  "job-panicked": () => "A background job crashed unexpectedly. Orivo kept running.",
  "job-failed": (detail) => `A background job failed${detail ? `: ${detail}` : "."}`,
  cancelled: () => "A queued request was cancelled before it ran.",
  degraded: () => "Orivo paused this plugin after repeated failures.",
  resumed: () => "This plugin was resumed.",
  "submit-refused": (detail) => `Orivo could not queue a request${detail ? `: ${detail}` : "."}`,
  "host-call-budget": () =>
    "This plugin made too many requests to Orivo in one call and was cut off.",
  "capability-refused": (detail) =>
    `This plugin tried something it has no permission for${detail ? `: ${detail}` : "."}`,
  "scope-refused": () => "This plugin asked for a folder it was not granted.",
  "files-truncated": (detail) => `A folder listing was shortened${detail ? `: ${detail}` : "."}`,
  "identity-mismatch": () => "This plugin's component does not match what it announced.",
  // Neutral on purpose: a plugin that simply has no use for a capability logs
  // this on every call it makes, which is expected behaviour, not wrongdoing —
  // "asked for" would accuse a plugin that is doing exactly what it should.
  "capability-unlinked": (detail) => detail || "A capability this plugin's manifest does not declare is unavailable to it.",
  trap: (detail) => `This plugin's request failed${detail ? `: ${detail}` : "."}`,
  "get-identity": (detail) => `Reading this plugin's identity failed${detail ? `: ${detail}` : "."}`,
  "health-check": (detail) => `The last health check failed${detail ? `: ${detail}` : "."}`,
  "validate-profile": (detail) => `Checking a runner profile failed${detail ? `: ${detail}` : "."}`,
  "discover-page": (detail) => `Looking for games failed${detail ? `: ${detail}` : "."}`,
  "prepare-launch": (detail) => `Preparing a launch failed${detail ? `: ${detail}` : "."}`,
};

function fallbackPhrase(decision: string, detail: string): string {
  const words = decision.replace(/-/g, " ");
  const sentence = words.charAt(0).toUpperCase() + words.slice(1);
  return detail ? `${sentence}: ${detail}` : `${sentence}.`;
}

/** A readable line for one journal entry, with its repeat count if it repeated. */
export function phraseJournalEntry(entry: PluginJournalEntryView): string {
  const phrase = DECISION_PHRASES[entry.decision]?.(entry.detail) ?? fallbackPhrase(entry.decision, entry.detail);
  return entry.repeats > 1 ? `${phrase} (×${entry.repeats})` : phrase;
}

/** What the health badge says next to a plugin's name. Empty when there is nothing to say. */
export function pluginHealthSummary(health: PluginHealthView | null): string {
  if (!health) return "";
  if (health.degraded) {
    return health.consecutiveFailures === 1
      ? "Degraded · 1 failure"
      : `Degraded · ${health.consecutiveFailures} failures`;
  }
  if (health.running > 0) return health.running === 1 ? "Running 1 job" : `Running ${health.running} jobs`;
  if (health.queued > 0) return health.queued === 1 ? "1 job queued" : `${health.queued} jobs queued`;
  return "";
}
