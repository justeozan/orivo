// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { gameFolderScanMessage, readGameFolderScan, readGameFolders, renderGameFoldersPanel } from "./game-folders";

const folder = (overrides: Record<string, unknown> = {}) => ({
  label: "Shortcuts",
  state: "ok",
  entries: 0,
  games: 0,
  added: 0,
  known: 0,
  unusable: 0,
  unrecognized: [],
  ...overrides,
});

const scan = (overrides: Record<string, unknown> = {}) =>
  readGameFolderScan({ requested: true, added: 0, addedIds: [], folders: [folder()], ...overrides });

describe("games folders", () => {
  it("reads the folder list and drops malformed rows", () => {
    expect(
      readGameFolders([
        { id: "folder:1", label: "Games", available: true },
        { id: "folder:2", label: "USB", available: false },
        { label: "no id" },
        "nonsense",
      ]),
    ).toEqual([
      { id: "folder:1", label: "Games", available: true },
      { id: "folder:2", label: "USB", available: false },
    ]);
    expect(readGameFolders(null)).toEqual([]);
  });

  it("still reads the bare count an older host sent, as a quiet scan", () => {
    const event = readGameFolderScan(3);
    expect(event).toEqual({
      requested: false,
      added: 3,
      addedIds: [],
      repointed: 0,
      hidden: 0,
      relocated: [],
      missingChanged: false,
      folders: [],
    });
    expect(gameFolderScanMessage(event)).toBe("3 games added from your games folder.");
  });

  it("says nothing about a background scan that added nothing", () => {
    expect(gameFolderScanMessage(scan({ requested: false }))).toBeNull();
    // A card turning dim is shown, not announced.
    expect(gameFolderScanMessage(scan({ requested: false, missingChanged: true }))).toBeNull();
  });

  it("names the game that moved and was found, even after a background scan", () => {
    const one = scan({ requested: false, relocated: ["Hades"], missingChanged: true });
    expect(one.relocated).toEqual(["Hades"]);
    expect(one.missingChanged).toBe(true);
    expect(gameFolderScanMessage(one)).toBe("Hades moved: Orivo found it.");
    expect(gameFolderScanMessage(scan({ requested: false, relocated: ["Hades", "Celeste"] }))).toBe(
      "2 games moved: Orivo found them.",
    );
  });

  it("names the folder games were added from, and the shortcuts it could not use", () => {
    expect(
      gameFolderScanMessage(
        scan({ added: 12, addedIds: ["a"], folders: [folder({ games: 13, added: 12, unusable: 1 })] }),
      ),
    ).toBe("12 games added from Shortcuts. 1 shortcut could not be used.");
  });

  it("speaks of folders when games came from several", () => {
    expect(
      gameFolderScanMessage(
        scan({ requested: false, added: 5, folders: [folder({ added: 1 }), folder({ label: "Games", added: 4 })] }),
      ),
    ).toBe("5 games added from your games folders.");
  });

  it("says how many cards that opened a tool were fixed", () => {
    expect(gameFolderScanMessage(scan({ requested: false, repointed: 9, hidden: 3 }))).toBe(
      "12 cards that opened a tool instead of their game (checksum checker, plugin, crack copy) were fixed.",
    );
    expect(
      gameFolderScanMessage(scan({ requested: false, added: 1, repointed: 1, folders: [folder({ added: 1 })] })),
    ).toBe(
      "1 game added from Shortcuts. 1 card that opened a tool instead of its game (checksum checker, plugin, crack copy) was fixed.",
    );
  });

  it("answers a chosen folder that added nothing, and says why", () => {
    expect(gameFolderScanMessage(scan())).toBe("Shortcuts is empty.");
    expect(gameFolderScanMessage(scan({ folders: [folder({ entries: 2, unrecognized: ["", "txt"] })] }))).toBe(
      "No games in Shortcuts: Orivo found only extension-less, .txt files. It adds apps, .exe files, game folders and shortcuts (.lnk, .url, aliases).",
    );
    expect(gameFolderScanMessage(scan({ folders: [folder({ entries: 4, games: 4, known: 4 })] }))).toBe(
      "Nothing new in Shortcuts: its 4 games are already in your library.",
    );
    // A rescan: what is there comes first, the shortcuts it cannot use after.
    expect(
      gameFolderScanMessage(scan({ folders: [folder({ entries: 8, games: 7, known: 5, unusable: 2 })] })),
    ).toBe("Nothing new in Shortcuts: its 5 games are already in your library. 2 shortcuts could not be used.");
    expect(gameFolderScanMessage(scan({ folders: [folder({ entries: 2, games: 2, unusable: 2 })] }))).toBe(
      "The 2 shortcuts in Shortcuts are broken, or cannot be opened on this device.",
    );
    expect(gameFolderScanMessage(scan({ folders: [folder({ state: "missing" })] }))).toBe(
      "Shortcuts cannot be found. If it is on an external drive, connect it and try again.",
    );
    expect(gameFolderScanMessage(scan({ folders: [] }))).toBe(
      "No games folder yet. Add one and every game in it joins your library.",
    );
  });

  it("lists the folders in Settings, each with its own actions, and stays out where it cannot work", () => {
    const panel = document.createElement("section");
    renderGameFoldersPanel(
      panel,
      [
        { id: "folder:a", label: "<b>Shortcuts</b>", available: true },
        { id: "folder:b", label: "USB", available: false },
      ],
      true,
    );
    expect(panel.hidden).toBe(false);
    const actions = [...panel.querySelectorAll<HTMLButtonElement>("[data-game-folder-action]")].map(
      (button) => `${button.dataset.gameFolderAction}:${button.dataset.folderId ?? ""}:${button.disabled}`,
    );
    expect(actions).toEqual([
      "add::false",
      "rescan:folder:a:false",
      "remove:folder:a:false",
      "rescan:folder:b:true",
      "remove:folder:b:false",
    ]);
    // A folder's name is text, never markup.
    expect(panel.querySelector("b")).toBeNull();
    expect(panel.textContent).toContain("<b>Shortcuts</b>");

    renderGameFoldersPanel(panel, [], false);
    expect(panel.hidden).toBe(true);
    expect(panel.children).toHaveLength(0);
  });
});
