import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute, GameSummary, ProviderStatus, StoreOffer, StoreProvider } from "./contracts";
import { PageLifecycleHost } from "./page-lifecycle";
import type { StoreBrowsePage, StoreBrowseRequest, StoreHomeView } from "./store-model";
import { EDITORIAL_GAMES } from "./store-model";
import { createStorePage, type StorePageClient } from "./store-page";

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

function status(
  provider: StoreProvider,
  health: ProviderStatus["health"],
  message: string,
): ProviderStatus {
  return { provider, label: provider, health, message, refreshedAt: null };
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
  return { page: "store", category: "for-you", platforms: [], query: "", ...overrides };
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
  vi.unstubAllGlobals();
});

describe("Store page filtering", () => {
  const shortSteam = game({
    id: "short-steam",
    title: "Unrailed",
    tags: ["Short Sessions"],
    offers: [offer({ gameId: "short-steam", provider: "steam" })],
  });
  const shortInstant = game({
    id: "short-instant",
    title: "Astro Duel 2",
    tags: ["Short Sessions"],
    offers: [
      offer({
        gameId: "short-instant",
        provider: "instant-gaming",
        providerLabel: "Instant Gaming",
      }),
    ],
  });
  const storySteam = game({
    id: "story-steam",
    title: "Baldur's Gate 3",
    tags: ["Strong Stories"],
    offers: [offer({ gameId: "story-steam", provider: "steam" })],
  });

  it("combines the category filter with the provider filter without a backend round trip", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [shortSteam, shortInstant, storySteam] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(cardTitles(mounted.container)).toEqual(["Unrailed", "Astro Duel 2", "Baldur's Gate 3"]);

    byFocusKey(mounted.container, "category-short-sessions").click();
    expect(cardTitles(mounted.container)).toEqual(["Unrailed", "Astro Duel 2"]);

    byFocusKey(mounted.container, "provider-steam").click();
    expect(cardTitles(mounted.container)).toEqual(["Unrailed"]);

    expect(byFocusKey(mounted.container, "category-short-sessions").getAttribute("aria-pressed")).toBe("true");
    expect(byFocusKey(mounted.container, "provider-steam").getAttribute("aria-pressed")).toBe("true");
    expect(mounted.navigations.at(-1)).toEqual({
      page: "store",
      category: "short-sessions",
      platforms: [],
      query: "",
    });
    expect(fake.browseRequests).toEqual([]);
  });

  it("sends the combined filters to the backend when the route already carries them", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [shortSteam, shortInstant, storySteam] }),
      browse: async () => browsePage({ games: [shortSteam], nextCursor: null }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(
      storeRoute({ category: "short-sessions", platforms: [], query: "unrailed" }),
    );
    await flush();

    expect(fake.browseRequests).toEqual([
      {
        category: "short-sessions",
        platforms: [],
        query: "unrailed",
        cursor: null,
        limit: 30,
      },
    ]);
    expect(cardTitles(mounted.container)).toEqual(["Unrailed"]);
  });
});

describe("Store page request ordering", () => {
  it("never paints a browse response that a newer request already superseded", async () => {
    const first = deferred<StoreBrowsePage>();
    const second = deferred<StoreBrowsePage>();
    const pending = [first, second];
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [game({ id: "home", title: "Home Pick" })] }),
      browse: async () => pending.shift()?.promise ?? browsePage(),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    const search = byFocusKey(mounted.container, "store-search") as HTMLInputElement;
    search.value = "first";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));

    const laterSearch = byFocusKey(mounted.container, "store-search") as HTMLInputElement;
    laterSearch.value = "second";
    laterSearch.dispatchEvent(new Event("input", { bubbles: true }));
    laterSearch.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));

    expect(fake.browseRequests.map((request) => request.query)).toEqual(["first", "second"]);

    second.resolve(browsePage({ games: [game({ id: "b", title: "Second Result" })] }));
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Second Result"]);

    first.resolve(browsePage({ games: [game({ id: "a", title: "First Result" })] }));
    await flush();
    expect(cardTitles(mounted.container)).toEqual(["Second Result"]);
  });

  it("drops a response that resolves after the activation was cancelled", async () => {
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

describe("Store page degraded sources", () => {
  it("keeps the rest of the page usable when one provider is unavailable", async () => {
    const fake = createFakeClient({
      getHome: async () =>
        homeView({
          games: [
            game({ id: "steam-game", title: "Steam Game" }),
            game({
              id: "apple-game",
              title: "Apple Game",
              offers: [offer({ gameId: "apple-game", provider: "apple", providerLabel: "Apple" })],
            }),
          ],
          providerStatuses: [
            status("steam", "available", "Live catalog."),
            status("ubisoft", "unavailable", "No authorized catalog feed is configured."),
          ],
        }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    const ubisoftPill = byFocusKey(mounted.container, "provider-ubisoft");
    expect(ubisoftPill.dataset.health).toBe("unavailable");
    expect(ubisoftPill.title).toBe("No authorized catalog feed is configured.");
    expect(byFocusKey(mounted.container, "provider-steam").dataset.health).toBe("available");
    expect(
      mounted.container.querySelector(".store-provider-statuses__summary")?.textContent,
    ).toBe("1 source notice");

    expect(cardTitles(mounted.container)).toEqual(["Steam Game", "Apple Game"]);
    byFocusKey(mounted.container, "provider-steam").click();
    expect(cardTitles(mounted.container)).toEqual(["Steam Game"]);
    expect((byFocusKey(mounted.container, "refresh-store") as HTMLButtonElement).disabled).toBe(false);
  });

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
    expect(fake.commands).toEqual(["getHome"]);
    const titles = cardTitles(mounted.container);
    expect(titles.length).toBe(EDITORIAL_GAMES.length);
    expect(titles).toContain("Elden Ring");
    expect(mounted.container.querySelector(".store-hero")).not.toBeNull();
  });
});

describe("Store page offer facts", () => {
  it("does not invent a price and flags verification that is no longer recent", async () => {
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

    // No known price: the card shows no offer block at all rather than a
    // fabricated price line.
    const unpriced = cardFor(mounted.container, "unpriced");
    expect(unpriced.querySelector(".store-card__offer")).toBeNull();
    expect(unpriced.querySelector(".store-card__price")).toBeNull();

    const outdated = cardFor(mounted.container, "outdated");
    expect(outdated.querySelector(".store-card__price")?.textContent).toMatch(/19[.,]99/);
    expect(outdated.querySelector(".store-card__offer-detail")?.textContent).toBe(
      "Steam · may be outdated",
    );
    expect(outdated.querySelector(".store-card__offer--stale")).not.toBeNull();

    const verified = cardFor(mounted.container, "verified");
    expect(verified.querySelector(".store-card__price")?.textContent).toMatch(/29[.,]99/);
    expect(verified.querySelector(".store-card__offer-detail")?.textContent).toBe("Steam · verified");
    expect(verified.querySelector(".store-card__offer--stale")).toBeNull();
  });

  it("shows the Instant Gaming reference price with its discount on editorial cards", async () => {
    const eldenRing = EDITORIAL_GAMES.find((entry) => entry.title === "Elden Ring");
    const astroDuel = EDITORIAL_GAMES.find((entry) => entry.title === "Astro Duel 2");
    if (!eldenRing || !astroDuel) throw new Error("Editorial seeds changed unexpectedly.");
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [eldenRing, astroDuel] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    const priced = cardFor(mounted.container, eldenRing.id);
    expect(priced.querySelector(".store-card__price")?.textContent).toContain("34,99");
    expect(priced.querySelector(".store-card__price")?.textContent).toContain("€");
    expect(priced.querySelector(".store-card__price-original")?.textContent).toContain("59,99");
    expect(priced.querySelector(".store-card__discount")?.textContent).toBe("-42%");
    expect(priced.querySelector(".store-card__offer-detail")?.textContent).toBe("Instant Gaming");
    expect(priced.querySelector(".store-card__offer--stale")).toBeNull();

    const unpriced = cardFor(mounted.container, astroDuel.id);
    expect(unpriced.querySelector(".store-card__offer")).toBeNull();
    expect(unpriced.querySelector(".store-card__price")).toBeNull();
    expect(unpriced.querySelector(".store-card__discount")).toBeNull();
  });

  it("explains a recommendation with plain facts only", async () => {
    const fake = createFakeClient({
      getHome: async () =>
        homeView({
          games: [
            game({
              id: "featured",
              title: "Featured Game",
              recommendationReasons: [
                "Because you play strategy games",
                "Available on macOS",
                "Works in short sessions",
              ],
            }),
          ],
        }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    const panel = mounted.container.querySelector(".store-reasons");
    expect(panel).not.toBeNull();
    expect([...(panel?.querySelectorAll(".store-reasons__fact") ?? [])].map((n) => n.textContent)).toEqual([
      "Because you play strategy games",
      "Available on macOS",
      "Works in short sessions",
    ]);
    expect(panel?.textContent ?? "").not.toMatch(
      /\b(ai|neuro\w*|cognitive|brain|mindful\w*|dopamine|wellbeing|well-being|therapeutic)\b/i,
    );
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
    expect(fake.commands).toEqual(["getHome", "refreshSources", "getHome", "setWishlist"]);
    expect(byFocusKey(mounted.container, "wishlist-store:w").getAttribute("aria-pressed")).toBe("true");
    expect(mounted.navigations.some((route) => route.page === "library")).toBe(false);

    byFocusKey(mounted.container, "game-store:w").click();
    expect(mounted.navigations.at(-1)).toEqual({
      page: "game",
      gameId: "store:w",
      from: "store",
    });
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
  it("restores filters, scroll position and focus after deactivate and activate", async () => {
    const games = [
      game({ id: "short-steam", title: "Unrailed", tags: ["Short Sessions"] }),
      game({ id: "story-steam", title: "Baldur's Gate 3", tags: ["Strong Stories"] }),
    ];
    const fake = createFakeClient({
      getHome: async () => homeView({ games }),
      browse: async () => browsePage({ games: [games[0]] }),
    });
    const mounted = mountStore(fake.client);
    const route = storeRoute({
      category: "short-sessions",
      platforms: [],
      query: "unrailed",
    });

    await mounted.host.activate(route);
    await flush();

    const root = pageRoot(mounted.container);
    Object.defineProperty(root, "scrollTop", { configurable: true, writable: true, value: 240 });
    byFocusKey(mounted.container, "category-relaxing").focus();

    const restoreState = mounted.host.deactivate();
    expect(restoreState).toEqual({
      scrollTop: 240,
      focusKey: "category-relaxing",
      query: "unrailed",
      filters: ["short-sessions", "steam"],
    });

    await mounted.host.activate(route, restoreState);
    await flush();

    expect(byFocusKey(mounted.container, "category-short-sessions").getAttribute("aria-pressed")).toBe("true");
    expect(byFocusKey(mounted.container, "provider-steam").getAttribute("aria-pressed")).toBe("true");
    expect((byFocusKey(mounted.container, "store-search") as HTMLInputElement).value).toBe("unrailed");
    expect(pageRoot(mounted.container).scrollTop).toBe(240);
    expect((document.activeElement as HTMLElement | null)?.dataset.focusKey).toBe("category-relaxing");
    expect(cardTitles(mounted.container)).toEqual(["Unrailed"]);
  });

  it("keeps focus on the search field across a background refresh", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [game({ id: "a", title: "A Game" })] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    const search = byFocusKey(mounted.container, "store-search") as HTMLInputElement;
    search.focus();
    byFocusKey(mounted.container, "refresh-store").click();
    await flush();

    expect((document.activeElement as HTMLElement | null)?.dataset.focusKey).toBe("store-search");
  });
});
