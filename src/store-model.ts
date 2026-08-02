import type {
  GameSummary,
  ProviderStatus,
  StoreCategory,
  StoreOffer,
  StoreProvider,
} from "./contracts";

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
  providers: StoreProvider[];
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
  providers: StoreProvider[];
  query: string;
  browseGames: GameSummary[] | null;
  nextCursor: string | null;
  activeRequestId: number;
  errorMessage: string;
}

export type StorePageAction =
  | {
      type: "activate";
      category: StoreCategory;
      providers: StoreProvider[];
      query: string;
      online: boolean;
    }
  | { type: "request-started"; requestId: number; refresh: boolean }
  | { type: "home-loaded"; requestId: number; home: StoreHomeView }
  | { type: "browse-loaded"; requestId: number; page: StoreBrowsePage; append: boolean }
  | { type: "request-failed"; requestId: number; message: string; offline: boolean }
  | { type: "category-changed"; category: StoreCategory }
  | { type: "providers-changed"; providers: StoreProvider[] }
  | { type: "query-changed"; query: string }
  | { type: "wishlist-changed"; gameId: string; wishlisted: boolean }
  | { type: "connectivity-changed"; online: boolean };

export interface StoreCategoryOption {
  id: StoreCategory;
  label: string;
}

export interface StoreProviderOption {
  id: StoreProvider;
  label: string;
}

export const STORE_CATEGORIES: readonly StoreCategoryOption[] = [
  { id: "for-you", label: "For You" },
  { id: "short-sessions", label: "Short Sessions" },
  { id: "strong-stories", label: "Strong Stories" },
  { id: "relaxing", label: "Relaxing" },
  { id: "all-games", label: "All Games" },
];

export const STORE_PROVIDERS: readonly StoreProviderOption[] = [
  { id: "steam", label: "Steam" },
  { id: "ubisoft", label: "Ubisoft" },
  { id: "microsoft", label: "Microsoft/Xbox" },
  { id: "apple", label: "Apple App Store" },
  { id: "google-play", label: "Google Play" },
  { id: "instant-gaming", label: "Instant Gaming" },
];

const providerStatus = (
  provider: StoreProvider,
  label: string,
  health: ProviderStatus["health"],
  message: string,
): ProviderStatus => ({ provider, label, health, message, refreshedAt: null });

export const EDITORIAL_PROVIDER_STATUSES: ProviderStatus[] = [
  providerStatus("steam", "Steam", "not-configured", "Connect a host-side Steam Web API key for live catalog updates."),
  providerStatus("ubisoft", "Ubisoft", "unavailable", "No authorized catalog feed is configured."),
  providerStatus("microsoft", "Microsoft/Xbox", "unavailable", "A licensed XStore context is required."),
  providerStatus("apple", "Apple App Store", "degraded", "Live App Store search will appear when a network connection is available."),
  providerStatus("google-play", "Google Play", "not-configured", "Registered third-party store access is required."),
  providerStatus("instant-gaming", "Instant Gaming", "unavailable", "No authorized commercial feed is configured."),
];

const media = (kind: "covers" | "heroes" | "landscapes", file: string): string =>
  `/media/igdb/${kind}/${file}`;

function editorialOffer(gameId: string, appId: string): StoreOffer {
  return {
    id: `offer_ed_${appId}`,
    gameId,
    provider: "steam",
    providerLabel: "Steam",
    priceMinor: null,
    currency: null,
    region: "US",
    verifiedAt: null,
    availability: "unknown",
    stale: true,
  };
}

interface EditorialGameSeed {
  appId: string;
  title: string;
  file: string;
  heroFile?: string;
  description: string;
  genres: string[];
  tags: string[];
  platforms: GameSummary["supportedPlatforms"];
  reasons: string[];
  /** Instant Gaming reference price in EUR; omitted when no price is known. */
  pricing?: { price: number; originalPrice?: number; discountPercent?: number };
}

const editorialSeeds: EditorialGameSeed[] = [
  {
    appId: "1245620",
    title: "Elden Ring",
    file: "elden-ring.jpg",
    heroFile: "elden-ring-wallpaper.png",
    description: "Explore a vast open world shaped by discovery, difficult encounters, and player choice.",
    genres: ["Action", "RPG"],
    tags: ["Open World", "Strong Stories", "Long Sessions"],
    platforms: ["windows"],
    reasons: ["Matches action RPGs", "Tagged open world", "Single-player campaign"],
    pricing: { price: 34.99, originalPrice: 59.99, discountPercent: 42 },
  },
  {
    appId: "1091500",
    title: "Cyberpunk 2077",
    file: "cyberpunk-2077.jpg",
    heroFile: "cyberpunk-2077.webp",
    description: "Build a mercenary's story across the dense districts and shifting alliances of Night City.",
    genres: ["Action", "RPG"],
    tags: ["Strong Stories", "Open World", "Single-player"],
    platforms: ["windows", "macos"],
    reasons: ["Story-rich campaign", "Matches action RPGs", "Available on macOS"],
    pricing: { price: 23.49, originalPrice: 59.99, discountPercent: 61 },
  },
  {
    appId: "1086940",
    title: "Baldur's Gate 3",
    file: "baldurs-gate-3.jpg",
    description: "Shape a party-driven adventure where combat, conversation, and exploration share the stage.",
    genres: ["RPG", "Strategy"],
    tags: ["Strong Stories", "Choices Matter", "Co-op"],
    platforms: ["windows", "macos"],
    reasons: ["Available on macOS", "Story-rich campaign", "Supports co-op"],
    pricing: { price: 46.79, originalPrice: 59.99, discountPercent: 22 },
  },
  {
    appId: "1145350",
    title: "Hades II",
    file: "hades-2.jpg",
    description: "Battle beyond the Underworld in focused runs that reveal more of the story each time.",
    genres: ["Action", "Roguelike"],
    tags: ["Short Sessions", "Replayable", "Strong Stories"],
    platforms: ["windows", "macos"],
    reasons: ["Works in short sessions", "Available on macOS", "Replayable runs"],
    pricing: { price: 24.49, originalPrice: 28.99, discountPercent: 16 },
  },
  {
    appId: "1174180",
    title: "Red Dead Redemption 2",
    file: "red-dead-redemption-2.jpg",
    description: "Travel with an outlaw gang through a changing frontier and a long-form character story.",
    genres: ["Action", "Adventure"],
    tags: ["Strong Stories", "Open World", "Atmospheric"],
    platforms: ["windows"],
    reasons: ["Story-rich campaign", "Tagged atmospheric", "Single-player adventure"],
    pricing: { price: 19.79, originalPrice: 59.99, discountPercent: 67 },
  },
  {
    appId: "292030",
    title: "The Witcher 3: Wild Hunt",
    file: "the-witcher-3-wild-hunt.jpg",
    description: "Track monsters and follow interwoven quests across a broad fantasy world.",
    genres: ["RPG", "Adventure"],
    tags: ["Strong Stories", "Open World", "Choices Matter"],
    platforms: ["windows"],
    reasons: ["Matches RPGs", "Story-rich campaign", "Tagged choices matter"],
    pricing: { price: 8.49, originalPrice: 29.99, discountPercent: 72 },
  },
  {
    appId: "2420110",
    title: "Horizon Forbidden West",
    file: "horizon-forbidden-west.jpg",
    description: "Cross a colorful frontier of machine encounters, ruins, and character-led quests.",
    genres: ["Action", "Adventure"],
    tags: ["Strong Stories", "Open World", "Exploration"],
    platforms: ["windows"],
    reasons: ["Story-rich campaign", "Tagged exploration", "Open-world adventure"],
    pricing: { price: 32.99, originalPrice: 59.99, discountPercent: 45 },
  },
  {
    appId: "1593500",
    title: "God of War",
    file: "god-of-war.jpg",
    description: "Follow Kratos and Atreus through a focused journey across the Norse realms.",
    genres: ["Action", "Adventure"],
    tags: ["Strong Stories", "Single-player", "Cinematic"],
    platforms: ["windows"],
    reasons: ["Story-rich campaign", "Single-player adventure", "Matches action games"],
    pricing: { price: 15.99, originalPrice: 49.99, discountPercent: 68 },
  },
  {
    appId: "1016920",
    title: "Unrailed!",
    file: "unrailed.jpg",
    description: "Build a railway together in quick procedural rounds before the train outruns the track.",
    genres: ["Co-op", "Strategy"],
    tags: ["Short Sessions", "Relaxing", "Local Co-op"],
    platforms: ["windows", "macos", "linux"],
    reasons: ["Works in short sessions", "Available on macOS", "Supports local co-op"],
    pricing: { price: 4.79, originalPrice: 19.99, discountPercent: 76 },
  },
  {
    appId: "655350",
    title: "Astro Duel 2",
    file: "astro-duel-2.jpg",
    description: "Switch between ship combat and on-foot action in compact competitive matches.",
    genres: ["Action", "Arcade"],
    tags: ["Short Sessions", "Local Multiplayer", "Campaign"],
    platforms: ["windows", "macos"],
    reasons: ["Works in short sessions", "Available on macOS", "Supports local multiplayer"],
  },
];

export const EDITORIAL_GAMES: GameSummary[] = editorialSeeds.map((seed) => {
  const gameId = `steam:${seed.appId}`;
  return {
    id: gameId,
    title: seed.title,
    source: "store",
    shortDescription: seed.description,
    coverUrl: media("covers", seed.file),
    heroUrl: media("heroes", seed.heroFile ?? seed.file),
    landscapeUrl: media("landscapes", seed.file.replace(/\.jpg$/, seed.heroFile?.endsWith(".webp") ? ".webp" : ".jpg")),
    genres: seed.genres,
    tags: seed.tags,
    supportedPlatforms: seed.platforms,
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: seed.reasons,
    offers: [editorialOffer(gameId, seed.appId)],
  };
});

/**
 * Card pricing facts. Prices are major currency units (24.99 renders as
 * "24,99 €"). `originalPrice` and `discountPercent` are only present for a
 * discounted offer; `priceProvider` names the store the price came from.
 */
export interface GamePricing {
  price: number;
  currency: string;
  originalPrice?: number;
  discountPercent?: number;
  priceProvider: string;
}

/**
 * Instant Gaming reference prices (EUR) for the built-in catalog. A game
 * absent from this map simply shows no price on its card. Live offers keep
 * their own price data through `selectGamePricing`.
 */
export const EDITORIAL_PRICING: Readonly<Record<string, GamePricing>> = Object.fromEntries(
  editorialSeeds.flatMap((seed) =>
    seed.pricing
      ? [[
          `steam:${seed.appId}`,
          { currency: "EUR", priceProvider: "Instant Gaming", ...seed.pricing } satisfies GamePricing,
        ]]
      : [],
  ),
);

const ZERO_DECIMAL_CURRENCIES = ["JPY", "KRW"];

function priceFractionDigits(currency: string): number {
  return ZERO_DECIMAL_CURRENCIES.includes(currency.toUpperCase()) ? 0 : 2;
}

/** Formats "24,99 €" style prices; falls back to "24.99 EUR" if Intl rejects the code. */
export function formatPrice(price: number, currency = "EUR"): string {
  const fractionDigits = priceFractionDigits(currency);
  try {
    return new Intl.NumberFormat("fr-FR", {
      style: "currency",
      currency,
      minimumFractionDigits: fractionDigits,
      maximumFractionDigits: fractionDigits,
    }).format(price);
  } catch {
    return `${price.toFixed(fractionDigits)} ${currency}`;
  }
}

export const EDITORIAL_STORE_HOME: StoreHomeView = {
  games: EDITORIAL_GAMES,
  providerStatuses: EDITORIAL_PROVIDER_STATUSES,
  recommendationMode: "editorial",
  recommendationHeading: "Editorial picks",
  refreshedAt: null,
};

export function createInitialStoreState(): StorePageState {
  return {
    phase: "loading",
    home: EDITORIAL_STORE_HOME,
    category: "for-you",
    providers: [],
    query: "",
    browseGames: null,
    nextCursor: null,
    activeRequestId: 0,
    errorMessage: "",
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
        providers: [...action.providers],
        query: action.query,
        browseGames: null,
        nextCursor: null,
        errorMessage: action.online ? state.errorMessage : "You are offline. Showing saved picks.",
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
        home: action.home.games.length > 0 ? action.home : { ...action.home, games: state.home.games },
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
      return { ...state, category: action.category, browseGames: null, nextCursor: null };
    case "providers-changed":
      return { ...state, providers: [...action.providers], browseGames: null, nextCursor: null };
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
    case "connectivity-changed":
      return action.online
        ? { ...state, phase: state.phase === "offline" ? "degraded" : state.phase }
        : { ...state, phase: "offline", errorMessage: "You are offline. Showing saved picks." };
  }
}

function mergeGames(current: GameSummary[], incoming: GameSummary[]): GameSummary[] {
  const merged = new Map(current.map((game) => [game.id, game]));
  for (const game of incoming) merged.set(game.id, game);
  return [...merged.values()];
}

function normalizedSearchText(value: string): string {
  return value
    .normalize("NFKD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLocaleLowerCase()
    .trim();
}

function matchesCategory(game: GameSummary, category: StoreCategory): boolean {
  if (category === "for-you" || category === "all-games") return true;
  const facts = normalizedSearchText([...game.tags, ...game.genres].join(" "));
  if (category === "short-sessions") return facts.includes("short session");
  if (category === "strong-stories") {
    return facts.includes("strong stor") || facts.includes("story rich") || facts.includes("story-rich");
  }
  return facts.includes("relaxing") || facts.includes("cozy");
}

export function selectStoreGames(state: StorePageState): GameSummary[] {
  if (state.browseGames) return state.browseGames;
  const query = normalizedSearchText(state.query);
  return state.home.games.filter((game) => {
    if (!matchesCategory(game, state.category)) return false;
    if (
      state.providers.length > 0 &&
      !game.offers.some((offer) => state.providers.includes(offer.provider))
    ) {
      return false;
    }
    if (!query) return true;
    return normalizedSearchText(
      [game.title, game.shortDescription, ...game.genres, ...game.tags].join(" "),
    ).includes(query);
  });
}

export function storeCategoryLabel(category: StoreCategory): string {
  return STORE_CATEGORIES.find((option) => option.id === category)?.label ?? "All Games";
}

export function isOfferStale(offer: StoreOffer, now = Date.now()): boolean {
  if (offer.stale || !offer.verifiedAt) return true;
  const verifiedAt = Date.parse(offer.verifiedAt);
  return !Number.isFinite(verifiedAt) || now - verifiedAt > 24 * 60 * 60 * 1_000;
}

export function selectBestOffer(game: GameSummary): StoreOffer | null {
  return (
    [...game.offers].sort((left, right) => {
      const availability = Number(right.availability === "available") - Number(left.availability === "available");
      if (availability !== 0) return availability;
      const freshness = Number(isOfferStale(left)) - Number(isOfferStale(right));
      if (freshness !== 0) return freshness;
      if (left.priceMinor === null) return 1;
      if (right.priceMinor === null) return -1;
      return left.priceMinor - right.priceMinor;
    })[0] ?? null
  );
}

/**
 * Price facts for a card: a live priced offer wins, then the Instant Gaming
 * reference price for the built-in catalog. Returns null when no price is
 * known — the card then shows nothing rather than a fabricated price.
 */
export function selectGamePricing(game: GameSummary): GamePricing | null {
  const offer = selectBestOffer(game);
  if (offer && offer.priceMinor !== null && offer.currency) {
    return {
      price: offer.priceMinor / 10 ** priceFractionDigits(offer.currency),
      currency: offer.currency,
      priceProvider: offer.providerLabel,
    };
  }
  return EDITORIAL_PRICING[game.id] ?? null;
}
