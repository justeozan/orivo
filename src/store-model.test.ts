import { describe, expect, it } from "vitest";
import type { GameSummary, ProviderStatus, StoreOffer, StoreProvider } from "./contracts";
import { setLanguage } from "./i18n";
import {
  adjacentStore,
  createInitialStoreState,
  criticScoreOf,
  EDITORIAL_GAMES,
  editorialProviderStatuses,
  isOfferStale,
  reduceStorePageState,
  selectBestOffer,
  selectStoreGames,
  selectStoreSections,
  selectTabGames,
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
  it("starts on the editorial catalog, on every store", () => {
    const state = createInitialStoreState();

    expect(state.phase).toBe("loading");
    expect(state.store).toBe("all");
    expect(state.query).toBe("");
    expect(state.browseGames).toBeNull();
    expect(state.nextCursor).toBeNull();
    expect(state.activeRequestId).toBe(0);
    expect(state.errorMessage).toBe("");
    expect(state.home.games.length).toBeGreaterThan(0);
  });

  it("never fabricates a price for editorial offers", () => {
    const pricedProviders = new Set<StoreProvider>();
    for (const editorial of EDITORIAL_GAMES) {
      for (const editorialOffer of editorial.offers) {
        const where = `${editorial.id} / ${editorialOffer.provider}`;
        if (editorialOffer.priceMinor === null) {
          // No quote means no quote: nothing is dated, nothing is presented as
          // current.
          expect(editorialOffer.currency, where).toBeNull();
          expect(editorialOffer.verifiedAt, where).toBeNull();
          expect(editorialOffer.stale, where).toBe(true);
          continue;
        }
        // A price only exists because a shop returned it, so it carries the
        // currency it was quoted in and the day it was read.
        expect(editorialOffer.priceMinor, where).toBeGreaterThanOrEqual(0);
        expect(Number.isInteger(editorialOffer.priceMinor), where).toBe(true);
        expect(editorialOffer.currency, where).toMatch(/^[A-Z]{3}$/);
        expect(editorialOffer.verifiedAt, where).toBeTruthy();
        expect(Number.isNaN(Date.parse(editorialOffer.verifiedAt ?? "")), where).toBe(false);
        pricedProviders.add(editorialOffer.provider);
      }
    }

    // A shop is only called available when the catalog actually carries a
    // verified price from it; the console storefronts, whose prices are unknown,
    // stay degraded, and a shop with no feed at all is never claimed to work.
    for (const entry of editorialProviderStatuses()) {
      if (entry.health === "available") {
        expect(pricedProviders.has(entry.provider), entry.provider).toBe(true);
      } else {
        expect(pricedProviders.has(entry.provider), entry.provider).toBe(false);
      }
    }

    const instantGaming = editorialProviderStatuses().find(
      (entry) => entry.provider === "instant-gaming",
    );
    expect(instantGaming?.health).toBe("not-configured");
    expect(
      EDITORIAL_GAMES.some((entry) =>
        entry.offers.some((editorialOffer) => editorialOffer.provider === "instant-gaming"),
      ),
    ).toBe(false);
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
  it("applies the route's store and search on activate while online", () => {
    const next = reduceStorePageState(stateWith({ phase: "ready", browseGames: [game()] }), {
      type: "activate",
      store: "nintendo",
      query: "unrailed",
      online: true,
    });

    expect(next.phase).toBe("ready");
    expect(next.store).toBe("nintendo");
    expect(next.query).toBe("unrailed");
    expect(next.browseGames).toBeNull();
    expect(next.errorMessage).toBe("");
  });

  it("activates into the offline phase without dropping saved picks", () => {
    const next = reduceStorePageState(stateWith({ phase: "ready" }), {
      type: "activate",
      store: "all",
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

  it("resets pagination when the search changes, and keeps it across a change of store", () => {
    const browsing = stateWith({ browseGames: [game()], nextCursor: "store_30" });

    const byStore = reduceStorePageState(browsing, { type: "store-changed", store: "xbox" });
    expect(byStore.store).toBe("xbox");
    expect(byStore.browseGames).toEqual(browsing.browseGames);

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

describe("selectTabGames", () => {
  const plain = game({ id: "plain", title: "Plain" });
  const other = game({ id: "other", title: "Other" });

  it("keeps the whole shelf under Tout while no game is exclusive, and leaves a brand empty", () => {
    const shelf = [plain, other];
    expect(selectTabGames(shelf, "all").map((entry) => entry.id)).toEqual(["plain", "other"]);
    expect(selectTabGames(shelf, "playstation")).toEqual([]);
    expect(selectTabGames(shelf, "steam")).toEqual([]);
  });

  it("shows only exclusives once one exists, each brand its own", () => {
    const astro = game({ id: "astro", title: "Astro Bot", exclusiveTo: "playstation" });
    const halo = game({ id: "halo", title: "Halo", exclusiveTo: "xbox" });
    const shelf = [plain, astro, other, halo];
    expect(selectTabGames(shelf, "all").map((entry) => entry.id)).toEqual(["astro", "halo"]);
    expect(selectTabGames(shelf, "playstation").map((entry) => entry.id)).toEqual(["astro"]);
    expect(selectTabGames(shelf, "xbox").map((entry) => entry.id)).toEqual(["halo"]);
    expect(selectTabGames(shelf, "nintendo")).toEqual([]);
  });

  it("shows a plugin tab's own games and nothing from the catalogue", () => {
    const fromPlugin = game({ id: "io.orivo.demo:one", title: "From the plugin" });
    expect(selectTabGames([plain], "plugin:io.orivo.demo", [fromPlugin])).toEqual([fromPlugin]);
    expect(selectTabGames([plain], "plugin:io.orivo.demo", null)).toEqual([]);
  });
});

describe("selectStoreGames", () => {
  const unrailed = game({ id: "unrailed", title: "Unrailed", tags: ["Short Sessions"] });
  const astro = game({ id: "astro", title: "Astro Duel 2", tags: ["Short Sessions"] });
  const baldur = game({ id: "baldur", title: "Baldur's Gate 3", genres: ["RPG"] });
  const home = homeView({ games: [unrailed, astro, baldur] });

  it("never offers a game the library already has, by id or by title", () => {
    expect(
      selectStoreGames(stateWith({ home, ownedGameIds: ["unrailed", "baldursgate3"] })).map((entry) => entry.id),
    ).toEqual(["astro"]);
  });

  it("searches title, description, genres and tags, ignoring case and accents", () => {
    expect(selectStoreGames(stateWith({ home, query: "  BALDUR'S  " })).map((entry) => entry.id)).toEqual(["baldur"]);
    expect(selectStoreGames(stateWith({ home, query: "RPG" })).map((entry) => entry.id)).toEqual(["baldur"]);
    expect(selectStoreGames(stateWith({ home, query: "ástro" })).map((entry) => entry.id)).toEqual(["astro"]);
  });

  it("takes the host's answer to a search on Tout whole", () => {
    const fromHost = game({ id: "from-host", title: "Nothing like the query" });
    const state = stateWith({ home, query: "baldur", browseGames: [fromHost] });
    expect(selectStoreGames(state).map((entry) => entry.id)).toEqual(["from-host"]);
  });

  it("narrows a brand tab rather than searching the whole catalogue", () => {
    const exclusive = game({ id: "halo", title: "Halo Infinite", exclusiveTo: "xbox" });
    const shelf = homeView({ games: [baldur, exclusive] });
    expect(selectStoreGames(stateWith({ home: shelf, store: "xbox", query: "baldur" }))).toEqual([]);
    expect(selectStoreGames(stateWith({ home: shelf, store: "xbox", query: "halo" })).map((entry) => entry.id)).toEqual([
      "halo",
    ]);
  });

  it("returns an empty list rather than inventing results", () => {
    expect(selectStoreGames(stateWith({ home, query: "zzz" }))).toEqual([]);
  });
});

describe("adjacentStore", () => {
  const order = ["all", "playstation", "xbox"] as const;
  it("steps both ways and loops at the ends", () => {
    expect(adjacentStore(order, "all", 1)).toBe("playstation");
    expect(adjacentStore(order, "xbox", 1)).toBe("all");
    expect(adjacentStore(order, "all", -1)).toBe("xbox");
  });
  it("starts over from an end when the store on screen has gone", () => {
    expect(adjacentStore(order, "plugin:io.orivo.gone", 1)).toBe("all");
    expect(adjacentStore(order, "plugin:io.orivo.gone", -1)).toBe("xbox");
  });
});

describe("selectStoreSections", () => {
  const scored = (id: string, criticScore?: number): GameSummary => game({ id, title: id, criticScore });

  it("ranks on published scores only, takes the ranking first and repeats no game", () => {
    const games = [
      scored("a"),
      scored("b", 70),
      scored("c", 91),
      scored("d"),
      scored("e", 85),
      scored("f", 60),
      scored("g"),
    ];
    const sections = selectStoreSections(games);

    expect(sections.topRated.map((entry) => [entry.game.id, entry.rank, entry.score])).toEqual([
      ["c", 1, 91],
      ["e", 2, 85],
      ["b", 3, 70],
    ]);
    expect(sections.featured.map((entry) => entry.id)).toEqual(["a", "d", "f"]);
    expect(sections.rest.map((entry) => entry.id)).toEqual(["g"]);
    const shown = [...sections.featured, ...sections.topRated.map((entry) => entry.game), ...sections.rest];
    expect(new Set(shown.map((entry) => entry.id)).size).toBe(games.length);
  });

  it("numbers nobody when no game carries a score", () => {
    const sections = selectStoreSections(["a", "b", "c", "d", "e"].map((id) => scored(id)));
    expect(sections.topRated).toEqual([]);
    expect(sections.featured.map((entry) => entry.id)).toEqual(["a", "b", "c"]);
    expect(sections.rest.map((entry) => entry.id)).toEqual(["d", "e"]);
  });

  it("keeps a tab of three or fewer on one row", () => {
    const sections = selectStoreSections([scored("a", 90), scored("b", 80)]);
    expect(sections).toEqual({ featured: [scored("a", 90), scored("b", 80)], topRated: [], rest: [] });
  });
});

describe("criticScoreOf", () => {
  it("prefers the game's own score, then the bundled catalogue's, else none", () => {
    const bundled = EDITORIAL_GAMES.find((entry) => criticScoreOf(entry) !== null);
    expect(bundled).toBeDefined();
    expect(criticScoreOf({ ...bundled!, criticScore: 12 })).toBe(12);
    expect(criticScoreOf(game({ id: "nobody:knows" }))).toBeNull();
  });
});

describe("editorialProviderStatuses", () => {
  it("reads its messages in the language the interface is in when asked", () => {
    try {
      setLanguage("fr");
      const french = editorialProviderStatuses().find((entry) => entry.provider === "steam")?.message;
      setLanguage("en");
      const english = editorialProviderStatuses().find((entry) => entry.provider === "steam")?.message;
      expect(french).not.toBe(english);
    } finally {
      setLanguage("en");
    }
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

describe("selectBestOffer ordering", () => {
  // selectBestOffer reads the wall clock for staleness, so the stamp has to be
  // recent against it rather than against the fixture's frozen NOW.
  const fresh = new Date(Date.now() - HOUR).toISOString();

  it("never lets an unprintable quote displace a real price", () => {
    // A row carrying an amount but no currency renders as an empty string, so
    // it must not win over a shop that quoted something Orivo can print.
    const priced = offer({
      id: "priced",
      priceMinor: 10_000,
      currency: "EUR",
      availability: "available",
      verifiedAt: null,
      stale: true,
    });
    const currencyless = offer({
      id: "currencyless",
      priceMinor: 5,
      currency: null,
      availability: "available",
      verifiedAt: fresh,
      stale: false,
    });

    expect(selectBestOffer(game({ offers: [priced, currencyless] }), "EUR")?.id).toBe("priced");
    expect(selectBestOffer(game({ offers: [currencyless, priced] }), "EUR")?.id).toBe("priced");
  });

  it("orders the same whichever way the offers arrive", () => {
    const expensive = offer({
      id: "expensive",
      priceMinor: 90_000,
      currency: "EUR",
      availability: "available",
      verifiedAt: fresh,
      stale: false,
    });
    const cheap = offer({
      id: "cheap",
      priceMinor: 10_000,
      currency: "EUR",
      availability: "available",
      verifiedAt: null,
      stale: true,
    });
    const currencyless = offer({
      id: "currencyless",
      priceMinor: 5,
      currency: null,
      availability: "available",
      verifiedAt: null,
      stale: true,
    });

    for (const offers of [
      [expensive, cheap, currencyless],
      [currencyless, cheap, expensive],
      [cheap, currencyless, expensive],
    ]) {
      expect(selectBestOffer(game({ offers }), "EUR")?.id).toBe("cheap");
    }
  });

  it("falls back to freshness when no shop quoted anything", () => {
    const stale = offer({ id: "stale", availability: "available", verifiedAt: null, stale: true });
    const verified = offer({
      id: "verified",
      availability: "available",
      verifiedAt: fresh,
      stale: false,
    });

    expect(selectBestOffer(game({ offers: [stale, verified] }), "EUR")?.id).toBe("verified");
    expect(selectBestOffer(game({ offers: [verified, stale] }), "EUR")?.id).toBe("verified");
  });
});
