import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute, GameSummary, ProviderStatus, StoreOffer, StoreProvider } from "./contracts";
import { PageLifecycleHost } from "./page-lifecycle";
import type { StoreBrowsePage, StoreBrowseRequest, StoreHomeView } from "./store-model";
import { DEFAULT_HIGHLIGHTS, EDITORIAL_GAMES, formatPrice, selectBestOffer } from "./store-model";
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

/**
 * Hovering a card moves the hero onto it. The offer line — price, shop and how
 * fresh the reading is — lives there, so the facts are read through a preview.
 */
const previewOfferLine = async (root: HTMLElement, gameId: string): Promise<string> => {
  cardFor(root, gameId).dispatchEvent(new Event("pointerenter"));
  await new Promise((resolve) => setTimeout(resolve, 90));
  return root.querySelector(".store-hero__offer")?.textContent ?? "";
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
  const shortConsole = game({
    id: "short-console",
    title: "Astro Duel 2",
    tags: ["Short Sessions"],
    offers: [
      offer({
        gameId: "short-console",
        provider: "playstation",
        providerLabel: "PlayStation Store",
      }),
    ],
  });
  const storySteam = game({
    id: "story-steam",
    title: "Baldur's Gate 3",
    tags: ["Strong Stories"],
    offers: [offer({ gameId: "story-steam", provider: "steam" })],
  });

  it("combines the category filter with the platform filter without waiting on the backend", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [shortSteam, shortConsole, storySteam] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(cardTitles(mounted.container)).toEqual(["Unrailed", "Astro Duel 2", "Baldur's Gate 3"]);

    // Every assertion below runs before the browse each click starts can
    // resolve: the shelf repaints from what the page already holds.
    byFocusKey(mounted.container, "category-short-sessions").click();
    expect(cardTitles(mounted.container)).toEqual(["Unrailed", "Astro Duel 2"]);

    byFocusKey(mounted.container, "platform-playstation").click();
    expect(cardTitles(mounted.container)).toEqual(["Astro Duel 2"]);

    expect(byFocusKey(mounted.container, "category-short-sessions").getAttribute("aria-pressed")).toBe("true");
    expect(byFocusKey(mounted.container, "platform-playstation").getAttribute("aria-pressed")).toBe("true");
    expect(mounted.navigations.at(-1)).toEqual({
      page: "store",
      category: "short-sessions",
      platforms: ["playstation"],
      query: "",
    });
    expect(fake.browseRequests.at(-1)).toEqual({
      category: "short-sessions",
      platforms: ["playstation"],
      query: "",
      cursor: null,
      limit: 30,
    });
  });

  it("sends the combined filters to the backend when the route already carries them", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [shortSteam, shortConsole, storySteam] }),
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

    byFocusKey(mounted.container, "category-short-sessions").click();
    byFocusKey(mounted.container, "category-all-games").click();

    expect(fake.browseRequests.map((request) => request.category)).toEqual([
      "short-sessions",
      "all-games",
    ]);

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
              id: "console-game",
              title: "Console Game",
              offers: [
                offer({
                  gameId: "console-game",
                  provider: "playstation",
                  providerLabel: "PlayStation Store",
                }),
              ],
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

    // The filter bar speaks machines; shop health is reported in the "Plus"
    // panel, where a source that cannot answer says so in its own words.
    byFocusKey(mounted.container, "platform-more").click();
    const ubisoft = mounted.container.querySelector<HTMLElement>(
      '.store-more-panel__item[data-provider="ubisoft"]',
    );
    expect(ubisoft?.dataset.health).toBe("unavailable");
    expect(ubisoft?.querySelector(".store-more-panel__message")?.textContent).toBe(
      "No authorized catalog feed is configured.",
    );
    expect(
      mounted.container.querySelector<HTMLElement>('.store-more-panel__item[data-provider="steam"]')
        ?.dataset.health,
    ).toBe("available");

    expect(cardTitles(mounted.container)).toEqual(["Steam Game", "Console Game"]);
    byFocusKey(mounted.container, "platform-playstation").click();
    expect(cardTitles(mounted.container)).toEqual(["Console Game"]);
    expect((byFocusKey(mounted.container, "hero-action") as HTMLButtonElement).disabled).toBe(false);
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
    expect(fake.commands).toEqual(["listOwnedGameIds", "getHome"]);
    // Cards, not titles: the handful of games that fall back to capsule art
    // drop the text title so the wordmark is not printed twice.
    expect(mounted.container.querySelectorAll(".store-card").length).toBe(EDITORIAL_GAMES.length);
    expect(cardFor(mounted.container, EDITORIAL_GAMES[0].id)).not.toBeNull();
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

    // No known price: the card leaves the slot empty rather than printing a
    // fabricated price line.
    expect(cardFor(mounted.container, "unpriced").querySelector(".store-card__price")).toBeNull();
    expect(await previewOfferLine(mounted.container, "unpriced")).toBe(
      "Disponible sur Instant Gaming · tarif non vérifié",
    );

    const outdated = cardFor(mounted.container, "outdated");
    expect(outdated.querySelector(".store-card__price")?.textContent).toMatch(/19[.,]99/);
    // A reading older than a day is still shown, dated, so the shopper knows
    // how fresh it is instead of being told it is today's price.
    expect(await previewOfferLine(mounted.container, "outdated")).toMatch(
      /^Dès .*19[.,]99.* sur Steam · relevé le .+$/,
    );

    const verified = cardFor(mounted.container, "verified");
    expect(verified.querySelector(".store-card__price")?.textContent).toMatch(/29[.,]99/);
    expect(await previewOfferLine(mounted.container, "verified")).toMatch(
      /^Dès .*29[.,]99.* sur Steam · vérifié aujourd'hui$/,
    );
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

  it("explains the selection with plain facts only", async () => {
    const fake = createFakeClient({
      getHome: async () => homeView({ games: [game({ id: "featured", title: "Featured Game" })] }),
    });
    const mounted = mountStore(fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(
      [...mounted.container.querySelectorAll(".store-highlight__title")].map((n) => n.textContent),
    ).toEqual(DEFAULT_HIGHLIGHTS.map((highlight) => highlight.title));

    byFocusKey(mounted.container, "hero-action").click();
    const why = mounted.container.querySelector<HTMLElement>(".store-why");
    expect(why?.hidden).toBe(false);
    expect(mounted.container.querySelector(".store-hero")?.textContent ?? "").not.toMatch(
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
    expect(fake.commands).toEqual([
      "listOwnedGameIds",
      "getHome",
      "refreshSources",
      "getHome",
      "setWishlist",
    ]);
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
      platforms: ["pc"],
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
      filters: ["short-sessions", "pc"],
    });

    await mounted.host.activate(route, restoreState);
    await flush();

    expect(byFocusKey(mounted.container, "category-short-sessions").getAttribute("aria-pressed")).toBe("true");
    expect(byFocusKey(mounted.container, "platform-pc").getAttribute("aria-pressed")).toBe("true");
    // The query has no field of its own any more: it rides the route, and the
    // restored page asks the backend for it again.
    expect(fake.browseRequests.at(-1)).toEqual({
      category: "short-sessions",
      platforms: ["pc"],
      query: "unrailed",
      cursor: null,
      limit: 30,
    });
    expect(pageRoot(mounted.container).scrollTop).toBe(240);
    expect((document.activeElement as HTMLElement | null)?.dataset.focusKey).toBe("category-relaxing");
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
