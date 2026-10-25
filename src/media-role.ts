import type { GameMediaView, WallpaperCategory } from "./contracts";

/** The card slot a stored image was picked for. */
export type MediaRole = WallpaperCategory;

/**
 * Title endings the wallpaper search has always written, in the order they are
 * tried. They are the only trace of the row an image was picked in on records
 * saved before `role` existed, because every one of those was stored with the
 * `wallpaper` kind whatever it was. "Key art" and "header" are the wide card
 * art; a bare "artwork" or "background" is deliberately absent, since IGDB
 * numbers its artworks identically in every row and nothing can be said.
 */
const LEGACY_TITLE_ROLES: ReadonlyArray<readonly [RegExp, MediaRole]> = [
  [/(?:[—–]|\s-)\s*(?:library\s+)?logo\b[^—–]*$/i, "logo"],
  [/(?:[—–]|\s-)\s*cover\b[^—–]*$/i, "cover"],
  [/(?:[—–]|\s-)\s*(?:capsule|header|key art)\b[^—–]*$/i, "landscape"],
];

/**
 * Which slot an image was chosen for, or `null` when nothing says.
 *
 * In order: the role stored on the record; the card slot the record's kind
 * already stands for (a `cover`-kind item is a cover); and, for legacy records,
 * the suffix of its title. A `null` is read by the callers as "could be a
 * background", which is what every record was treated as before.
 */
export function inferMediaRole(
  item: Pick<GameMediaView, "title" | "kind"> & { role?: MediaRole },
): MediaRole | null {
  if (item.role) return item.role;
  if (item.kind === "cover") return "cover";
  for (const [pattern, role] of LEGACY_TITLE_ROLES) {
    if (pattern.test(item.title.trim())) return role;
  }
  return null;
}

/**
 * Whether an image may be offered, previewed or painted as a background: it was
 * picked as one, or nothing says it was picked as anything else. This is the
 * single place that decides, so the picker's row and the hero cannot disagree.
 */
export function isBackgroundMedia(
  item: Pick<GameMediaView, "title" | "kind"> & { role?: MediaRole },
): boolean {
  const role = inferMediaRole(item);
  return role === null || role === "background";
}

/** The saved wallpapers that are really backgrounds. */
export function backgroundMedia(media: GameMediaView[]): GameMediaView[] {
  return media.filter(
    (item) => item.kind === "wallpaper" && isBackgroundMedia(item),
  );
}
