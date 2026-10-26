import type {
  GameSummary,
  ProviderStatus,
  StoreCategory,
  StoreCuration,
  StoreFitStat,
  StoreHighlight,
  StoreOffer,
  StorePlatform,
  StoreProvider,
} from "./contracts";
import { currentLanguage, t } from "./i18n";
import { STORE_CATALOG } from "./store-catalog.generated";

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
  category: StoreCategory;
  platforms: StorePlatform[];
  query: string;
  browseGames: GameSummary[] | null;
  nextCursor: string | null;
  activeRequestId: number;
  errorMessage: string;
  /** Ids already in the library; the Store never offers a game you own. */
  ownedGameIds: string[];
  /** The card the hero is currently describing. */
  previewGameId: string | null;
}

export type StorePageAction =
  | {
      type: "activate";
      category: StoreCategory;
      platforms: StorePlatform[];
      query: string;
      online: boolean;
    }
  | { type: "request-started"; requestId: number; refresh: boolean }
  | { type: "home-loaded"; requestId: number; home: StoreHomeView }
  | { type: "browse-loaded"; requestId: number; page: StoreBrowsePage; append: boolean }
  | { type: "request-failed"; requestId: number; message: string; offline: boolean }
  | { type: "category-changed"; category: StoreCategory }
  | { type: "platforms-changed"; platforms: StorePlatform[] }
  | { type: "query-changed"; query: string }
  | { type: "wishlist-changed"; gameId: string; wishlisted: boolean }
  | { type: "owned-games-loaded"; gameIds: string[] }
  | { type: "preview-changed"; gameId: string | null }
  | { type: "connectivity-changed"; online: boolean };

export interface StoreCategoryOption {
  id: StoreCategory;
  label: string;
}

export interface StorePlatformOption {
  id: StorePlatform;
  label: string;
  icon: "windows" | "playstation" | "xbox" | "switch" | "emulator";
}

/**
 * The filter bar. The four mood shelves (brain, short, stories, relaxing) were
 * dropped from the bar: the catalog still carries those tags, but the shelves
 * are no longer offered, and an old link to one lands on "Pour toi".
 */
export const STORE_CATEGORIES: readonly StoreCategoryOption[] = [
  { id: "for-you", label: t("store.category.forYou") },
  { id: "all-games", label: t("store.category.allGames") },
];

export const STORE_PLATFORMS: readonly StorePlatformOption[] = [
  { id: "pc", label: "PC", icon: "windows" },
  { id: "playstation", label: "PlayStation", icon: "playstation" },
  { id: "xbox", label: "Xbox", icon: "xbox" },
  { id: "switch", label: "Switch", icon: "switch" },
  { id: "emulators", label: t("store.platform.emulators"), icon: "emulator" },
];

/**
 * A provider is a shop, a platform is the machine. Filtering happens on the
 * platform because that is the question a shopper actually asks; the price
 * comparison stays on providers.
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

export const EDITORIAL_PROVIDER_STATUSES: ProviderStatus[] = [
  providerStatus("steam", "Steam", "available", t("store.provider.steam")),
  providerStatus("gog", "GOG", "available", t("store.provider.usd")),
  providerStatus("epic", "Epic Games Store", "available", t("store.provider.usd")),
  providerStatus("humble", "Humble Store", "available", t("store.provider.usd")),
  providerStatus("fanatical", "Fanatical", "available", t("store.provider.usd")),
  providerStatus(
    "instant-gaming",
    "Instant Gaming",
    "not-configured",
    t("store.provider.notConfigured"),
  ),
  providerStatus("playstation", "PlayStation Store", "degraded", t("store.provider.unverified")),
  providerStatus("microsoft", "Microsoft Store", "degraded", t("store.provider.unverified")),
  providerStatus("nintendo", "Nintendo eShop", "degraded", t("store.provider.unverified")),
];

/**
 * The catalog shipped with the app. Every title, description, genre, platform
 * and price comes from the game's own French store listing (see
 * `scripts/fetch-store-catalog.mjs`); the editorial copy in `curation` is
 * Orivo's. It is what the page shows before — and instead of — a live feed.
 */
export const EDITORIAL_GAMES: GameSummary[] = STORE_CATALOG;

export const EDITORIAL_STORE_HOME: StoreHomeView = {
  games: EDITORIAL_GAMES,
  providerStatuses: EDITORIAL_PROVIDER_STATUSES,
  recommendationMode: "editorial",
  recommendationHeading: t("store.recommended"),
  refreshedAt: null,
};

export function createInitialStoreState(): StorePageState {
  return {
    phase: "loading",
    home: EDITORIAL_STORE_HOME,
    category: "for-you",
    platforms: [],
    query: "",
    browseGames: null,
    nextCursor: null,
    activeRequestId: 0,
    errorMessage: "",
    ownedGameIds: [],
    previewGameId: null,
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
        category: action.category,
        platforms: [...action.platforms],
        query: action.query,
        browseGames: null,
        nextCursor: null,
        previewGameId: null,
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
    case "category-changed":
      return {
        ...state,
        category: action.category,
        browseGames: null,
        nextCursor: null,
        previewGameId: null,
      };
    case "platforms-changed":
      return {
        ...state,
        platforms: [...action.platforms],
        browseGames: null,
        nextCursor: null,
        previewGameId: null,
      };
    case "query-changed":
      return { ...state, query: action.query, browseGames: null, nextCursor: null, previewGameId: null };
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
    case "preview-changed":
      return { ...state, previewGameId: action.gameId };
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
 * The host's list, verbatim — only the editorial copy is borrowed back from the
 * bundled row when the host did not carry it. Nothing is ever added.
 */
function withCuration(bundled: GameSummary[], incoming: GameSummary[]): GameSummary[] {
  const copy = new Map(bundled.map((game) => [game.id, game.curation]));
  return incoming.map((game) => (game.curation ? game : { ...game, curation: copy.get(game.id) }));
}

function normalizedSearchText(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase()
    .trim();
}

const CATEGORY_KEYWORDS: Readonly<Record<string, string[]>> = {
  "good-for-brain": ["puzzle", "reflexion", "strategie", "strategy", "logique", "cartes", "enquete"],
  "short-sessions": ["courte", "short", "arcade", "roguelike"],
  "strong-stories": ["recits", "story", "stories", "histoire", "narration", "aventure"],
  // "occasionnel" is what Steam calls Casual in French, and the genre a live
  // listing arrives with. Without it the tab stays empty for every game the
  // shop sends, however well it fits.
  relaxing: ["relaxant", "relax", "cozy", "detente", "contemplat", "simulation", "occasionnel"],
};

export function matchesCategory(game: GameSummary, category: StoreCategory): boolean {
  if (category === "for-you" || category === "all-games") return true;
  if (game.curation?.categories.includes(category)) return true;
  const facts = normalizedSearchText([...game.tags, ...game.genres].join(" "));
  return (CATEGORY_KEYWORDS[category] ?? []).some((keyword) => facts.includes(keyword));
}

/**
 * Every platform a game is available on: the union of what the shell merged
 * (`availablePlatforms`, which includes the IGDB lookup), the curator's list,
 * its offers and its declared builds. A union, because each is partial: a game
 * sold on Steam has no PlayStation offer even when it ships on PlayStation, so
 * the PlayStation filter must not depend on offers alone. Without IGDB
 * credentials `availablePlatforms` only repeats what offers and the curated
 * catalogue prove, so a live multi-platform game shows up under PC only.
 */
export function gamePlatforms(game: GameSummary): StorePlatform[] {
  const platforms = new Set<StorePlatform>(game.curation?.platforms ?? []);
  for (const platform of game.availablePlatforms ?? []) platforms.add(platform);
  for (const offer of game.offers) platforms.add(PROVIDER_PLATFORM[offer.provider]);
  if (game.supportedPlatforms.some((platform) => platform !== "ios" && platform !== "android")) {
    platforms.add("pc");
  }
  // A Windows-only title on a Mac is exactly what the emulation layer exists
  // for, so it is honestly offered under "Emulateurs" too.
  if (game.supportedPlatforms.length === 1 && game.supportedPlatforms[0] === "windows") {
    platforms.add("emulators");
  }
  return [...platforms];
}

export function matchesPlatforms(game: GameSummary, platforms: StorePlatform[]): boolean {
  if (platforms.length === 0) return true;
  const available = new Set(gamePlatforms(game));
  return platforms.some((platform) => available.has(platform));
}

/**
 * A library entry and a Store row can carry different ids for the same game — a
 * Steam import versus a catalogue row, say — so ownership is also matched on a
 * normalised title.
 */
export function ownershipKey(title: string): string {
  return normalizedSearchText(title).replace(/[^a-z0-9]+/g, "");
}

export interface StoreFilters {
  category: StoreCategory;
  platforms: StorePlatform[];
  query: string;
}

/** Whether a game survives the category, platform and text filters. */
export function matchesStoreFilters(game: GameSummary, filters: StoreFilters): boolean {
  if (!matchesCategory(game, filters.category)) return false;
  if (!matchesPlatforms(game, filters.platforms)) return false;
  const query = normalizedSearchText(filters.query);
  if (!query) return true;
  return normalizedSearchText(
    [game.title, game.shortDescription, ...game.genres, ...game.tags].join(" "),
  ).includes(query);
}

/** Every game the current filters allow, minus everything already owned. */
export function selectStoreGames(state: StorePageState): GameSummary[] {
  const owned = new Set(state.ownedGameIds);
  const isOwned = (game: GameSummary): boolean =>
    owned.has(game.id) || owned.has(ownershipKey(game.title)) || game.owned;
  // A browse page arrives already filtered, so its answer is kept whole rather
  // than run through the local heuristic again. Ownership still filters,
  // because the library can change after the page was fetched.
  if (state.browseGames) return state.browseGames.filter((game) => !isOwned(game));
  return state.home.games.filter((game) => !isOwned(game) && matchesStoreFilters(game, state));
}

export function storeCategoryLabel(category: StoreCategory): string {
  return STORE_CATEGORIES.find((option) => option.id === category)?.label ?? t("store.category.allGames");
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

/**
 * The five-dot scores, and only where they are real.
 *
 * They used to be invented for a game with no editorial entry — three of its
 * tags, scored `3 + hash(id) % 3` and announced to a screen reader as "4 sur
 * 5". Nothing showed it while every game on the shelf was written by hand, but
 * the moment a game arrives from a shop's own catalogue the shelf starts rating
 * games at random. A card with nothing to say here says nothing.
 */
export function fitStats(game: GameSummary): StoreFitStat[] {
  return game.curation?.stats.slice(0, 3) ?? [];
}

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

export const DEFAULT_HERO_TITLE = t("store.hero.title");
export const DEFAULT_HERO_LEAD = t("store.hero.lead");
export const DEFAULT_HIGHLIGHTS: StoreHighlight[] = [
  {
    icon: "brain",
    title: t("store.hero.highlight.brain.title"),
    text: t("store.hero.highlight.brain.text"),
  },
  {
    icon: "clock",
    title: t("store.hero.highlight.time.title"),
    text: t("store.hero.highlight.time.text"),
  },
  {
    icon: "book",
    title: t("store.hero.highlight.story.title"),
    text: t("store.hero.highlight.story.text"),
  },
];

export interface StoreHeroCopy {
  eyebrow: string;
  title: string;
  lead: string;
  highlights: StoreHighlight[];
  backgroundUrl: string;
  actionLabel: string;
  gameId: string | null;
}

export const DEFAULT_STORE_BACKGROUND = "/media/store/hero-mountains.jpg";

/** The hero follows the previewed card; with nothing previewed it is the page's own pitch. */
export function selectHeroCopy(state: StorePageState, games: GameSummary[]): StoreHeroCopy {
  const preview = state.previewGameId
    ? games.find((game) => game.id === state.previewGameId) ?? null
    : null;
  if (!preview) {
    return {
      eyebrow: t("store.recommended"),
      title: DEFAULT_HERO_TITLE,
      lead: DEFAULT_HERO_LEAD,
      highlights: DEFAULT_HIGHLIGHTS,
      backgroundUrl: DEFAULT_STORE_BACKGROUND,
      actionLabel: t("store.hero.discoverWhy"),
      gameId: null,
    };
  }
  const curation: StoreCuration | undefined = preview.curation;
  return {
    eyebrow: t("store.recommended"),
    title: curation?.heroTitle || DEFAULT_HERO_TITLE,
    lead: curation?.heroLead || preview.shortDescription || DEFAULT_HERO_LEAD,
    highlights: curation?.highlights.length ? curation.highlights : DEFAULT_HIGHLIGHTS,
    backgroundUrl: preview.heroUrl || preview.landscapeUrl || DEFAULT_STORE_BACKGROUND,
    actionLabel: t("store.hero.discover", { title: preview.title }),
    gameId: preview.id,
  };
}
