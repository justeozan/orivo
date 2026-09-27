import { describe, expect, it } from "vitest";

import {
  CONSOLE_EMULATORS,
  consoleReviewList,
  consoleReviewPrompt,
  consoleRomsToOffer,
  normaliseConsoleImportResult,
  normaliseConsoleRomFolder,
} from "./console-source";

const rom = (title: string, fileName = `${title}.nes`): Record<string, unknown> => ({
  gameRef: `rom:${title.toLowerCase()}`,
  title,
  systemLabel: "NES",
  fileName,
  folderPath: "",
  duplicateTitle: "none",
  alreadyImported: false,
});

const folder = (found: unknown[]): unknown => ({
  connected: true,
  emulator: "retroarch",
  emulatorLabel: "RetroArch",
  folderLabel: "Roms",
  found,
  message: "Connected Roms.",
});

describe("the emulators Orivo offers", () => {
  // The slug is the only thing the WebView may name, and the host refuses
  // anything outside its own closed set — so this list has to match it exactly.
  it("names each emulator by the slug the host knows", () => {
    expect(CONSOLE_EMULATORS.map((emulator) => emulator.slug)).toEqual(["retroarch", "ppsspp"]);
  });
});

describe("normaliseConsoleRomFolder", () => {
  it("reads a connected folder and what is in it", () => {
    const answer = normaliseConsoleRomFolder(folder([rom("Alter Ego")]));
    expect(answer).toEqual({
      connected: true,
      emulator: "retroarch",
      emulatorLabel: "RetroArch",
      folderLabel: "Roms",
      found: [
        {
          gameRef: "rom:alter ego",
          title: "Alter Ego",
          systemLabel: "NES",
          fileName: "Alter Ego.nes",
          folderPath: "",
          duplicateTitle: "none",
          alreadyImported: false,
        },
      ],
      message: "Connected Roms.",
    });
  });

  // The file name is what makes a planted file answerable, so an entry without
  // one is dropped rather than shown under a title it chose for itself.
  it("drops an entry it cannot read rather than offering a nameless one", () => {
    const answer = normaliseConsoleRomFolder(
      folder([
        rom("Alter Ego"),
        { ...rom("No reference"), gameRef: "" },
        { ...rom("No file name"), fileName: "" },
        { ...rom("No console"), systemLabel: 7 },
        "not a rom",
        null,
      ]),
    );
    expect(answer?.found.map((found) => found.title)).toEqual(["Alter Ego"]);
  });

  it("refuses an answer it cannot read rather than inventing a cheerful one", () => {
    for (const payload of [null, "connected", {}, { connected: true }, { connected: "yes", message: "x" }]) {
      expect(normaliseConsoleRomFolder(payload)).toBe(null);
    }
  });
});

describe("what the user is asked", () => {
  it("offers only the games that are not already cards", () => {
    const answer = normaliseConsoleRomFolder(
      folder([rom("Alter Ego"), { ...rom("Uwol"), alreadyImported: true }]),
    )!;
    expect(consoleRomsToOffer(answer).map((found) => found.title)).toEqual(["Alter Ego"]);
  });

  it("names the games instead of only counting them", () => {
    const answer = normaliseConsoleRomFolder(folder([rom("Alter Ego"), rom("Uwol")]))!;
    expect(consoleReviewPrompt(answer.found, "RetroArch")).toBe(
      "Add these 2 games to your library, to play in RetroArch?",
    );
    expect(consoleReviewPrompt(answer.found.slice(0, 1), "RetroArch")).toBe(
      "Add “Alter Ego” to your library, to play in RetroArch?",
    );
  });

  // A ROM's title is its file name, so the row has to say which file and which
  // console, and flag a name that is already somebody else's.
  it("names the console, the file and the folder holding it", () => {
    const answer = normaliseConsoleRomFolder(
      folder([
        rom("Alter Ego"),
        {
          ...rom("Alter Ego"),
          gameRef: "rom:planted",
          fileName: "Free Coins.nes",
          folderPath: "new",
          duplicateTitle: "folder",
        },
      ]),
    )!;
    const { entries } = consoleReviewList(answer.found, "Roms");
    expect(entries[0]!.origin).toBe("NES · Roms/Alter Ego.nes");
    expect(entries[1]!.origin).toBe("NES · Roms/new/Free Coins.nes");
    expect(entries[1]!.collision).toBe("Another file in this folder uses this name");
    expect(entries[0]!.collision).toBe(null);
  });

  it("keeps a long list readable without hiding that it is long", () => {
    const answer = normaliseConsoleRomFolder(
      folder(Array.from({ length: 9 }, (_, index) => rom(`Game ${index}`))),
    )!;
    const { entries, remaining } = consoleReviewList(answer.found, "Roms");
    expect(entries).toHaveLength(6);
    expect(remaining).toBe(3);
  });
});

describe("normaliseConsoleImportResult", () => {
  it("reads what was added", () => {
    expect(
      normaliseConsoleImportResult({ importedIds: ["runner:a", 7, ""], message: "One added." }),
    ).toEqual({ importedIds: ["runner:a"], message: "One added." });
  });

  it("refuses an answer it cannot read", () => {
    for (const payload of [null, "added", {}, { importedIds: ["a"] }]) {
      expect(normaliseConsoleImportResult(payload)).toBe(null);
    }
  });
});
