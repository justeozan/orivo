import type {
  GameSummary,
  ProviderStatus,
  StoreCategory,
  StoreOffer,
  StorePlatform,
  StoreProvider,
  StoreTabId,
} from "./contracts";
import { currentLanguage, t } from "./i18n";
import { STORE_CATALOG, STORE_CATALOG_META } from "./generated/store-catalog";

export type StoreLoadPhase =
  | "loading"
  | "ready"
  | "refreshing"
  | "degraded"
  | "offline"
  | "error";

export interface StoreHomeView {
  games: GameSummary[];
  providerStatuses: ProviderStatus[];
  recommendationMode: "editorial" | "personalized";
  recommendationHeading: string;
  refreshedAt: string | null;
}

/**
 * What the host's browse command takes. The page no longer filters on mood or
 * machine, so it only ever asks for a search across every game; the fields stay
 * because the command still reads them.
 */
export interface StoreBrowseRequest {
  category: StoreCategory;
  platforms: StorePlatform[];
  query: string;
  cursor: string | null;
  limit: number;
}

export interface StoreBrowsePage {
  games: GameSummary[];
  nextCursor: string | null;
  providerStatuses: ProviderStatus[];
}

export interface StorePageState {
  phase: StoreLoadPhase;
  home: StoreHomeView;
  /** The tab on screen: every exclusive, one brand, or a plugin's. */
  store: StoreTabId;
  query: string;
  /** The host's answer to the current search, when it has given one. */
  browseGames: GameSummary[] | null;
  nextCursor: string | null;
  activeRequestId: number;
  errorMessage: string;
  /** Ids already in the library; the Store never offers a game you own. */
  ownedGameIds: string[];
}

export type StorePageAction =
  | { type: "activate"; store: StoreTabId; query: string; online: boolean }
  | { type: "request-started"; requestId: number; refresh: boolean }
  | { type: "home-loaded"; requestId: number; home: StoreHomeView }
  | { type: "browse-loaded"; requestId: number; page: StoreBrowsePage; append: boolean }
  | { type: "request-failed"; requestId: number; message: string; offline: boolean }
  | { type: "store-changed"; store: StoreTabId }
  | { type: "query-changed"; query: string }
  | { type: "wishlist-changed"; gameId: string; wishlisted: boolean }
  | { type: "owned-games-loaded"; gameIds: string[] }
  | { type: "connectivity-changed"; online: boolean };

/**
 * A provider is a shop, a platform is the machine. The host's browse command
 * still filters on shops, so the platform a request names is turned into the
 * shops that sell for it.
 */
export const PROVIDER_PLATFORM: Readonly<Record<StoreProvider, StorePlatform>> = {
  steam: "pc",
  "instant-gaming": "pc",
  epic: "pc",
  gog: "pc",
  humble: "pc",
  fanatical: "pc",
  "green-man-gaming": "pc",
  ubisoft: "pc",
  microsoft: "xbox",
  playstation: "playstation",
  nintendo: "switch",
  apple: "pc",
  "google-play": "pc",
};

export function providersForPlatforms(platforms: StorePlatform[]): StoreProvider[] {
  const wanted = new Set(platforms);
  return (Object.keys(PROVIDER_PLATFORM) as StoreProvider[]).filter((provider) =>
    wanted.has(PROVIDER_PLATFORM[provider]),
  );
}

const providerStatus = (
  provider: StoreProvider,
  label: string,
  health: ProviderStatus["health"],
  message: string,
): ProviderStatus => ({ provider, label, health, message, refreshedAt: null });

/**
 * The bundled sources. Built on demand rather than at import, so the messages
 * are read in the language the interface is in when they are asked for.
 */
export function editorialProviderStatuses(): ProviderStatus[] {
  return [
    providerStatus("steam", "Steam", "available", t("store.provider.steam")),
    providerStatus("gog", "GOG", "available", t("store.provider.usd")),
    providerStatus("epic", "Epic Games Store", "available", t("store.provider.usd")),
    providerStatus("humble", "Humble Store", "available", t("store.provider.usd")),
    providerStatus("fanatical", "Fanatical", "available", t("store.provider.usd")),
    providerStatus("instant-gaming", "Instant Gaming", "not-configured", t("store.provider.notConfigured")),
    providerStatus("playstation", "PlayStation Store", "degraded", t("store.provider.unverified")),
    providerStatus("microsoft", "Microsoft Store", "degraded", t("store.provider.unverified")),
    providerStatus("nintendo", "Nintendo eShop", "degraded", t("store.provider.unverified")),
  ];
}

/**
 * The catalog shipped with the app. Every title, description, genre, platform
 * and price comes from the game's own store listing (see
 * `scripts/fetch-store-catalog.mjs`); the editorial copy in `curation` is
 * Orivo's. It is what the page shows before — and instead of — a live feed.
 */
export const EDITORIAL_GAMES: GameSummary[] = STORE_CATALOG;

export function editorialStoreHome(): StoreHomeView {
  return {
    games: EDITORIAL_GAMES,
    providerStatuses: editorialProviderStatuses(),
    recommendationMode: "editorial",
    recommendationHeading: t("store.recommended"),
    refreshedAt: null,
  };
}

export function createInitialStoreState(): StorePageState {
  return {
    phase: "loading",
    home: editorialStoreHome(),
    store: "all",
    query: "",
    browseGames: null,
    nextCursor: null,
    activeRequestId: 0,
    errorMessage: "",
    ownedGameIds: [],
  };
}

function updateWishlist(games: GameSummary[], gameId: string, wishlisted: boolean): GameSummary[] {
  return games.map((game) => (game.id === gameId ? { ...game, wishlisted } : game));
}

export function reduceStorePageState(
  state: StorePageState,
  action: StorePageAction,
): StorePageState {
  switch (action.type) {
    case "activate":
      return {
        ...state,
        phase: action.online ? state.phase : "offline",
        store: action.store,
        query: action.query,
        browseGames: null,
        nextCursor: null,
        errorMessage: action.online ? state.errorMessage : t("store.offline"),
      };
    case "request-started":
      return {
        ...state,
        activeRequestId: action.requestId,
        phase: action.refresh && state.home.games.length > 0 ? "refreshing" : "loading",
        errorMessage: "",
      };
    case "home-loaded":
      if (action.requestId !== state.activeRequestId) return state;
      return {
        ...state,
        phase: action.home.providerStatuses.some((status) => status.health === "degraded")
          ? "degraded"
          : "ready",
        // The host's answer REPLACES the bundled shelf rather than merging into
        // it. Merging would resurrect exactly the games the host just removed
        // because the library already owns them. A feed that returns nothing at
        // all is the one case where the bundled shelf is kept, so the page is
        // never empty.
        home:
          action.home.games.length > 0
            ? { ...action.home, games: withCuration(state.home.games, action.home.games) }
            : { ...action.home, games: state.home.games },
        browseGames: null,
        errorMessage: "",
      };
    case "browse-loaded": {
      if (action.requestId !== state.activeRequestId) return state;
      const browseGames = action.append
        ? mergeGames(state.browseGames ?? [], action.page.games)
        : action.page.games;
      return {
        ...state,
        phase: action.page.providerStatuses.some((status) => status.health === "degraded")
          ? "degraded"
          : "ready",
        home: { ...state.home, providerStatuses: action.page.providerStatuses },
        browseGames,
        nextCursor: action.page.nextCursor,
        errorMessage: "",
      };
    }
    case "request-failed":
      if (action.requestId !== state.activeRequestId) return state;
      return {
        ...state,
        phase: action.offline ? "offline" : state.home.games.length > 0 ? "degraded" : "error",
        errorMessage: action.message,
      };
    case "store-changed":
      return { ...state, store: action.store };
    case "query-changed":
      return { ...state, query: action.query, browseGames: null, nextCursor: null };
    case "wishlist-changed":
      return {
        ...state,
        home: {
          ...state.home,
          games: updateWishlist(state.home.games, action.gameId, action.wishlisted),
        },
        browseGames: state.browseGames
          ? updateWishlist(state.browseGames, action.gameId, action.wishlisted)
          : null,
      };
    case "owned-games-loaded":
      return { ...state, ownedGameIds: [...new Set(action.gameIds)] };
    case "connectivity-changed":
      return action.online
        ? { ...state, phase: state.phase === "offline" ? "degraded" : state.phase }
        : {
            ...state,
            phase: "offline",
            errorMessage: t("store.offline"),
          };
  }
}

/** Appends live rows to a list, keeping the order and de-duplicating by id. */
function mergeGames(current: GameSummary[], incoming: GameSummary[]): GameSummary[] {
  const merged = new Map(current.map((game) => [game.id, game]));
  for (const game of incoming) {
    const existing = merged.get(game.id);
    // A live row wins on facts, but it never drops the editorial copy that the
    // card layout depends on.
    merged.set(game.id, existing?.curation ? { ...game, curation: game.curation ?? existing.curation } : game);
  }
  return [...merged.values()];
}

/**
 * The host's list, verbatim — only what Orivo curated itself is borrowed back
 * from the bundled row when the host did not carry it: the editorial copy, the
 * exclusivity, the gallery and the critics' score. Nothing is ever added.
 */
function withCuration(bundled: GameSummary[], incoming: GameSummary[]): GameSummary[] {
  const rows = new Map(bundled.map((game) => [game.id, game]));
  return incoming.map((game) => {
    const row = rows.get(game.id);
    if (!row) return game;
    return {
      ...game,
      curation: game.curation ?? row.curation,
      exclusiveTo: game.exclusiveTo ?? row.exclusiveTo,
      gallery: game.gallery ?? row.gallery,
      criticScore: game.criticScore ?? row.criticScore,
    };
  });
}

function normalizedSearchText(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase()
    .trim();
}

/**
 * A library entry and a Store row can carry different ids for the same game — a
 * Steam import versus a catalogue row, say — so ownership is also matched on a
 * normalised title.
 */
export function ownershipKey(title: string): string {
  return normalizedSearchText(title).replace(/[^a-z0-9]+/g, "");
}

/** Whether a game answers a search, on its title, pitch, genres and tags. */
export function matchesQuery(game: GameSummary, query: string): boolean {
  const wanted = normalizedSearchText(query);
  if (!wanted) return true;
  return normalizedSearchText(
    [game.title, game.shortDescription, ...game.genres, ...game.tags].join(" "),
  ).includes(wanted);
}

/**
 * The store one step along `order`, looping at both ends. A store that is not
 * in the order — a plugin tab whose plugin went away — steps from the start.
 */
export function adjacentStore(order: readonly StoreTabId[], current: StoreTabId, step: 1 | -1): StoreTabId {
  if (order.length === 0) return "all";
  const index = order.indexOf(current);
  if (index === -1) return order[step > 0 ? 0 : order.length - 1];
  return order[(index + step + order.length) % order.length];
}

/** Whether "Tout" is showing exclusives, or the whole shelf because none exist yet. */
export function hasExclusives(games: GameSummary[]): boolean {
  return games.some((game) => game.exclusiveTo !== undefined);
}

/**
 * The shelf a tab stands for. A brand shows its exclusives and nothing else, so
 * a brand nobody has curated yet is honestly empty. "Tout" gathers every
 * exclusive — and, while the catalogue has none at all, keeps the whole shelf
 * so the Store never opens on nothing.
 */
export function selectTabGames(
  games: GameSummary[],
  store: StoreTabId,
  pluginGames: GameSummary[] | null = null,
): GameSummary[] {
  if (store.startsWith("plugin:")) return pluginGames ?? [];
  if (store === "all") return hasExclusives(games) ? games.filter((game) => game.exclusiveTo) : games;
  return games.filter((game) => game.exclusiveTo === store);
}

/**
 * Every game the page shows, minus everything already owned. A search on
 * "Tout" reaches every game the Store knows — the host's answer when it has
 * one — because a name typed in full is a game wanted whatever it is exclusive
 * to; on a brand or a plugin tab it narrows that tab.
 */
export function selectStoreGames(
  state: StorePageState,
  pluginGames: GameSummary[] | null = null,
): GameSummary[] {
  const owned = new Set(state.ownedGameIds);
  const isOwned = (game: GameSummary): boolean =>
    owned.has(game.id) || owned.has(ownershipKey(game.title)) || game.owned;
  const query = state.query.trim();
  if (query && state.store === "all") {
    // A browse page arrives already searched, so it is kept whole rather than
    // run through the local match again. Ownership still filters, because the
    // library can change after the page was fetched.
    if (state.browseGames) return state.browseGames.filter((game) => !isOwned(game));
    return state.home.games.filter((game) => !isOwned(game) && matchesQuery(game, query));
  }
  return selectTabGames(state.home.games, state.store, pluginGames).filter(
    (game) => !isOwned(game) && matchesQuery(game, query),
  );
}

/** The critics' aggregate: the game's own, else the bundled catalogue's record. */
export function criticScoreOf(game: GameSummary): number | null {
  if (typeof game.criticScore === "number" && Number.isFinite(game.criticScore)) return game.criticScore;
  return STORE_CATALOG_META[game.id]?.metacritic ?? null;
}

export interface RankedGame {
  game: GameSummary;
  rank: number;
  score: number;
}

export interface StoreSections {
  /** The opening row, in the shelf's own order. */
  featured: GameSummary[];
  /** The three best-reviewed, numbered. Empty when no score is known. */
  topRated: RankedGame[];
  /** Everything else, once each. */
  rest: GameSummary[];
}

/**
 * The page as the mockups lay it out: a row of picks, a numbered row, then the
 * rest. The numbered row is ranked on published critic scores and on nothing
 * else — a game nobody scored is never given a place in it — and it is taken
 * first, so its "1" is the best-reviewed game of the tab, not of what was left.
 * A tab of three or fewer is one row.
 */
export function selectStoreSections(games: GameSummary[]): StoreSections {
  if (games.length <= 3) return { featured: games, topRated: [], rest: [] };
  const topRated = games
    .map((game, index) => ({ game, index, score: criticScoreOf(game) }))
    .filter((entry): entry is { game: GameSummary; index: number; score: number } => entry.score !== null)
    .sort((left, right) => right.score - left.score || left.index - right.index)
    .slice(0, 3)
    .map((entry, index) => ({ game: entry.game, rank: index + 1, score: entry.score }));
  const ranked = new Set(topRated.map((entry) => entry.game.id));
  const remaining = games.filter((game) => !ranked.has(game.id));
  return { featured: remaining.slice(0, 3), topRated, rest: remaining.slice(3) };
}

export function isOfferStale(offer: StoreOffer, now = Date.now()): boolean {
  if (offer.stale || !offer.verifiedAt) return true;
  const verifiedAt = Date.parse(offer.verifiedAt);
  return !Number.isFinite(verifiedAt) || now - verifiedAt > 24 * 60 * 60 * 1_000;
}

/** "2 août" — the day a price was actually read from the shop. */
export function offerVerifiedOn(offer: StoreOffer): string {
  if (!offer.verifiedAt) return "";
  const verifiedAt = new Date(offer.verifiedAt);
  if (Number.isNaN(verifiedAt.getTime())) return "";
  try {
    return new Intl.DateTimeFormat(currentLanguage(), { day: "numeric", month: "long" }).format(verifiedAt);
  } catch {
    return "";
  }
}

/** The shopper's own currency, so a euro price is never compared to a dollar one. */
export function displayCurrency(): string {
  try {
    const resolved = new Intl.NumberFormat(undefined, { style: "currency", currency: "EUR" }).resolvedOptions();
    return resolved.currency ?? "EUR";
  } catch {
    return "EUR";
  }
}

/**
 * A quote Orivo can actually print. Free needs no currency to be true, which is
 * the one case `formatPrice` also answers without one.
 */
function isPriced(offer: StoreOffer): boolean {
  return offer.priceMinor !== null && (offer.priceMinor === 0 || Boolean(offer.currency));
}

function priceRank(offer: StoreOffer, currency: string): number {
  const price = offer.priceMinor ?? 0;
  // Only same-currency prices are comparable; a foreign quote is ranked behind
  // every local one rather than converted at a rate Orivo cannot verify.
  return offer.currency === currency || price === 0 ? price : price + 1_000_000;
}

/**
 * The cheapest verified offer. Availability comes first, then a real price,
 * then freshness — a shop that quoted nothing never wins over one that did.
 */
export function selectBestOffer(game: GameSummary, currency = displayCurrency()): StoreOffer | null {
  return (
    [...game.offers].sort((left, right) => {
      const availability =
        Number(right.availability === "available") - Number(left.availability === "available");
      if (availability !== 0) return availability;
      // Ranking only ever compares two printable quotes, so the comparator
      // stays a total order instead of returning NaN on a pair of blanks.
      const leftPriced = isPriced(left);
      const rightPriced = isPriced(right);
      if (leftPriced !== rightPriced) return leftPriced ? -1 : 1;
      if (leftPriced) {
        const rank = priceRank(left, currency) - priceRank(right, currency);
        if (rank !== 0) return rank;
      }
      return Number(isOfferStale(left)) - Number(isOfferStale(right));
    })[0] ?? null
  );
}

export function formatPrice(offer: StoreOffer | null): string {
  if (!offer || offer.priceMinor === null) return "";
  // Free needs no currency to be true, so it is answered before one is required.
  if (offer.priceMinor === 0) return t("store.free");
  if (!offer.currency) return "";
  const fractionDigits = ["JPY", "KRW"].includes(offer.currency.toUpperCase()) ? 0 : 2;
  const amount = offer.priceMinor / 10 ** fractionDigits;
  try {
    return new Intl.NumberFormat("fr-FR", {
      style: "currency",
      currency: offer.currency,
      minimumFractionDigits: fractionDigits,
      maximumFractionDigits: fractionDigits,
    }).format(amount);
  } catch {
    return `${amount} ${offer.currency}`;
  }
}

// ---------------------------------------------------------------------------
// Presentation fallbacks. A game with no editorial copy still fills every slot
// the card layout owns, from its own facts.
// ---------------------------------------------------------------------------

export function sessionLabel(game: GameSummary): string {
  if (game.curation?.duration) return game.curation.duration;
  const hours = Math.round(game.playTimeSeconds / 3_600);
  return hours > 0 ? `${hours}h` : t("store.session.variable");
}

export function modeLabel(game: GameSummary): string {
  if (game.curation?.mode) return game.curation.mode;
  return game.tags.some((tag) => /co-?op|multi/i.test(tag)) ? t("store.mode.coop") : t("store.mode.solo");
}

export function genreLabel(game: GameSummary): string {
  const genres = game.curation?.genres.length ? game.curation.genres : game.genres;
  return genres.slice(0, 2).join(", ") || t("store.genre.unverified");
}

export function taglineLabel(game: GameSummary): string {
  return game.curation?.tagline || game.shortDescription;
}
