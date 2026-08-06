export type GameId = string;

/**
 * A store Orivo can sign into and read an owned library from. These tokens are
 * the single vocabulary shared by the backend's launch targets, the Sources
 * list and the source badge, so a game can never be labelled by one name and
 * launched by another.
 */
export type ConnectedSource =
  | "epic"
  | "gog"
  | "ubisoft"
  | "xbox"
  | "microsoft-store"
  | "instant-gaming";

export type GameSource =
  | "steam"
  | "wine"
  | "local"
  | "showcase"
  | "store"
  | ConnectedSource;

/**
 * How a store proves who you are. `token` stores an OAuth credential in the
 * system keychain and syncs in the background afterwards; `session` has no
 * account API at all, so its sign-in window stays signed in and each sync runs
 * inside it. The UI says which, because the two feel different to use.
 */
export type SourceConnectStyle = "token" | "session";

export interface SourceAccountStatus {
  provider: ConnectedSource;
  label: string;
  description: string;
  connected: boolean;
  accountLabel: string;
  style: SourceConnectStyle;
  /** Other sources that share this one's sign-in (Xbox and Microsoft Store). */
  sharesSignInWith: ConnectedSource[];
  launchable: boolean;
}

export interface SourceSyncResult {
  provider: ConnectedSource;
  label: string;
  totalGames: number;
  importedGames: number;
  updatedGames: number;
  /** Entries the store returned that Orivo could not read. Never hidden. */
  skippedGames: number;
}
export type StoreProvider =
  | "steam"
  | "instant-gaming"
  | "epic"
  | "gog"
  | "humble"
  | "fanatical"
  | "green-man-gaming"
  | "ubisoft"
  | "microsoft"
  | "playstation"
  | "nintendo"
  | "apple"
  | "google-play";

/**
 * What the shopper filters on. A provider is a shop; a platform is the machine
 * the game runs on. One platform is served by several providers, so the two
 * stay separate: the filter bar speaks platforms, the price logic speaks
 * providers.
 */
export type StorePlatform = "pc" | "playstation" | "xbox" | "switch" | "emulators";

export type ProviderHealth = "available" | "degraded" | "unavailable" | "not-configured";
export type StoreCategory =
  | "for-you"
  | "good-for-brain"
  | "short-sessions"
  | "strong-stories"
  | "relaxing"
  | "all-games";

export type SettingsSection =
  | "general"
  | "libraries"
  | "plugins"
  | "appearance"
  | "data"
  | "about";

export type GameMediaKind = "wallpaper" | "video" | "icon" | "cover";

export interface GameMediaView {
  id: string;
  kind: GameMediaKind;
  title: string;
  previewUrl: string;
  posterUrl?: string;
  origin: "bundled" | "provider" | "imported" | "downloaded";
  selected: boolean;
  availableOffline: boolean;
}

export type WallpaperSource =
  | "steam-store"
  | "wikimedia"
  | "openverse"
  | "igdb"
  | "google-images";

export type WallpaperSearchPhase = "ready" | "not-configured" | "error";

/**
 * The shape of artwork a row asks for. A search is scoped to exactly one:
 * the picker fills each row from its own request, so a wide screenshot can
 * never land in the portrait cover row.
 *
 * - `cover`     portrait box art, ~2:3
 * - `landscape` wide key art, ~16:9
 * - `background` atmospheric backgrounds and screenshots
 */
export type WallpaperCategory = "cover" | "landscape" | "background";

export const WALLPAPER_CATEGORIES: readonly WallpaperCategory[] = [
  "cover",
  "landscape",
  "background",
];

/** One result in a wallpaper search, addressable only by its opaque id. */
export interface WallpaperCandidateView {
  id: string;
  title: string;
  thumbnailUrl: string;
  category: WallpaperCategory;
}

export interface WallpaperSearchView {
  phase: WallpaperSearchPhase;
  source: WallpaperSource;
  category: WallpaperCategory;
  query: string;
  message: string;
  candidates: WallpaperCandidateView[];
}

/** API keys for the optional wallpaper sources, stored by Orivo on disk. */
export interface WallpaperCredentials {
  igdbClientId: string;
  igdbClientSecret: string;
  googleApiKey: string;
  googleCseId: string;
  /**
   * Optional. SteamGridDB is the one source with 4K-class art for all three
   * library roles, so "Reset the covers" prefers it when a key is present and
   * falls back to Steam's official artwork when it is not.
   */
  steamgriddbApiKey: string;
}

export type WallpaperCredentialsUpdate = Partial<WallpaperCredentials>;

export interface StoreOffer {
  id: string;
  gameId: GameId;
  provider: StoreProvider;
  providerLabel: string;
  priceMinor: number | null;
  currency: string | null;
  region: string;
  verifiedAt: string | null;
  availability: "available" | "unavailable" | "unknown";
  stale: boolean;
}

export interface ProviderStatus {
  provider: StoreProvider;
  label: string;
  health: ProviderHealth;
  message: string;
  refreshedAt: string | null;
}

/** One "fit" read-out row on a Store card: a French label and a 1-5 strength. */
export interface StoreFitStat {
  label: string;
  value: number;
}

/** One row of the hero's right-hand panel. */
export interface StoreHighlight {
  icon: string;
  title: string;
  text: string;
}

/**
 * Editorial copy Orivo writes about a game, keyed by game id and merged onto
 * whatever the catalog returns. It is presentation only: it never influences
 * ranking, and a game without an entry still renders from its own facts.
 */
export interface StoreCuration {
  genres: string[];
  duration: string;
  mode: string;
  stats: StoreFitStat[];
  tagline: string;
  heroTitle: string;
  heroLead: string;
  highlights: StoreHighlight[];
  categories: StoreCategory[];
  platforms: StorePlatform[];
}

export interface GameSummary {
  id: GameId;
  title: string;
  source: GameSource;
  shortDescription: string;
  coverUrl: string;
  heroUrl: string;
  landscapeUrl: string;
  genres: string[];
  tags: string[];
  supportedPlatforms: Array<"windows" | "macos" | "linux" | "ios" | "android">;
  owned: boolean;
  launchable: boolean;
  wishlisted: boolean;
  playTimeSeconds: number;
  lastPlayedAt: string | null;
  recommendationReasons: string[];
  offers: StoreOffer[];
  /** Present only for Store games Orivo has written editorial copy for. */
  curation?: StoreCuration;
}

export interface GameDetailView extends GameSummary {
  about: string;
  developer: string | null;
  publisher: string | null;
  releaseDate: string | null;
  features: string[];
  achievements: {
    unlocked: number;
    total: number;
    items: Array<{ id: string; title: string; iconUrl: string }>;
  } | null;
  media: GameMediaView[];
  relatedGames: GameSummary[];
  primaryAction:
    | "play"
    | "install-steam"
    | "configure-wine"
    | "view-offer"
    | "unavailable";
}

export type AppRoute =
  | { page: "library" }
  | {
      page: "store";
      category: StoreCategory;
      platforms: StorePlatform[];
      query: string;
    }
  | { page: "game"; gameId: GameId; from: "library" | "store" | null }
  | { page: "settings"; section: SettingsSection; attachGameId: GameId | null }
  | { page: "me" }
  | { page: "not-found"; path: string };

export interface PageRestoreState {
  scrollTop: number;
  focusKey: string | null;
  selectedGameId?: GameId;
  query?: string;
  filters?: string[];
}
