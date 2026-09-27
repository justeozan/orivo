import { describe, expect, it } from "vitest";

import {
  isWinlatorHost,
  normaliseWinlatorExportFolder,
  readWinlatorAdoptedCount,
  winlatorAdoptionToast,
} from "./winlator-source";

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
  it("reads a connected folder", () => {
    expect(
      normaliseWinlatorExportFolder({
        connected: true,
        folderLabel: "Frontend",
        adopted: 2,
        message: "Connected Frontend. 2 Winlator games are in your library.",
      }),
    ).toEqual({
      connected: true,
      folderLabel: "Frontend",
      adopted: 2,
      message: "Connected Frontend. 2 Winlator games are in your library.",
    });
  });

  it("reads a chooser the user backed out of", () => {
    const answer = normaliseWinlatorExportFolder({
      connected: false,
      folderLabel: null,
      adopted: 0,
      message: "No folder was connected.",
    });
    expect(answer?.connected).toBe(false);
    expect(answer?.folderLabel).toBe(null);
  });

  it("refuses an answer it cannot read rather than inventing a cheerful one", () => {
    for (const payload of [
      null,
      "connected",
      {},
      { connected: "yes", message: "hello" },
      { connected: true },
    ]) {
      expect(normaliseWinlatorExportFolder(payload)).toBe(null);
    }
  });

  it("does not let a broken count become a negative or fractional one", () => {
    for (const adopted of [-3, 1.5, Number.NaN, "2"]) {
      expect(
        normaliseWinlatorExportFolder({ connected: true, adopted, message: "Connected." })?.adopted,
      ).toBe(0);
    }
  });
});

describe("the background adoption pass", () => {
  it("only speaks up when it actually found something", () => {
    expect(readWinlatorAdoptedCount({ adopted: 3 })).toBe(3);
    for (const payload of [null, {}, { adopted: 0 }, { adopted: -1 }, { adopted: "3" }]) {
      expect(readWinlatorAdoptedCount(payload)).toBe(null);
    }
  });

  it("counts in the singular when there is one game", () => {
    expect(winlatorAdoptionToast(1)).toBe("One Winlator game was added to your library.");
    expect(winlatorAdoptionToast(4)).toBe("4 Winlator games were added to your library.");
  });
});
