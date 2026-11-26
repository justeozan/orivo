import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute } from "./contracts";
import type { AppPage } from "./page-lifecycle";
import { mountApp } from "./app";

/**
 * Settings › Storage: the total, its bar, and the two actions it offers. The
 * host is mocked at `invoke`, which is the whole of what the panel talks to.
 */
const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  convertFileSrc: (path: string) => path,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => Promise.resolve("0.4.4"),
  getTauriVersion: () => Promise.resolve("2.11.5"),
}));
vi.mock("@tauri-apps/api/path", () => ({
  appCacheDir: () => Promise.resolve("/cache"),
  join: (...parts: string[]) => Promise.resolve(parts.join("/")),
}));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: () => Promise.resolve(null) }));

function stubPage(): AppPage {
  return {
    mount() {},
    activate(_activation: { route: AppRoute }) {},
    deactivate() {
      return null;
    },
  };
}

async function settle(): Promise<void> {
  for (let index = 0; index < 4; index += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

const USAGE = { derivedCacheBytes: 3 * 1024 ** 2, derivedCacheEntries: 12, refreshedAtEpochMs: null };

let detachShell: (() => void) | null = null;
let root: HTMLElement;

async function mountOnStorage(features: Record<string, boolean> = {}): Promise<void> {
  tauri.invoke.mockImplementation(async (command: string) => {
    if (command === "get_platform_features") return features;
    if (command === "get_data_usage") return USAGE;
    if (command === "clear_derived_cache") return { derivedCacheBytes: 0, derivedCacheEntries: 0 };
    return undefined;
  });
  document.body.replaceChildren();
  root = document.createElement("div");
  document.body.append(root);
  detachShell = mountApp(root, { storePage: stubPage(), gameDetailPage: stubPage() });
  window.location.hash = "#/settings/data";
  await settle();
}

beforeEach(() => {
  window.location.hash = "";
  window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
  tauri.invoke.mockReset();
});

afterEach(() => {
  detachShell?.();
  detachShell = null;
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  window.location.hash = "";
});

describe("Settings › Storage", () => {
  const bar = (): HTMLElement => root.querySelector<HTMLElement>("#derived-cache-bar")!;

  it("shows the measured cache as the total, with a full bar", async () => {
    await mountOnStorage();
    expect(root.querySelector("#derived-cache-size")?.textContent).toBe("3.0 MB");
    expect(bar().style.width).toBe("100%");
  });

  it("empties the bar when the cache is cleared", async () => {
    await mountOnStorage();
    root.querySelector<HTMLButtonElement>("#clear-derived-cache")!.click();
    await settle();
    expect(tauri.invoke).toHaveBeenCalledWith("clear_derived_cache");
    expect(root.querySelector("#derived-cache-size")?.textContent).toBe("0 B");
    expect(bar().style.width).toBe("0%");
  });

  it("asks the host to show the data folder, naming no path", async () => {
    await mountOnStorage();
    root.querySelector<HTMLButtonElement>("#open-data-folder")!.click();
    await settle();
    expect(tauri.invoke).toHaveBeenCalledWith("open_data_folder");
  });

  it("offers no data folder where the host has no file manager", async () => {
    await mountOnStorage({ showsDataFolder: false });
    expect(root.querySelector<HTMLElement>("#data-folder-row")?.hidden).toBe(true);
    root.querySelector<HTMLButtonElement>("#open-data-folder")!.click();
    await settle();
    expect(tauri.invoke).not.toHaveBeenCalledWith("open_data_folder");
  });
});
