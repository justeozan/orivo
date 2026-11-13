// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { readStoreFolderAccess, renderStoreFolders } from "./store-folders";

describe("store folders in the Mac App Store edition", () => {
  it("asks for nothing when the host does not say it is sandboxed", () => {
    expect(readStoreFolderAccess(undefined)).toEqual({ needsGrant: false, steam: false, epic: false });
    const panel = document.createElement("section");
    renderStoreFolders(panel, readStoreFolderAccess({ steam: true }));
    expect(panel.hidden).toBe(true);
    expect(panel.children).toHaveLength(0);
  });

  it("offers a button only for the stores still to grant", () => {
    const panel = document.createElement("section");
    renderStoreFolders(panel, readStoreFolderAccess({ needsGrant: true, steam: true, epic: false }));
    expect(panel.hidden).toBe(false);
    const buttons = [...panel.querySelectorAll<HTMLButtonElement>("[data-store-folder-grant]")];
    expect(buttons.map((button) => button.dataset.storeFolderGrant)).toEqual(["epic"]);
  });
});
