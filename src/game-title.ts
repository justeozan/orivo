/**
 * The title a game is shown by, when no wordmark stands in for it.
 *
 * Titles reach the library the way their source spelled them: a streaming
 * host names an app `mio-memories-in-orbit`, a folder import keeps
 * `Assassin's Creed - Mirage`. Neither is wrong as an identifier, and both
 * read as a filename once they are set in the hero's display type. This is a
 * presentation pass only — the stored title, and every comparison made on it,
 * stays as it was. The host applies the same rule before it searches for
 * artwork (`tidy_title` in game_artwork.rs).
 */

/** Words a title keeps in lower case unless they open or close it. */
const SMALL_WORDS = new Set(["a", "an", "and", "as", "at", "by", "for", "in", "of", "on", "or", "the", "to", "vs"]);

/**
 * A slug is all lower case, with no spaces, and joined by separators often
 * enough to be one: `half-life` has a single dash and is a real spelling,
 * `the-perfect-pencil` is not.
 */
const isSlug = (title: string): boolean =>
  /^[a-z0-9_-]+$/.test(title) &&
  /[a-z]/.test(title) &&
  (title.includes("_") || (title.match(/-/g)?.length ?? 0) >= 2);

const capitalise = (word: string): string => word.charAt(0).toUpperCase() + word.slice(1);

export function displayTitle(raw: string): string {
  const cleaned = raw.replace(/[™®©]/g, " ").replace(/\s+/g, " ").trim();
  if (!cleaned) return raw.trim();

  if (isSlug(cleaned)) {
    const words = cleaned.split(/[-_]+/).filter(Boolean);
    return words
      .map((word, index) =>
        index > 0 && index < words.length - 1 && SMALL_WORDS.has(word) ? word : capitalise(word),
      )
      .join(" ");
  }

  return cleaned
    .replace(/\s+[-–—]\s+/g, ": ")
    .replace(/_+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}
