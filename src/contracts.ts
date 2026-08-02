export type GameId = string;

export type GameSource = "steam" | "wine" | "local" | "showcase" | "store";
export type StoreProvider =
  | "steam"
  | "ubisoft"
  | "microsoft"
  | "apple"
  | "google-play"
  | "instant-gaming";

export type ProviderHealth = "available" | "degraded" | "unavailable" | "not-configured";
export type StoreCategory =
  | "for-you"
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

/** One result in a wallpaper search, addressable only by its opaque id. */
export interface WallpaperCandidateView {
  id: string;
  title: string;
  thumbnailUrl: string;
}

export interface WallpaperSearchView {
  phase: WallpaperSearchPhase;
  source: WallpaperSource;
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
      providers: StoreProvider[];
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
