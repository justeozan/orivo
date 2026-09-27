/**
 * The Winlator source entry point.
 *
 * Winlator is an Android application, and the folder it exports its shortcuts
 * into is one the user has to hand over once through the system folder chooser.
 * Both facts are host facts, so this module holds the decisions the frontend is
 * allowed to make about them — whether to offer the entry point, how to read the
 * host's answers, and what to put in front of the user before anything is
 * added — and nothing else. The sentences come from Rust, which is the side that
 * knows what happened.
 *
 * Nothing here adds a game. A `.desktop` file carries a command Winlator runs,
 * so a shortcut Orivo found is a question, never an answer: the host imports
 * only the references that come back from this list.
 */

/** Whose name a found shortcut is also using, as the host judged it. */
export type WinlatorTitleCollision = "none" | "library" | "folder";

/** One shortcut the host found in the connected folder. */
export interface WinlatorShortcut {
  gameRef: string;
  title: string;
  /** The file itself. A `Name=` line is not an identity; this is. */
  fileName: string;
  /** The folders between the connected one and the file, `/`-joined. */
  folderPath: string;
  duplicateTitle: WinlatorTitleCollision;
  alreadyImported: boolean;
}

/** One line of the confirmation: what it is called, where it is, who else has that name. */
export interface WinlatorReviewEntry {
  title: string;
  /** The connected folder, the folders under it, and the file. Never an absolute path. */
  origin: string;
  /** A sentence, or `null` when this name is nobody else's. */
  collision: string | null;
}

/** What `connect_winlator_export_folder` answers. */
export interface WinlatorExportFolder {
  connected: boolean;
  folderLabel: string | null;
  found: WinlatorShortcut[];
  message: string;
}

/** What `import_winlator_shortcuts` answers. */
export interface WinlatorImportResult {
  importedIds: string[];
  message: string;
}

/** At most this many titles are listed before the rest become a count. */
export const MAX_LISTED_WINLATOR_SHORTCUTS = 6;

/**
 * Is this the platform Winlator runs on?
 *
 * Read from the user agent rather than from a catalog field, because the
 * question is about the session and is asked while the shell is built, before
 * any game has been loaded.
 */
export function isWinlatorHost(userAgent: string): boolean {
  return /\bAndroid\b/.test(userAgent);
}

/**
 * Read the host's answer without trusting its shape. An answer that does not
 * parse is not turned into a cheerful default: the caller says it failed.
 */
export function normaliseWinlatorExportFolder(payload: unknown): WinlatorExportFolder | null {
  if (!payload || typeof payload !== "object") return null;
  const record = payload as Record<string, unknown>;
  if (typeof record.connected !== "boolean" || typeof record.message !== "string") return null;
  return {
    connected: record.connected,
    folderLabel: typeof record.folderLabel === "string" && record.folderLabel.trim() ? record.folderLabel : null,
    found: normaliseWinlatorShortcuts(record.found),
    message: record.message,
  };
}

function normaliseWinlatorShortcuts(payload: unknown): WinlatorShortcut[] {
  if (!Array.isArray(payload)) return [];
  const shortcuts: WinlatorShortcut[] = [];
  for (const entry of payload) {
    if (!entry || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    if (typeof record.gameRef !== "string" || !record.gameRef) continue;
    if (typeof record.title !== "string" || !record.title) continue;
    // The file name is what makes a planted shortcut answerable at all, so an
    // entry arriving without one is dropped rather than shown under its own
    // `Name=` — which is exactly the thing that cannot be trusted.
    if (typeof record.fileName !== "string" || !record.fileName) continue;
    shortcuts.push({
      gameRef: record.gameRef,
      title: record.title,
      fileName: record.fileName,
      folderPath: typeof record.folderPath === "string" ? record.folderPath : "",
      duplicateTitle: normaliseWinlatorTitleCollision(record.duplicateTitle),
      alreadyImported: record.alreadyImported === true,
    });
  }
  return shortcuts;
}

/** A value outside the closed set is read as no collision, never rendered. */
function normaliseWinlatorTitleCollision(payload: unknown): WinlatorTitleCollision {
  return payload === "library" || payload === "folder" ? payload : "none";
}

export function normaliseWinlatorImportResult(payload: unknown): WinlatorImportResult | null {
  if (!payload || typeof payload !== "object") return null;
  const record = payload as Record<string, unknown>;
  if (typeof record.message !== "string") return null;
  const importedIds = Array.isArray(record.importedIds)
    ? record.importedIds.filter((id): id is string => typeof id === "string" && id.length > 0)
    : [];
  return { importedIds, message: record.message };
}

/**
 * The shortcuts worth asking about: the ones that are not already a card. A
 * folder whose every shortcut is imported has nothing to confirm.
 */
export function winlatorShortcutsToOffer(folder: WinlatorExportFolder): WinlatorShortcut[] {
  return folder.found.filter((shortcut) => !shortcut.alreadyImported);
}

/**
 * What the confirmation asks. It names the games, because "add 3 games" hides
 * exactly the thing the user is being asked to vouch for: a `.desktop` file
 * nobody recognises is one they should refuse.
 */
export function winlatorReviewPrompt(shortcuts: WinlatorShortcut[]): string {
  if (shortcuts.length === 1) {
    return `Add “${shortcuts[0]!.title}” to your library?`;
  }
  return `Add these ${shortcuts.length} Winlator games to your library?`;
}

/**
 * The lines to show, and how many were left out of that list.
 *
 * Each line names the file and the folder holding it, because the title came out
 * of the file and a file can claim any title. `folderLabel` is the connected
 * folder's own name as the host reported it; without one the line starts at the
 * grant rather than inventing a root.
 */
export function winlatorReviewList(
  shortcuts: WinlatorShortcut[],
  folderLabel: string | null,
): { entries: WinlatorReviewEntry[]; remaining: number } {
  const entries = shortcuts.slice(0, MAX_LISTED_WINLATOR_SHORTCUTS).map((shortcut) => ({
    title: shortcut.title,
    origin: [folderLabel, shortcut.folderPath, shortcut.fileName].filter((part) => part).join("/"),
    collision: winlatorCollisionSentence(shortcut.duplicateTitle),
  }));
  return { entries, remaining: Math.max(0, shortcuts.length - entries.length) };
}

function winlatorCollisionSentence(collision: WinlatorTitleCollision): string | null {
  if (collision === "library") return "A game already in your library uses this name";
  if (collision === "folder") return "Another file in this folder uses this name";
  return null;
}

/**
 * What the background pass has to say, if anything. It imports nothing, so this
 * is an invitation to look rather than news that the library changed.
 */
export function winlatorWaitingToast(payload: unknown): string | null {
  if (!payload || typeof payload !== "object") return null;
  const record = payload as Record<string, unknown>;
  const count = (value: unknown): number =>
    typeof value === "number" && Number.isSafeInteger(value) && value > 0 ? value : 0;
  const pending = count(record.pending);
  const changed = count(record.changed);
  if (pending === 0 && changed === 0) return null;
  const parts: string[] = [];
  if (pending === 1) parts.push("one new shortcut");
  else if (pending > 1) parts.push(`${pending} new shortcuts`);
  if (changed === 1) parts.push("one that changed");
  else if (changed > 1) parts.push(`${changed} that changed`);
  return `Winlator has ${parts.join(" and ")}. Open Sources to review them.`;
}
