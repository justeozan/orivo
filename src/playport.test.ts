import { afterEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(command: string) => Promise<unknown>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

import { loadPlayportInstalled, playportTakes } from "./playport";

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  tauri.invoke.mockReset();
});

describe("Playport", () => {
  it("takes a GOG or Epic game that ships for Windows, or says nothing about where it runs", () => {
    expect(playportTakes({ source: "gog", supportedPlatforms: ["windows", "macos"] })).toBe(true);
    expect(playportTakes({ source: "epic", supportedPlatforms: ["windows"] })).toBe(true);
    expect(playportTakes({ source: "gog", supportedPlatforms: [] })).toBe(true);
    expect(playportTakes({ source: "epic" })).toBe(true);
  });

  it("leaves a game with nothing for Windows, and every other store, alone", () => {
    expect(playportTakes({ source: "gog", supportedPlatforms: ["linux"] })).toBe(false);
    expect(playportTakes({ source: "gog", supportedPlatforms: ["macos", "linux"] })).toBe(false);
    // Steam's answers never depend on this: its Install always has one.
    expect(playportTakes({ source: "steam", supportedPlatforms: ["windows"] })).toBe(false);
    expect(playportTakes({ source: "ubisoft", supportedPlatforms: ["windows"] })).toBe(false);
    expect(playportTakes({ source: "local" })).toBe(false);
  });

  it("judges only the desktop platforms, as the host does", () => {
    // A mobile platform says nothing about a Windows build either way.
    expect(playportTakes({ source: "gog", supportedPlatforms: ["ios", "android"] })).toBe(true);
    expect(playportTakes({ source: "gog", supportedPlatforms: ["ios", "linux"] })).toBe(false);
  });

  it("is never installed where there is no Tauri to ask", async () => {
    await expect(loadPlayportInstalled()).resolves.toBe(false);
    expect(tauri.invoke).not.toHaveBeenCalled();
  });

  it("takes nothing but a plain true from the host", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.invoke.mockResolvedValueOnce(true);
    await expect(loadPlayportInstalled()).resolves.toBe(true);
    expect(tauri.invoke).toHaveBeenCalledWith("is_playport_installed");
    tauri.invoke.mockResolvedValueOnce("yes");
    await expect(loadPlayportInstalled()).resolves.toBe(false);
  });

  it("answers no when the host predates the command", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.invoke.mockRejectedValue("unknown command is_playport_installed");
    await expect(loadPlayportInstalled()).resolves.toBe(false);
  });
});
