/**
 * How the Library is browsed: by where each game comes from, in an order the
 * player picks.
 *
 * The bar used to cycle four modes (Activity, Genre, Source, Platform), each
 * with its own row of segments. Four ways to slice the same shelf meant the
 * row under the rail changed shape every time the button was pressed. It is
 * one row now, always the same: All Games, then every source the library
 * actually holds. The button beside it no longer changes what the row is; it
 * changes the order the rail is in — Recent, New, Most played.
 *
 * Everything here is pure: the page owns the DOM, this owns the answers.
 */

import { t } from "./i18n";
import type { IconName } from "./icons";
import type { LibraryGame } from "./mock-library";
import { sourceBadge } from "./source-model";

/** What kind of source a segment is. It decides where the segment sits in the row. */
export type SourceKind = "all" | "store" | "console" | "plugin" | "local" | "stream";

export interface BrowseSegment {
  /** Unique in the row, and the key of the segment's mark (`source-marks.ts`). */
  id: string;
  label: string;
  kind: SourceKind;
}

/** Always the first segment: the whole library, sorted. */
export const ALL_GAMES_SEGMENT: BrowseSegment = {
  id: "all",
  label: t("store.browse.allGames"),
  kind: "all",
};

export type LibrarySort = "recent" | "new" | "most-played";

/** The cycle order of the sort button, its word and its glyph. */
export const LIBRARY_SORTS: ReadonlyArray<{ sort: LibrarySort; label: string; icon: IconName }> = [
  { sort: "recent", label: t("store.browse.sort.recent"), icon: "clock" },
  { sort: "new", label: t("store.browse.sort.new"), icon: "sparkle" },
  { sort: "most-played", label: t("store.browse.sort.mostPlayed"), icon: "chart" },
];

export function librarySort(sort: LibrarySort): { sort: LibrarySort; label: string; icon: IconName } {
  return LIBRARY_SORTS.find((entry) => entry.sort === sort) ?? LIBRARY_SORTS[0];
}

export function nextLibrarySort(sort: LibrarySort): LibrarySort {
  const index = LIBRARY_SORTS.findIndex((entry) => entry.sort === sort);
  return LIBRARY_SORTS[(index + 1) % LIBRARY_SORTS.length].sort;
}

/**
 * What the backend returns as a game's genre when no store published one.
 *
 * Epic and GOG both hand back entitlements with no genre at all
 * (`source_epic.rs`, `source_gog.rs`), so this is a common value, not a rare
 * one — and it is an absence wearing a label. Treating it as a real genre put
 * a "Library" pill on the hero, which told nobody anything.
 */
export const UNKNOWN_GENRE = "Library";

/** The game's genre when it has one, and the empty string when it does not. */
export function knownGenre(genre: string | undefined): string {
  const text = (genre ?? "").trim();
  return text === UNKNOWN_GENRE ? "" : text;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const WEEK = 7 * DAY;
const MONTH = 30 * DAY;
const YEAR = 365 * DAY;

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

/**
 * The libraries, in the order the Settings list connects them. A store game
 * stays its store's whatever starts it: an Epic game Winlator runs on a phone
 * is still an Epic game, and a Steam game that can also be streamed is still
 * a Steam game.
 */
const STORE_ORDER = [
  "steam",
  "epic",
  "gog",
  "ubisoft",
  "xbox",
  "microsoft-store",
  "instant-gaming",
  "android",
] as const;

const isStore = (value: string): boolean => (STORE_ORDER as readonly string[]).includes(value);

/**
 * The consoles a plugin can bring, newest family first. A game an emulator
 * plugin opens is filed under its console — Ryujinx adds `Switch`, PPSSPP adds
 * `PSP` — because that is the name a player has for those games, not the
 * emulator's.
 */
const CONSOLE_ORDER = ["switch", "psp", "gba", "gb", "snes", "nes", "megadrive"] as const;

type ConsoleKey = (typeof CONSOLE_ORDER)[number];

const CONSOLE_LABELS: Record<ConsoleKey, string> = {
  switch: "Switch",
  psp: "PSP",
  gba: "GBA",
  gb: "Game Boy",
  snes: "SNES",
  nes: "NES",
  megadrive: "Mega Drive",
};

const isConsole = (value: string): value is ConsoleKey =>
  (CONSOLE_ORDER as readonly string[]).includes(value);

/** A host from before `systems` still names the emulator; its console follows from it. */
const RUNNER_CONSOLES: ReadonlyMap<string, ConsoleKey> = new Map([
  ["com.orivo.ryujinx", "switch"],
  ["com.orivo.ppsspp", "psp"],
]);

/** Runners that stream the game from another machine. */
const STREAM_RUNNERS: ReadonlySet<string> = new Set(["com.orivo.gamestream"]);

/**
 * Runners that start a file from a folder on this machine. Their games are
 * the player's own folders, whatever compatibility layer opens them.
 */
const LOCAL_RUNNERS: ReadonlySet<string> = new Set(["com.orivo.wine-staging", "com.orivo.winlator"]);

/** The names Orivo gives a third-party runner it knows nothing about. */
const KNOWN_RUNNER_LABELS: ReadonlyMap<string, string> = new Map([["com.orivo.retroarch", "RetroArch"]]);

const LOCAL_SEGMENT = (): BrowseSegment => ({ id: "local", label: t("misc.source.local"), kind: "local" });
const STREAM_SEGMENT = (): BrowseSegment => ({ id: "stream", label: t("store.browse.stream"), kind: "stream" });

/**
 * Where a game comes from, as the bar files it. Every game lands somewhere:
 *
 * 1. A store library is its store — Steam, Epic, GOG… and the phone's own apps.
 * 2. A game a client streams from another machine (Moonlight, or a plugin
 *    profile set to stream) is `Stream`, however many hosts it comes from.
 * 3. A game a plugin opens is the plugin's console when it has one, and the
 *    plugin's own name when it does not.
 * 4. Everything else — a folder the player added, a Wine or Winlator entry —
 *    is `Local`.
 */
export function sourceOf(game: LibraryGame): BrowseSegment {
  const source = game.source ?? "";
  if (isStore(source)) {
    const badge = sourceBadge(game.source);
    if (badge) return { id: source, label: badge.label, kind: "store" };
  }

  const runner = game.runner ?? "";
  // The card's own way of starting is what the game is; a stream offered as a
  // second way does not make a local game a streamed one.
  if (STREAM_RUNNERS.has(runner) || (runner && game.launchOptions?.[0]?.remote === true)) {
    return STREAM_SEGMENT();
  }

  const system = (game.systems ?? []).find(isConsole) ?? RUNNER_CONSOLES.get(runner);
  if (system) return { id: system, label: CONSOLE_LABELS[system], kind: "console" };

  if (runner && !LOCAL_RUNNERS.has(runner)) {
    // A plugin's runner: its own name, as the profile the player set up calls
    // it, rather than lumping it in with what is local.
    const label = KNOWN_RUNNER_LABELS.get(runner) || game.launchOptions?.[0]?.label || t("store.browse.other");
    return { id: `runner:${runner}`, label, kind: "plugin" };
  }

  return LOCAL_SEGMENT();
}

const KIND_ORDER: readonly SourceKind[] = ["all", "store", "console", "plugin", "local", "stream"];

/** Where a segment sits in the row: by kind, then in its kind's own order. */
function rowPosition(segment: BrowseSegment): [number, number] {
  const kind = KIND_ORDER.indexOf(segment.kind);
  if (segment.kind === "store") return [kind, (STORE_ORDER as readonly string[]).indexOf(segment.id)];
  if (segment.kind === "console") return [kind, CONSOLE_ORDER.indexOf(segment.id as ConsoleKey)];
  return [kind, 0];
}

/**
 * The row: All Games, then every source this library actually holds — a
 * `PlayStation` tab with nothing behind it would be a promise the library
 * cannot keep. The order is fixed rather than by size, so a source keeps its
 * place in the row as the library grows: libraries, then what plugins bring,
 * then `Local`, then `Stream`.
 */
export function browseSegments(games: readonly LibraryGame[]): BrowseSegment[] {
  const found = new Map<string, BrowseSegment>();
  for (const game of games) {
    const source = sourceOf(game);
    if (!found.has(source.id)) found.set(source.id, source);
  }
  const sources = [...found.values()].sort((left, right) => {
    const [leftKind, leftIndex] = rowPosition(left);
    const [rightKind, rightIndex] = rowPosition(right);
    return leftKind - rightKind || leftIndex - rightIndex || left.label.localeCompare(right.label);
  });
  return [ALL_GAMES_SEGMENT, ...sources];
}

type Ranked = { game: LibraryGame; index: number; playedAt: number | null };

/** Most recent session first; a game never started goes after every one that was. */
function byRecency(left: Ranked, right: Ranked): number {
  if (left.playedAt === right.playedAt) return 0;
  if (left.playedAt === null) return 1;
  if (right.playedAt === null) return -1;
  return right.playedAt - left.playedAt;
}

const SORTS: Record<LibrarySort, (left: Ranked, right: Ranked) => number> = {
  recent: byRecency,
  // What has never been started, newest arrival first — the catalogue appends
  // what it imports, so a later place in it is a later arrival. Then what has
  // been touched least.
  new: (left, right) => {
    const leftNew = left.game.playTimeSeconds <= 0 && left.playedAt === null;
    const rightNew = right.game.playTimeSeconds <= 0 && right.playedAt === null;
    if (leftNew !== rightNew) return leftNew ? -1 : 1;
    if (leftNew) return right.index - left.index;
    return left.game.playTimeSeconds - right.game.playTimeSeconds || byRecency(left, right);
  },
  "most-played": (left, right) => right.game.playTimeSeconds - left.game.playTimeSeconds || byRecency(left, right),
};

/**
 * Put games in the order the player picked. Sorting is stable, so games that
 * tie keep the order the catalogue gave them.
 */
export function sortGames(games: readonly LibraryGame[], sort: LibrarySort, now = Date.now()): LibraryGame[] {
  const compare = SORTS[sort] ?? SORTS.recent;
  return games
    .map((game, index): Ranked => ({ game, index, playedAt: lastPlayedTime(game.lastPlayedAt, now) }))
    .sort(compare)
    .map((entry) => entry.game);
}

/** The rail for this segment: its source's games, in the order the player picked. */
export function browseGames(
  games: readonly LibraryGame[],
  segmentId: string,
  sort: LibrarySort,
  now = Date.now(),
): LibraryGame[] {
  const kept = segmentId === ALL_GAMES_SEGMENT.id ? games : games.filter((game) => sourceOf(game).id === segmentId);
  return sortGames(kept, sort, now);
}

/**
 * Which segment to show: the one last chosen while the library still holds it,
 * otherwise All Games.
 */
export function resolveSegment(segments: readonly BrowseSegment[], preferred: string | undefined): string {
  if (preferred && segments.some((segment) => segment.id === preferred)) return preferred;
  return segments[0]?.id ?? ALL_GAMES_SEGMENT.id;
}
