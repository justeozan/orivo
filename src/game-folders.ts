/* ---------------------------------------------------------------------------
   Games folders, as the interface sees them.

   The host keeps the list of folders and scans them (game_folder.rs): every
   game, app and shortcut directly inside a folder becomes a card. What comes
   over IPC is deliberately thin — an opaque id and a name per folder, and
   after a scan a count of what it found — so no path ever reaches the page.
   This module reads those answers and puts a scan into one sentence.
   --------------------------------------------------------------------------- */

import { t } from "./i18n";

export interface GameFolderView {
  id: string;
  label: string;
  /** False when the folder is gone, or its drive is not connected. */
  available: boolean;
}

export interface GameFolderScanSummary {
  label: string;
  state: "ok" | "missing" | "unreadable";
  entries: number;
  games: number;
  added: number;
  known: number;
  unusable: number;
  unrecognized: string[];
}

export interface GameFolderScanEvent {
  /** The player asked for this scan, so it is answered even when it found nothing. */
  requested: boolean;
  added: number;
  addedIds: string[];
  /** Cards an older scan pointed at a tool, now pointing at the game. */
  repointed: number;
  /** Such cards hidden: no game beside the tool, or already in the library. */
  hidden: number;
  /** Titles of the games that moved and were found where they went. */
  relocated: string[];
  /** Some card started, or stopped, being shown as not installed. */
  missingChanged: boolean;
  folders: GameFolderScanSummary[];
}

type Fields = Record<string, unknown>;

function record(value: unknown): Fields {
  return value && typeof value === "object" ? (value as Fields) : {};
}

function count(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.floor(value) : 0;
}

function strings(value: unknown): string[] {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : [];
}

export function readGameFolders(value: unknown): GameFolderView[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item) => {
    const entry = record(item);
    if (typeof entry.id !== "string" || typeof entry.label !== "string") return [];
    return [{ id: entry.id, label: entry.label, available: entry.available !== false }];
  });
}

/**
 * A scan's report. A host from before reports were added sent the number of
 * games added and nothing else; that still reads as a quiet scan.
 */
export function readGameFolderScan(value: unknown): GameFolderScanEvent {
  if (typeof value === "number") {
    return {
      requested: false,
      added: count(value),
      addedIds: [],
      repointed: 0,
      hidden: 0,
      relocated: [],
      missingChanged: false,
      folders: [],
    };
  }
  const event = record(value);
  const folders = Array.isArray(event.folders)
    ? event.folders.map((item): GameFolderScanSummary => {
        const folder = record(item);
        const state = folder.state === "missing" || folder.state === "unreadable" ? folder.state : "ok";
        return {
          label: typeof folder.label === "string" ? folder.label : "",
          state,
          entries: count(folder.entries),
          games: count(folder.games),
          added: count(folder.added),
          known: count(folder.known),
          unusable: count(folder.unusable),
          unrecognized: strings(folder.unrecognized),
        };
      })
    : [];
  return {
    requested: event.requested === true,
    added: count(event.added),
    addedIds: strings(event.addedIds),
    repointed: count(event.repointed),
    hidden: count(event.hidden),
    relocated: strings(event.relocated),
    missingChanged: event.missingChanged === true,
    folders,
  };
}

/** `.txt, .pdf` — or "files without an extension" for the empty one. */
function fileTypes(extensions: string[]): string {
  const shown = extensions.slice(0, 4).map((extension) => (extension ? `.${extension}` : t("app.folder.noExtension")));
  return extensions.length > 4 ? `${shown.join(", ")}…` : shown.join(", ");
}

/** What one folder held, when nothing new came out of it. */
function folderMessage(folder: GameFolderScanSummary): string {
  const name = folder.label || t("app.folder.thisFolder");
  if (folder.state === "missing") return t("app.folder.missing", { folder: name });
  if (folder.state === "unreadable") return t("app.folder.unreadable", { folder: name });
  if (folder.known > 0) {
    // Nothing new: say what is already there first, then what could not be used.
    const known =
      folder.known === 1
        ? t("app.folder.allKnownOne", { folder: name })
        : t("app.folder.allKnownMany", { folder: name, count: folder.known });
    if (folder.unusable === 0) return known;
    return `${known} ${
      folder.unusable === 1 ? t("app.folder.skippedOne") : t("app.folder.skippedMany", { count: folder.unusable })
    }`;
  }
  if (folder.unusable > 0) {
    return folder.unusable === 1
      ? t("app.folder.unusableOne", { folder: name })
      : t("app.folder.unusableMany", { folder: name, count: folder.unusable });
  }
  if (folder.entries === 0) return t("app.folder.empty", { folder: name });
  if (folder.unrecognized.length > 0) {
    return t("app.folder.noGamesTypes", { folder: name, types: fileTypes(folder.unrecognized) });
  }
  return t("app.folder.noGames", { folder: name });
}

/**
 * The sentence a scan deserves, or `null` when it should pass in silence: a
 * background scan that added nothing.
 */
export function gameFolderScanMessage(event: GameFolderScanEvent): string | null {
  const parts = [scanMessage(event), relocatedMessage(event), repairMessage(event)].filter(
    (part): part is string => part !== null,
  );
  return parts.length > 0 ? parts.join(" ") : null;
}

/** Games that moved, found where they went. Said even after a quiet scan:
 *  the player moved them, and their cards changed under them. */
function relocatedMessage(event: GameFolderScanEvent): string | null {
  if (event.relocated.length === 0) return null;
  return event.relocated.length === 1
    ? t("app.folder.relocatedOne", { title: event.relocated[0] })
    : t("app.folder.relocatedMany", { count: event.relocated.length });
}

/** Cards that pointed at a checksum tool, a plugin or a crack copy. */
function repairMessage(event: GameFolderScanEvent): string | null {
  const fixed = event.repointed + event.hidden;
  if (fixed === 0) return null;
  return fixed === 1 ? t("app.folder.repairedOne") : t("app.folder.repairedMany", { count: fixed });
}

function scanMessage(event: GameFolderScanEvent): string | null {
  if (event.added > 0) {
    const folder = event.folders.length === 1 ? event.folders[0].label : "";
    const unusable = event.folders.reduce((total, entry) => total + entry.unusable, 0);
    const added = folder
      ? event.added === 1
        ? t("app.folder.addedOneFrom", { folder })
        : t("app.folder.addedManyFrom", { folder, count: event.added })
      : event.folders.length > 1
        ? event.added === 1
          ? t("app.folder.addedOneFolders")
          : t("app.folder.addedManyFolders", { count: event.added })
        : event.added === 1
          ? t("app.folder.addedOne")
          : t("app.folder.addedMany", { count: event.added });
    if (unusable === 0) return added;
    return `${added} ${
      unusable === 1 ? t("app.folder.skippedOne") : t("app.folder.skippedMany", { count: unusable })
    }`;
  }
  if (!event.requested) return null;
  if (event.folders.length === 0) return t("app.folder.none");
  // Several folders and nothing new: the first one with something to say.
  const telling =
    event.folders.find((folder) => folder.state !== "ok") ??
    event.folders.find((folder) => folder.games === 0 || folder.unusable > 0) ??
    event.folders[0];
  return folderMessage(telling);
}

/**
 * Settings › Libraries: the watched folders, one row each (look again, stop
 * watching), and a way to add one. Folder names come from the disk, so they
 * go in through `textContent`. The library's menu keeps a single entry.
 */
export function renderGameFoldersPanel(panel: HTMLElement, folders: GameFolderView[], offered: boolean): void {
  panel.hidden = !offered;
  if (!offered) {
    panel.replaceChildren();
    return;
  }
  const heading = document.createElement("div");
  heading.className = "settings-row";
  const headingCopy = document.createElement("div");
  headingCopy.className = "settings-row__copy";
  const title = document.createElement("strong");
  title.textContent = t("app.menu.gamesFolder");
  const intro = document.createElement("small");
  intro.textContent = t("app.folder.settingsIntro");
  headingCopy.append(title, intro);
  const add = document.createElement("button");
  add.type = "button";
  add.className = "settings-button";
  add.dataset.gameFolderAction = "add";
  add.textContent = t("app.folder.add");
  heading.append(headingCopy, add);

  const rows = folders.map((folder) => {
    const row = document.createElement("div");
    row.className = "settings-row";
    const copy = document.createElement("div");
    copy.className = "settings-row__copy";
    const name = document.createElement("strong");
    name.textContent = folder.label;
    const state = document.createElement("small");
    state.textContent = folder.available ? t("app.folder.watching") : t("app.folder.unavailable");
    copy.append(name, state);
    const actions = document.createElement("div");
    actions.className = "game-folder-actions";
    const rescan = document.createElement("button");
    rescan.type = "button";
    rescan.className = "settings-button";
    rescan.dataset.gameFolderAction = "rescan";
    rescan.dataset.folderId = folder.id;
    rescan.disabled = !folder.available;
    rescan.textContent = t("app.folder.rescanShort");
    rescan.setAttribute("aria-label", t("app.folder.rescan", { folder: folder.label }));
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "settings-button";
    remove.dataset.gameFolderAction = "remove";
    remove.dataset.folderId = folder.id;
    remove.textContent = t("app.folder.removeShort");
    remove.setAttribute("aria-label", t("app.folder.remove", { folder: folder.label }));
    actions.append(rescan, remove);
    row.append(copy, actions);
    return row;
  });
  panel.replaceChildren(heading, ...rows);
}
