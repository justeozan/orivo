import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute } from "./contracts";
import type { AppPage } from "./page-lifecycle";
import { mountApp } from "./app";
import { t } from "./i18n";
import { RELEASE_HIGHLIGHTS } from "./release-highlights";

/**
 * Settings › About, past the updater (`app-updater.test.ts`): what is new,
 * the links, and the reset. The host is mocked at `invoke`.
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

let detachShell: (() => void) | null = null;
let root: HTMLElement;

async function mountOnAbout(features: Record<string, boolean> = {}): Promise<void> {
  tauri.invoke.mockImplementation(async (command: string) => {
    if (command === "get_platform_features") return features;
    return undefined;
  });
  document.body.replaceChildren();
  root = document.createElement("div");
  document.body.append(root);
  detachShell = mountApp(root, { storePage: stubPage(), gameDetailPage: stubPage() });
  window.location.hash = "#/settings/about";
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

describe("Settings › About", () => {
  it("names the running version and writes its highlights large", async () => {
    await mountOnAbout();
    await vi.waitFor(() => expect(root.querySelector<HTMLElement>("#about-news")?.hidden).toBe(false));
    expect(root.querySelector("#about-app-version")?.textContent).toBe("0.4.4");
    expect(root.querySelector("#about-news-title")?.textContent).toBe("What's new in 0.4.4");
    const items = [...root.querySelectorAll("#about-news-list .about-news__item")];
    const release = RELEASE_HIGHLIGHTS.find((entry) => entry.version === "0.4.4")!;
    expect(items.map((item) => item.querySelector("strong")?.textContent)).toEqual(
      release.items.map((item) => t(item.titleKey)),
    );
    expect(items.map((item) => item.querySelector("small")?.textContent)).toEqual(
      release.items.map((item) => t(item.lineKey)),
    );
    expect(items[0].querySelector(".plugin-icon")?.getAttribute("data-category")).toBe(release.items[0].tone);
  });

  it("asks the host for a page by name, never by address", async () => {
    await mountOnAbout();
    root.querySelector<HTMLButtonElement>("[data-about-page='releases']")!.click();
    root.querySelector<HTMLButtonElement>("[data-about-page='new-issue']")!.click();
    await settle();
    expect(tauri.invoke).toHaveBeenCalledWith("open_orivo_page", { page: "releases" });
    expect(tauri.invoke).toHaveBeenCalledWith("open_orivo_page", { page: "new-issue" });
  });

  it("resets the settings from its red link", async () => {
    await mountOnAbout();
    root.querySelector<HTMLButtonElement>("#settings-panel-about [data-settings-action='reset-preferences']")!.click();
    await settle();
    expect(tauri.invoke).toHaveBeenCalledWith("update_preferences", { update: { reset: true } });
  });

  it("offers no update button where a store delivers new versions", async () => {
    await mountOnAbout({ selfUpdates: false });
    expect(root.querySelector<HTMLElement>("#check-updates-button")?.hidden).toBe(true);
    expect(root.querySelector<HTMLElement>("#updates-row")?.hidden).toBe(true);
    expect(root.querySelector("#about-app-version")?.textContent).toBe("0.4.4");
  });
});
