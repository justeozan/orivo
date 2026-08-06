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

  it("lists connected sources and one add-source entry in the library menu", () => {
    root.querySelector<HTMLButtonElement>("#library-menu-button")!.click();
    const menu = root.querySelector<HTMLElement>("#library-source-menu")!;
    expect(menu.hidden).toBe(false);

    // No Steam source is connected in the fallback library, so only the
    // always-present local source is listed.
    expect(menu.querySelector("[data-library-action='source-steam']")).toBeNull();
    expect(menu.querySelector("#library-source-list")?.textContent).toContain("Local");
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
    expect(rows).toEqual([
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

  it("never lets a store's display name inject markup into the Sources menu", async () => {
    // The account label is whatever the store returned — an Epic display name,
    // a gamertag, a GOG username. It is untrusted text.
    backend.sources = [
      {
        provider: "epic",
        label: "Epic Games",
        connected: true,
        accountLabel: '<img src=x onerror="document.title=\'pwned\'">',
        style: "token",
        sharesSignInWith: [],
        launchable: true,
      },
    ];
    backend.library = [{ ...alpha, id: "epic:1", title: "Fall Guys", source: "epic" }];
    mount();
    await goto("#/settings/libraries");
    await goto("#/library");

    root.querySelector<HTMLButtonElement>("#library-menu-button")!.click();
    await settle();

    const entry = root.querySelector<HTMLElement>(
      "[data-library-action='source-connected'][data-source-provider='epic']",
    );
    expect(entry).not.toBeNull();
    // The name renders as text, never as an element.
    expect(entry!.querySelector("img")).toBeNull();
    expect(entry!.textContent).toContain("onerror");
    expect(document.title).not.toBe("pwned");
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
    // The hero badge inherits the near-white text colour, so it must stay
    // monochrome rather than carrying the brand palette onto the artwork.
    const heroMark = root.querySelector<HTMLElement>("#hero-source-icon");
    expect(heroMark?.innerHTML).toContain("currentColor");
    expect(heroMark?.innerHTML).not.toContain("#f25022");
  });

  it("lists a store in the library Sources menu once it has games", async () => {
    backend.library = [
      { ...alpha, id: "epic:Sugar", title: "Fall Guys", source: "epic" },
    ];
    mount();
    await settle();

    root.querySelector<HTMLButtonElement>("#library-menu-button")!.click();
    await settle();

    const entry = root.querySelector<HTMLElement>(
      "[data-library-action='source-connected'][data-source-provider='epic']",
    );
    expect(entry?.textContent).toContain("Epic Games");
    // Its badge is the store's own, never the local fallback.
    expect(root.querySelector("#hero-source-label")?.textContent).toBe("Epic Games");
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
