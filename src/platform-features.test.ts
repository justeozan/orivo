import { afterEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(command: string) => Promise<unknown>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

import { DESKTOP_FEATURES, loadPlatformFeatures, readPlatformFeatures } from "./platform-features";

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  tauri.invoke.mockReset();
});

describe("platform features", () => {
  it("reads what an iOS host says it can do", () => {
    expect(
      readPlatformFeatures({
        downloadablePlugins: false,
        localRunners: false,
        storeChoices: true,
        stepsAside: false,
      }),
    ).toEqual({ downloadablePlugins: false, localRunners: false, storeChoices: true, stepsAside: false });
  });

  // Like every other flag, an answer the host did not give is the desktop's.
  it("steps aside for a game unless the host says it cannot", () => {
    expect(readPlatformFeatures({ storeChoices: false }).stepsAside).toBe(true);
    expect(readPlatformFeatures({ stepsAside: false }).stepsAside).toBe(false);
  });

  it("keeps the desktop answer for any flag the host did not send as a boolean", () => {
    expect(readPlatformFeatures({ storeChoices: "yes", localRunners: false })).toEqual({
      ...DESKTOP_FEATURES,
      localRunners: false,
    });
    expect(readPlatformFeatures(null)).toEqual(DESKTOP_FEATURES);
  });

  it("answers for the desktop without asking when there is no Tauri", async () => {
    await expect(loadPlatformFeatures()).resolves.toEqual(DESKTOP_FEATURES);
    expect(tauri.invoke).not.toHaveBeenCalled();
  });

  it("falls back to the desktop answer when the host predates the command", async () => {
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.invoke.mockRejectedValue("unknown command get_platform_features");
    await expect(loadPlatformFeatures()).resolves.toEqual(DESKTOP_FEATURES);
  });
});
