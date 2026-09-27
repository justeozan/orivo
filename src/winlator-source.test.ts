import { describe, expect, it } from "vitest";

import {
  isWinlatorHost,
  normaliseWinlatorExportFolder,
  normaliseWinlatorImportResult,
  winlatorReviewList,
  winlatorReviewPrompt,
  winlatorShortcutsToOffer,
  winlatorWaitingToast,
  type WinlatorShortcut,
} from "./winlator-source";

const shortcut = (title: string): WinlatorShortcut => ({
  gameRef: `shortcut:${title.toLowerCase()}`,
  title,
  fileName: `${title}.desktop`,
  folderPath: "",
  duplicateTitle: "none",
  alreadyImported: false,
});

const folder = (found: unknown[]): unknown => ({
  connected: true,
  folderLabel: "Frontend",
  found,
  message: "Connected Frontend.",
});

describe("isWinlatorHost", () => {
  it("offers the entry point on Android and nowhere else", () => {
    expect(
      isWinlatorHost(
        "Mozilla/5.0 (Linux; Android 17; sdk_gphone64_arm64) AppleWebKit/537.36 (KHTML, like Gecko) Version/4.0 Chrome/140.0 Mobile Safari/537.36",
      ),
    ).toBe(true);
    // A Mac, a PC and a Linux desktop all run Orivo and none of them run Winlator.
    for (const userAgent of [
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15",
      "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36",
      "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36",
    ]) {
      expect(isWinlatorHost(userAgent)).toBe(false);
    }
  });
});

describe("normaliseWinlatorExportFolder", () => {
  it("reads a connected folder and what is in it", () => {
    expect(
      normaliseWinlatorExportFolder(
        folder([
          {
            gameRef: "shortcut:aa",
            title: "Celeste",
            fileName: "Celeste.desktop",
            folderPath: "",
            duplicateTitle: "none",
            alreadyImported: false,
          },
          {
            gameRef: "shortcut:bb",
            title: "Braid",
            fileName: "Braid.desktop",
            folderPath: "new",
            duplicateTitle: "library",
            alreadyImported: true,
          },
        ]),
      ),
    ).toEqual({
      connected: true,
      folderLabel: "Frontend",
      found: [
        {
          gameRef: "shortcut:aa",
          title: "Celeste",
          fileName: "Celeste.desktop",
          folderPath: "",
          duplicateTitle: "none",
          alreadyImported: false,
        },
        {
          gameRef: "shortcut:bb",
          title: "Braid",
          fileName: "Braid.desktop",
          folderPath: "new",
          duplicateTitle: "library",
          alreadyImported: true,
        },
      ],
      message: "Connected Frontend.",
    });
  });

  // The file name is what makes a planted shortcut answerable, so an entry that
  // arrives without one is dropped rather than shown under its own `Name=`.
  it("drops a shortcut it cannot read rather than offering a nameless one", () => {
    const answer = normaliseWinlatorExportFolder(
      folder([
        { gameRef: "shortcut:aa", title: "Celeste", fileName: "Celeste.desktop" },
        { gameRef: "", title: "No reference", fileName: "x.desktop" },
        { gameRef: "shortcut:cc", title: "", fileName: "x.desktop" },
        { gameRef: "shortcut:dd", title: "No file name" },
        { gameRef: "shortcut:ee", title: "Empty file name", fileName: "" },
        "not a shortcut",
        null,
      ]),
    );
    expect(answer?.found).toEqual([
      {
        gameRef: "shortcut:aa",
        title: "Celeste",
        fileName: "Celeste.desktop",
        folderPath: "",
        duplicateTitle: "none",
        alreadyImported: false,
      },
    ]);
  });

  it("reads an unknown collision as no collision rather than as markup", () => {
    const answer = normaliseWinlatorExportFolder(
      folder([
        {
          gameRef: "shortcut:aa",
          title: "Celeste",
          fileName: "Celeste.desktop",
          duplicateTitle: "<b>library</b>",
        },
      ]),
    );
    expect(answer?.found[0]!.duplicateTitle).toBe("none");
  });

  it("refuses an answer it cannot read rather than inventing a cheerful one", () => {
    for (const payload of [null, "connected", {}, { connected: "yes", message: "hello" }, { connected: true }]) {
      expect(normaliseWinlatorExportFolder(payload)).toBe(null);
    }
  });
});

describe("what the user is asked", () => {
  it("offers only the shortcuts that are not already cards", () => {
    const answer = normaliseWinlatorExportFolder(
      folder([
        { ...shortcut("Celeste"), alreadyImported: false },
        { ...shortcut("Braid"), alreadyImported: true },
      ]),
    )!;
    expect(winlatorShortcutsToOffer(answer).map((shortcut) => shortcut.title)).toEqual(["Celeste"]);
  });

  // A `.desktop` file runs a command, so the confirmation names what it would
  // add: a count alone hides the one thing the user is vouching for.
  it("names the games instead of only counting them", () => {
    const shortcuts = [shortcut("Celeste"), shortcut("Braid")];
    expect(winlatorReviewPrompt(shortcuts)).toBe("Add these 2 Winlator games to your library?");
    expect(winlatorReviewPrompt(shortcuts.slice(0, 1))).toBe("Add “Celeste” to your library?");
    // Sorted by title once nothing collides, so the same folder always reads the
    // same way whatever order the host answered in.
    expect(winlatorReviewList(shortcuts, "Frontend").entries.map((entry) => entry.title)).toEqual([
      "Braid",
      "Celeste",
    ]);
  });

  // A `Name=` line is not an identity: a file dropped into the folder can carry
  // the name of a game the user already has. What they can check is the file and
  // the folder holding it, so that is what the list says.
  it("names the file and the folder holding it, not only the title", () => {
    const entries = winlatorReviewList(
      [
        shortcut("Celeste"),
        { ...shortcut("Celeste"), gameRef: "shortcut:planted", fileName: "Free Coins.desktop", folderPath: "new" },
      ],
      "Frontend",
    ).entries;
    expect(entries[0]!.origin).toBe("Frontend/Celeste.desktop");
    expect(entries[1]!.origin).toBe("Frontend/new/Free Coins.desktop");
  });

  // The list is ordered by how much the row needs reading, so the library
  // collision leads and the untouched name is last.
  it("says whose name a shortcut is reusing", () => {
    const entries = winlatorReviewList(
      [
        { ...shortcut("Celeste"), duplicateTitle: "folder" },
        { ...shortcut("Braid"), gameRef: "shortcut:b", duplicateTitle: "library" },
        { ...shortcut("Doom"), gameRef: "shortcut:c" },
      ],
      "Frontend",
    ).entries;
    expect(entries.map((entry) => entry.title)).toEqual(["Braid", "Celeste", "Doom"]);
    expect(entries[0]!.collision).toBe("A game already in your library uses this name");
    expect(entries[1]!.collision).toBe("Another file in this folder uses this name");
    expect(entries[2]!.collision).toBe(null);
  });

  it("falls back to the connected folder's own name when the host could not give one", () => {
    const entries = winlatorReviewList([shortcut("Celeste")], null).entries;
    expect(entries[0]!.origin).toBe("Celeste.desktop");
  });

  // "Add these 41 games" has to mean the 41 rows above it. A list that stopped at
  // six was asking the user to vouch for 35 files it never showed them.
  it("shows every shortcut it would import, not the first few", () => {
    const shortcuts = Array.from({ length: 41 }, (_, index) => ({
      ...shortcut(`Game ${index}`),
      gameRef: `shortcut:${index}`,
    }));
    const { entries } = winlatorReviewList(shortcuts, "Frontend");
    expect(entries).toHaveLength(41);
  });

  it("puts the names that collide at the top", () => {
    const entries = winlatorReviewList(
      [
        shortcut("Zelda"),
        { ...shortcut("Celeste"), gameRef: "shortcut:planted", duplicateTitle: "library" },
        shortcut("Alter Ego"),
      ],
      "Frontend",
    ).entries;
    expect(entries.map((entry) => entry.title)).toEqual(["Celeste", "Alter Ego", "Zelda"]);
  });
});

describe("the background pass", () => {
  it("invites the user to look instead of announcing a change", () => {
    expect(winlatorWaitingToast({ pending: 2, changed: 0 })).toBe(
      "Winlator has 2 new shortcuts. Open Sources to review them.",
    );
    expect(winlatorWaitingToast({ pending: 1, changed: 1 })).toBe(
      "Winlator has one new shortcut and one that changed. Open Sources to review them.",
    );
  });

  it("stays silent when there is nothing waiting", () => {
    for (const payload of [null, {}, { pending: 0, changed: 0 }, { pending: -1 }, { pending: "2" }]) {
      expect(winlatorWaitingToast(payload)).toBe(null);
    }
  });
});

describe("normaliseWinlatorImportResult", () => {
  it("reads what was added", () => {
    expect(
      normaliseWinlatorImportResult({ importedIds: ["runner:a", 7, ""], message: "One added." }),
    ).toEqual({ importedIds: ["runner:a"], message: "One added." });
  });

  it("refuses an answer it cannot read", () => {
    for (const payload of [null, "added", {}, { importedIds: ["a"] }]) {
      expect(normaliseWinlatorImportResult(payload)).toBe(null);
    }
  });
});
