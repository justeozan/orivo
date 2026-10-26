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
import { t, type MessageKey } from "./i18n";

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
  /**
   * `null` means the read failed — no Tauri, an older host binary, a dropped
   * IPC call — and is distinct from an empty array, which means the read
   * succeeded and the plugin simply has nothing recent to show. A caller that
   * folded both into "no recent activity" would show a broken plugin the same
   * blank screen as a healthy, quiet one.
   */
  getJournal(pluginId: string, signal: AbortSignal): Promise<PluginJournalEntryView[] | null>;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("The request was cancelled.", "AbortError");
}

export function pluginHealthErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return t("runners.health.readError");
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
        throw new Error(t("runners.health.resumeDesktopOnly"));
      }
      assertActive(signal);
      const view = await invoke<unknown>("resume_plugin", { pluginId });
      const [health] = readHealthView(view);
      if (!health) throw new Error(t("runners.health.resumeError"));
      return health;
    },

    async getJournal(pluginId, signal) {
      try {
        // No Tauri here is not a failed read — it is the browser fallback,
        // which never had a log to read in the first place.
        if (!isTauriRuntime()) return [];
        assertActive(signal);
        const entries = await invoke<unknown>("get_plugin_journal", { pluginId });
        assertActive(signal);
        return readJournal(entries);
      } catch {
        return null;
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
/** A phrase with the host's detail after it, or ending on its own. */
function withDetail(phrase: MessageKey, detail: string): string {
  return detail
    ? t("runners.health.withDetail", { phrase: t(phrase), detail })
    : t("runners.health.noDetail", { phrase: t(phrase) });
}

const DECISION_PHRASES: Readonly<Record<string, (detail: string) => string>> = {
  "job-panicked": () => t("runners.health.jobPanicked"),
  "job-failed": (detail) => withDetail("runners.health.jobFailed", detail),
  cancelled: () => t("runners.health.cancelled"),
  degraded: () => t("runners.health.degraded"),
  resumed: () => t("runners.health.resumed"),
  "submit-refused": (detail) => withDetail("runners.health.submitRefused", detail),
  "host-call-budget": () => t("runners.health.hostCallBudget"),
  "capability-refused": (detail) => withDetail("runners.health.capabilityRefused", detail),
  "scope-refused": () => t("runners.health.scopeRefused"),
  "files-truncated": (detail) => withDetail("runners.health.filesTruncated", detail),
  "identity-mismatch": () => t("runners.health.identityMismatch"),
  // Neutral on purpose: a plugin that simply has no use for a capability logs
  // this on every call it makes, which is expected behaviour, not wrongdoing —
  // "asked for" would accuse a plugin that is doing exactly what it should.
  "capability-unlinked": (detail) => detail || t("runners.health.capabilityUnlinked"),
  trap: (detail) => withDetail("runners.health.trap", detail),
  "get-identity": (detail) => withDetail("runners.health.getIdentity", detail),
  "health-check": (detail) => withDetail("runners.health.healthCheck", detail),
  "validate-profile": (detail) => withDetail("runners.health.validateProfile", detail),
  "discover-page": (detail) => withDetail("runners.health.discoverPage", detail),
  "prepare-launch": (detail) => withDetail("runners.health.prepareLaunch", detail),
};

function fallbackPhrase(decision: string, detail: string): string {
  const words = decision.replace(/-/g, " ");
  const sentence = words.charAt(0).toUpperCase() + words.slice(1);
  return detail
    ? t("runners.health.withDetail", { phrase: sentence, detail })
    : t("runners.health.noDetail", { phrase: sentence });
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
      ? t("runners.health.degradedOne")
      : t("runners.health.degradedMany", { count: health.consecutiveFailures });
  }
  if (health.running > 0) {
    return health.running === 1
      ? t("runners.health.runningOne")
      : t("runners.health.runningMany", { count: health.running });
  }
  if (health.queued > 0) {
    return health.queued === 1
      ? t("runners.health.queuedOne")
      : t("runners.health.queuedMany", { count: health.queued });
  }
  return "";
}
