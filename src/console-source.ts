/**
 * The console emulator source entry points.
 *
 * Android emulators are separate applications, and the folder their ROMs live in
 * is one the user has to hand over once through the system folder chooser. Both
 * facts are host facts, so this module holds only the decisions the frontend is
 * allowed to make about them — whether to offer the entry point, how to read the
 * host's answers, and what to put in front of the user before anything is
 * added. The sentences come from Rust, which is the side that knows what
 * happened.
 *
 * Nothing here adds a game. A file in a shared folder is a question, never an
 * answer: the host imports only the references that come back from this list.
 */

import {
  type ReviewedFile,
  type SourceReviewEntry,
  type TitleCollision,
  normaliseTitleCollision,
  sourceReviewList,
} from "./source-review";

/**
 * The emulators Orivo can hand a game to, by the slug the host knows.
 *
 * The slug is the only thing the WebView may name — it is choosing a menu row,
 * not a package — and the host refuses anything outside its own closed set, so
 * this list exists to be offered, not to be trusted.
 */
export const CONSOLE_EMULATORS = [
  {
    slug: "retroarch",
    label: "RetroArch",
    description: "NES, SNES, Game Boy, GBA and Mega Drive, through RetroArch's cores",
  },
  {
    slug: "ppsspp",
    label: "PPSSPP",
    description: "PSP games, opened straight from the folder you grant",
  },
] as const;

/** One ROM the host found in the connected folder. */
export interface ConsoleRom {
  gameRef: string;
  title: string;
  /** The console its extension names, which is what picks the core. */
  systemLabel: string;
  fileName: string;
  folderPath: string;
  duplicateTitle: TitleCollision;
  alreadyImported: boolean;
}

/** What `connect_console_rom_folder` answers. */
export interface ConsoleRomFolder {
  connected: boolean;
  /** Which scan this list came from; handed back with the answer. */
  token: number;
  emulator: string;
  emulatorLabel: string;
  /** Is the app this list would hand a game to actually installed, and by whom? */
  emulatorInstalled: boolean;
  emulatorInstaller: string | null;
  folderLabel: string | null;
  found: ConsoleRom[];
  message: string;
}

/** What `import_console_roms` answers. */
export interface ConsoleImportResult {
  importedIds: string[];
  message: string;
}

/**
 * Is this the platform these emulators run on?
 *
 * Read from the user agent rather than from a catalog field, because the question
 * is about the session and is asked while the shell is built, before any game has
 * been loaded.
 */
export function isConsoleEmulatorHost(userAgent: string): boolean {
  return /\bAndroid\b/.test(userAgent);
}

/**
 * Read the host's answer without trusting its shape. An answer that does not
 * parse is not turned into a cheerful default: the caller says it failed.
 */
export function normaliseConsoleRomFolder(payload: unknown): ConsoleRomFolder | null {
  if (!payload || typeof payload !== "object") return null;
  const record = payload as Record<string, unknown>;
  if (typeof record.connected !== "boolean" || typeof record.message !== "string") return null;
  if (typeof record.emulator !== "string" || !record.emulator) return null;
  return {
    connected: record.connected,
    // A token that is not a safe integer is read as none: the host refuses that
    // and asks for the folder again, which is better than an answer landing on a
    // list nobody is looking at.
    token:
      typeof record.token === "number" && Number.isSafeInteger(record.token) && record.token > 0
        ? record.token
        : 0,
    emulatorInstalled: record.emulatorInstalled === true,
    emulatorInstaller:
      typeof record.emulatorInstaller === "string" && record.emulatorInstaller.trim()
        ? record.emulatorInstaller
        : null,
    emulator: record.emulator,
    emulatorLabel:
      typeof record.emulatorLabel === "string" && record.emulatorLabel.trim()
        ? record.emulatorLabel
        : record.emulator,
    folderLabel:
      typeof record.folderLabel === "string" && record.folderLabel.trim() ? record.folderLabel : null,
    found: normaliseConsoleRoms(record.found),
    message: record.message,
  };
}

function normaliseConsoleRoms(payload: unknown): ConsoleRom[] {
  if (!Array.isArray(payload)) return [];
  const roms: ConsoleRom[] = [];
  for (const entry of payload) {
    if (!entry || typeof entry !== "object") continue;
    const record = entry as Record<string, unknown>;
    if (typeof record.gameRef !== "string" || !record.gameRef) continue;
    if (typeof record.title !== "string" || !record.title) continue;
    // The file name and the console are what make the row answerable at all, so
    // an entry arriving without either is dropped rather than shown under the
    // title it chose for itself.
    if (typeof record.fileName !== "string" || !record.fileName) continue;
    if (typeof record.systemLabel !== "string" || !record.systemLabel) continue;
    roms.push({
      gameRef: record.gameRef,
      title: record.title,
      systemLabel: record.systemLabel,
      fileName: record.fileName,
      folderPath: typeof record.folderPath === "string" ? record.folderPath : "",
      duplicateTitle: normaliseTitleCollision(record.duplicateTitle),
      alreadyImported: record.alreadyImported === true,
    });
  }
  return roms;
}

export function normaliseConsoleImportResult(payload: unknown): ConsoleImportResult | null {
  if (!payload || typeof payload !== "object") return null;
  const record = payload as Record<string, unknown>;
  if (typeof record.message !== "string") return null;
  const importedIds = Array.isArray(record.importedIds)
    ? record.importedIds.filter((id): id is string => typeof id === "string" && id.length > 0)
    : [];
  return { importedIds, message: record.message };
}

/**
 * The ROMs worth asking about: the ones that are not already a card. A folder
 * whose every game is imported has nothing to confirm.
 */
export function consoleRomsToOffer(folder: ConsoleRomFolder): ConsoleRom[] {
  return folder.found.filter((rom) => !rom.alreadyImported);
}

/**
 * What the confirmation asks. It names the games, because "add 3 games" hides
 * exactly the thing the user is being asked to vouch for — and it names the
 * emulator, because that is the app that will open them.
 */
export function consoleReviewPrompt(roms: ConsoleRom[], emulatorLabel: string): string {
  if (roms.length === 1) {
    return `Add “${roms[0]!.title}” to your library, to play in ${emulatorLabel}?`;
  }
  return `Add these ${roms.length} games to your library, to play in ${emulatorLabel}?`;
}

/** The lines to show, each naming the console, the file and its folder. */
export function consoleReviewList(
  roms: ConsoleRom[],
  folderLabel: string | null,
): { entries: SourceReviewEntry[] } {
  const files: ReviewedFile[] = roms.map((rom) => ({ ...rom, label: rom.systemLabel }));
  return sourceReviewList(files, folderLabel);
}

/**
 * What the host could say about the emulator's own install, in one line.
 *
 * The user is about to hand a game to that app. Who put it on the device is the
 * part Orivo cannot judge for them, so it is reported rather than acted on — and
 * when the platform said nothing at all, saying *that* is better than implying
 * the app is absent.
 */
export function consoleEmulatorNote(folder: {
  emulatorLabel: string;
  emulatorInstalled: boolean;
  emulatorInstaller: string | null;
}): string | null {
  if (!folder.emulatorInstalled) return null;
  return folder.emulatorInstaller
    ? `${folder.emulatorLabel} on this device was installed by ${folder.emulatorInstaller}.`
    : `${folder.emulatorLabel} on this device was installed by hand, not from a store.`;
}
