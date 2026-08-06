/* ---------------------------------------------------------------------------
   Plugin manager — the Settings › Plugins catalogue.

   Same contract as the Quiky installer: Settings talks to an interface, never
   to Tauri. A host binary without these commands, a browser with no Tauri at
   all, a registry that answers garbage — all three collapse to an empty
   catalogue, and the Plugins panel then renders exactly what it rendered
   before this feature existed.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** How the host graded a package it found on disk. */
export type PluginState = "ready" | "incompatible" | "invalid";

export interface InstalledPluginView {
  id: string;
  name: string;
  version: string;
  extensions: string[];
  state: PluginState;
  message: string;
  /** True when the package carried a trusted Ed25519 signature. */
  trusted: boolean;
}

export interface AvailablePluginView {
  id: string;
  name: string;
  version: string;
  summary: string;
  sizeBytes: number;
  /** True when this exact id is already installed. */
  installed: boolean;
}

export interface PluginCatalogView {
  installed: InstalledPluginView[];
  available: AvailablePluginView[];
}

export type PluginInstallPhase =
  | "downloading"
  | "verifying"
  | "installing"
  | "installed"
  | "failed";

export interface PluginInstallProgress {
  pluginId: string;
  phase: PluginInstallPhase;
  /** 0..100, already smoothed by the host. */
  percent: number;
  message: string;
}

/**
 * The host side of the panel. Settings only ever talks to this interface, so a
 * test can hand it a recorder and never touch Tauri.
 */
export interface PluginManagerClient {
  getCatalog(signal: AbortSignal): Promise<PluginCatalogView>;
  installFromRegistry(pluginId: string, signal: AbortSignal): Promise<void>;
  /** Opens a native picker. Resolves to the installed id, or null if cancelled. */
  installFromFile(signal: AbortSignal): Promise<string | null>;
  uninstall(pluginId: string, signal: AbortSignal): Promise<void>;
  subscribe(onProgress: (progress: PluginInstallProgress) => void): () => void;
}

/** One live plugin panel, owned by one activation of Settings. */
export interface PluginManagerController {
  load(signal: AbortSignal): Promise<void>;
  catalog(): PluginCatalogView;
  progressFor(pluginId: string): PluginInstallProgress | null;
  installFromRegistry(pluginId: string): Promise<void>;
  /** Rejects with the host's message so the caller can surface it in a toast. */
  installFromFile(): Promise<string | null>;
  /** Rejects with the host's message; the catalogue reloads on success. */
  uninstall(pluginId: string): Promise<void>;
  onChange(callback: () => void): () => void;
  dispose(): void;
}

/** The single event the host pushes while an install runs. */
export const PLUGIN_INSTALL_EVENT = "plugin-install-status";

const PLUGIN_STATES: ReadonlySet<string> = new Set<PluginState>([
  "ready",
  "incompatible",
  "invalid",
]);

const INSTALL_PHASES: ReadonlySet<string> = new Set<PluginInstallPhase>([
  "downloading",
  "verifying",
  "installing",
  "installed",
  "failed",
]);

const BUSY_PHASES: ReadonlySet<PluginInstallPhase> = new Set<PluginInstallPhase>([
  "downloading",
  "verifying",
  "installing",
]);

/** What Settings sees whenever the host cannot answer. */
export function emptyPluginCatalog(): PluginCatalogView {
  return { installed: [], available: [] };
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("La requête plugin a été annulée.", "AbortError");
}

export function pluginErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return "L'opération n'a pas abouti.";
}

/* ---------------------------------------------------------------------------
   Reading the host's answer

   The catalogue crosses a process boundary and half of it is written by third
   parties. A row without an id can neither be installed nor removed, so it is
   dropped rather than rendered as a button that cannot name its target.
   --------------------------------------------------------------------------- */

function readInstalled(value: unknown): InstalledPluginView[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<InstalledPluginView>;
  if (typeof raw.id !== "string" || !raw.id) return [];
  return [
    {
      id: raw.id,
      name: typeof raw.name === "string" && raw.name ? raw.name : raw.id,
      version: typeof raw.version === "string" ? raw.version : "",
      extensions: Array.isArray(raw.extensions)
        ? raw.extensions.filter((entry): entry is string => typeof entry === "string")
        : [],
      // An unknown grade is not a working plugin: "invalid" is the honest
      // reading, and it is the one state that never promises a launch.
      state:
        typeof raw.state === "string" && PLUGIN_STATES.has(raw.state)
          ? (raw.state as PluginState)
          : "invalid",
      message: typeof raw.message === "string" ? raw.message : "",
      // Trust is opt-in: anything that is not an explicit `true` is unsigned.
      trusted: raw.trusted === true,
    },
  ];
}

function readAvailable(value: unknown): AvailablePluginView[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<AvailablePluginView>;
  if (typeof raw.id !== "string" || !raw.id) return [];
  return [
    {
      id: raw.id,
      name: typeof raw.name === "string" && raw.name ? raw.name : raw.id,
      version: typeof raw.version === "string" ? raw.version : "",
      summary: typeof raw.summary === "string" ? raw.summary : "",
      sizeBytes:
        typeof raw.sizeBytes === "number" && Number.isFinite(raw.sizeBytes) && raw.sizeBytes > 0
          ? raw.sizeBytes
          : 0,
      installed: raw.installed === true,
    },
  ];
}

export function readPluginCatalog(value: unknown): PluginCatalogView {
  if (!value || typeof value !== "object") return emptyPluginCatalog();
  const raw = value as Partial<PluginCatalogView>;
  return {
    installed: Array.isArray(raw.installed) ? raw.installed.flatMap(readInstalled) : [],
    available: Array.isArray(raw.available) ? raw.available.flatMap(readAvailable) : [],
  };
}

function readProgress(value: unknown): PluginInstallProgress | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Partial<PluginInstallProgress>;
  if (typeof raw.pluginId !== "string" || !raw.pluginId) return null;
  if (typeof raw.phase !== "string" || !INSTALL_PHASES.has(raw.phase)) return null;
  return {
    pluginId: raw.pluginId,
    phase: raw.phase as PluginInstallPhase,
    percent: typeof raw.percent === "number" && Number.isFinite(raw.percent) ? raw.percent : 0,
    message: typeof raw.message === "string" ? raw.message : "",
  };
}

/* ---------------------------------------------------------------------------
   The real host
   --------------------------------------------------------------------------- */

export function createDefaultPluginManagerClient(): PluginManagerClient {
  return {
    async getCatalog(signal) {
      // Every failure mode collapses to the same answer. A host binary built
      // before this command existed and a browser tab both throw here, and
      // neither is a reason for Settings to stop painting.
      try {
        if (!isTauriRuntime()) return emptyPluginCatalog();
        assertActive(signal);
        const view = await invoke<PluginCatalogView>("get_plugin_catalog");
        assertActive(signal);
        return readPluginCatalog(view);
      } catch {
        return emptyPluginCatalog();
      }
    },

    async installFromRegistry(pluginId, signal) {
      // Deliberately not caught, here and below: a refused install is news the
      // user needs, and the row has a place to print it.
      if (!isTauriRuntime()) {
        throw new Error("L'installation de plugins est réservée à l'application Orivo.");
      }
      assertActive(signal);
      await invoke("install_plugin_from_registry", { pluginId });
    },

    async installFromFile(signal) {
      // No native picker outside the desktop shell: a null reads as "cancelled",
      // which is exactly what happened from the user's point of view.
      if (!isTauriRuntime()) return null;
      assertActive(signal);
      const installed = await invoke<string | null>("install_plugin_from_file");
      return typeof installed === "string" && installed ? installed : null;
    },

    async uninstall(pluginId, signal) {
      if (!isTauriRuntime()) return;
      assertActive(signal);
      await invoke("uninstall_plugin", { pluginId });
    },

    subscribe(onProgress) {
      if (!isTauriRuntime()) return () => {};
      let unlisten: UnlistenFn | null = null;
      let stopped = false;
      void listen<PluginInstallProgress>(PLUGIN_INSTALL_EVENT, (event) => {
        if (!stopped) onProgress(event.payload);
      })
        .then((teardown) => {
          // `listen` registers a frame later than the caller subscribes, so a
          // subscription torn down in between still has to release the channel.
          if (stopped) void teardown();
          else unlisten = teardown;
        })
        .catch(() => {
          // No event bus, no progress. Buttons stay on their last known phase.
        });
      return () => {
        stopped = true;
        if (!unlisten) return;
        void unlisten();
        unlisten = null;
      };
    },
  };
}

/* ---------------------------------------------------------------------------
   Copy
   --------------------------------------------------------------------------- */

/** 0..100, integral: the host smooths the number, this only keeps it in range. */
export function pluginPercent(progress: PluginInstallProgress | null): number {
  if (!progress || !Number.isFinite(progress.percent)) return 0;
  return Math.max(0, Math.min(100, Math.round(progress.percent)));
}

/** Phases where the Install button has nothing left to offer. */
export function isPluginInstallBusy(progress: PluginInstallProgress | null): boolean {
  return progress !== null && BUSY_PHASES.has(progress.phase);
}

/**
 * The one line an installed row prints. Signature comes second because it
 * qualifies the state rather than replacing it: an unsigned plugin that runs is
 * still installed, and the user is told both facts in that order.
 */
export function formatPluginStatus(plugin: InstalledPluginView): string {
  const state =
    plugin.state === "ready"
      ? "Installed"
      : plugin.state === "incompatible"
        ? "Incompatible"
        : "Invalid";
  return plugin.trusted ? state : `${state} · unsigned`;
}

export function formatInstallLabel(
  progress: PluginInstallProgress | null,
  available: AvailablePluginView,
): string {
  if (!progress) return available.installed ? "Installed" : "Install";
  switch (progress.phase) {
    case "downloading":
      return `Downloading ${pluginPercent(progress)}%`;
    case "verifying":
      return "Verifying…";
    case "installing":
      return "Installing…";
    case "installed":
      return "Installed";
    case "failed":
      return "Retry";
  }
}

/** Download weights, in the units a French desktop uses. */
export function formatPluginSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "";
  if (bytes < 1_000_000) return `${Math.max(1, Math.round(bytes / 1_000))} ko`;
  if (bytes < 1_000_000_000) return `${(bytes / 1_000_000).toFixed(1).replace(".", ",")} Mo`;
  return `${(bytes / 1_000_000_000).toFixed(1).replace(".", ",")} Go`;
}

/* ---------------------------------------------------------------------------
   The controller
   --------------------------------------------------------------------------- */

export function createPluginManagerController(
  client: PluginManagerClient,
): PluginManagerController {
  const listeners = new Set<() => void>();
  const progress = new Map<string, PluginInstallProgress>();
  // Installs outlive the request that started them, so they are bound to the
  // controller rather than to the activation signal `load` was handed.
  const lifetime = new AbortController();
  let catalog = emptyPluginCatalog();
  let unsubscribe: (() => void) | null = null;
  let disposed = false;

  const notify = (): void => {
    for (const listener of [...listeners]) listener();
  };

  const record = (next: PluginInstallProgress): void => {
    progress.set(next.pluginId, next);
    notify();
  };

  const onProgress = (raw: PluginInstallProgress): void => {
    if (disposed) return;
    const next = readProgress(raw);
    if (next) record(next);
  };

  const failure = (pluginId: string, error: unknown): PluginInstallProgress => ({
    pluginId,
    phase: "failed",
    percent: progress.get(pluginId)?.percent ?? 0,
    message: pluginErrorMessage(error),
  });

  /**
   * A finished install or removal changes the catalogue, not just the bar, so
   * every mutation ends by asking the host what it now holds. The refresh runs
   * on the controller's lifetime, not on the caller's activation: a user who
   * leaves Settings mid-install still comes back to an accurate list.
   */
  const refresh = async (): Promise<void> => {
    if (disposed) return;
    let next: PluginCatalogView;
    try {
      next = await client.getCatalog(lifetime.signal);
    } catch {
      // The default client never throws; an injected one might, and a broken
      // client reads exactly like a host with no plugin registry.
      next = emptyPluginCatalog();
    }
    if (disposed) return;
    catalog = next;
    notify();
  };

  return {
    async load(signal) {
      if (disposed) return;
      let next: PluginCatalogView;
      try {
        next = await client.getCatalog(signal);
      } catch {
        next = emptyPluginCatalog();
      }
      if (disposed || signal.aborted) return;
      catalog = next;
      // One channel per controller, opened on the first load and held until
      // dispose: a second activation must not double every progress tick.
      if (!unsubscribe) unsubscribe = client.subscribe(onProgress);
      notify();
    },

    catalog() {
      return catalog;
    },

    progressFor(pluginId) {
      return progress.get(pluginId) ?? null;
    },

    async installFromRegistry(pluginId) {
      if (disposed) return;
      // Optimistic: the user clicked now, the host answers in its own time.
      // Downloading is where the host would put it anyway, so nothing is
      // invented — only the first tick is brought forward.
      record({ pluginId, phase: "downloading", percent: 0, message: "" });
      try {
        await client.installFromRegistry(pluginId, lifetime.signal);
      } catch (error) {
        if (disposed) return;
        record(failure(pluginId, error));
        return;
      }
      if (disposed) return;
      // The host emits `installed` too, and writing the same phase twice is
      // cheaper than a button stuck on "Downloading 0%" when the event bus
      // is missing.
      record({ pluginId, phase: "installed", percent: 100, message: "" });
      await refresh();
    },

    async installFromFile() {
      if (disposed) return null;
      const installed = await client.installFromFile(lifetime.signal);
      // A cancelled picker changed nothing on disk; reloading would only make
      // the panel flicker.
      if (installed) await refresh();
      return installed;
    },

    async uninstall(pluginId) {
      if (disposed) return;
      await client.uninstall(pluginId, lifetime.signal);
      if (disposed) return;
      // The row is gone, and so is any progress it accumulated: a reinstall
      // must start from "Installer" rather than from the old bar.
      progress.delete(pluginId);
      await refresh();
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
      unsubscribe?.();
      unsubscribe = null;
      listeners.clear();
      progress.clear();
      catalog = emptyPluginCatalog();
    },
  };
}
