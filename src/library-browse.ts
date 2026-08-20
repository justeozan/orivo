/**
 * How the Library is browsed: one mode at a time, each offering its own row of
 * segments.
 *
 * The rail used to carry two decorative dropdowns ("All Games", "Recent") that
 * did nothing. This replaces them with a single control that cycles through the
 * four ways a library is actually read — what you played, what kind of game it
 * is, where it came from, what it runs on — and a row of segments whose content
 * follows the mode.
 *
 * Everything here is pure: the page owns the DOM, this owns the answers.
 */

import type { LibraryGame } from "./mock-library";
import { sourceBadge } from "./source-model";

export type BrowseMode = "activity" | "genre" | "source" | "platform";

export interface BrowseSegment {
  /** Unique inside its mode; the mode itself is the rest of the key. */
  id: string;
  label: string;
}

/** The cycle order of the single mode button, and its wording. */
export const BROWSE_MODES: ReadonlyArray<{ mode: BrowseMode; label: string }> = [
  { mode: "activity", label: "Activity" },
  { mode: "genre", label: "Genre" },
  { mode: "source", label: "Source" },
  { mode: "platform", label: "Platform" },
];

/**
 * A mode that has nothing to divide the library by still needs one segment, or
 * the bar would read as broken. A library with no genres simply shows the lot.
 */
export const ALL_GAMES_SEGMENT: BrowseSegment = { id: "all", label: "All Games" };

export function browseModeLabel(mode: BrowseMode): string {
  return BROWSE_MODES.find((entry) => entry.mode === mode)?.label ?? "Activity";
}

export function nextBrowseMode(mode: BrowseMode): BrowseMode {
  const index = BROWSE_MODES.findIndex((entry) => entry.mode === mode);
  return BROWSE_MODES[(index + 1) % BROWSE_MODES.length].mode;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

/** Under an hour on the clock is a game you have looked at, not one you play. */
const STARTED_THRESHOLD_SECONDS = 3_600;

/**
 * The bar is one line across the foot of the scene, so a derived mode shows the
 * busiest segments and stops. A Steam library carries thirty genres; laying all
 * thirty out turns every label into two ellipsised letters and the row stops
 * being readable at a glance. What falls off the end is still reachable through
 * search.
 */
export const MAX_BROWSE_SEGMENTS = 8;

const RELATIVE_LABEL = /^(\d+)\s+(minute|min|hour|day|week|month|year)s?\s+ago$/;

const RELATIVE_UNIT: Record<string, number> = {
  minute: MINUTE,
  min: MINUTE,
  hour: HOUR,
  day: DAY,
  week: WEEK,
  month: MONTH,
  year: YEAR,
};

/**
 * `lastPlayedAt` arrives in two dialects: the fixtures speak English ("2 days
 * ago") and the connectors hand back a raw instant ("2024-02-26T19:22:09Z").
 * Both have to become one comparable number before anything can be sorted.
 */
export function lastPlayedTime(value: string | undefined, now = Date.now()): number | null {
  const text = (value ?? "").trim();
  if (!text) return null;

  const lower = text.toLocaleLowerCase();
  if (lower === "today" || lower === "just now") return now;
  if (lower === "yesterday") return now - DAY;

  const relative = RELATIVE_LABEL.exec(lower);
  if (relative) {
    const amount = Number(relative[1]);
    const unit = RELATIVE_UNIT[relative[2]];
    if (Number.isFinite(amount) && unit !== undefined) return now - amount * unit;
  }

  const parsed = Date.parse(text);
  return Number.isNaN(parsed) ? null : parsed;
}

const plural = (amount: number, unit: string): string =>
  `${amount} ${unit}${amount === 1 ? "" : "s"} ago`;

/**
 * The hero prints "Last played …", so this returns only the tail. A source that
 * already speaks English keeps its own wording; only a machine timestamp is
 * rewritten, which is what stops Xbox titles reading
 * "Last played 2024-02-26T19:22:09.4406448Z".
 */
export function formatLastPlayed(value: string | undefined, now = Date.now()): string {
  const text = (value ?? "").trim();
  if (!text) return "";

  const at = Date.parse(text);
  if (Number.isNaN(at)) return text;

  const elapsed = now - at;
  if (elapsed < HOUR) return "just now";
  if (elapsed < DAY) return plural(Math.floor(elapsed / HOUR), "hour");
  if (elapsed < 2 * DAY) return "yesterday";
  if (elapsed < WEEK) return plural(Math.floor(elapsed / DAY), "day");
  if (elapsed < MONTH) return plural(Math.floor(elapsed / WEEK), "week");
  if (elapsed < YEAR) return plural(Math.floor(elapsed / MONTH), "month");
  return plural(Math.floor(elapsed / YEAR), "year");
}

type PlatformKey = "macos" | "windows" | "linux" | "other";

/** Native support first: a Mac library is read by what runs on the Mac. */
const PLATFORM_ORDER: readonly PlatformKey[] = ["macos", "windows", "linux", "other"];

const PLATFORM_LABELS: Record<PlatformKey, string> = {
  macos: "Apple",
  windows: "Windows",
  linux: "Linux",
  other: "Other",
};

/**
 * A game belongs to every platform it actually declares.
 *
 * `hostPlatform` is deliberately not consulted: the backend fills it with the
 * OS of the machine running Orivo, so it is the same value for every game in
 * the library. Reading it here would put the whole library under one segment
 * and label a Windows-only title "Apple" on a Mac.
 *
 * That leaves `supportedPlatforms`, which today only Steam reports, plus the
 * one inference that cannot be wrong: a Wine entry is a Windows build, that
 * being the entire reason it needs Wine. A game with nothing to declare stays
 * out of this mode rather than being guessed into a segment.
 */
function platformsOf(game: LibraryGame): PlatformKey[] {
  const keys = new Set<PlatformKey>(game.supportedPlatforms ?? []);
  if (game.source === "wine") keys.add("windows");
  return [...keys];
}

interface ActivityShelf extends BrowseSegment {
  /** Absent means the shelf keeps the whole library and only reorders it. */
  keep?: (game: LibraryGame, now: number) => boolean;
  rank: (game: LibraryGame, now: number) => number;
}

/**
 * The five ways a session starts. The first two only reorder — the Library must
 * never open on an empty rail — while the last three are real shelves and are
 * offered only when they hold something.
 */
const ACTIVITY_SHELVES: readonly ActivityShelf[] = [
  {
    id: "recent",
    label: "Recently Played",
    rank: (game, now) => -(lastPlayedTime(game.lastPlayedAt, now) ?? Number.NEGATIVE_INFINITY),
  },
  {
    id: "most-played",
    label: "Most Played",
    rank: (game) => -game.playTimeSeconds,
  },
  {
    id: "play-next",
    label: "Play Next",
    keep: (game) => game.launchable && game.playTimeSeconds < STARTED_THRESHOLD_SECONDS,
    // The least-touched first: this shelf exists to surface what is waiting.
    rank: (game) => game.playTimeSeconds,
  },
  {
    id: "resume",
    label: "Resume",
    keep: (game) => game.launchable && game.playTimeSeconds >= STARTED_THRESHOLD_SECONDS,
    rank: (game, now) => -(lastPlayedTime(game.lastPlayedAt, now) ?? Number.NEGATIVE_INFINITY),
  },
  {
    id: "never",
    label: "Never Played",
    keep: (game, now) => game.playTimeSeconds <= 0 && lastPlayedTime(game.lastPlayedAt, now) === null,
    rank: () => 0,
  },
];

/**
 * Sorting is stable, so games that tie keep the order the catalog gave them —
 * that is what lets the default shelf reorder nothing at all when a library
 * already arrives newest-first.
 */
function sorted(
  games: readonly LibraryGame[],
  rank: (game: LibraryGame, now: number) => number,
  now: number,
): LibraryGame[] {
  return [...games].sort((left, right) => rank(left, now) - rank(right, now));
}

/** Recency first, then A→Z, for every mode that filters rather than sorts. */
function byRecency(games: readonly LibraryGame[], now: number): LibraryGame[] {
  return [...games].sort((left, right) => {
    const leftAt = lastPlayedTime(left.lastPlayedAt, now);
    const rightAt = lastPlayedTime(right.lastPlayedAt, now);
    if (leftAt !== rightAt) {
      if (leftAt === null) return 1;
      if (rightAt === null) return -1;
      return rightAt - leftAt;
    }
    return left.title.localeCompare(right.title);
  });
}

/** Busiest first, so the segment you are most likely to want leads the row. */
function byCount(entries: Map<string, { label: string; count: number }>): BrowseSegment[] {
  return [...entries]
    .sort(([, left], [, right]) => right.count - left.count || left.label.localeCompare(right.label))
    .slice(0, MAX_BROWSE_SEGMENTS)
    .map(([id, entry]) => ({ id, label: entry.label }));
}

function tally(
  games: readonly LibraryGame[],
  keysOf: (game: LibraryGame) => Array<{ id: string; label: string }>,
): BrowseSegment[] {
  const entries = new Map<string, { label: string; count: number }>();
  for (const game of games) {
    for (const key of keysOf(game)) {
      const existing = entries.get(key.id);
      if (existing) existing.count += 1;
      else entries.set(key.id, { label: key.label, count: 1 });
    }
  }
  return byCount(entries);
}

/**
 * The segments a mode offers for this library. A derived mode lists only what
 * the library actually holds — an empty "PlayStation" tab would be a promise
 * the library cannot keep.
 */
export function browseSegments(
  games: readonly LibraryGame[],
  mode: BrowseMode,
  now = Date.now(),
): BrowseSegment[] {
  let segments: BrowseSegment[];

  switch (mode) {
    case "activity":
      segments = ACTIVITY_SHELVES.filter(
        (shelf) => !shelf.keep || games.some((game) => shelf.keep?.(game, now)),
      ).map((shelf) => ({ id: shelf.id, label: shelf.label }));
      break;
    case "genre":
      segments = tally(games, (game) =>
        game.genre.trim() ? [{ id: game.genre.trim(), label: game.genre.trim() }] : [],
      );
      break;
    case "source":
      segments = tally(games, (game) => {
        const badge = sourceBadge(game.source);
        return badge && game.source ? [{ id: game.source, label: badge.label }] : [];
      });
      break;
    case "platform":
      // Platforms are a fixed, tiny set, so they read best in their own order
      // rather than by how many games happen to carry each one.
      segments = tally(games, (game) =>
        platformsOf(game).map((key) => ({ id: key, label: PLATFORM_LABELS[key] })),
      ).sort(
        (left, right) =>
          PLATFORM_ORDER.indexOf(left.id as PlatformKey) -
          PLATFORM_ORDER.indexOf(right.id as PlatformKey),
      );
      break;
  }

  return segments.length > 0 ? segments : [ALL_GAMES_SEGMENT];
}

/** The library as this segment shows it: filtered where it filters, always sorted. */
export function browseGames(
  games: readonly LibraryGame[],
  mode: BrowseMode,
  segmentId: string,
  now = Date.now(),
): LibraryGame[] {
  if (segmentId === ALL_GAMES_SEGMENT.id) return [...games];

  if (mode === "activity") {
    const shelf = ACTIVITY_SHELVES.find((entry) => entry.id === segmentId);
    if (!shelf) return [...games];
    const kept = shelf.keep ? games.filter((game) => shelf.keep?.(game, now)) : games;
    return sorted(kept, shelf.rank, now);
  }

  const matches = (game: LibraryGame): boolean => {
    if (mode === "genre") return game.genre.trim() === segmentId;
    if (mode === "source") return game.source === segmentId;
    return platformsOf(game).includes(segmentId as PlatformKey);
  };

  return byRecency(games.filter(matches), now);
}

/**
 * Which segment a mode should land on: the one last used when it still exists,
 * otherwise the first. Keeping the choice per mode is what makes the button
 * feel like four views rather than one that forgets.
 */
export function resolveSegment(segments: readonly BrowseSegment[], preferred: string | undefined): string {
  if (preferred && segments.some((segment) => segment.id === preferred)) return preferred;
  return segments[0]?.id ?? ALL_GAMES_SEGMENT.id;
}
