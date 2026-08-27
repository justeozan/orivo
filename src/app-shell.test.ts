import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute } from "./contracts";
import type { GameDetailPageOptions } from "./game-detail-page";
import { mountApp } from "./app";
import type { AppPage } from "./page-lifecycle";

/**
 * The shell owns launching, routing, and every shared load. These doubles let
 * the tests reach that wiring without driving the Store or detail page markup,
 * which another module owns.
 */
const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
  detailOptions: null as GameDetailPageOptions | null,
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  convertFileSrc: (path: string) => path,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => Promise.resolve("0.0.0-test"),
  getTauriVersion: () => Promise.resolve("0.0.0-test"),
}));
vi.mock("@tauri-apps/api/path", () => ({
  appCacheDir: () => Promise.resolve("/cache"),
  join: (...parts: string[]) => Promise.resolve(parts.join("/")),
}));
vi.mock("./game-detail-page", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./game-detail-page")>();
  return {
    ...actual,
    // Captures the wiring the shell hands the detail page — `play` in
    // particular — so a test can request a launch the way the page does.
    createGameDetailPage: (options: GameDetailPageOptions): AppPage => {
      tauri.detailOptions = options;
      return {
        mount() {},
        activate() {},
        deactivate() {
          return null;
        },
      };
    },
  };
});

interface StubPage extends AppPage {
  readonly routes: AppRoute[];
  readonly deactivations: number[];
}

function stubPage(label: string): StubPage {
  const routes: AppRoute[] = [];
  const deactivations: number[] = [];
  return {
    routes,
    deactivations,
    mount(container) {
      const heading = document.createElement("h1");
      heading.textContent = label;
      container.append(heading);
    },
    activate(activation) {
      routes.push(activation.route);
    },
    deactivate() {
      deactivations.push(routes.length);
      return null;
    },
  };
}

/** Lets the hash listener and the async page activation settle. */
async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 0));
  await new Promise((resolve) => setTimeout(resolve, 0));
}

async function goto(hash: string): Promise<void> {
  window.location.hash = hash;
  await settle();
}

function visiblePages(root: HTMLElement): string[] {
  return Array.from(root.querySelectorAll<HTMLElement>(".app-page"))
    .filter((page) => !page.hidden)
    .map((page) => page.id);
}

function currentNavLabels(root: HTMLElement): string[] {
  return Array.from(root.querySelectorAll<HTMLElement>("[aria-current]")).map(
    (link) => link.textContent?.trim() ?? "",
  );
}

describe("application shell", () => {
  let root: HTMLElement;
  let store: StubPage;
  let detail: StubPage;

  beforeEach(async () => {
    window.location.hash = "";
    // jsdom ships no `matchMedia`; selecting a card animates the hero title.
    window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
    document.body.replaceChildren();
    root = document.createElement("div");
    document.body.append(root);
    store = stubPage("Store");
    detail = stubPage("Game");
    mountApp(root, { storePage: store, gameDetailPage: detail });
    await settle();
  });

  it("mounts the topbar once and shows only the routed page", async () => {
    expect(visiblePages(root)).toEqual(["app-page-library"]);

    await goto("#/store");
    expect(visiblePages(root)).toEqual(["app-page-store"]);

    await goto("#/settings/appearance");
    expect(visiblePages(root)).toEqual(["app-page-settings"]);

    await goto("#/library");
    expect(visiblePages(root)).toEqual(["app-page-library"]);
    expect(root.querySelectorAll(".topbar")).toHaveLength(1);
  });

  it("marks exactly one navigation link as current on every route", async () => {
    expect(currentNavLabels(root)).toEqual(["Library"]);

    await goto("#/store");
    expect(currentNavLabels(root)).toEqual(["Store"]);

    await goto("#/settings/data");
    expect(currentNavLabels(root)).toEqual(["Settings"]);

    // A detail page keeps the origin section marked, never zero and never two.
    await goto("#/games/steam%3A42?from=store");
    expect(currentNavLabels(root)).toEqual(["Store"]);

    await goto("#/nowhere");
    expect(currentNavLabels(root)).toEqual(["Library"]);
  });

  it("hands the game route to the detail page and deactivates the origin", async () => {
    await goto("#/games/steam%3A42?from=library");
    expect(detail.routes.at(-1)).toEqual({
      page: "game",
      gameId: "steam:42",
      from: "library",
    });
    expect(visiblePages(root)).toEqual(["app-page-game"]);
  });

  it("re-activates the store page in place when only its filters change", async () => {
    await goto("#/store");
    expect(store.routes).toHaveLength(1);

    await goto("#/store?category=relaxing");
    expect(store.routes).toHaveLength(2);
    // Filter changes never tear the page down and back up.
    expect(store.deactivations).toHaveLength(0);
  });

  it("switches settings sections without leaving the settings page", async () => {
    await goto("#/settings/plugins");
    const active = root.querySelector<HTMLElement>("[data-settings-panel]:not([hidden])");
    expect(active?.dataset.settingsPanel).toBe("plugins");
    expect(
      root.querySelector<HTMLButtonElement>("[data-settings-section='plugins']")?.getAttribute(
        "aria-selected",
      ),
    ).toBe("true");

    root.querySelector<HTMLButtonElement>("[data-settings-section='about']")?.click();
    await settle();
    expect(window.location.hash).toBe("#/settings/about");
    expect(
      root.querySelector<HTMLElement>("[data-settings-panel]:not([hidden])")?.dataset.settingsPanel,
    ).toBe("about");
  });

  it("makes the contextual search follow the route", async () => {
    const search = root.querySelector<HTMLInputElement>("#topbar-search")!;
    expect(search.placeholder).toBe("Search games…");

    await goto("#/store");
    expect(search.placeholder).toBe("Search the store…");

    await goto("#/settings/general");
    expect(search.placeholder).toBe("Search settings…");

    // The bar looks the same on every route: the field is never blanked out,
    // it simply searches the library from a page that has no search of its own.
    await goto("#/games/steam%3A42");
    expect(search.disabled).toBe(false);
    expect(search.placeholder).toBe("Search games…");

    await goto("#/nowhere");
    expect(search.disabled).toBe(false);
    expect(search.placeholder).toBe("Search games…");
  });

  it("keeps the search visible and takes its query to the Library from a detail page", async () => {
    const control = root.querySelector<HTMLElement>(".search-control")!;
    const search = root.querySelector<HTMLInputElement>("#topbar-search")!;

    await goto("#/games/steam%3A42?from=store");
    // No hidden 352px hole mid-bar on the pages without their own search.
    expect(control.classList.contains("is-hidden")).toBe(false);
    expect(control.getAttribute("aria-hidden")).toBeNull();

    search.value = "hollow";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    await settle();
    expect(window.location.hash).toBe("#/library");
    expect(search.value).toBe("hollow");
  });

  it("keeps the topbar a banner outside every page's main landmark", () => {
    const topbar = root.querySelector<HTMLElement>(".topbar")!;
    expect(topbar.tagName).toBe("HEADER");
    // A <header> inside <main>/<section>/<article> is not a banner.
    expect(topbar.closest("main, section, article, aside, nav")).toBeNull();
    for (const page of root.querySelectorAll<HTMLElement>(".app-page")) {
      expect(page.contains(topbar)).toBe(false);
    }
  });

  it("gives the settings tablist a single Tab stop", async () => {
    await goto("#/settings/plugins");
    const tabs = Array.from(root.querySelectorAll<HTMLButtonElement>("[role='tab']"));
    expect(tabs.length).toBeGreaterThan(1);
    expect(tabs.filter((tab) => tab.tabIndex === 0).map((tab) => tab.dataset.settingsSection)).toEqual(
      ["plugins"],
    );
    expect(tabs.filter((tab) => tab.tabIndex === -1)).toHaveLength(tabs.length - 1);
  });

  it("keeps library shortcuts off every other page", async () => {
    await goto("#/store");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await settle();
    expect(window.location.hash).toBe("#/store");

    await goto("#/settings/general");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "i", bubbles: true }));
    await settle();
    expect(window.location.hash).toBe("#/settings/general");
  });

  it("renders a not-found page with a way back to the library", async () => {
    await goto("#/nowhere");
    expect(visiblePages(root)).toEqual(["app-page-not-found"]);
    const back = root.querySelector<HTMLButtonElement>("[data-app-action='go-library']");
    expect(back).not.toBeNull();

    back?.click();
    await settle();
    expect(visiblePages(root)).toEqual(["app-page-library"]);
  });

  it("deploys on the first card click and opens the detail page on a second click", async () => {
    const cards = Array.from(root.querySelectorAll<HTMLButtonElement>("#game-cards .game-card"));
    expect(cards.length).toBeGreaterThan(1);
    const card = cards[1];
    expect(card.classList.contains("is-selected")).toBe(false);

    card.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    card.click();
    await settle();
    expect(card.classList.contains("is-selected")).toBe(true);
    expect(window.location.hash.startsWith("#/games/")).toBe(false);

    card.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    card.click();
    await settle();
    expect(window.location.hash.startsWith("#/games/")).toBe(true);
    expect(detail.routes.at(-1)?.page).toBe("game");
  });

  it("offers one add-source entry and no store rows in the library menu", () => {
    root.querySelector<HTMLButtonElement>("#library-menu-button")!.click();
    const menu = root.querySelector<HTMLElement>("#library-source-menu")!;
    expect(menu.hidden).toBe(false);

    // Connected stores are managed in Settings, so the Sources list holds
    // nothing at all until a Steam account brings its installed-games import.
    expect(menu.querySelector("[data-library-action='source-steam']")).toBeNull();
    expect(menu.querySelector("#library-source-list")?.textContent).toBe("");
    expect(menu.querySelectorAll("[data-library-action='add-source']")).toHaveLength(1);
  });
});

/**
 * These cover the silent, expensive failures: launching the wrong game, losing
 * a deep-linked Wine attachment, and dropping an import that lands after the
 * user moved on. All three need the desktop backend, so the runtime is faked.
 */
describe("application shell against the desktop backend", () => {
  const alpha = {
    id: "local:alpha",
    title: "Alpha",
    source: "local",
    launchable: true,
  };
  const beta = {
    id: "local:beta",
    title: "Beta",
    source: "local",
    launchable: true,
  };
  let root: HTMLElement;
  const backend = {
    library: [] as Record<string, unknown>[],
    /** When set, `get_library` blocks on it, so a load can be made to land late. */
    gate: null as Promise<void> | null,
    sources: [] as Record<string, unknown>[],
    sourceSync: null as Record<string, unknown> | null,
    providers: [] as Record<string, unknown>[],
  };

  const mount = (): void => {
    root = document.createElement("div");
    document.body.append(root);
    mountApp(root, { storePage: stubPage("Store") });
  };

  const launchedGameIds = (): unknown[] =>
    tauri.invoke.mock.calls
      .filter(([command]) => command === "launch_game")
      .map(([, args]) => args?.gameId);

  const libraryCardIds = (): string[] =>
    Array.from(root.querySelectorAll<HTMLElement>("#game-cards .game-card")).map(
      (card) => card.dataset.gameId ?? "",
    );

  beforeEach(() => {
    window.location.hash = "";
    document.body.replaceChildren();
    // jsdom ships no `matchMedia`; the hero title transition asks for it.
    window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
    backend.library = [alpha];
    backend.gate = null;
    backend.sources = [];
    backend.sourceSync = null;
    backend.providers = [];
    tauri.detailOptions = null;
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.invoke.mockImplementation(async (command) => {
      switch (command) {
        case "get_library":
          if (backend.gate) await backend.gate;
          return backend.library;
        case "import_game":
          return { id: beta.id };
        case "get_preferences":
          return {};
        case "get_steam_account_status":
          return { connected: false, steamId: "", method: "" };
        case "get_wine_runner_settings":
          return {
            runner: { state: "ready", available: true, version: "9.0", message: "" },
            profiles: [],
          };
        case "get_source_accounts":
          return backend.sources;
        case "get_store_home":
          return { providerStatuses: backend.providers };
        case "sync_source_library":
          return backend.sourceSync;
        case "disconnect_source_account":
          return 0;
        default:
          return undefined;
      }
    });
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("never launches anything when the requested game is not in the library", async () => {
    mount();
    await settle();

    // A deep link opened before the library landed names an id the shell has
    // never seen. Falling back to the Library's selection here would launch a
    // completely different game than the one the user is looking at.
    tauri.detailOptions?.play("steam:does-not-exist");
    await settle();
    expect(launchedGameIds()).toEqual([]);

    tauri.detailOptions?.play(alpha.id);
    await settle();
    expect(launchedGameIds()).toEqual([alpha.id]);
  });

  it("lists every connectable store and signs into the one that was clicked", async () => {
    backend.sources = [
      { provider: "gog", label: "GOG", connected: true, accountLabel: "player-one", style: "token", sharesSignInWith: [], launchable: true },
    ];
    mount();
    await goto("#/settings/libraries");

    const rows = Array.from(
      root.querySelectorAll<HTMLElement>("[data-source-row]"),
    ).map((row) => row.dataset.sourceRow);
    // Steam heads the same list as the rest: it is a store like any other to
    // the person reading this page, even though it has its own backend.
    expect(rows).toEqual([
      "steam",
      "epic",
      "gog",
      "ubisoft",
      "xbox",
      "microsoft-store",
      "instant-gaming",
    ]);

    // A connected store offers a sync, a disconnected one offers a connect.
    const action = (provider: string, name: string): HTMLButtonElement | null =>
      root.querySelector(`[data-source-action='${name}'][data-source-provider='${provider}']`);
    expect(action("gog", "sync")).not.toBeNull();
    expect(action("gog", "connect")).toBeNull();
    expect(action("epic", "connect")).not.toBeNull();

    action("epic", "connect")!.click();
    await settle();
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "connect_source_account"),
    ).toEqual([["connect_source_account", { provider: "epic" }]]);
  });

  it("syncs a connected store and reports what could not be read", async () => {
    backend.sources = [
      { provider: "epic", label: "Epic Games", connected: true, accountLabel: "player-one", style: "token", sharesSignInWith: [], launchable: true },
    ];
    backend.sourceSync = {
      provider: "epic",
      label: "Epic Games",
      totalGames: 5,
      importedGames: 4,
      updatedGames: 0,
      skippedGames: 1,
    };
    mount();
    await goto("#/settings/libraries");

    root
      .querySelector<HTMLButtonElement>(
        "[data-source-action='sync'][data-source-provider='epic']",
      )!
      .click();
    await settle();

    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "sync_source_library"),
    ).toEqual([["sync_source_library", { provider: "epic" }]]);
    // A partial import is never rounded up to a clean one.
    expect(root.querySelector("#source-accounts-body")?.textContent).toContain(
      "1 game could not be read",
    );
  });

  it("asks whether to keep imported games before disconnecting a store", async () => {
    backend.sources = [
      { provider: "gog", label: "GOG", connected: true, accountLabel: "player-one", style: "token", sharesSignInWith: [], launchable: true },
    ];
    mount();
    await goto("#/settings/libraries");

    root
      .querySelector<HTMLButtonElement>(
        "[data-source-action='disconnect'][data-source-provider='gog']",
      )!
      .click();
    await settle();

    // Signing out and forgetting a library are two decisions, so nothing is
    // sent until the second one is made.
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "disconnect_source_account"),
    ).toEqual([]);

    root
      .querySelector<HTMLButtonElement>(
        "[data-source-action='disconnect-keep'][data-source-provider='gog']",
      )!
      .click();
    await settle();
    expect(
      tauri.invoke.mock.calls.filter(([command]) => command === "disconnect_source_account"),
    ).toEqual([["disconnect_source_account", { provider: "gog", forgetGames: false }]]);
  });

  it("never paints one game's artwork onto another", async () => {
    // Cached artwork arrives as an opaque `cache:` token, which cannot be
    // resolved to a URL synchronously. Falling back to the first fixture here
    // is what put Elden Ring's cover on every synced game, and only the first
    // handful of cards were ever hydrated back to the truth.
    backend.library = [
      { ...alpha, id: "xbox:1", title: "Sea of Thieves", source: "xbox", coverUrl: "cache:x1-usercover-7.jpg", heroUrl: "", landscapeUrl: "" },
      { ...alpha, id: "xbox:2", title: "It Takes Two", source: "xbox", coverUrl: "", heroUrl: "", landscapeUrl: "" },
    ];
    mount();
    await settle();

    const covers = Array.from(
      root.querySelectorAll<HTMLImageElement>("#game-cards .game-card img"),
    ).map((image) => image.getAttribute("src") ?? "");
    // Whatever a card shows, it is never the artwork of a different game.
    expect(covers.some((source) => source.includes("elden"))).toBe(false);
    expect(covers.some((source) => source.includes("cyberpunk"))).toBe(false);
  });

  it("hydrates every rendered card, not just the first screenful", async () => {
    backend.library = Array.from({ length: 40 }, (_, index) => ({
      ...alpha,
      id: `xbox:${index}`,
      title: `Game ${index}`,
      source: "xbox",
      coverUrl: `cache:game-${index}.jpg`,
      heroUrl: "",
      landscapeUrl: "",
    }));
    mount();
    await settle();
    await settle();

    // A card past the old 16-item cap kept its placeholder forever, because
    // nothing re-runs hydration for a card that is already on screen.
    const cards = root.querySelectorAll("#game-cards .game-card");
    expect(cards.length).toBeGreaterThan(16);
    const covers = Array.from(
      root.querySelectorAll<HTMLImageElement>("#game-cards .game-card img"),
    ).map((image) => image.getAttribute("src") ?? "");
    expect(covers.some((source) => source.includes("elden"))).toBe(false);
  });

  it("shows a game's wordmark in place of the hero title, and falls back to it", async () => {
    // The wordmark is decoded off-screen first. Pointing the visible <img> at
    // it directly brought the title back for the length of every load, so
    // walking the rail was a strobe of titles with the occasional logo.
    const probes: HTMLImageElement[] = [];
    const RealImage = window.Image;
    window.Image = function (this: unknown) {
      const probe = new RealImage();
      Object.defineProperty(probe, "naturalWidth", { value: 512, configurable: true });
      probes.push(probe);
      return probe;
    } as unknown as typeof window.Image;

    try {
      backend.library = [
        { ...alpha, id: "steam:1", title: "Alpha", logoUrl: "https://cdn.example/alpha-logo.png" },
      ];
      mount();
      await settle();

      const logo = root.querySelector<HTMLImageElement>("#hero-logo");
      const title = root.querySelector<HTMLElement>("#hero-title");
      if (!logo || !title) throw new Error("the hero must carry both a logo and a title");

      // Nothing on screen changes until the decode answers.
      expect(title.hidden).toBe(false);
      expect(logo.hidden).toBe(true);
      expect(logo.getAttribute("src")).toBeNull();

      const probe = probes.find((image) => image.src.includes("alpha-logo"));
      expect(probe, "the wordmark must be decoded off-screen").toBeDefined();
      probe?.onload?.(new Event("load"));

      expect(logo.getAttribute("src")).toBe("https://cdn.example/alpha-logo.png");
      expect(logo.hidden).toBe(false);
      expect(logo.alt).toBe("Alpha");
      expect(title.hidden).toBe(true);
    } finally {
      window.Image = RealImage;
    }
  });

  it("keeps the title when a wordmark never decodes", async () => {
    const probes: HTMLImageElement[] = [];
    const RealImage = window.Image;
    window.Image = function (this: unknown) {
      const probe = new RealImage();
      probes.push(probe);
      return probe;
    } as unknown as typeof window.Image;

    try {
      backend.library = [
        { ...alpha, id: "steam:3", title: "Gamma", logoUrl: "https://cdn.example/gone.png" },
      ];
      mount();
      await settle();

      probes.find((image) => image.src.includes("gone"))?.onerror?.(new Event("error"));

      expect(root.querySelector<HTMLImageElement>("#hero-logo")?.hidden).toBe(true);
      expect(root.querySelector<HTMLElement>("#hero-title")?.hidden).toBe(false);
    } finally {
      window.Image = RealImage;
    }
  });

  it("leaves the hero title alone for a game with no wordmark", async () => {
    backend.library = [{ ...alpha, id: "steam:2", title: "Beta", logoUrl: "" }];
    mount();
    await settle();

    expect(root.querySelector<HTMLImageElement>("#hero-logo")?.hidden).toBe(true);
    expect(root.querySelector<HTMLElement>("#hero-title")?.hidden).toBe(false);
  });

  it("keeps a card that stays on screen attached while the window slides", async () => {
    // The rail renders a 48-card window around the selection. Walking through
    // the middle of a longer library slides that window every step, and
    // rebuilding the rail there detached every card — which cancels its
    // transitions, so the cover stopped growing from portrait to landscape for
    // the whole middle of the list and only started again at the ends, where
    // the window is pinned and stops moving.
    backend.library = Array.from({ length: 70 }, (_, index) => ({
      ...alpha,
      id: `xbox:${index}`,
      title: `Game ${index}`,
      source: "xbox",
      // Ordered oldest-last so the rail keeps the backend's order.
      lastPlayedAt: `${index + 1} days ago`,
    }));
    mount();
    await settle();
    await settle();

    const cardFor = (id: string): HTMLElement | null =>
      root.querySelector<HTMLElement>(`#game-cards .game-card[data-game-id="${id}"]`);
    const rail = root.querySelector<HTMLElement>("#game-cards");
    if (!rail) throw new Error("the rail must exist");

    // Land in the middle, where the window really does slide on every step.
    cardFor("xbox:40")?.click();
    await settle();
    const survivor = cardFor("xbox:41");
    expect(survivor, "a card either side of the selection is on screen").not.toBeNull();

    const detached: Node[] = [];
    const observer = new MutationObserver((records) => {
      for (const record of records) detached.push(...Array.from(record.removedNodes));
    });
    observer.observe(rail, { childList: true });

    cardFor("xbox:41")?.click();
    await settle();
    observer.takeRecords().forEach((record) => detached.push(...Array.from(record.removedNodes)));
    observer.disconnect();

    // Something did leave the window, or this test would be proving nothing.
    expect(detached.length).toBeGreaterThan(0);
    expect(detached).not.toContain(survivor);
    expect(cardFor("xbox:41")).toBe(survivor);
  });

  it("shows each store's price-data health on its own row instead of a second card", async () => {
    backend.sources = [
      { provider: "gog", label: "GOG", connected: true, accountLabel: "player-one", style: "token", sharesSignInWith: [], launchable: true },
    ];
    backend.providers = [
      { provider: "gog", label: "GOG", health: "available", message: "Store prices are live.", refreshedAt: null },
      { provider: "humble", label: "Humble", health: "not-configured", message: "No feed configured.", refreshedAt: null },
    ];
    mount();
    await goto("#/settings/libraries");

    // The connectable store carries its health inline, as a quiet dot beside
    // the name rather than a pill that reads like an error about the store.
    const gogRow = root.querySelector<HTMLElement>("[data-source-row='gog']");
    const dot = gogRow?.querySelector(".source-account-row__dot");
    expect(dot?.classList.contains("source-account-row__dot--available")).toBe(true);
    expect(dot?.getAttribute("aria-label")).toBe("Store data: available");
    // …and is not repeated in the store-data-only list below it.
    const remaining = Array.from(
      root.querySelectorAll<HTMLElement>("#provider-status-list .provider-status-row strong"),
    ).map((entry) => entry.textContent);
    expect(remaining).toEqual(["Humble"]);
    // One card now, not two.
    expect(root.querySelectorAll("#provider-status-list")).toHaveLength(1);
    expect(root.querySelector("#source-accounts-panel #provider-status-list")).not.toBeNull();
  });

  it("presents each store in its own colours in Settings and in white in the library", async () => {
    backend.library = [
      { ...alpha, id: "microsoft-store:1", title: "Minecraft", source: "microsoft-store" },
    ];
    mount();
    await goto("#/settings/libraries");

    const settingsMark = root.querySelector<HTMLElement>(
      "[data-source-row='microsoft-store'] .source-account-row__mark",
    );
    expect(settingsMark?.innerHTML).toContain("#f25022");

    await goto("#/library");
    // The store's mark belongs to Settings, the rail and the game's own page.
    // The hero says what the game is and who made it, and nothing else.
    expect(root.querySelector("#hero-source-icon")).toBeNull();
    expect(root.querySelector("#hero-source-label")).toBeNull();
  });

  it("keeps the Play button in one place whatever the game above it is", async () => {
    backend.library = [{ ...alpha, id: "local:aaa", genre: "RPG" }];
    mount();
    await settle();

    // jsdom computes no layout, so the guarantee is asserted structurally: the
    // Play row must be the last thing in the hero, with everything that changes
    // height between two games — the wordmark, a one- or two-line title, the
    // meta line — sealed in the block above it. A row added between the two is
    // exactly what moved the button 64px on every selection.
    const hero = root.querySelector<HTMLElement>(".hero-content");
    const identity = hero?.querySelector<HTMLElement>(".hero-identity");
    const feedback = hero?.querySelector<HTMLElement>("#launch-feedback");
    const actions = hero?.querySelector<HTMLElement>(".hero-actions");
    expect(identity).not.toBeNull();
    expect(actions).not.toBeNull();
    for (const selector of ["#hero-logo", "#hero-title", ".hero-meta"]) {
      expect(identity?.querySelector(selector)).not.toBeNull();
    }
    // The actions row is LAST. Everything that changes height between two
    // games — and the launch status, which appears mid-session — sits above it,
    // where the identity block absorbs the difference.
    expect(Array.from(hero?.children ?? [])).toEqual([identity, feedback, actions]);
  });

  it("gives the hero one line: the genre, then the studio", async () => {
    backend.library = [
      {
        ...alpha,
        id: "epic:Sugar",
        title: "Fall Guys",
        source: "epic",
        genre: "Party",
        developer: "E-Line Media",
      },
    ];
    mount();
    await settle();

    const line = Array.from(
      root.querySelectorAll<HTMLElement>(".hero-meta > span:not([hidden])"),
    ).map((entry) => entry.textContent);
    expect(line).toEqual(["Party", "E-Line Media"]);
  });

  it("leaves the pill off a game whose store published no genre", async () => {
    backend.library = [
      // "Library" is what the backend returns when nothing published a genre,
      // which is every Epic and GOG entitlement. It is an absence, so the pill
      // goes rather than printing a word that says nothing about the game.
      { ...alpha, id: "epic:Sugar", source: "epic", genre: "Library", developer: "E-Line Media" },
    ];
    mount();
    await settle();

    expect(root.querySelector<HTMLElement>("#hero-genre")?.hidden).toBe(true);
    expect(root.querySelector<HTMLElement>("#hero-studio")?.textContent).toBe("E-Line Media");
  });

  it("keeps runtime state out of the studio", async () => {
    // `metadata` is a mixed field: Steam fills it with install state, Wine with
    // the runner name, the bundled demo with an achievement count. None of them
    // is a company, so the hero reads `developer` and nothing else.
    backend.library = [
      { ...alpha, id: "steam:1", source: "steam", metadata: "Not installed" },
      { ...alpha, id: "showcase:1", source: "showcase", metadata: "Achievements 67/82" },
    ];
    mount();
    await settle();

    expect(root.querySelector<HTMLElement>("#hero-studio")?.hidden).toBe(true);
  });

  it("says a game has no macOS build in the Play button, and nowhere else", async () => {
    backend.library = [
      {
        ...alpha,
        id: "epic:Sugar",
        title: "Fall Guys",
        source: "epic",
        launchable: false,
        hostPlatform: "macos",
        installState: "not-installed",
        macCompatibility: "not-native",
      },
    ];
    mount();
    await settle();

    // The chip row is gone: the button is the one control that answers this.
    expect(root.querySelector("#hero-status")).toBeNull();
    // Installing it would download a build this machine cannot run, so the
    // button states the reason and goes quiet instead of offering the download.
    const play = root.querySelector<HTMLButtonElement>("#play-button");
    expect(play?.textContent).toContain("Windows only");
    expect(play?.textContent).not.toContain("Install");
    expect(play?.disabled).toBe(true);
    expect(play?.classList.contains("is-blocked")).toBe(true);
    expect(play?.getAttribute("aria-label")).toBe("Fall Guys has no macOS version");
  });

  it("names what a blocked game does run on instead of assuming Windows", async () => {
    backend.library = [
      {
        ...alpha,
        id: "gog:1",
        title: "Linux Thing",
        source: "gog",
        launchable: false,
        hostPlatform: "macos",
        supportedPlatforms: ["linux"],
        macCompatibility: "not-native",
      },
    ];
    mount();
    await settle();

    // "Windows only" would be a confident lie about a Linux-only GOG title.
    const play = root.querySelector<HTMLButtonElement>("#play-button");
    expect(play?.textContent).toContain("Linux only");
    expect(play?.disabled).toBe(true);
  });

  it("leaves the Play button alone when the same game is browsed on Windows", async () => {
    backend.library = [
      {
        ...alpha,
        id: "epic:Sugar",
        title: "Fall Guys",
        source: "epic",
        launchable: false,
        hostPlatform: "windows",
        installState: "not-installed",
        macCompatibility: "not-native",
      },
    ];
    mount();
    await settle();

    // "No macOS build" is a fact about the game, not about this machine.
    const play = root.querySelector<HTMLButtonElement>("#play-button");
    expect(play?.textContent).toContain("Install");
    expect(play?.disabled).toBe(false);
  });

  it("shows a running Epic download as a percentage instead of a dead button", async () => {
    backend.library = [
      {
        ...alpha,
        id: "epic:Sugar",
        title: "Fall Guys",
        source: "epic",
        launchable: false,
        hostPlatform: "macos",
        installState: "installing",
        installPercent: 37,
        macCompatibility: "native",
      },
    ];
    mount();
    await settle();

    // The button is the progress bar: it carries the number and fills to it.
    const play = root.querySelector<HTMLButtonElement>("#play-button");
    expect(play?.textContent).toContain("Downloading 37%");
    const fill = play?.querySelector<HTMLElement>(".play-button__fill");
    expect(fill?.hidden).toBe(false);
    expect(fill?.style.width).toBe("37%");
    // The label is its own element: writing it into the fill left every button
    // reading "Play" whatever state it was in.
    expect(fill?.textContent).toBe("");
  });

  it("applies a library refresh that lands after the user left the Library", async () => {
    mount();
    await settle();
    expect(libraryCardIds()).toEqual([alpha.id]);

    let releaseLibrary!: () => void;
    backend.gate = new Promise<void>((resolve) => {
      releaseLibrary = resolve;
    });
    root.querySelector<HTMLButtonElement>("[data-library-action='local']")!.click();
    await settle();

    // The import succeeded; the user moves on before the reload lands.
    backend.library = [alpha, beta];
    await goto("#/store");
    backend.gate = null;
    releaseLibrary();
    await settle();

    // A library refresh mutates state every page reads, so navigating away must
    // never discard it.
    await goto("#/library");
    expect(libraryCardIds()).toContain(beta.id);
  });
});

/**
 * The Library when it holds nothing. The bundled showcase games used to stand
 * in for a real catalogue here, which made a fresh install look like a library
 * of ten titles nobody owns — and left the one screen whose whole job is to ask
 * for a connection with nothing to ask for.
 */
describe("the library welcome screen", () => {
  let root: HTMLElement;
  const backend = {
    library: [] as Record<string, unknown>[],
    sources: [] as Record<string, unknown>[],
  };

  const mount = (): void => {
    root = document.createElement("div");
    document.body.append(root);
    mountApp(root, { storePage: stubPage("Store") });
  };

  const onboarding = (): HTMLElement => root.querySelector<HTMLElement>("#library-onboarding")!;
  const panel = (): HTMLElement => root.querySelector<HTMLElement>("#onboarding-panel")!;
  const rowLabels = (): string[] =>
    Array.from(panel().querySelectorAll<HTMLElement>(".onboarding__row strong")).map(
      (label) => label.textContent ?? "",
    );
  const press = (selector: string): void => {
    panel().querySelector<HTMLButtonElement>(selector)!.click();
  };
  const commandArgs = (command: string): unknown[] =>
    tauri.invoke.mock.calls.filter(([name]) => name === command).map(([, args]) => args);

  beforeEach(() => {
    window.location.hash = "";
    document.body.replaceChildren();
    window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
    window.localStorage.clear();
    backend.library = [];
    backend.sources = [];
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.invoke.mockReset();
    tauri.invoke.mockImplementation(async (command) => {
      switch (command) {
        case "get_library":
          return backend.library;
        case "get_source_accounts":
          return backend.sources;
        case "get_steam_account_status":
          return { connected: false, steamId: "", method: "" };
        case "import_game":
          return { id: "local:imported" };
        case "get_preferences":
          return {};
        default:
          return undefined;
      }
    });
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
  });

  it("shows the welcome screen rather than the bundled showcase games", async () => {
    mount();
    await settle();

    expect(onboarding().hidden).toBe(false);
    expect(root.querySelector(".app-page--library")?.classList.contains("is-empty")).toBe(true);
    expect(root.querySelectorAll("#game-cards .game-card")).toHaveLength(0);
    // Not in the rail, and not in the hero either: the shell used to ship a
    // fixture's title as the heading's placeholder text.
    expect(root.querySelector(".app-page--library")?.textContent).not.toContain("Elden Ring");
    expect(root.querySelector("#hero-title")?.textContent).toBe("");
    expect(panel().textContent).toContain("Your library is empty");
  });

  it("plays its arrival once, and never again on a status repaint", async () => {
    mount();
    await settle();

    expect(onboarding().classList.contains("is-entering")).toBe(true);
    // A view that has not changed must not slide: a sign-in window reporting
    // back repaints the same panel several times in a row.
    expect(panel().querySelector(".onboarding__view--forward")).toBeNull();
    expect(panel().querySelector(".onboarding__view--back")).toBeNull();
  });

  it("walks from the choice to a store's sign-in page and back again", async () => {
    mount();
    await settle();

    press("[data-onboarding-action='sources']");
    expect(rowLabels()).toEqual([
      "Steam",
      "Epic Games",
      "GOG",
      "Ubisoft Connect",
      "Xbox",
      "Microsoft Store",
      "Instant Gaming",
    ]);
    expect(panel().querySelector(".onboarding__view--forward")).not.toBeNull();

    // The account statuses land in the same tick the row was pressed. That
    // repaint must not replace the DOM the slide has just started on.
    await settle();
    expect(panel().querySelector(".onboarding__view--forward")).not.toBeNull();
    expect(rowLabels()).toHaveLength(7);

    press("[data-onboarding-action='choose-source'][data-onboarding-provider='epic']");
    expect(panel().textContent).toContain("Connect Epic Games");
    // Every connect page says how the sign-in will feel before it starts one.
    expect(panel().querySelector(".onboarding__facts")?.textContent).toMatch(/keychain/i);

    press("[data-onboarding-action='back']");
    expect(panel().querySelector(".onboarding__view--back")).not.toBeNull();
    expect(rowLabels()).toHaveLength(7);
  });

  it("says up front which stores it cannot launch", async () => {
    mount();
    await settle();

    press("[data-onboarding-action='sources']");
    await settle();
    press("[data-onboarding-action='choose-source'][data-onboarding-provider='xbox']");
    expect(panel().textContent).toContain("cannot launch them yet");
  });

  it("connects the store it was pointed at, and leaves as soon as a game lands", async () => {
    mount();
    await settle();

    press("[data-onboarding-action='sources']");
    await settle();
    press("[data-onboarding-action='choose-source'][data-onboarding-provider='gog']");

    backend.library = [{ id: "gog:one", title: "One", source: "gog", launchable: true }];
    press("[data-onboarding-action='connect']");
    await settle();

    expect(commandArgs("connect_source_account")).toEqual([{ provider: "gog" }]);
    expect(onboarding().hidden).toBe(true);
    expect(root.querySelector(".app-page--library")?.classList.contains("is-empty")).toBe(false);
    expect(
      Array.from(root.querySelectorAll<HTMLElement>("#game-cards .game-card")).map(
        (card) => card.dataset.gameId,
      ),
    ).toEqual(["gog:one"]);
  });

  it("imports a local game without leaving the welcome screen", async () => {
    mount();
    await settle();

    press("[data-onboarding-action='local']");
    await settle();
    expect(commandArgs("import_game")).toHaveLength(1);
  });

  it("gives Escape back to the panel instead of to the rail", async () => {
    mount();
    await settle();

    press("[data-onboarding-action='sources']");
    await settle();
    expect(rowLabels()).toHaveLength(7);

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(panel().textContent).toContain("Your library is empty");
  });

});

/**
 * The bell. Everything it carries is optional advice about an optional key, so
 * the rules that matter are the quiet ones: no dot without something unread,
 * and no notice ever shown twice.
 */
describe("application shell notifications", () => {
  let root: HTMLElement;

  const mount = (): void => {
    root = document.createElement("div");
    document.body.append(root);
    mountApp(root, { storePage: stubPage("Store") });
  };

  const bell = (): HTMLButtonElement =>
    root.querySelector<HTMLButtonElement>("#notifications-button")!;
  const dot = (): HTMLElement => root.querySelector<HTMLElement>("#notifications-dot")!;
  const panel = (): HTMLElement => root.querySelector<HTMLElement>("#notifications-panel")!;

  beforeEach(() => {
    window.location.hash = "";
    document.body.replaceChildren();
    window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
    window.localStorage.clear();
    tauri.invoke.mockReset();
    tauri.invoke.mockResolvedValue(undefined);
  });

  it("stays quiet with nothing to say", async () => {
    mount();
    await settle();

    expect(dot().hidden).toBe(true);
    expect(bell().getAttribute("aria-label")).toBe("Notifications");

    bell().click();
    expect(panel().hidden).toBe(false);
    expect(panel().textContent).toContain("Nothing to report");
  });

  it("carries a delivered notice, clears its dot when read, and forgets it when dismissed", async () => {
    window.localStorage.setItem(
      "orivo.notifications.v1",
      JSON.stringify({ delivered: ["artwork-keys"], read: [], dismissed: [] }),
    );
    mount();
    await settle();

    expect(dot().hidden).toBe(false);
    expect(bell().getAttribute("aria-label")).toBe("Notifications, 1 unread");

    bell().click();
    // Opening is what counts as reading — but the card keeps its highlight
    // until the panel closes, or the new notice loses it on arrival.
    expect(dot().hidden).toBe(true);
    expect(panel().querySelector(".notification-card")?.classList.contains("is-unread")).toBe(
      true,
    );
    expect(panel().textContent).toContain("Sharper artwork");

    panel().querySelector<HTMLButtonElement>("[data-notification-action='dismiss']")!.click();
    expect(panel().querySelector(".notification-card")).toBeNull();

    // Dismissal is final, and it survives a restart.
    document.body.replaceChildren();
    mount();
    await settle();
    expect(root.querySelector(".notification-card")).toBeNull();
    expect(dot().hidden).toBe(true);
  });

  it("takes the artwork notice straight to the keys it is about", async () => {
    window.localStorage.setItem(
      "orivo.notifications.v1",
      JSON.stringify({ delivered: ["artwork-keys"], read: [], dismissed: [] }),
    );
    mount();
    await settle();

    bell().click();
    panel().querySelector<HTMLButtonElement>("[data-notification-action='open']")!.click();
    await settle();

    expect(window.location.hash).toBe("#/settings/plugins");
    // The keys live one level inside the plugin browser, which no route names.
    expect(root.querySelector<HTMLElement>("#wallpaper-plugin-panel")!.hidden).toBe(false);
    expect(panel().hidden).toBe(true);
  });

  it("retires the price notice once the provider list has been seen", async () => {
    mount();
    await settle();
    await goto("#/settings/libraries");

    const stored = JSON.parse(window.localStorage.getItem("orivo.notifications.v1") ?? "{}");
    expect(stored.visitedLibrarySources).toBe(true);
  });
});
