import { describe, expect, it } from "vitest";
import type { GameSummary, ProviderStatus, StoreOffer, StoreProvider } from "./contracts";
import {
  createInitialStoreState,
  EDITORIAL_GAMES,
  EDITORIAL_PROVIDER_STATUSES,
  isOfferStale,
  reduceStorePageState,
  selectBestOffer,
  selectStoreGames,
  storeCategoryLabel,
  STORE_CATEGORIES,
  type StoreBrowsePage,
  type StoreHomeView,
  type StorePageState,
} from "./store-model";

const NOW = Date.parse("2026-08-01T12:00:00.000Z");
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
    recommendationReasons: [],
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

function status(provider: StoreProvider, health: ProviderStatus["health"]): ProviderStatus {
  return { provider, label: provider, health, message: `${provider} ${health}`, refreshedAt: null };
}

function stateWith(overrides: Partial<StorePageState> = {}): StorePageState {
  return { ...createInitialStoreState(), ...overrides };
}

describe("createInitialStoreState", () => {
  it("starts on the editorial catalog with no filters applied", () => {
    const state = createInitialStoreState();

    expect(state.phase).toBe("loading");
    expect(state.category).toBe("for-you");
    expect(state.providers).toEqual([]);
    expect(state.query).toBe("");
    expect(state.browseGames).toBeNull();
    expect(state.nextCursor).toBeNull();
    expect(state.activeRequestId).toBe(0);
    expect(state.errorMessage).toBe("");
    expect(state.home.games.length).toBeGreaterThan(0);
  });

  it("never fabricates a price or an availability claim for editorial offers", () => {
    for (const editorial of EDITORIAL_GAMES) {
      for (const editorialOffer of editorial.offers) {
        expect(editorialOffer.priceMinor).toBeNull();
        expect(editorialOffer.currency).toBeNull();
        expect(editorialOffer.availability).toBe("unknown");
        expect(editorialOffer.stale).toBe(true);
      }
    }

    const instantGaming = EDITORIAL_PROVIDER_STATUSES.find(
      (entry) => entry.provider === "instant-gaming",
    );
    expect(instantGaming?.health).toBe("unavailable");
    expect(EDITORIAL_PROVIDER_STATUSES.some((entry) => entry.health === "available")).toBe(false);
  });

  it("keeps recommendation reasons factual", () => {
    const banned = /\b(ai|neuro\w*|cognitive|brain|mindful\w*|dopamine|wellbeing|well-being|therapeutic|iq|smarter)\b/i;
    for (const editorial of EDITORIAL_GAMES) {
      for (const reason of editorial.recommendationReasons) {
        expect(reason, `reason "${reason}"`).not.toMatch(banned);
        expect(reason.length).toBeGreaterThan(0);
      }
    }
  });
});

describe("reduceStorePageState", () => {
  it("applies route filters on activate while online", () => {
    const next = reduceStorePageState(stateWith({ phase: "ready", browseGames: [game()] }), {
      type: "activate",
      category: "relaxing",
      providers: ["steam"],
      query: "unrailed",
      online: true,
    });

    expect(next.phase).toBe("ready");
    expect(next.category).toBe("relaxing");
    expect(next.providers).toEqual(["steam"]);
    expect(next.query).toBe("unrailed");
    expect(next.browseGames).toBeNull();
    expect(next.errorMessage).toBe("");
  });

  it("activates into the offline phase without dropping saved picks", () => {
    const next = reduceStorePageState(stateWith({ phase: "ready" }), {
      type: "activate",
      category: "for-you",
      providers: [],
      query: "",
      online: false,
    });

    expect(next.phase).toBe("offline");
    expect(next.errorMessage).toMatch(/offline/i);
    expect(next.home.games.length).toBeGreaterThan(0);
  });

  it("marks a refresh over existing content as refreshing and a cold load as loading", () => {
    const refreshing = reduceStorePageState(stateWith(), {
      type: "request-started",
      requestId: 7,
      refresh: true,
    });
    expect(refreshing.phase).toBe("refreshing");
    expect(refreshing.activeRequestId).toBe(7);

    const cold = reduceStorePageState(stateWith({ home: homeView({ games: [] }) }), {
      type: "request-started",
      requestId: 8,
      refresh: true,
    });
    expect(cold.phase).toBe("loading");

    const initial = reduceStorePageState(stateWith(), {
      type: "request-started",
      requestId: 9,
      refresh: false,
    });
    expect(initial.phase).toBe("loading");
  });

  it("ignores a home payload from a superseded request", () => {
    const current = stateWith({ activeRequestId: 4 });
    const next = reduceStorePageState(current, {
      type: "home-loaded",
      requestId: 3,
      home: homeView({ games: [game({ id: "store:late", title: "Late" })] }),
    });

    expect(next).toBe(current);
  });

  it("stores a loaded home and flags degraded sources", () => {
    const next = reduceStorePageState(stateWith({ activeRequestId: 2 }), {
      type: "home-loaded",
      requestId: 2,
      home: homeView({
        games: [game({ id: "store:new" })],
        providerStatuses: [status("steam", "available"), status("apple", "degraded")],
      }),
    });

    expect(next.phase).toBe("degraded");
    expect(next.home.games.map((entry) => entry.id)).toEqual(["store:new"]);

    const healthy = reduceStorePageState(stateWith({ activeRequestId: 2 }), {
      type: "home-loaded",
      requestId: 2,
      home: homeView({ providerStatuses: [status("steam", "available")] }),
    });
    expect(healthy.phase).toBe("ready");
  });

  it("keeps the previous catalog when a home payload arrives empty", () => {
    const previous = stateWith({ activeRequestId: 1, home: homeView({ games: [game({ id: "store:kept" })] }) });
    const next = reduceStorePageState(previous, {
      type: "home-loaded",
      requestId: 1,
      home: homeView({ games: [] }),
    });

    expect(next.home.games.map((entry) => entry.id)).toEqual(["store:kept"]);
  });

  it("replaces or appends browse results and dedupes by game id", () => {
    const first = reduceStorePageState(stateWith({ activeRequestId: 5 }), {
      type: "browse-loaded",
      requestId: 5,
      page: browsePage({ games: [game({ id: "a" }), game({ id: "b" })], nextCursor: "store_2" }),
      append: false,
    });
    expect(first.browseGames?.map((entry) => entry.id)).toEqual(["a", "b"]);
    expect(first.nextCursor).toBe("store_2");

    const appended = reduceStorePageState(first, {
      type: "browse-loaded",
      requestId: 5,
      page: browsePage({ games: [game({ id: "b" }), game({ id: "c" })], nextCursor: null }),
      append: true,
    });
    expect(appended.browseGames?.map((entry) => entry.id)).toEqual(["a", "b", "c"]);
    expect(appended.nextCursor).toBeNull();
  });

  it("ignores a browse payload from a superseded request", () => {
    const current = stateWith({ activeRequestId: 12 });
    const next = reduceStorePageState(current, {
      type: "browse-loaded",
      requestId: 11,
      page: browsePage({ games: [game({ id: "stale" })] }),
      append: false,
    });

    expect(next).toBe(current);
  });

  it("degrades rather than errors when saved picks are still available", () => {
    const degraded = reduceStorePageState(stateWith({ activeRequestId: 3 }), {
      type: "request-failed",
      requestId: 3,
      message: "Live sources unreachable.",
      offline: false,
    });
    expect(degraded.phase).toBe("degraded");
    expect(degraded.errorMessage).toBe("Live sources unreachable.");

    const errored = reduceStorePageState(
      stateWith({ activeRequestId: 3, home: homeView({ games: [] }) }),
      { type: "request-failed", requestId: 3, message: "No catalog.", offline: false },
    );
    expect(errored.phase).toBe("error");

    const offline = reduceStorePageState(stateWith({ activeRequestId: 3 }), {
      type: "request-failed",
      requestId: 3,
      message: "Offline.",
      offline: true,
    });
    expect(offline.phase).toBe("offline");

    const superseded = stateWith({ activeRequestId: 4 });
    expect(
      reduceStorePageState(superseded, {
        type: "request-failed",
        requestId: 3,
        message: "Ignored.",
        offline: false,
      }),
    ).toBe(superseded);
  });

  it("resets pagination when a filter changes", () => {
    const browsing = stateWith({ browseGames: [game()], nextCursor: "store_30" });

    const byCategory = reduceStorePageState(browsing, {
      type: "category-changed",
      category: "short-sessions",
    });
    expect(byCategory.category).toBe("short-sessions");
    expect(byCategory.browseGames).toBeNull();
    expect(byCategory.nextCursor).toBeNull();

    const byProvider = reduceStorePageState(browsing, {
      type: "providers-changed",
      providers: ["steam", "apple"],
    });
    expect(byProvider.providers).toEqual(["steam", "apple"]);
    expect(byProvider.browseGames).toBeNull();

    const byQuery = reduceStorePageState(browsing, { type: "query-changed", query: "hades" });
    expect(byQuery.query).toBe("hades");
    expect(byQuery.browseGames).toBeNull();
    expect(byQuery.nextCursor).toBeNull();
  });

  it("mirrors a wishlist change into both the home and browse lists without touching ownership", () => {
    const target = game({ id: "store:w" });
    const next = reduceStorePageState(
      stateWith({ home: homeView({ games: [target] }), browseGames: [target] }),
      { type: "wishlist-changed", gameId: "store:w", wishlisted: true },
    );

    expect(next.home.games[0].wishlisted).toBe(true);
    expect(next.browseGames?.[0].wishlisted).toBe(true);
    expect(next.home.games[0].owned).toBe(false);
    expect(next.home.games[0].launchable).toBe(false);
    expect(next.home.games[0].source).toBe("store");
  });

  it("tracks connectivity changes", () => {
    const offline = reduceStorePageState(stateWith({ phase: "ready" }), {
      type: "connectivity-changed",
      online: false,
    });
    expect(offline.phase).toBe("offline");
    expect(offline.errorMessage).toMatch(/offline/i);

    const recovered = reduceStorePageState(offline, { type: "connectivity-changed", online: true });
    expect(recovered.phase).toBe("degraded");

    const untouched = reduceStorePageState(stateWith({ phase: "ready" }), {
      type: "connectivity-changed",
      online: true,
    });
    expect(untouched.phase).toBe("ready");
  });
});

describe("selectStoreGames", () => {
  const shortSteam = game({
    id: "short-steam",
    title: "Unrailed",
    tags: ["Short Sessions", "Relaxing"],
    offers: [offer({ gameId: "short-steam", provider: "steam" })],
  });
  const shortInstant = game({
    id: "short-instant",
    title: "Astro Duel 2",
    tags: ["Short Sessions"],
    offers: [
      offer({ gameId: "short-instant", provider: "instant-gaming", providerLabel: "Instant Gaming" }),
    ],
  });
  const storySteam = game({
    id: "story-steam",
    title: "Baldur's Gate 3",
    tags: ["Strong Stories"],
    genres: ["RPG"],
    offers: [offer({ gameId: "story-steam", provider: "steam" })],
  });
  const home = homeView({ games: [shortSteam, shortInstant, storySteam] });

  it("prefers server browse results when they exist", () => {
    const browsed = game({ id: "from-backend" });
    const state = stateWith({ home, browseGames: [browsed], category: "strong-stories" });

    expect(selectStoreGames(state).map((entry) => entry.id)).toEqual(["from-backend"]);
  });

  it("passes everything through for for-you and all-games", () => {
    expect(selectStoreGames(stateWith({ home, category: "for-you" }))).toHaveLength(3);
    expect(selectStoreGames(stateWith({ home, category: "all-games" }))).toHaveLength(3);
  });

  it("combines the category filter with the provider filter", () => {
    const state = stateWith({ home, category: "short-sessions", providers: ["steam"] });

    expect(selectStoreGames(state).map((entry) => entry.id)).toEqual(["short-steam"]);
  });

  it("matches the story and relaxing categories on tags and genres", () => {
    expect(
      selectStoreGames(stateWith({ home, category: "strong-stories" })).map((entry) => entry.id),
    ).toEqual(["story-steam"]);
    expect(
      selectStoreGames(stateWith({ home, category: "relaxing" })).map((entry) => entry.id),
    ).toEqual(["short-steam"]);
  });

  it("searches title, description, genres and tags, ignoring case and accents", () => {
    expect(
      selectStoreGames(stateWith({ home, query: "  BALDUR'S  " })).map((entry) => entry.id),
    ).toEqual(["story-steam"]);
    expect(selectStoreGames(stateWith({ home, query: "baldur" })).map((entry) => entry.id)).toEqual([
      "story-steam",
    ]);
    expect(selectStoreGames(stateWith({ home, query: "RPG" })).map((entry) => entry.id)).toEqual([
      "story-steam",
    ]);
    expect(selectStoreGames(stateWith({ home, query: "ástro" })).map((entry) => entry.id)).toEqual([
      "short-instant",
    ]);
  });

  it("returns an empty list rather than inventing results", () => {
    expect(selectStoreGames(stateWith({ home, providers: ["google-play"] }))).toEqual([]);
  });
});

describe("storeCategoryLabel", () => {
  it("labels every known category in English", () => {
    expect(STORE_CATEGORIES.map((option) => storeCategoryLabel(option.id))).toEqual([
      "For You",
      "Short Sessions",
      "Strong Stories",
      "Relaxing",
      "All Games",
    ]);
  });

  it("falls back to All Games for an unknown category", () => {
    expect(storeCategoryLabel("mystery" as never)).toBe("All Games");
  });
});

describe("isOfferStale", () => {
  it("treats an explicit stale flag or a missing timestamp as stale", () => {
    expect(isOfferStale(offer({ stale: true, verifiedAt: new Date(NOW).toISOString() }), NOW)).toBe(true);
    expect(isOfferStale(offer({ stale: false, verifiedAt: null }), NOW)).toBe(true);
  });

  it("treats an unparsable timestamp as stale", () => {
    expect(isOfferStale(offer({ stale: false, verifiedAt: "not-a-date" }), NOW)).toBe(true);
  });

  it("expires verification after 24 hours", () => {
    const fresh = offer({ stale: false, verifiedAt: new Date(NOW - 2 * HOUR).toISOString() });
    const old = offer({ stale: false, verifiedAt: new Date(NOW - 30 * HOUR).toISOString() });

    expect(isOfferStale(fresh, NOW)).toBe(false);
    expect(isOfferStale(old, NOW)).toBe(true);
  });
});

describe("selectBestOffer", () => {
  it("returns null when a game carries no offers", () => {
    expect(selectBestOffer(game({ offers: [] }))).toBeNull();
  });

  it("prefers an available offer over an unknown one", () => {
    const unknown = offer({ id: "unknown", availability: "unknown" });
    const available = offer({
      id: "available",
      availability: "available",
      stale: false,
      verifiedAt: new Date().toISOString(),
      priceMinor: 5_999,
      currency: "USD",
    });

    expect(selectBestOffer(game({ offers: [unknown, available] }))?.id).toBe("available");
  });

  it("prefers a freshly verified offer when availability matches", () => {
    const staleOffer = offer({ id: "stale", availability: "available", stale: true });
    const freshOffer = offer({
      id: "fresh",
      availability: "available",
      stale: false,
      verifiedAt: new Date().toISOString(),
    });

    expect(selectBestOffer(game({ offers: [staleOffer, freshOffer] }))?.id).toBe("fresh");
  });

  it("prefers the cheaper verified offer and sinks offers with no price", () => {
    const verifiedAt = new Date().toISOString();
    const cheap = offer({ id: "cheap", availability: "available", stale: false, verifiedAt, priceMinor: 1_999, currency: "USD" });
    const pricey = offer({ id: "pricey", availability: "available", stale: false, verifiedAt, priceMinor: 5_999, currency: "USD" });
    const unpriced = offer({ id: "unpriced", availability: "available", stale: false, verifiedAt, priceMinor: null });

    expect(selectBestOffer(game({ offers: [pricey, unpriced, cheap] }))?.id).toBe("cheap");
    expect(selectBestOffer(game({ offers: [unpriced, pricey] }))?.id).toBe("pricey");
  });

  it("does not mutate the game's offer order", () => {
    const first = offer({ id: "first", availability: "unknown" });
    const second = offer({ id: "second", availability: "available" });
    const subject = game({ offers: [first, second] });

    selectBestOffer(subject);

    expect(subject.offers.map((entry) => entry.id)).toEqual(["first", "second"]);
  });
});
