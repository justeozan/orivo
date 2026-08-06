/* ---------------------------------------------------------------------------
   Quiky — the plugin-gated installer.

   Quiky ships as a plugin, not as part of Orivo, so every call in this file has
   to survive its absence. A host without the plugin, a host with an older host
   binary that has no `get_quiky_status` command, a browser with no Tauri at all:
   all three answer the same thing, an unavailable status, and the Store then
   renders exactly what it rendered before this feature existed.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface QuikyTitle {
  slug: string;
  title: string;
  /** Normalised alternative titles used to match a store card. */
  matchTitles: string[];
  downloadBytes: number;
  installed: boolean;
  installPath: string | null;
}

export interface QuikyStatus {
  /** False when the plugin is absent: the whole feature stays hidden. */
  available: boolean;
  pluginName: string;
  version: string;
  message: string;
  titles: QuikyTitle[];
}

export type QuikyPhase =
  | "queued"
  | "downloading"
  | "extracting"
  | "installed"
  | "failed"
  | "cancelled";

export interface QuikyProgress {
  slug: string;
  phase: QuikyPhase;
  /** 0..100, already smoothed by the host. */
  percent: number;
  message: string;
  installPath: string | null;
}

/**
 * The host side of the feature. The Store only ever talks to this interface, so
 * a test can hand it a recorder and never touch Tauri.
 */
export interface QuikyClient {
  getStatus(signal: AbortSignal): Promise<QuikyStatus>;
  startInstall(slug: string, gameId: string | null, signal: AbortSignal): Promise<void>;
  cancelInstall(slug: string, signal: AbortSignal): Promise<void>;
  subscribe(onProgress: (progress: QuikyProgress) => void): () => void;
}

/** One live install session, owned by one activation of the Store page. */
export interface QuikyController {
  load(signal: AbortSignal): Promise<void>;
  status(): QuikyStatus;
  progressFor(slug: string): QuikyProgress | null;
  install(slug: string, gameId?: string | null): Promise<void>;
  cancel(slug: string): Promise<void>;
  onChange(callback: () => void): () => void;
  dispose(): void;
}

/** The single event the host pushes while an install runs. */
export const QUIKY_PROGRESS_EVENT = "quiky-install-status";

const BUSY_PHASES: ReadonlySet<QuikyPhase> = new Set<QuikyPhase>([
  "queued",
  "downloading",
  "extracting",
]);

/** What the Store sees whenever Quiky is missing, broken or unreachable. */
export function unavailableQuikyStatus(): QuikyStatus {
  return { available: false, pluginName: "", version: "", message: "", titles: [] };
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("La requête Quiky a été annulée.", "AbortError");
}

export function quikyErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return "L'installation n'a pas abouti.";
}

/**
 * The catalog comes from a plugin, which is to say from outside Orivo. A row
 * missing its slug is dropped rather than trusted: a card whose button cannot
 * name what it installs is worse than a card with no button.
 */
function readQuikyTitle(value: unknown): QuikyTitle[] {
  if (!value || typeof value !== "object") return [];
  const raw = value as Partial<QuikyTitle>;
  if (typeof raw.slug !== "string" || !raw.slug) return [];
  return [
    {
      slug: raw.slug,
      title: typeof raw.title === "string" && raw.title ? raw.title : raw.slug,
      matchTitles: Array.isArray(raw.matchTitles)
        ? raw.matchTitles.filter((entry): entry is string => typeof entry === "string")
        : [],
      downloadBytes: typeof raw.downloadBytes === "number" ? raw.downloadBytes : 0,
      installed: raw.installed === true,
      installPath: typeof raw.installPath === "string" ? raw.installPath : null,
    },
  ];
}

function readQuikyStatus(value: unknown): QuikyStatus {
  if (!value || typeof value !== "object") return unavailableQuikyStatus();
  const raw = value as Partial<QuikyStatus>;
  if (raw.available !== true) return unavailableQuikyStatus();
  return {
    available: true,
    pluginName: typeof raw.pluginName === "string" ? raw.pluginName : "",
    version: typeof raw.version === "string" ? raw.version : "",
    message: typeof raw.message === "string" ? raw.message : "",
    titles: Array.isArray(raw.titles) ? raw.titles.flatMap(readQuikyTitle) : [],
  };
}

export function createDefaultQuikyClient(): QuikyClient {
  return {
    async getStatus(signal) {
      // Every failure mode collapses to the same answer. A plugin that is not
      // installed and a host command that does not exist both throw here, and
      // neither is a reason for the Store to stop painting.
      try {
        if (!isTauriRuntime()) return unavailableQuikyStatus();
        assertActive(signal);
        const status = await invoke<QuikyStatus>("get_quiky_status");
        assertActive(signal);
        return readQuikyStatus(status);
      } catch {
        return unavailableQuikyStatus();
      }
    },
    async startInstall(slug, gameId, signal) {
      if (!isTauriRuntime()) return;
      assertActive(signal);
      // The card id lets the host keep the Store's artwork on the installed
      // game. It is a presentation hint, never a path or a command.
      // Deliberately not caught: a refused install is news the shopper needs.
      await invoke("start_quiky_install", { slug, gameId });
    },
    async cancelInstall(slug, signal) {
      if (!isTauriRuntime()) return;
      assertActive(signal);
      await invoke("cancel_quiky_install", { slug });
    },
    subscribe(onProgress) {
      if (!isTauriRuntime()) return () => {};
      let unlisten: UnlistenFn | null = null;
      let stopped = false;
      void listen<QuikyProgress>(QUIKY_PROGRESS_EVENT, (event) => {
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

/**
 * "Ōkami HD: Rebirth — Édition Deluxe!" and "okami hd rebirth edition deluxe"
 * are the same game. Store titles and plugin titles come from different feeds
 * and neither punctuates the same way twice.
 */
export function normaliseTitle(value: string): string {
  return value
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .trim();
}

export function matchTitle(titles: QuikyTitle[], storeTitle: string): QuikyTitle | null {
  const needle = normaliseTitle(storeTitle);
  if (!needle) return null;
  for (const title of titles) {
    if (normaliseTitle(title.title) === needle) return title;
    // The contract says these arrive normalised; normalising again costs
    // nothing and means a hand-written catalog entry still matches.
    if (title.matchTitles.some((alternative) => normaliseTitle(alternative) === needle)) {
      return title;
    }
  }
  return null;
}

/** Phases where the control cancels instead of installing. */
export function isQuikyBusy(progress: QuikyProgress | null): boolean {
  return progress !== null && BUSY_PHASES.has(progress.phase);
}

/** The host smooths the percentage; this only keeps it inside the bar. */
export function quikyPercent(progress: QuikyProgress | null): number {
  if (!progress || !Number.isFinite(progress.percent)) return 0;
  return Math.max(0, Math.min(100, Math.round(progress.percent)));
}

export function formatQuikyLabel(progress: QuikyProgress | null, title: QuikyTitle): string {
  const idle = title.installed ? "Installé" : "Installer";
  if (!progress) return idle;
  switch (progress.phase) {
    case "queued":
      return "En attente…";
    case "downloading":
      return `Téléchargement ${quikyPercent(progress)}%`;
    case "extracting":
      return `Extraction ${quikyPercent(progress)}%`;
    case "installed":
      return "Installé";
    case "failed":
      return "Réessayer";
    case "cancelled":
      // A cancelled install left nothing behind, so the offer is the first one.
      return idle;
  }
}

export function createQuikyController(client: QuikyClient): QuikyController {
  const listeners = new Set<() => void>();
  const progress = new Map<string, QuikyProgress>();
  // Installs outlive the request that started them, so they are bound to the
  // controller rather than to the activation signal `load` was handed.
  const lifetime = new AbortController();
  let status = unavailableQuikyStatus();
  let unsubscribe: (() => void) | null = null;
  let disposed = false;

  const notify = (): void => {
    for (const listener of [...listeners]) listener();
  };

  /**
   * A finished install changes the catalog, not just the bar: the button has to
   * keep reading "Installé" after the progress map is forgotten.
   */
  const markInstalled = (next: QuikyProgress): void => {
    if (next.phase !== "installed") return;
    const index = status.titles.findIndex((title) => title.slug === next.slug);
    if (index < 0) return;
    const titles = [...status.titles];
    titles[index] = { ...titles[index], installed: true, installPath: next.installPath };
    status = { ...status, titles };
  };

  const record = (next: QuikyProgress): void => {
    progress.set(next.slug, next);
    markInstalled(next);
    notify();
  };

  const onProgress = (next: QuikyProgress): void => {
    if (disposed || !next || typeof next.slug !== "string") return;
    record(next);
  };

  const failure = (slug: string, error: unknown): QuikyProgress => ({
    slug,
    phase: "failed",
    percent: progress.get(slug)?.percent ?? 0,
    message: quikyErrorMessage(error),
    installPath: null,
  });

  return {
    async load(signal) {
      if (disposed) return;
      let next: QuikyStatus;
      try {
        next = await client.getStatus(signal);
      } catch {
        // The default client never throws; an injected one might, and the Store
        // treats a broken client exactly like a missing plugin.
        next = unavailableQuikyStatus();
      }
      if (disposed || signal.aborted) return;
      status = next;
      // Nothing to listen to while the plugin is absent, and no reason to hold
      // an event channel open for a feature that will never paint.
      if (status.available && !unsubscribe) unsubscribe = client.subscribe(onProgress);
      notify();
    },

    status() {
      return status;
    },

    progressFor(slug) {
      return progress.get(slug) ?? null;
    },

    async install(slug, gameId = null) {
      if (disposed) return;
      // Optimistic: the shopper clicked now, the host answers in its own time.
      // Queued is where the host would put it anyway, so nothing is invented.
      record({ slug, phase: "queued", percent: 0, message: "", installPath: null });
      try {
        await client.startInstall(slug, gameId, lifetime.signal);
      } catch (error) {
        if (disposed) return;
        record(failure(slug, error));
      }
    },

    async cancel(slug) {
      if (disposed) return;
      try {
        await client.cancelInstall(slug, lifetime.signal);
      } catch (error) {
        if (disposed) return;
        record(failure(slug, error));
      }
      // The cancelled phase stays the host's to announce: writing it here would
      // race a `downloading` tick already in flight and flip the button twice.
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
      status = unavailableQuikyStatus();
    },
  };
}
