import { formatDataSize } from "./settings-model";

/**
 * The update flow as a pure state machine.
 *
 * Nothing in this module imports Tauri. The plugin lives behind a dynamic
 * import in the shell, so the browser preview, vitest, and the dev server all
 * load this file without a desktop runtime present. Everything the About panel
 * renders — labels, detail lines, the button caption, the progress width — is
 * derived here, which is what makes the flow testable without a real download.
 */
export type UpdateStatus =
  | "idle"
  | "checking"
  | "available"
  | "up-to-date"
  | "downloading"
  | "ready"
  | "error";

/**
 * Which half of the flow failed. Asking the release feed and installing the
 * package fail for entirely different reasons, and the panel must not tell the
 * user a check failed when the bytes were what went wrong.
 */
export type UpdateFailure = "check" | "download";

export interface UpdateState {
  status: UpdateStatus;
  /** The version the user is running right now, or "" until it is known. */
  currentVersion: string;
  /** The version waiting to be installed, or null when there is none. */
  availableVersion: string | null;
  /** Release notes for `availableVersion`, trimmed, or null when empty. */
  notes: string | null;
  downloadedBytes: number;
  /** Null while the download length is unknown — a valid server response. */
  totalBytes: number | null;
  error: string | null;
  /** Set only while `status` is "error". */
  failure: UpdateFailure | null;
}

/**
 * The shape the updater plugin's `Update` handle satisfies structurally. Typing
 * the argument this way keeps `applyCheckResult` free of any plugin import
 * while still accepting the real object unchanged.
 */
export interface UpdateManifest {
  version: string;
  currentVersion?: string;
  body?: string | null;
}

export interface UpdateDescription {
  label: string;
  detail: string;
  buttonLabel: string;
  buttonDisabled: boolean;
}

export const INITIAL_UPDATE_STATE: UpdateState = {
  status: "idle",
  currentVersion: "",
  availableVersion: null,
  notes: null,
  downloadedBytes: 0,
  totalBytes: null,
  error: null,
  failure: null,
};

/**
 * Starts a check. Any previous result is dropped: showing "version 0.4.0 is
 * available" while re-checking would be a lie the moment the server answers
 * differently.
 */
export function startCheck(state: UpdateState): UpdateState {
  return {
    ...state,
    status: "checking",
    availableVersion: null,
    notes: null,
    downloadedBytes: 0,
    totalBytes: null,
    error: null,
    failure: null,
  };
}

/** `null` means the backend found nothing newer, which is a success, not a gap. */
export function applyCheckResult(state: UpdateState, update: UpdateManifest | null): UpdateState {
  if (!update) {
    return {
      ...state,
      status: "up-to-date",
      availableVersion: null,
      notes: null,
      downloadedBytes: 0,
      totalBytes: null,
      error: null,
      failure: null,
    };
  }
  return {
    ...state,
    status: "available",
    currentVersion: nonEmpty(update.currentVersion) ?? state.currentVersion,
    availableVersion: nonEmpty(update.version) ?? state.availableVersion,
    notes: nonEmpty(update.body),
    downloadedBytes: 0,
    totalBytes: null,
    error: null,
    failure: null,
  };
}

export function startDownload(state: UpdateState): UpdateState {
  return {
    ...state,
    status: "downloading",
    downloadedBytes: 0,
    totalBytes: null,
    error: null,
    failure: null,
  };
}

/**
 * Folds one download event into the state.
 *
 * `contentLength` is only ever reported on the first event and is optional in
 * the protocol, so an absent or nonsensical value leaves `totalBytes` as it
 * was: an unknown total renders as an indeterminate bar, never as 0%.
 */
export function applyProgress(
  state: UpdateState,
  chunkBytes: number,
  contentLength: number | null = null,
): UpdateState {
  const total = positiveInteger(contentLength);
  return {
    ...state,
    status: "downloading",
    downloadedBytes: state.downloadedBytes + nonNegativeInteger(chunkBytes),
    totalBytes: total ?? state.totalBytes,
    error: null,
    failure: null,
  };
}

/**
 * The bytes are on disk and the installer has run. The counter is snapped to
 * the announced total so a stream that reported slightly fewer bytes than
 * advertised cannot leave the bar frozen at 99%.
 */
export function markReady(state: UpdateState): UpdateState {
  return {
    ...state,
    status: "ready",
    downloadedBytes:
      state.totalBytes === null ? state.downloadedBytes : Math.max(state.downloadedBytes, state.totalBytes),
    error: null,
    failure: null,
  };
}

/**
 * Records a failure against the step it happened in. The step is read off the
 * status being left behind, so callers cannot label a download failure as a
 * failed check by forgetting an argument.
 */
export function applyError(state: UpdateState, err: unknown): UpdateState {
  return {
    ...state,
    status: "error",
    error: errorMessage(err),
    failure: state.status === "downloading" ? "download" : "check",
  };
}

/**
 * Download progress as a whole percentage, or null when the total is unknown.
 * Clamped, because a server that under-reports `contentLength` must not push
 * the bar past the end of its track.
 */
export function updateProgressPercent(state: UpdateState): number | null {
  const total = state.totalBytes;
  if (total === null || total <= 0) return null;
  const ratio = state.downloadedBytes / total;
  if (!Number.isFinite(ratio)) return null;
  return Math.round(Math.min(1, Math.max(0, ratio)) * 100);
}

/** Every string the About panel shows, so the DOM layer stays a projection. */
export function describeUpdateState(state: UpdateState): UpdateDescription {
  switch (state.status) {
    case "checking":
      return {
        label: "Checking for updates…",
        detail: "Asking the Orivo release feed for a newer version.",
        buttonLabel: "Checking…",
        buttonDisabled: true,
      };
    case "available":
      return {
        label: `Version ${state.availableVersion ?? "unknown"} is available`,
        detail:
          state.notes ??
          "Download it now. Orivo installs the update and restarts once to finish.",
        buttonLabel: "Download and install",
        buttonDisabled: false,
      };
    case "up-to-date":
      return {
        label: "You're on the latest version.",
        detail: state.currentVersion
          ? `Orivo ${state.currentVersion} is the newest release.`
          : "There is nothing newer to install.",
        buttonLabel: "Check again",
        buttonDisabled: false,
      };
    case "downloading": {
      const percent = updateProgressPercent(state);
      return {
        label: percent === null ? "Downloading update…" : `Downloading update — ${percent}%`,
        detail:
          state.totalBytes === null
            ? `${formatBytes(state.downloadedBytes)} downloaded`
            : `${formatBytes(state.downloadedBytes)} of ${formatBytes(state.totalBytes)}`,
        buttonLabel: "Downloading…",
        buttonDisabled: true,
      };
    }
    case "ready":
      return {
        label: "Update ready to install",
        detail: state.availableVersion
          ? `Restart Orivo to finish updating to version ${state.availableVersion}.`
          : "Restart Orivo to finish the update.",
        buttonLabel: "Restart to update",
        buttonDisabled: false,
      };
    case "error":
      return {
        label:
          state.failure === "download"
            ? "The update could not be installed"
            : "The update check failed",
        detail: state.error ?? "Something went wrong. Try again in a moment.",
        buttonLabel: "Try again",
        buttonDisabled: false,
      };
    case "idle":
      return {
        label: "Orivo has not checked for updates yet.",
        detail: state.currentVersion
          ? `You are running version ${state.currentVersion}.`
          : "Check whenever you like — nothing is downloaded until you ask.",
        buttonLabel: "Check for updates",
        buttonDisabled: false,
      };
  }
}

/**
 * Byte counters read the same everywhere in Settings, so this is the derived
 * cache formatter rather than a second, subtly different one.
 */
export function formatBytes(bytes: number): string {
  return formatDataSize(bytes);
}

function errorMessage(err: unknown): string {
  if (typeof err === "string" && err.trim()) return err.trim();
  if (err instanceof Error && err.message.trim()) return err.message.trim();
  // Tauri commands can reject with a plain serialised record; a `message` field
  // there is far more useful to the user than "[object Object]".
  if (typeof err === "object" && err !== null) {
    const message = (err as { message?: unknown }).message;
    if (typeof message === "string" && message.trim()) return message.trim();
  }
  return "The update could not be completed.";
}

function nonEmpty(value: string | null | undefined): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function nonNegativeInteger(value: number): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0;
}

function positiveInteger(value: number | null | undefined): number | null {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  const floored = Math.floor(value);
  return floored > 0 ? floored : null;
}
