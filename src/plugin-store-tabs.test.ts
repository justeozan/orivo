import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: tauri.invoke }));

import {
  PLUGIN_STORE_TABS_EVENT,
  openPluginStoreGamePage,
  resetPluginStoreTabsForTests,
  syncPluginStoreTabs,
  watchPluginStoreTabs,
} from "./plugin-store-tabs";
import { listPluginStoreTabs, onStoreTabsChange, registerStoreTab, resetStoreTabsForTests } from "./store-tabs";

const PLUGIN = "com.orivo.fixture-store";

/** `list_plugin_store_tabs`'s shape, as `plugin_store_tabs.rs` serialises it. */
function wireTab(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    pluginId: PLUGIN,
    key: "deals",
    label: "Fixture Deals",
    accent: "#e4572e",
    games: [
      {
        id: `${PLUGIN}:alpha`,
        title: "Alpha Quest",
        shortDescription: "A short trip.",
        coverUrl: "https://cdn.cloudflare.steamstatic.com/steam/apps/620/library_600x900.jpg",
        heroUrl: "",
        landscapeUrl: "",
        logoUrl: "",
        genres: ["Puzzle"],
        price: { currency: "EUR", amountMinor: 499, regularAmountMinor: 1999 },
        hasPage: true,
      },
    ],
    ...overrides,
  };
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

function inTauri(): void {
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
}

beforeEach(() => {
  tauri.invoke.mockReset();
  inTauri();
});

afterEach(() => {
  delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  resetPluginStoreTabsForTests();
  resetStoreTabsForTests();
});

describe("syncPluginStoreTabs", () => {
  it("registers what the host lists, as a Store shelf of plugin games", async () => {
    tauri.invoke.mockResolvedValueOnce([wireTab(), wireTab({ key: "free", label: "Free", accent: null })]);
    await syncPluginStoreTabs();

    expect(tauri.invoke).toHaveBeenCalledWith("list_plugin_store_tabs");
    const [deals, free] = listPluginStoreTabs();
    expect(deals.id).toBe(`plugin:${PLUGIN}:deals`);
    expect(deals.accent).toBe("#e4572e");
    expect(free.id).toBe(`plugin:${PLUGIN}:free`);
    const alpha = deals.games[0];
    expect(alpha).toMatchObject({
      id: `${PLUGIN}:alpha`,
      title: "Alpha Quest",
      source: "store",
      offers: [],
      owned: false,
      launchable: false,
      pluginPrice: { currency: "EUR", amountMinor: 499, regularAmountMinor: 1999 },
      hasPluginPage: true,
    });
    expect(alpha.logoUrl).toBeUndefined();
  });

  it("removes the tabs of a plugin the host no longer lists, and only its own", async () => {
    registerStoreTab({ pluginId: "com.example.built-in", label: "Built in", games: [] });
    tauri.invoke.mockResolvedValueOnce([wireTab(), wireTab({ key: "free", label: "Free" })]);
    await syncPluginStoreTabs();
    expect(listPluginStoreTabs()).toHaveLength(3);

    // Uninstalled: the host lists nothing for it any more.
    tauri.invoke.mockResolvedValueOnce([wireTab({ key: "free", label: "Free" })]);
    await syncPluginStoreTabs();
    expect(listPluginStoreTabs().map((tab) => tab.id)).toEqual([
      "plugin:com.example.built-in",
      `plugin:${PLUGIN}:free`,
    ]);

    tauri.invoke.mockResolvedValueOnce([]);
    await syncPluginStoreTabs();
    expect(listPluginStoreTabs().map((tab) => tab.id)).toEqual(["plugin:com.example.built-in"]);
  });

  it("does not repaint the Store for an answer that did not change", async () => {
    tauri.invoke.mockResolvedValue([wireTab()]);
    await syncPluginStoreTabs();
    const listener = vi.fn();
    onStoreTabsChange(listener);
    await syncPluginStoreTabs();
    expect(listener).not.toHaveBeenCalled();

    tauri.invoke.mockResolvedValue([wireTab({ label: "Weekend Deals" })]);
    await syncPluginStoreTabs();
    expect(listener).toHaveBeenCalledTimes(1);
    expect(listPluginStoreTabs()[0].label).toBe("Weekend Deals");
  });

  it("keeps the registry when the host cannot answer, and drops an answer overtaken by a newer one", async () => {
    tauri.invoke.mockResolvedValueOnce([wireTab()]);
    await syncPluginStoreTabs();

    tauri.invoke.mockRejectedValueOnce(new Error("no such command"));
    await syncPluginStoreTabs();
    tauri.invoke.mockResolvedValueOnce({ not: "a list" });
    await syncPluginStoreTabs();
    expect(listPluginStoreTabs()).toHaveLength(1);

    let answerSlowly: (value: unknown) => void = () => {};
    tauri.invoke.mockReturnValueOnce(new Promise((resolve) => (answerSlowly = resolve)));
    const slow = syncPluginStoreTabs();
    tauri.invoke.mockResolvedValueOnce([]);
    await syncPluginStoreTabs();
    answerSlowly([wireTab({ key: "stale", label: "Stale" })]);
    await slow;
    expect(listPluginStoreTabs()).toEqual([]);
  });

  it("goes through the registry's checks again rather than trusting the wire", async () => {
    tauri.invoke.mockResolvedValueOnce([
      wireTab({ accent: "red", label: "‮Deals" }),
      wireTab({ pluginId: "../../etc", key: "x" }),
      "rubbish",
      null,
    ]);
    await syncPluginStoreTabs();
    const tabs = listPluginStoreTabs();
    expect(tabs).toHaveLength(1);
    expect(tabs[0].label).toBe("Deals");
    expect(tabs[0].accent).toBe("#7d54f4");
  });

  it("does nothing outside the desktop shell", async () => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    await syncPluginStoreTabs();
    await openPluginStoreGamePage(`${PLUGIN}:alpha`);
    expect(watchPluginStoreTabs(vi.fn())).toBeTypeOf("function");
    expect(tauri.invoke).not.toHaveBeenCalled();
  });
});

describe("openPluginStoreGamePage", () => {
  it("sends the host a game id, never an address, and passes its refusal on", async () => {
    tauri.invoke.mockResolvedValueOnce(undefined);
    await openPluginStoreGamePage(`${PLUGIN}:alpha`);
    expect(tauri.invoke).toHaveBeenCalledWith("open_plugin_store_page", { gameId: `${PLUGIN}:alpha` });

    tauri.invoke.mockRejectedValueOnce("This page is no longer available.");
    await expect(openPluginStoreGamePage(`${PLUGIN}:gone`)).rejects.toBe("This page is no longer available.");
  });
});

describe("watchPluginStoreTabs", () => {
  it("syncs after the first frame and again whenever the host says a plugin changed", async () => {
    tauri.invoke.mockResolvedValue([wireTab()]);
    const handlers = new Map<string, () => void>();
    const unlisten = vi.fn();
    const listen = vi.fn(async (event: string, handler: () => void) => {
      handlers.set(event, handler);
      return unlisten;
    });

    const stop = watchPluginStoreTabs(listen);
    expect(listen).toHaveBeenCalledWith(PLUGIN_STORE_TABS_EVENT, expect.any(Function));
    await flush();
    await flush();
    expect(tauri.invoke).toHaveBeenCalledTimes(1);
    expect(listPluginStoreTabs()).toHaveLength(1);

    tauri.invoke.mockResolvedValue([]);
    handlers.get(PLUGIN_STORE_TABS_EVENT)?.();
    await flush();
    expect(tauri.invoke).toHaveBeenCalledTimes(2);
    expect(listPluginStoreTabs()).toEqual([]);

    stop();
    await flush();
    expect(unlisten).toHaveBeenCalled();
  });
});
