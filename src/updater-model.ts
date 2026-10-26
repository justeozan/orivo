import { t } from "./i18n";
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
  /** What the new version brings, one short line each; empty when unknown. */
  highlights?: string[];
}

/** No more than a glance: the full notes are the changelog's job. */
const MAX_HIGHLIGHTS = 5;
/** A highlight is one line; a longer first sentence is cut at a word. */
const MAX_HIGHLIGHT_LENGTH = 110;

/**
 * The release notes as a handful of readable bullets.
 *
 * A release's body is the changelog section: long paragraphs, then download
 * instructions for people installing by hand. Shown as-is it was a wall of
 * text. This keeps the bullets of the "Added" and "Changed" sections, takes
 * each one's first sentence, strips the Markdown and stops at five. Anything
 * it cannot read as such gives nothing, and the panel then simply says a new
 * version is there.
 */
export function releaseHighlights(body: string | null | undefined): string[] {
  if (!body) return [];
  const highlights: string[] = [];
  let section = "";
  for (const raw of body.split(/\r?\n/)) {
    const line = raw.trim();
    const heading = /^#{2,4}\s+(.+)$/.exec(line);
    if (heading) {
      section = heading[1].trim().toLowerCase();
      continue;
    }
    if (section !== "added" && section !== "changed") continue;
    const bullet = /^[-*]\s+(.+)$/.exec(line);
    if (!bullet) continue;
    const text = bullet[1]
      .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
      .replace(/[*_`]+/g, "")
      .replace(/\s+/g, " ")
      .trim();
    const sentence = /^(.+?[.!?])(\s|$)/.exec(text)?.[1] ?? text;
    let short = sentence.replace(/[.]$/, "");
    if (short.length > MAX_HIGHLIGHT_LENGTH) {
      short = `${short.slice(0, MAX_HIGHLIGHT_LENGTH).replace(/\s+\S*$/, "")}…`;
    }
    if (short) highlights.push(short);
    if (highlights.length >= MAX_HIGHLIGHTS) break;
  }
  return highlights;
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
        label: t("misc.update.checking.label"),
        detail: t("misc.update.checking.detail"),
        buttonLabel: t("misc.update.checking.button"),
        buttonDisabled: true,
      };
    case "available":
      return {
        label: t("misc.update.available.label", {
          version: state.availableVersion ?? t("misc.update.unknownVersion"),
        }),
        detail: t("misc.update.available.detail"),
        buttonLabel: t("misc.update.available.button"),
        buttonDisabled: false,
        highlights: releaseHighlights(state.notes),
      };
    case "up-to-date":
      return {
        label: t("misc.update.upToDate.label"),
        detail: state.currentVersion
          ? t("misc.update.upToDate.detailCurrent", { version: state.currentVersion })
          : t("misc.update.upToDate.detail"),
        buttonLabel: t("misc.update.upToDate.button"),
        buttonDisabled: false,
      };
    case "downloading": {
      const percent = updateProgressPercent(state);
      return {
        label:
          percent === null
            ? t("misc.update.downloading.label")
            : t("misc.update.downloading.labelPercent", { percent }),
        detail:
          state.totalBytes === null
            ? t("misc.update.downloading.detailUnknown", { done: formatBytes(state.downloadedBytes) })
            : t("misc.update.downloading.detail", {
                done: formatBytes(state.downloadedBytes),
                total: formatBytes(state.totalBytes),
              }),
        buttonLabel: t("misc.update.downloading.button"),
        buttonDisabled: true,
      };
    }
    case "ready":
      return {
        label: t("misc.update.ready.label"),
        detail: state.availableVersion
          ? t("misc.update.ready.detailVersion", { version: state.availableVersion })
          : t("misc.update.ready.detail"),
        buttonLabel: t("misc.update.ready.button"),
        buttonDisabled: false,
      };
    case "error":
      return {
        label:
          state.failure === "download"
            ? t("misc.update.error.labelDownload")
            : t("misc.update.error.labelCheck"),
        detail: state.error ?? t("misc.update.error.detail"),
        buttonLabel: t("misc.update.error.button"),
        buttonDisabled: false,
      };
    case "idle":
      return {
        label: t("misc.update.idle.label"),
        detail: state.currentVersion
          ? t("misc.update.idle.detailVersion", { version: state.currentVersion })
          : t("misc.update.idle.detail"),
        buttonLabel: t("misc.update.idle.button"),
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
  return t("misc.update.error.fallback");
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
