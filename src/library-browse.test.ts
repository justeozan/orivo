import { describe, expect, it } from "vitest";
import type { LibraryGame } from "./mock-library";
import {
  ALL_GAMES_SEGMENT,
  LIBRARY_SORTS,
  browseGames,
  browseSegments,
  lastPlayedTime,
  nextLibrarySort,
  resolveSegment,
  sortGames,
  sourceOf,
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

const ids = (games: readonly LibraryGame[]): string[] => games.map((entry) => entry.id);

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

describe("nextLibrarySort", () => {
  it("cycles Recent, New, Most played and comes back round", () => {
    expect(LIBRARY_SORTS.map((entry) => entry.sort)).toEqual(["recent", "new", "most-played"]);
    expect(nextLibrarySort("recent")).toBe("new");
    expect(nextLibrarySort("new")).toBe("most-played");
    expect(nextLibrarySort("most-played")).toBe("recent");
  });
});

describe("sourceOf", () => {
  it("files a store game under its store, whatever starts it", () => {
    expect(sourceOf(game({ id: "a", source: "steam" }))).toMatchObject({ id: "steam", label: "Steam", kind: "store" });
    // An Epic game Winlator runs on a phone is still an Epic game.
    expect(sourceOf(game({ id: "b", source: "epic", runner: "com.orivo.winlator" }))).toMatchObject({
      id: "epic",
      kind: "store",
    });
    // A Steam game that can also be streamed is still a Steam game.
    expect(
      sourceOf(
        game({
          id: "c",
          source: "steam",
          launchOptions: [
            { id: "steam", label: "Steam", launchable: true, remote: false },
            { id: "stream", label: "Gaming PC", launchable: true, remote: true },
          ],
        }),
      ).id,
    ).toBe("steam");
    expect(sourceOf(game({ id: "d", source: "android" })).id).toBe("android");
  });

  it("puts every streamed game under one Stream, whichever client streams it", () => {
    expect(sourceOf(game({ id: "a", source: "local", runner: "com.orivo.gamestream" }))).toMatchObject({
      id: "stream",
      kind: "stream",
    });
    expect(
      sourceOf(
        game({
          id: "b",
          source: "local",
          runner: "com.example.cloud",
          launchOptions: [{ id: "x", label: "Cloud PC", launchable: true, remote: true }],
        }),
      ).id,
    ).toBe("stream");
  });

  it("files what a console plugin opens under the console", () => {
    expect(sourceOf(game({ id: "a", source: "local", runner: "com.orivo.ryujinx", systems: ["switch"] }))).toMatchObject({
      id: "switch",
      label: "Switch",
      kind: "console",
    });
    expect(sourceOf(game({ id: "b", source: "local", runner: "com.orivo.retroarch", systems: ["snes"] })).id).toBe("snes");
    // A host from before `systems` still names the emulator.
    expect(sourceOf(game({ id: "c", source: "local", runner: "com.orivo.ppsspp" })).id).toBe("psp");
  });

  it("names a plugin with no console by its own name", () => {
    expect(
      sourceOf(
        game({
          id: "a",
          source: "local",
          runner: "com.example.dolphin",
          launchOptions: [{ id: "x", label: "Dolphin", launchable: true, remote: false }],
        }),
      ),
    ).toMatchObject({ id: "runner:com.example.dolphin", label: "Dolphin", kind: "plugin" });
  });

  it("gathers every folder the player added under Local, Wine and Winlator included", () => {
    for (const entry of [
      game({ id: "a", source: "local" }),
      game({ id: "b", source: "wine" }),
      game({ id: "c", source: "wine", runner: "com.orivo.wine-staging" }),
      game({ id: "d", source: "local", runner: "com.orivo.winlator" }),
      game({ id: "e" }),
    ]) {
      expect(sourceOf(entry)).toMatchObject({ id: "local", kind: "local" });
    }
  });
});

describe("browseSegments", () => {
  it("leads with All Games and lists only the sources the library holds", () => {
    const library = [game({ id: "a", source: "steam" }), game({ id: "b", source: "steam" })];
    expect(browseSegments(library).map((segment) => segment.id)).toEqual(["all", "steam"]);
  });

  it("keeps a fixed order: libraries, consoles, plugins, Local, Stream", () => {
    const library = [
      game({ id: "stream", source: "local", runner: "com.orivo.gamestream" }),
      game({ id: "folder", source: "local" }),
      game({ id: "plugin", source: "local", runner: "com.example.mame", launchOptions: [{ id: "x", label: "MAME", launchable: true, remote: false }] }),
      game({ id: "snes", source: "local", runner: "com.orivo.retroarch", systems: ["snes"] }),
      game({ id: "switch", source: "local", runner: "com.orivo.ryujinx", systems: ["switch"] }),
      game({ id: "gog", source: "gog" }),
      game({ id: "epic", source: "epic" }),
      game({ id: "steam", source: "steam" }),
    ];
    expect(browseSegments(library).map((segment) => segment.id)).toEqual([
      "all",
      "steam",
      "epic",
      "gog",
      "switch",
      "snes",
      "runner:com.example.mame",
      "local",
      "stream",
    ]);
  });

  it("still offers All Games for an empty library", () => {
    expect(browseSegments([])).toEqual([ALL_GAMES_SEGMENT]);
  });
});

describe("sortGames", () => {
  const library = [
    game({ id: "old-favourite", lastPlayedAt: "3 weeks ago", playTimeSeconds: 200 * 3_600 }),
    game({ id: "yesterday", lastPlayedAt: "yesterday", playTimeSeconds: 2 * 3_600 }),
    game({ id: "unplayed-first" }),
    game({ id: "barely", lastPlayedAt: "2 days ago", playTimeSeconds: 600 }),
    game({ id: "unplayed-latest" }),
  ];

  it("puts the latest session first under Recent, and what was never started last", () => {
    expect(ids(sortGames(library, "recent", NOW))).toEqual([
      "yesterday",
      "barely",
      "old-favourite",
      "unplayed-first",
      "unplayed-latest",
    ]);
  });

  it("puts what was never started first under New, newest arrival first, then the least played", () => {
    expect(ids(sortGames(library, "new", NOW))).toEqual([
      "unplayed-latest",
      "unplayed-first",
      "barely",
      "yesterday",
      "old-favourite",
    ]);
  });

  it("ranks Most played by the clock, not by recency", () => {
    expect(ids(sortGames(library, "most-played", NOW))).toEqual([
      "old-favourite",
      "yesterday",
      "barely",
      "unplayed-first",
      "unplayed-latest",
    ]);
  });

  it("leaves an already-ordered library exactly as it was", () => {
    const ordered = [
      game({ id: "a", lastPlayedAt: "2 days ago" }),
      game({ id: "b", lastPlayedAt: "1 week ago" }),
      game({ id: "c" }),
      game({ id: "d" }),
    ];
    expect(ids(sortGames(ordered, "recent", NOW))).toEqual(["a", "b", "c", "d"]);
  });
});

describe("browseGames", () => {
  const library = [
    game({ id: "steam-old", source: "steam", lastPlayedAt: "1 week ago", playTimeSeconds: 90 * 3_600 }),
    game({ id: "folder", source: "local", lastPlayedAt: "2 days ago", playTimeSeconds: 3_600 }),
    game({ id: "steam-new", source: "steam", lastPlayedAt: "yesterday", playTimeSeconds: 3 * 3_600 }),
  ];

  it("shows the whole library on All Games, in the order picked", () => {
    expect(ids(browseGames(library, ALL_GAMES_SEGMENT.id, "recent", NOW))).toEqual(["steam-new", "folder", "steam-old"]);
  });

  it("keeps only the source's games, in the order picked", () => {
    expect(ids(browseGames(library, "steam", "recent", NOW))).toEqual(["steam-new", "steam-old"]);
    expect(ids(browseGames(library, "steam", "most-played", NOW))).toEqual(["steam-old", "steam-new"]);
    expect(ids(browseGames(library, "local", "recent", NOW))).toEqual(["folder"]);
  });
});

describe("resolveSegment", () => {
  const segments = browseSegments([game({ id: "a", source: "steam" }), game({ id: "b", source: "local" })]);

  it("keeps the remembered source while the library still holds it", () => {
    expect(resolveSegment(segments, "local")).toBe("local");
  });

  it("falls back to All Games when the remembered source is gone", () => {
    expect(resolveSegment(segments, "epic")).toBe("all");
    expect(resolveSegment(segments, undefined)).toBe("all");
  });
});
