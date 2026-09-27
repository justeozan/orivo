import { describe, expect, it } from "vitest";

import {
  CONSOLE_EMULATORS,
  consoleEmulatorNote,
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
  token: 7,
  emulator: "retroarch",
  emulatorLabel: "RetroArch",
  emulatorInstalled: true,
  emulatorInstaller: "com.android.vending",
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
      token: 7,
      emulator: "retroarch",
      emulatorLabel: "RetroArch",
      emulatorInstalled: true,
      emulatorInstaller: "com.android.vending",
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
    // The planted one leads, because its row is the one that has to be read.
    expect(entries[0]!.origin).toBe("NES · Roms/new/Free Coins.nes");
    expect(entries[0]!.collision).toBe("Another file in this folder uses this name");
    expect(entries[1]!.origin).toBe("NES · Roms/Alter Ego.nes");
    expect(entries[1]!.collision).toBe(null);
  });

  // The button under this list imports everything in it, so a list that stopped
  // at six would be asking the user to vouch for what it never showed — and the
  // host orders its answer by a hash of the pathname, so which six they saw would
  // be a draw. The list is complete and scrollable, and the rows that exist to be
  // read come first.
  it("shows every game it would import, not the first few", () => {
    const answer = normaliseConsoleRomFolder(
      folder(Array.from({ length: 41 }, (_, index) => rom(`Game ${index}`))),
    )!;
    const { entries } = consoleReviewList(answer.found, "Roms");
    expect(entries).toHaveLength(41);
  });

  it("puts the names that collide at the top, whatever the host's order was", () => {
    const answer = normaliseConsoleRomFolder(
      folder([
        rom("Zelda"),
        { ...rom("Pokemon Emerald"), gameRef: "rom:planted", duplicateTitle: "library" },
        rom("Alter Ego"),
        { ...rom("Uwol"), gameRef: "rom:twin", duplicateTitle: "folder" },
      ]),
    )!;
    const { entries } = consoleReviewList(answer.found, "Roms");
    expect(entries.map((entry) => entry.title)).toEqual([
      "Pokemon Emerald",
      "Uwol",
      "Alter Ego",
      "Zelda",
    ]);
  });
});

describe("what the host said about the emulator itself", () => {
  it("names who installed it, because that is the app the game goes to", () => {
    expect(
      consoleEmulatorNote({
        emulatorLabel: "RetroArch",
        emulatorInstalled: true,
        emulatorInstaller: "com.android.vending",
      }),
    ).toBe("RetroArch on this device was installed by com.android.vending.");
    expect(
      consoleEmulatorNote({
        emulatorLabel: "RetroArch",
        emulatorInstalled: true,
        emulatorInstaller: null,
      }),
    ).toBe("RetroArch on this device was installed by hand, not from a store.");
  });

  // Silence from the platform is not "the app is missing", and saying so would be
  // a sentence the user cannot act on.
  it("says nothing when the platform said nothing", () => {
    expect(
      consoleEmulatorNote({
        emulatorLabel: "PPSSPP",
        emulatorInstalled: false,
        emulatorInstaller: null,
      }),
    ).toBe(null);
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
