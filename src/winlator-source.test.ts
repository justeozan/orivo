import { describe, expect, it } from "vitest";

import {
  isWinlatorHost,
  normaliseWinlatorExportFolder,
  normaliseWinlatorImportResult,
  winlatorReviewList,
  winlatorReviewPrompt,
  winlatorShortcutsToOffer,
  winlatorWaitingToast,
} from "./winlator-source";

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
          { gameRef: "shortcut:aa", title: "Celeste", alreadyImported: false },
          { gameRef: "shortcut:bb", title: "Braid", alreadyImported: true },
        ]),
      ),
    ).toEqual({
      connected: true,
      folderLabel: "Frontend",
      found: [
        { gameRef: "shortcut:aa", title: "Celeste", alreadyImported: false },
        { gameRef: "shortcut:bb", title: "Braid", alreadyImported: true },
      ],
      message: "Connected Frontend.",
    });
  });

  it("drops a shortcut it cannot read rather than offering a nameless one", () => {
    const answer = normaliseWinlatorExportFolder(
      folder([
        { gameRef: "shortcut:aa", title: "Celeste" },
        { gameRef: "", title: "No reference" },
        { gameRef: "shortcut:cc", title: "" },
        "not a shortcut",
        null,
      ]),
    );
    expect(answer?.found).toEqual([
      { gameRef: "shortcut:aa", title: "Celeste", alreadyImported: false },
    ]);
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
        { gameRef: "shortcut:aa", title: "Celeste", alreadyImported: false },
        { gameRef: "shortcut:bb", title: "Braid", alreadyImported: true },
      ]),
    )!;
    expect(winlatorShortcutsToOffer(answer).map((shortcut) => shortcut.title)).toEqual(["Celeste"]);
  });

  // A `.desktop` file runs a command, so the confirmation names what it would
  // add: a count alone hides the one thing the user is vouching for.
  it("names the games instead of only counting them", () => {
    const shortcuts = [
      { gameRef: "a", title: "Celeste", alreadyImported: false },
      { gameRef: "b", title: "Braid", alreadyImported: false },
    ];
    expect(winlatorReviewPrompt(shortcuts)).toBe("Add these 2 Winlator games to your library?");
    expect(winlatorReviewPrompt(shortcuts.slice(0, 1))).toBe("Add “Celeste” to your library?");
    expect(winlatorReviewList(shortcuts).titles).toEqual(["Celeste", "Braid"]);
  });

  it("keeps a long list readable without hiding that it is long", () => {
    const shortcuts = Array.from({ length: 9 }, (_, index) => ({
      gameRef: `shortcut:${index}`,
      title: `Game ${index}`,
      alreadyImported: false,
    }));
    const { titles, remaining } = winlatorReviewList(shortcuts);
    expect(titles).toHaveLength(6);
    expect(remaining).toBe(3);
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
