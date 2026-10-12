import { describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

import { createStoreChoicesClient, fillStoreChoiceMenu, readStoreChoices } from "./store-choices";

const appStore = { id: "app-store", label: "iPhone & iPad version", detail: "App Store · ConcernedApe LLC" };
const steamPage = { id: "steam-page", label: "View on Steam", detail: "Its page on the Steam store" };

describe("store choices", () => {
  it("keeps only rows with an id and a label", () => {
    expect(
      readStoreChoices([appStore, { id: "", label: "No id" }, { id: "x" }, "rubbish", { id: "y", label: "Y" }]),
    ).toEqual([appStore, { id: "y", label: "Y", detail: "" }]);
    expect(readStoreChoices(undefined)).toEqual([]);
  });

  it("sends the host a game and a row id, never a destination", async () => {
    tauri.invoke.mockResolvedValueOnce([appStore, steamPage]).mockResolvedValueOnce(undefined);
    const client = createStoreChoicesClient();
    await expect(client.list("steam:413150")).resolves.toEqual([appStore, steamPage]);
    await client.open("steam:413150", "app-store");
    expect(tauri.invoke.mock.calls).toEqual([
      ["get_store_choices", { gameId: "steam:413150" }],
      ["open_store_choice", { gameId: "steam:413150", choiceId: "app-store" }],
    ]);
  });

  it("fills the Play menu with one row per choice, its destination on a second line", () => {
    const menu = document.createElement("div");
    const pick = vi.fn();
    fillStoreChoiceMenu(menu, [appStore, steamPage], pick);

    const rows = [...menu.querySelectorAll<HTMLButtonElement>("[role='menuitem']")];
    expect(rows.map((row) => row.dataset.storeChoice)).toEqual(["app-store", "steam-page"]);
    expect(rows[0].querySelector(".play-launch-option__label")?.textContent).toBe("iPhone & iPad version");
    expect(rows[0].querySelector(".play-launch-option__detail")?.textContent).toBe("App Store · ConcernedApe LLC");

    rows[1].click();
    expect(pick).toHaveBeenCalledWith(steamPage);
  });
});
