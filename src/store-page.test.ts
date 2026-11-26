import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute, GameSummary, StoreOffer } from "./contracts";
import { setLanguage } from "./i18n";
import { PageLifecycleHost } from "./page-lifecycle";
import type { StoreBrowsePage, StoreBrowseRequest, StoreHomeView } from "./store-model";
import { EDITORIAL_GAMES, formatPrice, selectBestOffer } from "./store-model";
import { createStorePage, DETAIL_SLIDE_MS, detailSlides, type StorePageClient } from "./store-page";
import { registerStoreTab, resetStoreTabsForTests, unregisterPluginStoreTabs } from "./store-tabs";

// The bridge to the host is mocked at its edge: the page must ask it for the
// plugins' stores, and open a plugin game's page through it.
const pluginBridge = vi.hoisted(() => ({
  sync: vi.fn(async () => {}),
  open: vi.fn(async (_gameId: string) => {}),
}));
vi.mock("./plugin-store-tabs", () => ({
  syncPluginStoreTabs: pluginBridge.sync,
  openPluginStoreGamePage: pluginBridge.open,
}));

const HOUR = 60 * 60 * 1_000;

function offer(overrides: Partial<StoreOffer> = {}): StoreOffer {
  return {
    id: "offer_1",
    gameId: "store:1",
    provider: "steam",
    providerLabel: "Steam",
    priceMinor: null,
    currency: null,
    region: "US",
    verifiedAt: null,
    availability: "unknown",
    stale: true,
    ...overrides,
  };
}

function game(overrides: Partial<GameSummary> = {}): GameSummary {
  const id = overrides.id ?? "store:1";
  return {
    id,
    title: "Test Game",
    source: "store",
    shortDescription: "A test entry.",
    coverUrl: "/media/cover.jpg",
    heroUrl: "/media/hero.jpg",
    landscapeUrl: "/media/landscape.jpg",
    genres: ["Action"],
    tags: ["Open World"],
    supportedPlatforms: ["windows"],
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: ["Because you play strategy games"],
    offers: [offer({ gameId: id })],
    ...overrides,
  };
}

function homeView(overrides: Partial<StoreHomeView> = {}): StoreHomeView {
  return {
    games: [game()],
    providerStatuses: [],
    recommendationMode: "editorial",
    recommendationHeading: "Editorial picks",
    refreshedAt: null,
    ...overrides,
  };
}

function browsePage(overrides: Partial<StoreBrowsePage> = {}): StoreBrowsePage {
  return { games: [], nextCursor: null, providerStatuses: [], ...overrides };
}

function storeRoute(overrides: Partial<Extract<AppRoute, { page: "store" }>> = {}): AppRoute {
  return { page: "store", store: "all", query: "", ...overrides };
}

interface RecordedClient {
  client: StorePageClient;
  commands: string[];
  browseRequests: StoreBrowseRequest[];
  wishlistCalls: Array<{ gameId: string; wishlisted: boolean }>;
  homeSignals: AbortSignal[];
}

function createFakeClient(overrides: Partial<StorePageClient> = {}): RecordedClient {
  const commands: string[] = [];
  const browseRequests: StoreBrowseRequest[] = [];
  const wishlistCalls: Array<{ gameId: string; wishlisted: boolean }> = [];
  const homeSignals: AbortSignal[] = [];
  const client: StorePageClient = {
    async getHome(signal) {
      commands.push("getHome");
      homeSignals.push(signal);
      return overrides.getHome ? overrides.getHome(signal) : homeView();
    },
    async listOwnedGameIds(signal) {
      commands.push("listOwnedGameIds");
      return overrides.listOwnedGameIds ? overrides.listOwnedGameIds(signal) : [];
    },
    async browse(request, signal) {
      commands.push("browse");
      browseRequests.push(request);
      return overrides.browse ? overrides.browse(request, signal) : browsePage();
    },
    async refreshSources(signal) {
      commands.push("refreshSources");
      if (overrides.refreshSources) await overrides.refreshSources(signal);
    },
    async setWishlist(gameId, wishlisted, signal) {
      commands.push("setWishlist");
      wishlistCalls.push({ gameId, wishlisted });
      if (overrides.setWishlist) await overrides.setWishlist(gameId, wishlisted, signal);
    },
    async openOffer(offerId, signal) {
      commands.push("openOffer");
      if (overrides.openOffer) await overrides.openOffer(offerId, signal);
    },
  };
  return { client, commands, browseRequests, wishlistCalls, homeSignals };
}

function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

let host: PageLifecycleHost | null = null;
let container: HTMLElement | null = null;

function mountStore(client: StorePageClient): {
  container: HTMLElement;
  host: PageLifecycleHost;
  navigations: AppRoute[];
} {
  const element = document.createElement("section");
  document.body.append(element);
  const navigations: AppRoute[] = [];
  const page = createStorePage({ navigate: (route) => navigations.push(route), client });
  const lifecycle = new PageLifecycleHost(element, page);
  container = element;
  host = lifecycle;
  return { container: element, host: lifecycle, navigations };
}

const pageRoot = (root: HTMLElement): HTMLElement => {
  const node = root.querySelector<HTMLElement>(".store-page");
  if (!node) throw new Error("The Store page did not mount.");
  return node;
};

const cardTitles = (root: HTMLElement): string[] =>
  [...root.querySelectorAll(".store-card__title")].map((node) => node.textContent ?? "");

const byFocusKey = (root: HTMLElement, key: string): HTMLElement => {
  const node = root.querySelector<HTMLElement>(`[data-focus-key="${key}"]`);
  if (!node) throw new Error(`No element with focus key ${key}`);
  return node;
};

const cardFor = (root: HTMLElement, gameId: string): HTMLElement => {
  const node = root.querySelector<HTMLElement>(`.store-card[data-game-id="${gameId}"]`);
  if (!node) throw new Error(`No card for ${gameId}`);
  return node;
};

const detail = (root: HTMLElement): HTMLElement => {
  const node = root.querySelector<HTMLElement>(".store-detail");
  if (!node) throw new Error("The Store page has no detail.");
  return node;
};

/** Opens a game's detail from its card and returns the line under its price. */
const detailOfferLine = (root: HTMLElement, gameId: string): string => {
  byFocusKey(root, `game-${gameId}`).click();
  return detail(root).querySelector(".store-detail__offer")?.textContent ?? "";
};

const next = (root: HTMLElement): HTMLElement => byFocusKey(root, "store-next");
const previous = (root: HTMLElement): HTMLElement => byFocusKey(root, "store-previous");

function setOnline(online: boolean): void {
  Object.defineProperty(window.navigator, "onLine", {
    configurable: true,
    get: () => online,
  });
}

beforeEach(() => {
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    callback(0);
    return 1;
  });
  setOnline(true);
});

afterEach(() => {
  host?.deactivate();
  host = null;
  container?.remove();
  container = null;
  resetStoreTabsForTests();
  setLanguage("en");
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("Store chevrons", () => {
  const plain = game({ id: "plain", title: "Plain Game" });
  const other = game({ id: "other", title: "Other Game" });
  const astro = game({ id: "astro", title: "Astro Bot", exclusiveTo: "playstation" });
  const halo = game({ id: "halo", title: "Halo Infinite", exclusiveTo: "xbox" });

  it("walks the stores in a loop, one chevron each way, and prints no bar of names", async () => {
    const mounted = mountStore(createFakeClient().client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(mounted.container.querySelector(".store-tabs, .store-tab")).toBeNull();
    expect(next(mounted.container).getAttribute("aria-label")).toBe("Next store: PlayStation");
    expect(previous(mounted.container).getAttribute("aria-label")).toBe("Previous store: Epic exclusives");
    expect(next(mounted.container).textContent?.trim()).toBe("");

    next(mounted.container).click();
    expect(mounted.navigations.at(-1)).toEqual({ page: "store", store: "playstation", query: "" });
    previous(mounted.container).click();
    previous(mounted.container).click();
    expect(mounted.navigations.at(-1)).toEqual({ page: "store", store: "epic", query: "" });
    next(mounted.container).click();
    expect(mounted.navigations.at(-1)).toEqual({ page: "store", store: "all", query: "" });
  });

  it("keeps the whole shelf under Tout while nothing is exclusive, and shows a brand empty", async () => {
    const fake = createFakeClient({ getHome: async () => homeView({ games: [plain, other] }) });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(cardTitles(mounted.container)).toEqual(["Plain Game", "Other Game"]);

    next(mounted.container).click();
    expect(pageRoot(mounted.container).dataset.store).toBe("playstation");
    expect(mounted.container.querySelectorAll(".store-card")).toHaveLength(0);
    const empty = mounted.container.querySelector<HTMLElement>(".store-empty");
    expect(empty?.hidden).toBe(false);
    expect(empty?.textContent).toBe("No exclusives yet");
  });

  it("shows only exclusives once one exists, each brand its own, without asking the host again", async () => {
    const fake = createFakeClient({ getHome: async () => homeView({ games: [plain, astro, other, halo] }) });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Astro Bot", "Halo Infinite"]);

    const homeCalls = fake.commands.filter((command) => command === "getHome").length;
    await mounted.host.activate(storeRoute({ store: "xbox" }));
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Halo Infinite"]);
    expect(pageRoot(mounted.container).dataset.store).toBe("xbox");
    expect(mounted.container.querySelector(".store-mast__title")?.textContent).toContain("Xbox");
    // The shelf a store filters is the one already loaded.
    expect(fake.commands.filter((command) => command === "getHome")).toHaveLength(homeCalls);
  });

  it("reads its labels in the interface's language when it renders, not when it loads", async () => {
    setLanguage("fr");
    const fake = createFakeClient({
      getHome: async () =>
        homeView({ games: ["a", "b", "c", "d"].map((id) => game({ id, title: id.toUpperCase() })) }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute({ store: "nintendo" }));
    await flush();

    expect(previous(mounted.container).getAttribute("aria-label")).toBe("Store précédent : Xbox");
    expect(next(mounted.container).getAttribute("aria-label")).toBe("Store suivant : Exclusivité Steam");
    await mounted.host.activate(storeRoute({ store: "all" }));
    await flush();
    expect(previous(mounted.container).getAttribute("aria-label")).toBe("Store précédent : Exclu Epic Games");
    expect(
      [...mounted.container.querySelectorAll(".store-section__title")].map((node) => node.textContent),
    ).toEqual(["Recommandé pour toi", "Tous les jeux"]);
  });

  it("numbers the best-reviewed games on their published score", async () => {
    const games = [
      game({ id: "a", title: "A" }),
      game({ id: "b", title: "B", criticScore: 88 }),
      game({ id: "c", title: "C" }),
      game({ id: "d", title: "D", criticScore: 95 }),
      game({ id: "e", title: "E" }),
    ];
    const mounted = mountStore(createFakeClient({ getHome: async () => homeView({ games }) }).client);
    await mounted.host.activate(storeRoute());
    await flush();

    const ranked = [...mounted.container.querySelectorAll<HTMLElement>(".store-card--ranked")];
    expect(ranked.map((card) => card.dataset.gameId)).toEqual(["d", "b"]);
    expect(ranked.map((card) => card.querySelector(".store-card__rank")?.textContent)).toEqual(["1", "2"]);
    expect(byFocusKey(mounted.container, "game-d").getAttribute("aria-label")).toBe("No. 1: D");
  });
});

describe("Store plugin tabs", () => {
  it("walks into a store a plugin adds while the Store is open, on its own colour, and out once removed", async () => {
    const mounted = mountStore(createFakeClient().client);
    await mounted.host.activate(storeRoute({ store: "epic" }));
    await flush();
    expect(next(mounted.container).getAttribute("aria-label")).toBe("Next store: All");

    registerStoreTab({
      pluginId: "io.orivo.demo",
      label: "Demo deals",
      accent: "#ff8800",
      games: [game({ id: "deal", title: "Plugin Deal" })],
    });
    expect(next(mounted.container).getAttribute("aria-label")).toBe("Next store: Demo deals");
    next(mounted.container).click();
    expect(mounted.navigations.at(-1)).toEqual({ page: "store", store: "plugin:io.orivo.demo", query: "" });

    await mounted.host.activate(storeRoute({ store: "plugin:io.orivo.demo" }));
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Plugin Deal"]);
    expect(mounted.container.querySelector(".store-mast__title")?.textContent).toBe("Demo deals");
    expect(pageRoot(mounted.container).dataset.store).toBe("plugin");
    expect(pageRoot(mounted.container).style.getPropertyValue("--store-accent")).toBe("#ff8800");

    unregisterPluginStoreTabs("io.orivo.demo");
    expect(mounted.container.querySelectorAll(".store-card")).toHaveLength(0);
    expect(next(mounted.container).getAttribute("aria-label")).toBe("Next store: All");
  });

  it("asks the host for the plugins' stores on every visit", async () => {
    pluginBridge.sync.mockClear();
    const mounted = mountStore(createFakeClient().client);
    await mounted.host.activate(storeRoute());
    await mounted.host.activate(storeRoute({ store: "xbox" }));
    expect(pluginBridge.sync).toHaveBeenCalledTimes(2);
  });

  it("prints a plugin game's own price and opens its page through the host", async () => {
    pluginBridge.open.mockClear();
    registerStoreTab({
      pluginId: "io.orivo.demo",
      label: "Demo deals",
      games: [
        {
          ...game({ id: "deal", title: "Plugin Deal", offers: [] }),
          pluginPrice: { currency: "EUR", amountMinor: 1_999, regularAmountMinor: null },
          hasPluginPage: true,
        },
      ],
    });
    const mounted = mountStore(createFakeClient().client);
    await mounted.host.activate(storeRoute({ store: "plugin:io.orivo.demo" }));
    await flush();

    const card = cardFor(mounted.container, "io.orivo.demo:deal");
    expect(card.querySelector(".store-card__price")?.textContent).toMatch(/19[.,]99/);
    // Not a shop's game: nothing to wishlist, no Orivo game page.
    expect(card.querySelector(".store-card__wishlist")).toBeNull();

    byFocusKey(mounted.container, "game-io.orivo.demo:deal").click();
    expect(detail(mounted.container).querySelector(".store-detail__offer")?.textContent).toBe("Demo deals");
    expect(mounted.container.querySelector("[data-focus-key='detail-page']")).toBeNull();
    byFocusKey(mounted.container, "detail-buy").click();
    expect(pluginBridge.open).toHaveBeenCalledWith("io.orivo.demo:deal");
  });

  it("never paints a plugin's colour that is not a plain hex colour", async () => {
    registerStoreTab({ pluginId: "io.orivo.demo", label: "Demo", accent: "red; background: url(x)", games: [] });
    const mounted = mountStore(createFakeClient().client);
    await mounted.host.activate(storeRoute({ store: "plugin:io.orivo.demo" }));
    await flush();
    expect(pageRoot(mounted.container).style.getPropertyValue("--store-accent")).toBe("#7d54f4");
  });
});

describe("Store search", () => {
  const unrailed = game({ id: "unrailed", title: "Unrailed" });
  const baldur = game({ id: "baldur", title: "Baldur's Gate 3" });

  it("asks the host for a search on Tout and shows its answer as results", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [unrailed, baldur] }),
      browse: async () => browsePage({ games: [unrailed] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute({ query: "unrailed" }));
    await flush();

    expect(fake.browseRequests).toEqual([
      { category: "all-games", platforms: [], query: "unrailed", cursor: null, limit: 48 },
    ]);
    expect(cardTitles(mounted.container)).toEqual(["Unrailed"]);
    expect(mounted.container.querySelector(".store-section__title")?.textContent).toBe("Results");
  });

  it("never paints a search answer that a newer one already superseded", async () => {
    const first = deferred<StoreBrowsePage>();
    const second = deferred<StoreBrowsePage>();
    const pending = [first, second];
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [unrailed, baldur] }),
      browse: async () => pending.shift()?.promise ?? browsePage(),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute({ query: "a" }));
    await flush();
    await mounted.host.activate(storeRoute({ query: "b" }));
    await flush();

    second.resolve(browsePage({ games: [game({ id: "second", title: "Second Result" })] }));
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Second Result"]);

    first.resolve(browsePage({ games: [game({ id: "first", title: "First Result" })] }));
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Second Result"]);
  });

  it("drops a home that resolves after the activation was cancelled", async () => {
    const first = deferred<StoreHomeView>();
    const second = deferred<StoreHomeView>();
    const pending = [first, second];
    const currentHome = homeView({ games: [game({ id: "current", title: "Current Home" })] });
    const fake = createFakeClient({
      // Later background refreshes re-read the same live home payload.
      getHome: async () => pending.shift()?.promise ?? currentHome,
    });
    const mounted = mountStore(fake.client);

    await mounted.host.activate(storeRoute());
    await mounted.host.activate(storeRoute());
    await flush();

    expect(fake.homeSignals[0].aborted).toBe(true);
    expect(fake.homeSignals[1].aborted).toBe(false);

    second.resolve(currentHome);
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Current Home"]);

    first.resolve(homeView({ games: [game({ id: "stale", title: "Cancelled Home" })] }));
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Current Home"]);
  });
});

describe("Store page offline", () => {
  it("still renders saved editorial picks when the Store is offline", async () => {
    setOnline(false);
    const fake = createFakeClient({
      getHome: async () => {
        throw new Error("Live Store sources could not be reached.");
      },
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(mounted.container.querySelector(".store-status--offline")).not.toBeNull();
    expect(fake.commands).toEqual(["listOwnedGameIds", "getHome"]);
    // Cards, not titles: games whose art carries their wordmark print no title.
    expect(mounted.container.querySelectorAll(".store-card").length).toBe(EDITORIAL_GAMES.length);
    expect(cardFor(mounted.container, EDITORIAL_GAMES[0].id)).not.toBeNull();
    expect(mounted.container.querySelector(".store-empty")?.hasAttribute("hidden")).toBe(true);
  });
});

describe("Store detail", () => {
  const exclusive = game({
    id: "astro",
    title: "Astro Bot",
    exclusiveTo: "playstation",
    heroUrl: "/media/astro/hero.jpg",
    landscapeUrl: "/media/astro/capsule.jpg",
    gallery: ["/media/astro/scene-1.jpg", "/media/astro/scene-2.jpg"],
    offers: [
      offer({
        id: "offer_ps_astro",
        gameId: "astro",
        provider: "playstation",
        providerLabel: "PlayStation Store",
        priceMinor: 5_999,
        currency: "EUR",
        availability: "available",
        stale: false,
        verifiedAt: new Date().toISOString(),
      }),
    ],
  });

  const openAstro = async (): Promise<ReturnType<typeof mountStore> & { fake: RecordedClient }> => {
    const fake = createFakeClient({ getHome: async () => homeView({ games: [exclusive] }) });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();
    byFocusKey(mounted.container, "game-astro").click();
    return { ...mounted, fake };
  };

  it("opens from its card with the price, the shop and the exclusivity, and shuts the page behind it", async () => {
    const mounted = await openAstro();
    const dialog = detail(mounted.container);

    expect(dialog.hidden).toBe(false);
    expect(dialog.getAttribute("role")).toBe("dialog");
    expect(dialog.getAttribute("aria-modal")).toBe("true");
    expect(dialog.querySelector(".store-detail__title")?.textContent).toBe("Astro Bot");
    expect(dialog.querySelector(".store-detail__amount")?.textContent).toMatch(/59[.,]99/);
    expect(dialog.querySelector(".store-detail__offer")?.textContent).toBe("PlayStation Store · verified today");
    expect(dialog.querySelector(".store-pill--brand")?.textContent).toBe("PlayStation exclusive");
    // The page behind is out of reach; the dialog is not inside what was shut.
    const body = mounted.container.querySelector<HTMLElement>(".store-page__body");
    expect(body?.inert).toBe(true);
    expect(body?.contains(dialog)).toBe(false);
    expect((document.activeElement as HTMLElement | null)?.dataset.focusKey).toBe("detail-buy");
  });

  it("buys through the shop, opens the game page, and closes on Escape back onto its card", async () => {
    const mounted = await openAstro();
    byFocusKey(mounted.container, "detail-buy").click();
    await flush();
    expect(mounted.fake.commands.at(-1)).toBe("openOffer");

    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", cancelable: true }));
    expect(detail(mounted.container).hidden).toBe(true);
    expect(mounted.container.querySelector<HTMLElement>(".store-page__body")?.inert).toBe(false);
    expect((document.activeElement as HTMLElement | null)?.dataset.focusKey).toBe("game-astro");

    byFocusKey(mounted.container, "game-astro").click();
    byFocusKey(mounted.container, "detail-page").click();
    expect(mounted.navigations.at(-1)).toEqual({ page: "game", gameId: "astro", from: "store" });
    expect(detail(mounted.container).hidden).toBe(true);
  });

  it("keeps Tab inside the dialog", async () => {
    const mounted = await openAstro();
    const dialog = detail(mounted.container);
    const stops = [...dialog.querySelectorAll<HTMLElement>("button")];
    const last = stops[stops.length - 1];
    last.focus();
    const forward = new KeyboardEvent("keydown", { key: "Tab", bubbles: true, cancelable: true });
    last.dispatchEvent(forward);
    expect(forward.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(stops[0]);

    const backward = new KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true, cancelable: true });
    stops[0].dispatchEvent(backward);
    expect(backward.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(last);
  });

  it("cycles its pictures on a timer, and leaves no timer behind once closed", async () => {
    expect(detailSlides(exclusive)).toEqual([
      "/media/astro/hero.jpg",
      "/media/astro/scene-1.jpg",
      "/media/astro/scene-2.jpg",
      "/media/astro/capsule.jpg",
    ]);
    const mounted = await openAstro();
    vi.useFakeTimers();
    const started = vi.spyOn(globalThis, "setInterval");
    const stopped = vi.spyOn(globalThis, "clearInterval");
    // Reopened under fake timers, so the slideshow's interval is one we hold.
    byFocusKey(mounted.container, "detail-close").click();
    byFocusKey(mounted.container, "game-astro").click();
    const active = (): string | null =>
      detail(mounted.container).querySelector<HTMLImageElement>(".store-detail__slide.is-active")?.getAttribute("src") ?? null;

    expect(active()).toBe("/media/astro/hero.jpg");
    vi.advanceTimersByTime(DETAIL_SLIDE_MS);
    expect(active()).toBe("/media/astro/scene-1.jpg");
    vi.advanceTimersByTime(DETAIL_SLIDE_MS * 3);
    expect(active()).toBe("/media/astro/hero.jpg");

    expect(started).toHaveBeenCalledTimes(1);
    const interval = started.mock.results[0].value;
    byFocusKey(mounted.container, "detail-close").click();
    expect(stopped).toHaveBeenCalledWith(interval);
  });

  it("holds the pictures still under reduced motion", async () => {
    document.documentElement.dataset.motion = "reduced";
    try {
      const mounted = await openAstro();
      const started = vi.spyOn(globalThis, "setInterval");
      byFocusKey(mounted.container, "detail-close").click();
      byFocusKey(mounted.container, "game-astro").click();
      expect(detail(mounted.container).hidden).toBe(false);
      expect(started).not.toHaveBeenCalled();
    } finally {
      delete document.documentElement.dataset.motion;
    }
  });

});

describe("Store page offer facts", () => {
  it("does not invent a price and dates a reading that is no longer recent", async () => {
    const now = Date.now();
    const fake = createFakeClient({
      getHome: async () =>
        homeView({
          games: [
            game({
              id: "unpriced",
              title: "Unpriced Game",
              offers: [
                offer({
                  gameId: "unpriced",
                  provider: "instant-gaming",
                  providerLabel: "Instant Gaming",
                  priceMinor: null,
                  currency: null,
                }),
              ],
            }),
            game({
              id: "outdated",
              title: "Outdated Price",
              offers: [
                offer({
                  gameId: "outdated",
                  priceMinor: 1_999,
                  currency: "USD",
                  availability: "available",
                  stale: false,
                  verifiedAt: new Date(now - 30 * HOUR).toISOString(),
                }),
              ],
            }),
            game({
              id: "verified",
              title: "Verified Price",
              offers: [
                offer({
                  gameId: "verified",
                  priceMinor: 2_999,
                  currency: "USD",
                  availability: "available",
                  stale: false,
                  verifiedAt: new Date(now - HOUR).toISOString(),
                }),
              ],
            }),
          ],
        }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    // No known price: the card leaves the slot empty, and so does the detail.
    expect(cardFor(mounted.container, "unpriced").querySelector(".store-card__price")).toBeNull();
    expect(detailOfferLine(mounted.container, "unpriced")).toBe("Available on Instant Gaming · price not verified");
    expect(detail(mounted.container).querySelector(".store-detail__amount")).toBeNull();
    byFocusKey(mounted.container, "detail-close").click();

    expect(cardFor(mounted.container, "outdated").querySelector(".store-card__price")?.textContent).toMatch(/19[.,]99/);
    expect(detailOfferLine(mounted.container, "outdated")).toMatch(/^Steam · read on .+$/);
    byFocusKey(mounted.container, "detail-close").click();

    expect(cardFor(mounted.container, "verified").querySelector(".store-card__price")?.textContent).toMatch(/29[.,]99/);
    expect(detailOfferLine(mounted.container, "verified")).toBe("Steam · verified today");
  });

  it("shows the bundled catalogue's own price, and nothing where no shop quoted one", async () => {
    const priced = EDITORIAL_GAMES.find((entry) => formatPrice(selectBestOffer(entry)) !== "");
    if (!priced) throw new Error("The bundled catalogue carries no priced game.");
    const expectedPrice = formatPrice(selectBestOffer(priced));
    const quoteless = game({ id: "quoteless", title: "Quoteless", offers: [] });
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [priced, quoteless] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(cardFor(mounted.container, priced.id).querySelector(".store-card__price")?.textContent).toBe(
      expectedPrice,
    );
    expect(expectedPrice).toMatch(/\d/);
    expect(cardFor(mounted.container, "quoteless").querySelector(".store-card__price")).toBeNull();
  });
});

describe("Store page wishlist", () => {
  it("wishlists without adding the game to the library", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [game({ id: "store:w", title: "Wishlist Game" })] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    byFocusKey(mounted.container, "wishlist-store:w").click();
    await flush();

    expect(fake.wishlistCalls).toEqual([{ gameId: "store:w", wishlisted: true }]);
    expect(fake.commands).toEqual([
      "listOwnedGameIds",
      "getHome",
      "refreshSources",
      "getHome",
      "setWishlist",
    ]);
    expect(byFocusKey(mounted.container, "wishlist-store:w").getAttribute("aria-pressed")).toBe("true");
    expect(mounted.navigations.some((route) => route.page === "library")).toBe(false);
  });

  it("rolls the wishlist state back when the backend rejects the change", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [game({ id: "store:w", title: "Wishlist Game" })] }),
      setWishlist: async () => {
        throw new Error("The wishlist could not be saved.");
      },
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    byFocusKey(mounted.container, "wishlist-store:w").click();
    await flush();

    expect(byFocusKey(mounted.container, "wishlist-store:w").getAttribute("aria-pressed")).toBe("false");
    expect(mounted.container.querySelector(".store-status__copy")?.textContent).toBe(
      "The wishlist could not be saved.",
    );
  });
});

describe("Store page lifecycle", () => {
  it("restores the store, the search, scroll position and focus after deactivate and activate", async () => {
    const games = [
      game({ id: "unrailed", title: "Unrailed" }),
      game({ id: "baldur", title: "Baldur's Gate 3" }),
    ];
    const fake = createFakeClient({
      getHome: async () => homeView({ games }),
      browse: async () => browsePage({ games: [games[0]] }),
    });
    const mounted = mountStore(fake.client);
    const route = storeRoute({ query: "unrailed" });

    await mounted.host.activate(route);
    await flush();

    // The host is the scroll container, as on every page.
    Object.defineProperty(mounted.container, "scrollTop", { configurable: true, writable: true, value: 240 });
    next(mounted.container).focus();

    const restoreState = mounted.host.deactivate();
    expect(restoreState).toEqual({
      scrollTop: 240,
      focusKey: "store-next",
      query: "unrailed",
      filters: ["all"],
    });

    await mounted.host.activate(route, restoreState);
    await flush();

    expect(pageRoot(mounted.container).dataset.store).toBe("all");
    // The query has no field of its own: it rides the route, and the restored
    // page asks the backend for it again.
    expect(fake.browseRequests.at(-1)).toEqual({
      category: "all-games",
      platforms: [],
      query: "unrailed",
      cursor: null,
      limit: 48,
    });
    expect(mounted.container.scrollTop).toBe(240);
    expect((document.activeElement as HTMLElement | null)?.dataset.focusKey).toBe("store-next");
    expect(cardTitles(mounted.container)).toEqual(["Unrailed"]);
  });

  it("keeps focus on a card across a background refresh that rebuilds the shelf", async () => {
    let homeCalls = 0;
    const fake = createFakeClient({
      getHome: async () => {
        homeCalls += 1;
        // The refresh that follows coming back online returns a changed shelf,
        // so the cards are rebuilt rather than merely re-read.
        return homeView({ games: [game({ id: "a", title: "A Game", wishlisted: homeCalls > 2 })] });
      },
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    byFocusKey(mounted.container, "game-a").focus();
    window.dispatchEvent(new Event("online"));
    await flush();

    expect(byFocusKey(mounted.container, "wishlist-a").getAttribute("aria-pressed")).toBe("true");
    expect((document.activeElement as HTMLElement | null)?.dataset.focusKey).toBe("game-a");
  });
});
