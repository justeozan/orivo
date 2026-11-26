import { afterEach, describe, expect, it, vi } from "vitest";
import {
  DEFAULT_PLUGIN_ACCENT,
  isStoreTabId,
  listPluginStoreTabs,
  onStoreTabsChange,
  pluginStoreTab,
  registerStoreTab,
  resetStoreTabsForTests,
  safeAccent,
  safeImageUrl,
  unregisterPluginStoreTabs,
  unregisterStoreTab,
  type PluginStoreGame,
  type StoreTabInput,
} from "./store-tabs";

const PLUGIN = "com.example.deals";
const OTHER = "com.example.other";

function game(overrides: Partial<PluginStoreGame> = {}): PluginStoreGame {
  return {
    id: "alpha",
    title: "Alpha Quest",
    source: "store",
    shortDescription: "A short trip.",
    coverUrl: "https://cdn.example.com/alpha.jpg",
    heroUrl: "",
    landscapeUrl: "",
    genres: ["Puzzle"],
    tags: [],
    supportedPlatforms: [],
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: [],
    offers: [],
    ...overrides,
  };
}

function tab(overrides: Partial<StoreTabInput> = {}): StoreTabInput {
  return { pluginId: PLUGIN, label: "Deals", games: [game()], ...overrides };
}

afterEach(() => resetStoreTabsForTests());

describe("isStoreTabId", () => {
  it("accepts every built-in tab and a well-formed plugin tab, registered or not", () => {
    for (const id of ["all", "playstation", "xbox", "nintendo", "steam", "epic", `plugin:${PLUGIN}`, `plugin:${PLUGIN}:week-1`]) {
      expect(isStoreTabId(id), id).toBe(true);
    }
  });

  it("refuses anything a route should not carry", () => {
    for (const id of ["", "switch", "plugin:", "plugin:Com.Example.Deals", "plugin:example.deals", `plugin:${PLUGIN}:Week`, `plugin:${PLUGIN}:a:b`, "javascript:alert(1)"]) {
      expect(isStoreTabId(id), id).toBe(false);
    }
  });
});

describe("safeAccent", () => {
  it("keeps #rrggbb, lowercased", () => {
    expect(safeAccent("#E4572E")).toBe("#e4572e");
    expect(safeAccent(" #00ff7f ")).toBe("#00ff7f");
  });

  it("refuses names, short and alpha forms, and anything that is not a string", () => {
    for (const value of ["orange", "#fff", "#e4572e80", "e4572e", "rgb(1,2,3)", "#e4572e;background:red", 0xe4572e, null]) {
      expect(safeAccent(value), String(value)).toBeNull();
    }
  });
});

describe("safeImageUrl", () => {
  it("keeps https and the app's own paths", () => {
    expect(safeImageUrl("https://cdn.example.com/a.jpg")).toBe("https://cdn.example.com/a.jpg");
    expect(safeImageUrl("/media/cover.jpg")).toBe("/media/cover.jpg");
  });

  it("refuses every other scheme and whatever could close a CSS url()", () => {
    for (const value of [
      "http://cdn.example.com/a.jpg",
      "javascript:alert(1)",
      "data:image/png;base64,AAAA",
      "//cdn.example.com/a.jpg",
      "https://cdn.example.com/a.jpg') ; x:url(y",
      'https://cdn.example.com/"a.jpg',
      "https://cdn.example.com/a b.jpg",
      "https://",
      42,
    ]) {
      expect(safeImageUrl(value), String(value)).toBe("");
    }
  });
});

describe("registerStoreTab", () => {
  it("names the tab after its plugin and key, and gives it Orivo's violet without a colour", () => {
    expect(registerStoreTab(tab())?.id).toBe(`plugin:${PLUGIN}`);
    const keyed = registerStoreTab(tab({ key: "free", accent: "#E4572E" }));
    expect(keyed?.id).toBe(`plugin:${PLUGIN}:free`);
    expect(keyed?.accent).toBe("#e4572e");
    expect(registerStoreTab(tab({ key: "odd", accent: "red" }))?.accent).toBe(DEFAULT_PLUGIN_ACCENT);
  });

  it("refuses a tab it could not name or label", () => {
    expect(registerStoreTab(tab({ pluginId: "deals" }))).toBeNull();
    expect(registerStoreTab(tab({ pluginId: "Com.Example.Deals" }))).toBeNull();
    expect(registerStoreTab(tab({ key: "Free" }))).toBeNull();
    expect(registerStoreTab(tab({ key: "a:b" }))).toBeNull();
    expect(registerStoreTab(tab({ label: " \u0007‮ " }))).toBeNull();
    expect(listPluginStoreTabs()).toEqual([]);
  });

  it("strips control and direction characters from the label and cuts it at 32", () => {
    expect(registerStoreTab(tab({ label: "‮Deals\u0000 " }))?.label).toBe("Deals");
    const long = registerStoreTab(tab({ label: "x".repeat(40) }))?.label ?? "";
    expect(long).toHaveLength(32);
    expect(long.endsWith("…")).toBe(true);
  });

  it("puts every game in the plugin's own namespace", () => {
    const games = registerStoreTab(
      tab({ games: [game({ id: "alpha" }), game({ id: `${PLUGIN}:beta` }), game({ id: `${OTHER}:gamma` })] }),
    )?.games.map((entry) => entry.id);
    expect(games).toEqual([`${PLUGIN}:alpha`, `${PLUGIN}:beta`, `${PLUGIN}:${OTHER}:gamma`]);
  });

  it("keeps a real game title instead of cutting it like a tab label", () => {
    const title = "The Legend of Zelda: Tears of the Kingdom";
    expect(registerStoreTab(tab({ games: [game({ title })] }))?.games[0].title).toBe(title);
    const cut = registerStoreTab(tab({ games: [game({ title: "t".repeat(120) })] }))?.games[0].title ?? "";
    expect(cut).toHaveLength(96);
  });

  it("drops a game with no title and pictures it may not show", () => {
    const [kept, ...rest] =
      registerStoreTab(
        tab({
          games: [
            game({
              shortDescription: "Line\u0007 one‮",
              coverUrl: "http://cdn.example.com/a.jpg",
              heroUrl: "javascript:alert(1)",
              landscapeUrl: "https://cdn.example.com/l.jpg",
              logoUrl: "data:image/png;base64,AAAA",
              gallery: ["https://cdn.example.com/1.jpg", "http://cdn.example.com/2.jpg", "x')"],
              genres: ["Puzzle", 7 as unknown as string, "\u0000", "Co-op", "RPG", "Indie", "Extra"],
              criticScore: 140,
              exclusiveTo: "playstation",
            }),
            game({ id: "untitled", title: "‮" }),
          ],
        }),
      )?.games ?? [];
    expect(rest).toEqual([]);
    expect(kept.shortDescription).toBe("Line one");
    expect([kept.coverUrl, kept.heroUrl, kept.landscapeUrl, kept.logoUrl]).toEqual([
      "",
      "",
      "https://cdn.example.com/l.jpg",
      undefined,
    ]);
    expect(kept.gallery).toEqual(["https://cdn.example.com/1.jpg"]);
    expect(kept.genres).toEqual(["Puzzle", "Co-op", "RPG", "Indie"]);
    expect(kept.criticScore).toBeUndefined();
    expect(kept.exclusiveTo).toBeUndefined();
  });

  it("keeps a plugin price only in minor units of a currency code, and a discount only when it is one", () => {
    const priced = (pluginPrice: unknown) =>
      registerStoreTab(tab({ games: [game({ pluginPrice: pluginPrice as PluginStoreGame["pluginPrice"] })] }))?.games[0]
        .pluginPrice;
    expect(priced({ currency: "EUR", amountMinor: 499, regularAmountMinor: 1999 })).toEqual({
      currency: "EUR",
      amountMinor: 499,
      regularAmountMinor: 1999,
    });
    expect(priced({ currency: "EUR", amountMinor: 500, regularAmountMinor: 400 })?.regularAmountMinor).toBeNull();
    expect(priced({ currency: "eur", amountMinor: 1, regularAmountMinor: null })).toBeUndefined();
    expect(priced({ currency: "EUR", amountMinor: 4.5, regularAmountMinor: null })).toBeUndefined();
    expect(priced({ currency: "EUR", amountMinor: -1, regularAmountMinor: null })).toBeUndefined();
    expect(priced("19,99 €")).toBeUndefined();
  });

  it("only believes a page exists when it is told so in a boolean", () => {
    const pages = registerStoreTab(
      tab({
        games: [
          game({ id: "a", hasPluginPage: true }),
          game({ id: "b", hasPluginPage: "yes" as unknown as boolean }),
          game({ id: "c" }),
        ],
      }),
    )?.games.map((entry) => entry.hasPluginPage);
    expect(pages).toEqual([true, false, false]);
  });

  it("keeps at most 120 games", () => {
    const games = Array.from({ length: 130 }, (_, index) => game({ id: `g${index}` }));
    expect(registerStoreTab(tab({ games }))?.games).toHaveLength(120);
  });

  it("replaces a tab in place, keeping the order tabs were first added in", () => {
    registerStoreTab(tab({ key: "one", label: "One" }));
    registerStoreTab(tab({ key: "two", label: "Two" }));
    registerStoreTab(tab({ key: "one", label: "Uno" }));
    expect(listPluginStoreTabs().map((entry) => entry.label)).toEqual(["Uno", "Two"]);
    expect(pluginStoreTab(`plugin:${PLUGIN}:one`)?.label).toBe("Uno");
  });
});

describe("unregistering and listeners", () => {
  it("removes one tab, or every tab of one plugin and no other's", () => {
    registerStoreTab(tab({ key: "one" }));
    registerStoreTab(tab({ key: "two" }));
    registerStoreTab(tab({ pluginId: OTHER }));

    unregisterStoreTab(`plugin:${PLUGIN}:one`);
    expect(listPluginStoreTabs().map((entry) => entry.id)).toEqual([`plugin:${PLUGIN}:two`, `plugin:${OTHER}`]);

    unregisterPluginStoreTabs(PLUGIN);
    expect(listPluginStoreTabs().map((entry) => entry.id)).toEqual([`plugin:${OTHER}`]);
    expect(pluginStoreTab(`plugin:${PLUGIN}:two`)).toBeNull();
  });

  it("tells listeners about a change, not about nothing, until they leave", () => {
    const listener = vi.fn();
    const stop = onStoreTabsChange(listener);

    registerStoreTab(tab());
    expect(listener).toHaveBeenCalledTimes(1);
    registerStoreTab(tab({ pluginId: "bad" }));
    unregisterStoreTab(`plugin:${OTHER}`);
    unregisterPluginStoreTabs(OTHER);
    expect(listener).toHaveBeenCalledTimes(1);

    unregisterPluginStoreTabs(PLUGIN);
    expect(listener).toHaveBeenCalledTimes(2);

    stop();
    registerStoreTab(tab());
    expect(listener).toHaveBeenCalledTimes(2);
  });
});
