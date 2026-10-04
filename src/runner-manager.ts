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
/**
 * The launch shape a profile authorises. `default` starts the emulator with
 * the game file as its one argument; `stream` means the folder holds stream
 * descriptions Orivo itself wrote and the host starts the client on the
 * remote machine's game instead. It is the user's own permission on their own
 * profile — the plugin is never asked about it (`runner_commands.rs`).
 */
export type RunnerLaunchMode = "default" | "stream";
export type RunnerImportPhase = "running" | "ready" | "cancelled" | "failed";

/**
 * What asking to pair produced.
 *
 * `alreadyPaired` is an answer rather than an error: it is the state the user
 * wants to be in, and it is not success either — a client that is already
 * paired refuses to start a handshake, so a PIN shown for one could only be
 * rejected by the other machine.
 *
 * Neither arm carries a credential, because pairing does not use one.
 */
export type GameStreamPairing =
  | { state: "alreadyPaired"; host: string }
  | { state: "started"; host: string; pin: string };

/** A streaming client Orivo found on this machine. A handle and a name — never a path. */
export interface DetectedClientView {
  id: string;
  label: string;
}

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
  launchMode: RunnerLaunchMode;
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
const LAUNCH_MODES: ReadonlySet<string> = new Set<RunnerLaunchMode>(["default", "stream"]);
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
  createProfile(
    pluginId: string,
    displayName: string,
    /** A handle from `findStreamClients`, or null to open the native picker. */
    clientId: string | null,
    signal: AbortSignal,
  ): Promise<RunnerProfileView>;
  renameProfile(profileId: string, displayName: string, signal: AbortSignal): Promise<RunnerProfileView>;
  setProfileEnabled(profileId: string, enabled: boolean, signal: AbortSignal): Promise<RunnerProfileView>;
  setProfileLaunchMode(
    profileId: string,
    launchMode: RunnerLaunchMode,
    signal: AbortSignal,
  ): Promise<RunnerProfileView>;
  beginPairing(profileId: string, signal: AbortSignal): Promise<GameStreamPairing>;
  findStreamClients(signal: AbortSignal): Promise<DetectedClientView[]>;
  /**
   * Fill in artwork for the cards an import of this profile created. Answers
   * how many were filled, so a run that found nothing costs no repaint.
   */
  fetchProfileArtwork(profileId: string, signal: AbortSignal): Promise<number>;
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
  /**
   * Whether artwork is being looked for right now. It runs after an import and
   * takes a request and up to four downloads per card, so a library's worth of
   * cards is minutes of silence unless the panel says so.
   */
  isFindingArtwork(profileId: string): boolean;
  createProfile(
    pluginId: string,
    displayName: string,
    clientId?: string,
  ): Promise<RunnerProfileView>;
  renameProfile(profileId: string, displayName: string): Promise<RunnerProfileView>;
  setEnabled(profileId: string, enabled: boolean): Promise<RunnerProfileView>;
  setLaunchMode(profileId: string, launchMode: RunnerLaunchMode): Promise<RunnerProfileView>;
  beginPairing(profileId: string): Promise<GameStreamPairing>;
  /** The streaming clients Orivo can see, so a profile can skip the file picker. */
  streamClients(): DetectedClientView[];
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

/**
 * An answer is only worth showing if it is one of the two shapes the host
 * produces, and a started pairing is only worth showing if its PIN is one the
 * machine can accept — otherwise it is four characters the user would type for
 * nothing.
 */
export function readPairing(value: unknown): GameStreamPairing {
  const raw = (value ?? {}) as Record<string, unknown>;
  const host = typeof raw.host === "string" ? raw.host : "";
  if (raw.state === "alreadyPaired") return { state: "alreadyPaired", host };
  const pin = typeof raw.pin === "string" ? raw.pin : "";
  if (raw.state !== "started" || !/^[0-9]{4}$/.test(pin)) {
    throw new Error("Pairing did not start. Try again.");
  }
  return { state: "started", host, pin };
}

function readDetectedClients(value: unknown): DetectedClientView[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    const raw = (entry ?? {}) as Partial<DetectedClientView>;
    if (typeof raw.id !== "string" || !raw.id) return [];
    return [{ id: raw.id, label: typeof raw.label === "string" && raw.label ? raw.label : raw.id }];
  });
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
      // A mode this build does not implement reads as the default one: a
      // profile is then shown as the ordinary kind it behaves like, rather
      // than as a shape the panel has no control for.
      launchMode:
        typeof raw.launchMode === "string" && LAUNCH_MODES.has(raw.launchMode)
          ? (raw.launchMode as RunnerLaunchMode)
          : "default",
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

    async createProfile(pluginId, displayName, clientId, signal) {
      if (!isTauriRuntime()) {
        throw new Error("Adding an emulator is only available in the Orivo desktop app.");
      }
      assertActive(signal);
      return readOneProfile(
        await invoke("create_runner_profile", { pluginId, displayName, clientId }),
      );
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

    async setProfileLaunchMode(profileId, launchMode, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return readOneProfile(
        await invoke("set_runner_profile_launch_mode", { profileId, launchMode }),
      );
    },

    async beginPairing(profileId, signal) {
      if (!isTauriRuntime()) throw new Error("This is only available in the Orivo desktop app.");
      assertActive(signal);
      return readPairing(await invoke("begin_gamestream_pairing", { profileId }));
    },

    async fetchProfileArtwork(profileId, signal) {
      if (!isTauriRuntime()) return 0;
      assertActive(signal);
      const result = await invoke<unknown>("fetch_runner_profile_artwork", { profileId });
      const filled = (result as { filled?: unknown } | null)?.filled;
      return typeof filled === "number" && Number.isFinite(filled) ? filled : 0;
    },

    async findStreamClients(signal) {
      try {
        if (!isTauriRuntime()) return [];
        assertActive(signal);
        const found = await invoke<unknown>("find_stream_clients");
        assertActive(signal);
        return readDetectedClients(found);
      } catch {
        // Not finding one is not a failure: the picker is still there.
        return [];
      }
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
  // What is installed on this machine, read once per visit beside the runners.
  // Finding nothing is a normal answer: the native picker is still there.
  let streamClients: DetectedClientView[] = [];
  const findingArtwork = new Set<string>();
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
          // The cards an import just created arrive with nothing to show. Art
          // is looked for once the games are in, never before: it is slow, it
          // is per card, and an import that has not finished does not yet know
          // which cards exist. It never fails the import either — a title no
          // source has art for is a normal answer.
          if (view.phase === "ready") {
            findingArtwork.add(profileId);
            notify();
            const filled = await client
              .fetchProfileArtwork(profileId, lifetime.signal)
              .catch(() => 0);
            findingArtwork.delete(profileId);
            // A run that found nothing changed nothing, so it costs no repaint
            // beyond the one that takes the message back down.
            if (filled > 0 && !disposed) await refresh();
            else notify();
          }
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
      // Asked alongside, and never allowed to fail the load: a machine with no
      // streaming client installed still has runners worth showing.
      const found = await client.findStreamClients(signal).catch(() => []);
      if (disposed || signal.aborted) return;
      runners = next;
      streamClients = found;
      notify();
    },

    runners() {
      return runners;
    },

    streamClients() {
      return streamClients;
    },

    isFindingArtwork(profileId) {
      return findingArtwork.has(profileId);
    },

    importFor(profileId) {
      return jobs.get(profileId) ?? null;
    },

    async createProfile(pluginId, displayName, clientId) {
      const created = await client.createProfile(
        pluginId,
        displayName,
        clientId ?? null,
        lifetime.signal,
      );
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

    async beginPairing(profileId) {
      // Nothing to reload: pairing changes no catalog state. The host state it
      // does change lives on the other machine.
      return client.beginPairing(profileId, lifetime.signal);
    },

    async setLaunchMode(profileId, launchMode) {
      // The host drops the import cursor when the mode changes, so a poll
      // still describing the old reading of this folder has nothing left to
      // report: it is dropped here rather than allowed to land on the card.
      invalidatePoll(profileId);
      jobs.delete(profileId);
      const updated = await client.setProfileLaunchMode(profileId, launchMode, lifetime.signal);
      if (!disposed) await refresh();
      return updated;
    },

    async deleteProfile(profileId) {
      invalidatePoll(profileId);
      jobs.delete(profileId);
      findingArtwork.delete(profileId);
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
