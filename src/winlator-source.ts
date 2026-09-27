/**
 * The Winlator source entry point.
 *
 * Winlator is an Android application, and the folder it exports its shortcuts
 * into is one the user has to hand over once through the system folder chooser.
 * Both facts are host facts, so this module holds the two decisions the frontend
 * is allowed to make about them — whether to offer the entry point at all, and
 * how to read the host's answer — and nothing else. The sentences the user sees
 * come from Rust, which is the side that knows what happened.
 */

/** What `connect_winlator_export_folder` answers. */
export interface WinlatorExportFolder {
  connected: boolean;
  folderLabel: string | null;
  adopted: number;
  message: string;
}

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
  const adopted = typeof record.adopted === "number" && Number.isSafeInteger(record.adopted) && record.adopted >= 0
    ? record.adopted
    : 0;
  return {
    connected: record.connected,
    folderLabel: typeof record.folderLabel === "string" && record.folderLabel.trim() ? record.folderLabel : null,
    adopted,
    message: record.message,
  };
}

/**
 * What to say when the background pass finds games the painted library does not
 * have. It runs after the first frame, so the alternative is cards appearing
 * with no explanation.
 */
export function winlatorAdoptionToast(adopted: number): string {
  if (adopted === 1) return "One Winlator game was added to your library.";
  return `${adopted} Winlator games were added to your library.`;
}

/** How many games one adoption event claims, or `null` if it claims nothing. */
export function readWinlatorAdoptedCount(payload: unknown): number | null {
  if (!payload || typeof payload !== "object") return null;
  const adopted = (payload as Record<string, unknown>).adopted;
  if (typeof adopted !== "number" || !Number.isSafeInteger(adopted) || adopted <= 0) return null;
  return adopted;
}
