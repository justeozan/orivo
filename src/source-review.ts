/**
 * What a folder on shared storage is allowed to say for itself.
 *
 * Two source flows put files from a shared folder in front of the user and ask
 * whether they should become library cards: Winlator's exported shortcuts and the
 * ROM folders of the console emulators. In both, the name shown came *out of the
 * file* — a `Name=` line, or a file name — and any app can create a file in a
 * shared folder with no permission at all. So the question the user answers has
 * to carry something they can check, and has to say when a name is already
 * somebody else's.
 *
 * Those two rules live here rather than in each flow, because a second copy of a
 * security-relevant sentence is a second place for it to be wrong.
 */

/** Whose name a found file is also using, as the host judged it. */
export type TitleCollision = "none" | "library" | "folder";

/** A value outside the closed set is read as no collision, never rendered. */
export function normaliseTitleCollision(payload: unknown): TitleCollision {
  return payload === "library" || payload === "folder" ? payload : "none";
}

/** What the host says about one file in the connected folder. */
export interface ReviewedFile {
  title: string;
  /** The file itself. A name the file chose for itself is not an identity; this is. */
  fileName: string;
  /** The folders between the connected one and the file, `/`-joined. */
  folderPath: string;
  duplicateTitle: TitleCollision;
  /** Something else worth putting first on the row, such as the console. */
  label?: string;
}

/** One line of a confirmation: what it is called, where it is, who else has that name. */
export interface SourceReviewEntry {
  title: string;
  /** Never an absolute path: the connected folder's own name is as deep as it goes. */
  origin: string;
  /** A sentence, or `null` when this name is nobody else's. */
  collision: string | null;
}

/** At most this many files are listed before the rest become a count. */
export const MAX_LISTED_REVIEW_ENTRIES = 6;

/**
 * The lines to show, and how many were left out of that list.
 *
 * `folderLabel` is the connected folder's own name as the host reported it;
 * without one the line starts at the grant rather than inventing a root.
 */
export function sourceReviewList(
  files: ReviewedFile[],
  folderLabel: string | null,
): { entries: SourceReviewEntry[]; remaining: number } {
  const entries = files.slice(0, MAX_LISTED_REVIEW_ENTRIES).map((file) => ({
    title: file.title,
    origin: reviewOrigin(file, folderLabel),
    collision: collisionSentence(file.duplicateTitle),
  }));
  return { entries, remaining: Math.max(0, files.length - entries.length) };
}

function reviewOrigin(file: ReviewedFile, folderLabel: string | null): string {
  const where = [folderLabel, file.folderPath, file.fileName].filter((part) => part).join("/");
  return file.label ? `${file.label} · ${where}` : where;
}

export function collisionSentence(collision: TitleCollision): string | null {
  if (collision === "library") return "A game already in your library uses this name";
  if (collision === "folder") return "Another file in this folder uses this name";
  return null;
}
