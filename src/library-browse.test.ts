import { describe, expect, it } from "vitest";
import type { LibraryGame } from "./mock-library";
import {
  ALL_GAMES_SEGMENT,
  MAX_BROWSE_SEGMENTS,
  browseGames,
  browseSegments,
  lastPlayedTime,
  nextBrowseMode,
  resolveSegment,
} from "./library-browse";

const NOW = Date.parse("2026-08-06T12:00:00Z");

function game(overrides: Partial<LibraryGame> & { id: string }): LibraryGame {
  return {
    title: overrides.id,
    description: "",
    metadata: "",
    genre: "RPG",
    heroUrl: "",
    coverUrl: "",
    landscapeUrl: "",
    lastPlayedAt: "",
    playTimeSeconds: 0,
    launchable: true,
    ...overrides,
  };
}

describe("lastPlayedTime", () => {
  it("reads both dialects the field arrives in", () => {
    expect(lastPlayedTime("2024-02-26T19:22:09.4406448Z", NOW)).toBe(
      Date.parse("2024-02-26T19:22:09.440Z"),
    );
    expect(lastPlayedTime("2 days ago", NOW)).toBe(NOW - 2 * 86_400_000);
    expect(lastPlayedTime("1 week ago", NOW)).toBe(NOW - 7 * 86_400_000);
    expect(lastPlayedTime("yesterday", NOW)).toBe(NOW - 86_400_000);
    expect(lastPlayedTime("today", NOW)).toBe(NOW);
  });

  it("has no answer for an empty or unreadable value", () => {
    expect(lastPlayedTime("", NOW)).toBeNull();
    expect(lastPlayedTime(undefined, NOW)).toBeNull();
    expect(lastPlayedTime("whenever", NOW)).toBeNull();
  });
});

describe("nextBrowseMode", () => {
  it("cycles the four modes and comes back round", () => {
    expect(nextBrowseMode("activity")).toBe("genre");
    expect(nextBrowseMode("genre")).toBe("source");
    expect(nextBrowseMode("source")).toBe("platform");
    expect(nextBrowseMode("platform")).toBe("activity");
  });
});

describe("browseSegments", () => {
  const library = [
    game({ id: "a", genre: "RPG", source: "steam", lastPlayedAt: "2 days ago", playTimeSeconds: 40 * 3_600, supportedPlatforms: ["macos"] }),
    game({ id: "b", genre: "Action", source: "xbox", lastPlayedAt: "1 week ago", playTimeSeconds: 9 * 3_600 }),
    game({ id: "c", genre: "RPG", source: "steam", playTimeSeconds: 0, supportedPlatforms: ["macos", "windows"] }),
  ];

  it("offers every activity shelf that holds something", () => {
    expect(browseSegments(library, "activity", NOW).map((segment) => segment.id)).toEqual([
      "recent",
      "most-played",
      "play-next",
      "resume",
      "never",
    ]);
  });

  it("drops an activity shelf with nothing on it", () => {
    const allPlayed = [game({ id: "a", playTimeSeconds: 40 * 3_600, lastPlayedAt: "2 days ago" })];
    expect(browseSegments(allPlayed, "activity", NOW).map((segment) => segment.id)).toEqual([
      "recent",
      "most-played",
      "resume",
    ]);
  });

  it("derives genres busiest first", () => {
    expect(browseSegments(library, "genre", NOW)).toEqual([
      { id: "RPG", label: "RPG" },
      { id: "Action", label: "Action" },
    ]);
  });

  it("names sources the way the rest of the app names them", () => {
    expect(browseSegments(library, "source", NOW)).toEqual([
      { id: "steam", label: "Steam" },
      { id: "xbox", label: "Xbox" },
    ]);
  });

  it("keeps platforms in their own order, native first", () => {
    expect(browseSegments(library, "platform", NOW)).toEqual([
      { id: "macos", label: "Mac" },
      { id: "windows", label: "Windows" },
    ]);
  });

  it("files a game by the macOS answer its store gave", () => {
    // The bug this covers: an Epic or GOG library declares no
    // `supportedPlatforms`, so Platform mode offered nothing but "All Games"
    // even though every one of those games already carried a per-game answer
    // about its macOS build — the same one that greys out the Play button.
    const connected = [
      game({ id: "windows-only", source: "epic", macCompatibility: "not-native" }),
      game({ id: "mac-native", source: "epic", macCompatibility: "native" }),
    ];
    expect(browseSegments(connected, "platform", NOW)).toEqual([
      { id: "macos", label: "Mac" },
      { id: "windows", label: "Windows" },
    ]);
    expect(browseGames(connected, "platform", "windows", NOW).map((entry) => entry.id)).toEqual([
      "windows-only",
    ]);
    expect(browseGames(connected, "platform", "macos", NOW).map((entry) => entry.id)).toEqual([
      "mac-native",
    ]);
  });

  it("does not file a Linux-only game under Windows", () => {
    // GOG sells Linux-only titles. Those honestly have no macOS build, so the
    // macOS answer is "not-native" — but reading that beside a real platform
    // matrix invented a Windows build that does not exist.
    const linuxOnly = [
      game({
        id: "linux-only",
        source: "gog",
        supportedPlatforms: ["linux"],
        macCompatibility: "not-native",
      }),
    ];
    expect(browseSegments(linuxOnly, "platform", NOW)).toEqual([{ id: "linux", label: "Linux" }]);
    expect(browseGames(linuxOnly, "platform", "windows", NOW)).toEqual([]);
  });

  it("still reads the macOS answer for a catalog synced before stores recorded a matrix", () => {
    // An entry from an older sync carries the macOS answer and nothing else.
    // Dropping the fallback would empty Platform mode again for those users
    // until they happened to re-sync.
    const legacy = [game({ id: "legacy-epic", source: "epic", macCompatibility: "not-native" })];
    expect(browseSegments(legacy, "platform", NOW)).toEqual([{ id: "windows", label: "Windows" }]);
  });

  it("leaves a game out of Platform when no store ever answered", () => {
    // "unknown" is an absent answer, not a "no". Guessing here would file
    // every unanswered game under Windows and make the mode a lie.
    const silent = [game({ id: "quiet", source: "gog", macCompatibility: "unknown" })];
    expect(browseSegments(silent, "platform", NOW)).toEqual([{ id: "all", label: "All Games" }]);
  });

  it("treats the backend's unknown-genre filler as no genre at all", () => {
    // `genre_for_game` in lib.rs returns "Library" when nothing published one,
    // which is every Epic and GOG entitlement. A "Library" shelf beside "RPG"
    // reads as a real category and is not one.
    const unlabelled = [
      game({ id: "known", genre: "RPG" }),
      game({ id: "filler", genre: "Library" }),
    ];
    expect(browseSegments(unlabelled, "genre", NOW)).toEqual([{ id: "RPG", label: "RPG" }]);
    expect(browseGames(unlabelled, "genre", "RPG", NOW).map((entry) => entry.id)).toEqual(["known"]);
  });

  it("ignores the host OS, which the backend stamps on every game alike", () => {
    // hostPlatform is the machine running Orivo, not the game's build. Reading
    // it would file a Windows-only Xbox title under Apple on a Mac.
    const hosted = [
      game({ id: "xbox-title", source: "xbox", hostPlatform: "macos" }),
      game({ id: "steam-title", source: "steam", hostPlatform: "macos" }),
    ];
    expect(browseSegments(hosted, "platform", NOW)).toEqual([ALL_GAMES_SEGMENT]);
  });

  it("counts a Wine entry as the Windows build it is", () => {
    const wine = [game({ id: "wine-title", source: "wine" })];
    expect(browseSegments(wine, "platform", NOW)).toEqual([{ id: "windows", label: "Windows" }]);
    expect(browseGames(wine, "platform", "windows", NOW).map((entry) => entry.id)).toEqual([
      "wine-title",
    ]);
  });

  it("shows only the busiest segments, so the row stays readable", () => {
    const many = Array.from({ length: 20 }, (_, index) =>
      game({ id: `g${index}`, genre: `Genre ${String(index).padStart(2, "0")}` }),
    );
    // One genre carries two games, so it must lead the row it is capped into.
    many.push(game({ id: "extra", genre: "Genre 19" }));
    const segments = browseSegments(many, "genre", NOW);
    expect(segments).toHaveLength(MAX_BROWSE_SEGMENTS);
    expect(segments[0].id).toBe("Genre 19");
  });

  it("never leaves the bar empty when a library has nothing to divide by", () => {
    const bare = [game({ id: "a", genre: "" })];
    expect(browseSegments(bare, "genre", NOW)).toEqual([ALL_GAMES_SEGMENT]);
    expect(browseSegments(bare, "source", NOW)).toEqual([ALL_GAMES_SEGMENT]);
    expect(browseSegments(bare, "platform", NOW)).toEqual([ALL_GAMES_SEGMENT]);
    expect(browseSegments([], "genre", NOW)).toEqual([ALL_GAMES_SEGMENT]);
  });
});

describe("browseGames", () => {
  const library = [
    game({ id: "recent-heavy", lastPlayedAt: "2 days ago", playTimeSeconds: 40 * 3_600 }),
    game({ id: "old-heaviest", lastPlayedAt: "3 weeks ago", playTimeSeconds: 90 * 3_600 }),
    game({ id: "fresh", lastPlayedAt: "", playTimeSeconds: 0 }),
    game({ id: "started", lastPlayedAt: "1 week ago", playTimeSeconds: 600 }),
  ];

  it("keeps the whole library on the default shelf, newest first", () => {
    // The Library must never open on an empty rail, so the default only sorts.
    expect(browseGames(library, "activity", "recent", NOW).map((entry) => entry.id)).toEqual([
      "recent-heavy",
      "started",
      "old-heaviest",
      "fresh",
    ]);
  });

  it("leaves an already-ordered library exactly as it was", () => {
    const ordered = [
      game({ id: "one", lastPlayedAt: "2 days ago" }),
      game({ id: "two", lastPlayedAt: "1 week ago" }),
      game({ id: "three", lastPlayedAt: "1 week ago" }),
      game({ id: "four", lastPlayedAt: "2 months ago" }),
    ];
    expect(browseGames(ordered, "activity", "recent", NOW).map((entry) => entry.id)).toEqual([
      "one",
      "two",
      "three",
      "four",
    ]);
  });

  it("ranks Most Played by the clock, not by recency", () => {
    expect(browseGames(library, "activity", "most-played", NOW).map((entry) => entry.id)).toEqual([
      "old-heaviest",
      "recent-heavy",
      "started",
      "fresh",
    ]);
  });

  it("splits what is waiting from what is under way", () => {
    expect(browseGames(library, "activity", "play-next", NOW).map((entry) => entry.id)).toEqual([
      "fresh",
      "started",
    ]);
    expect(browseGames(library, "activity", "resume", NOW).map((entry) => entry.id)).toEqual([
      "recent-heavy",
      "old-heaviest",
    ]);
    expect(browseGames(library, "activity", "never", NOW).map((entry) => entry.id)).toEqual([
      "fresh",
    ]);
  });

  it("filters the derived modes and sorts what is left by recency", () => {
    const mixed = [
      game({ id: "rpg-old", genre: "RPG", source: "steam", lastPlayedAt: "1 month ago" }),
      game({ id: "action", genre: "Action", source: "xbox", lastPlayedAt: "1 day ago" }),
      game({ id: "rpg-new", genre: "RPG", source: "steam", lastPlayedAt: "1 day ago" }),
    ];
    expect(browseGames(mixed, "genre", "RPG", NOW).map((entry) => entry.id)).toEqual([
      "rpg-new",
      "rpg-old",
    ]);
    expect(browseGames(mixed, "source", "xbox", NOW).map((entry) => entry.id)).toEqual(["action"]);
  });

  it("matches a game on every platform it declares", () => {
    const cross = [
      game({ id: "mac-only", supportedPlatforms: ["macos"] }),
      game({ id: "both", supportedPlatforms: ["macos", "windows"] }),
    ];
    expect(browseGames(cross, "platform", "macos", NOW).map((entry) => entry.id)).toEqual([
      "both",
      "mac-only",
    ]);
    expect(browseGames(cross, "platform", "windows", NOW).map((entry) => entry.id)).toEqual([
      "both",
    ]);
  });

  it("shows everything on the fallback segment", () => {
    expect(browseGames(library, "genre", ALL_GAMES_SEGMENT.id, NOW)).toHaveLength(library.length);
  });
});

describe("resolveSegment", () => {
  const segments = [
    { id: "recent", label: "Recently Played" },
    { id: "resume", label: "Resume" },
  ];

  it("keeps the remembered segment while it still exists", () => {
    expect(resolveSegment(segments, "resume")).toBe("resume");
  });

  it("falls back to the first when the remembered one is gone", () => {
    // A library refresh can retire a shelf, and the bar must not point at it.
    expect(resolveSegment(segments, "never")).toBe("recent");
    expect(resolveSegment(segments, undefined)).toBe("recent");
    expect(resolveSegment([], "recent")).toBe(ALL_GAMES_SEGMENT.id);
  });
});
