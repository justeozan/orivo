/* ---------------------------------------------------------------------------
   Runner manager — "Add an emulator" and the runner profiles inside
   Settings › Plugins.

   Same contract as the rest of Settings: talk to an interface, never to
   Tauri directly, so a host binary without these commands and a browser with
   no Tauri at all both collapse to an empty list instead of an error. The
   WebView never sees a path — `create_runner_profile` and
   `grant_runner_profile_directory` open their own native picker in Rust and
   hand back a profile that only carries ids and display text
   (`runner_commands.rs`).

   An import is the one command here with no event bus behind it: the host
   runs it on a worker thread and answers a poll. This module is the poller.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";
import type { PluginState } from "./plugin-manager";

export type RunnerProfileStatus = "unvalidated" | "valid" | "rejected";
export type RunnerImportPhase = "running" | "ready" | "cancelled" | "failed";

export interface RunnerDirectoryView {
  id: string;
  label: string;
  granted: boolean;
}

export interface RunnerProfileView {
  id: string;
  pluginId: string;
  displayName: string;
  status: RunnerProfileStatus;
  statusMessage: string | null;
  enabled: boolean;
  applicationLabel: string;
  directories: RunnerDirectoryView[];
  gameCount: number;
  importComplete: boolean;
  importResumable: boolean;
  lastImportedAt: number | null;
}

export interface InstalledRunnerView {
  id: string;
  name: string;
  version: string;
  state: PluginState;
  message: string;
  profiles: RunnerProfileView[];
}

export interface RunnerImportJobView {
  jobId: string;
  profileId: string;
  phase: RunnerImportPhase;
  imported: number;
  refreshed: number;
  skipped: number;
  pages: number;
  resumed: boolean;
  complete: boolean;
  message: string;
}

const RUNNER_STATES: ReadonlySet<string> = new Set<PluginState>(["ready", "incompatible", "invalid"]);
const PROFILE_STATUSES: ReadonlySet<string> = new Set<RunnerProfileStatus>([
  "unvalidated",
  "valid",
  "rejected",
]);
const IMPORT_PHASES: ReadonlySet<string> = new Set<RunnerImportPhase>([
  "running",
  "ready",
  "cancelled",
  "failed",
]);

/**
 * The host side of the panel. Settings only ever talks to this interface, so a
 * test can hand it a recorder and never touch Tauri.
 */
export interface RunnerManagerClient {
  getInstalledRunners(signal: AbortSignal): Promise<InstalledRunnerView[]>;
  createProfile(pluginId: string, displayName: string, signal: AbortSignal): Promise<RunnerProfileView>;
  renameProfile(profileId: string, displayName: string, signal: AbortSignal): Promise<RunnerProfileView>;
  setProfileEnabled(profileId: string, enabled: boolean, signal: AbortSignal): Promise<RunnerProfileView>;
  deleteProfile(profileId: string, signal: AbortSignal): Promise<boolean>;
  /** Opens a native folder picker. `slot` is the grant id; omit it for the default. */
  grantDirectory(
    profileId: string,
    slot: string | null,
    signal: AbortSignal,
  ): Promise<RunnerProfileView>;
  revokeDirectory(profileId: string, directoryId: string, signal: AbortSignal): Promise<RunnerProfileView>;
  startImport(profileId: string, signal: AbortSignal): Promise<RunnerImportJobView>;
  getImportStatus(jobId: string, signal: AbortSignal): Promise<RunnerImportJobView>;
  cancelImport(jobId: string, signal: AbortSignal): Promise<RunnerImportJobView>;
}

/** One live panel, owned by one activation of Settings. */
export interface RunnerManagerController {
  load(signal: AbortSignal): Promise<void>;
  runners(): InstalledRunnerView[];
  /** The most recent import job for a profile, or null if none ran this session. */
  importFor(profileId: string): RunnerImportJobView | null;
  createProfile(pluginId: string, displayName: string): Promise<RunnerProfileView>;
  renameProfile(profileId: string, displayName: string): Promise<RunnerProfileView>;
  setEnabled(profileId: string, enabled: boolean): Promise<RunnerProfileView>;
  deleteProfile(profileId: string): Promise<boolean>;
  grantDirectory(profileId: string, slot?: string): Promise<RunnerProfileView>;
  revokeDirectory(profileId: string, directoryId: string): Promise<RunnerProfileView>;
  startImport(profileId: string): Promise<void>;
  cancelImport(profileId: string): Promise<void>;
  onChange(callback: () => void): () => void;
  dispose(): void;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("La requête a été annulée.", "AbortError");
}

export function runnerErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return "This did not work. Try again.";
}

/* ---------------------------------------------------------------------------
   Reading the host's answer
   --------------------------------------------------------------------------- */

function readDirectory(value: unknown): RunnerDirectoryView[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<RunnerDirectoryView>;
  if (typeof raw.id !== "string" || !raw.id) return [];
  return [
    {
      id: raw.id,
      label: typeof raw.label === "string" && raw.label ? raw.label : raw.id,
      granted: raw.granted === true,
    },
  ];
}

function readProfile(value: unknown): RunnerProfileView[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<RunnerProfileView>;
  if (typeof raw.id !== "string" || !raw.id) return [];
  if (typeof raw.pluginId !== "string" || !raw.pluginId) return [];
  return [
    {
      id: raw.id,
      pluginId: raw.pluginId,
      displayName: typeof raw.displayName === "string" && raw.displayName ? raw.displayName : raw.id,
      status:
        typeof raw.status === "string" && PROFILE_STATUSES.has(raw.status)
          ? (raw.status as RunnerProfileStatus)
          : "unvalidated",
      statusMessage:
        typeof raw.statusMessage === "string" && raw.statusMessage ? raw.statusMessage : null,
      enabled: raw.enabled !== false,
      applicationLabel: typeof raw.applicationLabel === "string" ? raw.applicationLabel : "",
      directories: Array.isArray(raw.directories) ? raw.directories.flatMap(readDirectory) : [],
      gameCount:
        typeof raw.gameCount === "number" && Number.isFinite(raw.gameCount) && raw.gameCount >= 0
          ? Math.floor(raw.gameCount)
          : 0,
      importComplete: raw.importComplete === true,
      importResumable: raw.importResumable === true,
      lastImportedAt:
        typeof raw.lastImportedAt === "number" && Number.isFinite(raw.lastImportedAt)
          ? raw.lastImportedAt
          : null,
    },
  ];
}

function readRunner(value: unknown): InstalledRunnerView[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<InstalledRunnerView>;
  if (typeof raw.id !== "string" || !raw.id) return [];
  return [
    {
      id: raw.id,
      name: typeof raw.name === "string" && raw.name ? raw.name : raw.id,
      version: typeof raw.version === "string" ? raw.version : "",
      state: typeof raw.state === "string" && RUNNER_STATES.has(raw.state) ? (raw.state as PluginState) : "invalid",
      message: typeof raw.message === "string" ? raw.message : "",
      profiles: Array.isArray(raw.profiles) ? raw.profiles.flatMap(readProfile) : [],
    },
  ];
}

export function readInstalledRunners(value: unknown): InstalledRunnerView[] {
  return Array.isArray(value) ? value.flatMap(readRunner) : [];
}

function readOneProfile(value: unknown): RunnerProfileView {
  const [profile] = readProfile(value);
  if (!profile) throw new Error("Orivo n'a pas reconnu la réponse pour ce profil.");
  return profile;
}

function readImportJob(value: unknown): RunnerImportJobView {
  if (!value || typeof value !== "object") {
    throw new Error("Orivo n'a pas reconnu la réponse pour cet import.");
  }
  const raw = value as Partial<RunnerImportJobView>;
  if (typeof raw.jobId !== "string" || !raw.jobId || typeof raw.profileId !== "string") {
    throw new Error("Orivo n'a pas reconnu la réponse pour cet import.");
  }
  const count = (n: unknown): number => (typeof n === "number" && Number.isFinite(n) && n >= 0 ? n : 0);
  return {
    jobId: raw.jobId,
    profileId: raw.profileId,
    phase: typeof raw.phase === "string" && IMPORT_PHASES.has(raw.phase) ? (raw.phase as RunnerImportPhase) : "failed",
    imported: count(raw.imported),
    refreshed: count(raw.refreshed),
    skipped: count(raw.skipped),
    pages: count(raw.pages),
    resumed: raw.resumed === true,
    complete: raw.complete === true,
    message: typeof raw.message === "string" ? raw.message : "",
  };
}

/* ---------------------------------------------------------------------------
   The real host
   --------------------------------------------------------------------------- */

export function createDefaultRunnerManagerClient(): RunnerManagerClient {
  return {
    async getInstalledRunners(signal) {
      try {
        if (!isTauriRuntime()) return [];
        assertActive(signal);
        const view = await invoke<unknown>("get_installed_runners");
        assertActive(signal);
        return readInstalledRunners(view);
      } catch {
        return [];
      }
    },

    async createProfile(pluginId, displayName, signal) {
      if (!isTauriRuntime()) {
        throw new Error("Adding an emulator is only available in the Orivo desktop app.");
      }
      assertActive(signal);
      return readOneProfile(await invoke("create_runner_profile", { pluginId, displayName }));
    },

    async renameProfile(profileId, displayName, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return readOneProfile(await invoke("rename_runner_profile", { profileId, displayName }));
    },

    async setProfileEnabled(profileId, enabled, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return readOneProfile(await invoke("set_runner_profile_enabled", { profileId, enabled }));
    },

    async deleteProfile(profileId, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return (await invoke<boolean>("delete_runner_profile", { profileId })) === true;
    },

    async grantDirectory(profileId, slot, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return readOneProfile(await invoke("grant_runner_profile_directory", { profileId, slot }));
    },

    async revokeDirectory(profileId, directoryId, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return readOneProfile(
        await invoke("revoke_runner_profile_directory", { profileId, directoryId }),
      );
    },

    async startImport(profileId, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return readImportJob(await invoke("start_runner_import", { profileId }));
    },

    async getImportStatus(jobId, signal) {
      assertActive(signal);
      return readImportJob(await invoke("get_runner_import_status", { jobId }));
    },

    async cancelImport(jobId, signal) {
      assertActive(signal);
      return readImportJob(await invoke("cancel_runner_import", { jobId }));
    },
  };
}

/* ---------------------------------------------------------------------------
   Copy
   --------------------------------------------------------------------------- */

export function runnerProfileStatusLabel(profile: RunnerProfileView): string {
  switch (profile.status) {
    case "valid":
      return "Ready";
    case "rejected":
      return profile.statusMessage ? `Rejected: ${profile.statusMessage}` : "Rejected by the plugin";
    case "unvalidated":
      return "Checking with the plugin…";
  }
}

export function runnerImportSummary(job: RunnerImportJobView | null): string {
  if (!job) return "";
  switch (job.phase) {
    case "running":
      return job.resumed
        ? `Continuing… ${job.imported + job.refreshed} game(s) so far`
        : `Importing… ${job.imported + job.refreshed} game(s) so far`;
    case "ready":
      return job.message || `Imported ${job.imported} game(s), refreshed ${job.refreshed}.`;
    case "cancelled":
      return job.message || "Import stopped. Orivo kept its place.";
    case "failed":
      return job.message || "This import did not finish.";
  }
}

export function isRunnerImportRunning(job: RunnerImportJobView | null): boolean {
  return job !== null && job.phase === "running";
}

/* ---------------------------------------------------------------------------
   The controller
   --------------------------------------------------------------------------- */

/** Between polls of an import already in flight. Fast enough to feel live, slow enough to not be traffic. */
const IMPORT_POLL_INTERVAL_MS = 500;

function failedImportJob(profileId: string, error: unknown): RunnerImportJobView {
  return {
    jobId: "",
    profileId,
    phase: "failed",
    imported: 0,
    refreshed: 0,
    skipped: 0,
    pages: 0,
    resumed: false,
    complete: false,
    message: runnerErrorMessage(error),
  };
}

export function createRunnerManagerController(client: RunnerManagerClient): RunnerManagerController {
  const listeners = new Set<() => void>();
  const jobs = new Map<string, RunnerImportJobView>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  /**
   * `clearTimeout` only stops a poll that has not fired yet. One that has
   * already fired and is awaiting `getImportStatus` keeps running regardless,
   * and its continuation — same as `cancelImport`'s own — decides on its own
   * whether the job is still "running" and, if so, schedules another poll.
   * Without a token neither side knows about the other's decision, and both
   * can end up scheduling one: two live loops for one profile, the older one
   * no longer reachable through `timers` once the newer one overwrites it.
   * Each `poll()` call captures the generation current when it was scheduled;
   * a tick only acts if that generation is still current when it fires.
   */
  const pollGeneration = new Map<string, number>();
  // Profile mutations and import polling outlive the activation signal `load`
  // is handed, so they run on the controller's own lifetime.
  const lifetime = new AbortController();
  let runners: InstalledRunnerView[] = [];
  let disposed = false;

  const notify = (): void => {
    for (const listener of [...listeners]) listener();
  };

  /**
   * A profile mutation changes nested state (folders, status, game counts)
   * that only the host can recompute, so every mutation ends by re-reading the
   * whole list rather than patching one profile in place.
   */
  const refresh = async (): Promise<void> => {
    if (disposed) return;
    let next: InstalledRunnerView[];
    try {
      next = await client.getInstalledRunners(lifetime.signal);
    } catch {
      next = [];
    }
    if (disposed) return;
    runners = next;
    notify();
  };

  const clearTimer = (profileId: string): void => {
    const timer = timers.get(profileId);
    if (timer !== undefined) {
      clearTimeout(timer);
      timers.delete(profileId);
    }
  };

  /** Cancels a pending timer and marks any in-flight poll for this profile stale, so neither can act again. */
  const invalidatePoll = (profileId: string): void => {
    clearTimer(profileId);
    pollGeneration.set(profileId, (pollGeneration.get(profileId) ?? 0) + 1);
  };

  const poll = (profileId: string, jobId: string): void => {
    if (disposed) return;
    const generation = pollGeneration.get(profileId) ?? 0;
    const isCurrent = (): boolean => (pollGeneration.get(profileId) ?? 0) === generation;
    const timer = setTimeout(() => {
      void (async () => {
        if (disposed || !isCurrent()) return;
        let view: RunnerImportJobView;
        try {
          view = await client.getImportStatus(jobId, lifetime.signal);
        } catch (error) {
          if (disposed || !isCurrent()) return;
          timers.delete(profileId);
          jobs.set(profileId, failedImportJob(profileId, error));
          notify();
          return;
        }
        if (disposed || !isCurrent()) return;
        jobs.set(profileId, view);
        notify();
        if (view.phase === "running") {
          poll(profileId, jobId);
        } else {
          timers.delete(profileId);
          await refresh();
        }
      })();
    }, IMPORT_POLL_INTERVAL_MS);
    timers.set(profileId, timer);
  };

  return {
    async load(signal) {
      if (disposed) return;
      let next: InstalledRunnerView[];
      try {
        next = await client.getInstalledRunners(signal);
      } catch {
        next = [];
      }
      if (disposed || signal.aborted) return;
      runners = next;
      notify();
    },

    runners() {
      return runners;
    },

    importFor(profileId) {
      return jobs.get(profileId) ?? null;
    },

    async createProfile(pluginId, displayName) {
      const created = await client.createProfile(pluginId, displayName, lifetime.signal);
      if (!disposed) await refresh();
      return created;
    },

    async renameProfile(profileId, displayName) {
      const renamed = await client.renameProfile(profileId, displayName, lifetime.signal);
      if (!disposed) await refresh();
      return renamed;
    },

    async setEnabled(profileId, enabled) {
      const updated = await client.setProfileEnabled(profileId, enabled, lifetime.signal);
      if (!disposed) await refresh();
      return updated;
    },

    async deleteProfile(profileId) {
      invalidatePoll(profileId);
      jobs.delete(profileId);
      const removed = await client.deleteProfile(profileId, lifetime.signal);
      if (!disposed) await refresh();
      return removed;
    },

    async grantDirectory(profileId, slot) {
      const updated = await client.grantDirectory(profileId, slot ?? null, lifetime.signal);
      if (!disposed) await refresh();
      return updated;
    },

    async revokeDirectory(profileId, directoryId) {
      const updated = await client.revokeDirectory(profileId, directoryId, lifetime.signal);
      if (!disposed) await refresh();
      return updated;
    },

    async startImport(profileId) {
      if (disposed) return;
      invalidatePoll(profileId);
      let view: RunnerImportJobView;
      try {
        view = await client.startImport(profileId, lifetime.signal);
      } catch (error) {
        if (disposed) return;
        jobs.set(profileId, failedImportJob(profileId, error));
        notify();
        return;
      }
      if (disposed) return;
      jobs.set(profileId, view);
      notify();
      if (view.phase === "running") poll(profileId, view.jobId);
      else await refresh();
    },

    async cancelImport(profileId) {
      if (disposed) return;
      const job = jobs.get(profileId);
      if (!job || !job.jobId) return;
      invalidatePoll(profileId);
      try {
        const view = await client.cancelImport(job.jobId, lifetime.signal);
        if (disposed) return;
        jobs.set(profileId, view);
        notify();
        // Cancelling only sets a flag the host checks at its next chance to
        // look — the job can still answer "running" right after this call,
        // until the worker thread actually stops. Keep polling until it
        // genuinely does, otherwise the panel is left showing a job that will
        // never move again on its own.
        if (view.phase === "running") {
          poll(profileId, job.jobId);
        } else {
          await refresh();
        }
      } catch (error) {
        if (disposed) return;
        jobs.set(profileId, { ...job, phase: "failed", message: runnerErrorMessage(error) });
        notify();
      }
    },

    onChange(callback) {
      listeners.add(callback);
      return () => {
        listeners.delete(callback);
      };
    },

    dispose() {
      disposed = true;
      lifetime.abort();
      for (const timer of timers.values()) clearTimeout(timer);
      timers.clear();
      pollGeneration.clear();
      jobs.clear();
      listeners.clear();
      runners = [];
    },
  };
}
