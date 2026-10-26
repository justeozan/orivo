import type {
  ProviderHealth,
  ProviderStatus,
  SettingsSection,
  StoreProvider,
  WallpaperCredentials,
} from "./contracts";
import type { IconName } from "./icons";
import { LANGUAGE_PREFERENCES, t, type LanguagePreference } from "./i18n";

export const START_PAGES = ["library", "store"] as const;
export const STORE_REGIONS = ["automatic", "us", "ca", "gb", "fr", "de", "jp", "au"] as const;
export const MOTION_PREFERENCES = ["full", "system", "reduced"] as const;
/**
 * What Orivo does once a game it started is on screen: get out of its way —
 * hidden on macOS, minimised on Windows and Linux — or stay behind it.
 */
export const GAME_START_ACTIONS = ["step-aside", "stay"] as const;

export type StartPage = (typeof START_PAGES)[number];
export type StoreRegion = (typeof STORE_REGIONS)[number];
export type MotionPreference = (typeof MOTION_PREFERENCES)[number];
export type GameStartAction = (typeof GAME_START_ACTIONS)[number];

export interface Preferences {
  startPage: StartPage;
  /** "system" follows the OS; see i18n.ts. */
  language: LanguagePreference;
  storeRegion: StoreRegion;
  motion: MotionPreference;
  onGameStart: GameStartAction;
  /** Debug-only: seed the library with the bundled demo (showcase) games. */
  showShowcaseGames: boolean;
  /** Debug-only: fill detail pages with sample achievements, friends and activity. */
  debugSampleSocial: boolean;
  /** Surfaces that are not finished yet. Off by default; the Me page is one. */
  betaFeatures: boolean;
  /**
   * Debug-only: keep the games Orivo wrote by hand on the Store's shelf once a
   * storefront's own new releases are there to fill it. Off by default — they
   * stood in for a catalogue that did not exist yet, and they crowd out what
   * has actually just come out.
   */
  showEditorialGames: boolean;
  /** Opt-out: a game page plays its first trailer on its own unless set. */
  trailerAutoplayOff: boolean;
  /** Opt-out: a trailer that starts by itself plays with sound unless set. */
  trailerMuted: boolean;
}

export interface PreferencesUpdate {
  startPage?: StartPage;
  language?: LanguagePreference;
  storeRegion?: StoreRegion;
  motion?: MotionPreference;
  onGameStart?: GameStartAction;
  showShowcaseGames?: boolean;
  debugSampleSocial?: boolean;
  betaFeatures?: boolean;
  showEditorialGames?: boolean;
  trailerAutoplayOff?: boolean;
  trailerMuted?: boolean;
  reset?: boolean;
}

export interface DataUsage {
  derivedCacheBytes: number;
  derivedCacheEntries: number;
  refreshedAt: string | null;
}

export const DEFAULT_PREFERENCES: Readonly<Preferences> = Object.freeze({
  startPage: "library",
  language: "system",
  storeRegion: "automatic",
  motion: "system",
  onGameStart: "step-aside",
  showShowcaseGames: false,
  debugSampleSocial: false,
  betaFeatures: false,
  showEditorialGames: false,
  trailerAutoplayOff: false,
  trailerMuted: false,
});

export const SETTINGS_SECTIONS: ReadonlyArray<{
  id: SettingsSection;
  /** The page title: the full name of the section. */
  label: string;
  /** The sidebar tab: one word, next to the section's glyph. */
  tab: string;
  icon: IconName;
  description: string;
}> = [
  // Read through t() at import: the language is settled before this module
  // loads (i18n.ts reads it synchronously), and a change reloads the page.
  ...(
    [
      ["general", "cog"],
      ["libraries", "folder"],
      ["plugins", "puzzle"],
      ["appearance", "palette"],
      ["data", "database"],
      ["about", "info"],
    ] as const
  ).map(([id, icon]) => ({
    id,
    label: t(`settings.${id}.label`),
    tab: t(`settings.${id}.tab`),
    icon,
    description: t(`settings.${id}.description`),
  })),
] as const;

const providerLabels: Record<StoreProvider, string> = {
  steam: "Steam",
  "instant-gaming": "Instant Gaming",
  epic: "Epic Games Store",
  gog: "GOG",
  humble: "Humble Store",
  fanatical: "Fanatical",
  "green-man-gaming": "Green Man Gaming",
  ubisoft: "Ubisoft",
  microsoft: "Microsoft / Xbox",
  playstation: "PlayStation Store",
  nintendo: "Nintendo eShop",
  apple: "Apple App Store",
  "google-play": "Google Play",
};

const providers = Object.keys(providerLabels) as StoreProvider[];
const providerHealth = new Set<ProviderHealth>([
  "available",
  "degraded",
  "unavailable",
  "not-configured",
]);

export function normalisePreferences(value: unknown): Preferences {
  const record = asRecord(value);
  return {
    startPage: readEnum(record, ["startPage", "start_page"], START_PAGES, DEFAULT_PREFERENCES.startPage),
    language: readEnum(record, ["language"], LANGUAGE_PREFERENCES, DEFAULT_PREFERENCES.language),
    storeRegion: readEnum(
      record,
      ["storeRegion", "store_region"],
      STORE_REGIONS,
      DEFAULT_PREFERENCES.storeRegion,
    ),
    motion: readEnum(record, ["motion"], MOTION_PREFERENCES, DEFAULT_PREFERENCES.motion),
    onGameStart: readEnum(
      record,
      ["onGameStart", "on_game_start"],
      GAME_START_ACTIONS,
      DEFAULT_PREFERENCES.onGameStart,
    ),
    showShowcaseGames: record.showShowcaseGames === true || record.show_showcase_games === true,
    debugSampleSocial: record.debugSampleSocial === true || record.debug_sample_social === true,
    betaFeatures: record.betaFeatures === true || record.beta_features === true,
    showEditorialGames:
      record.showEditorialGames === true || record.show_editorial_games === true,
    trailerAutoplayOff:
      record.trailerAutoplayOff === true || record.trailer_autoplay_off === true,
    trailerMuted: record.trailerMuted === true || record.trailer_muted === true,
  };
}

export function applyPreferencesUpdate(
  current: Preferences,
  update: PreferencesUpdate,
): Preferences {
  if (update.reset) return { ...DEFAULT_PREFERENCES };
  return normalisePreferences({ ...current, ...update });
}

export const EMPTY_WALLPAPER_CREDENTIALS: WallpaperCredentials = {
  igdbClientId: "",
  igdbClientSecret: "",
  googleApiKey: "",
  googleCseId: "",
  steamgriddbApiKey: "",
  searchTermCover: "",
  searchTermLandscape: "",
  searchTermBackground: "",
  searchTermLogo: "",
};

export function normaliseWallpaperCredentials(value: unknown): WallpaperCredentials {
  const record = asRecord(value);
  return {
    igdbClientId: readString(record.igdbClientId ?? record.igdb_client_id),
    igdbClientSecret: readString(record.igdbClientSecret ?? record.igdb_client_secret),
    googleApiKey: readString(record.googleApiKey ?? record.google_api_key),
    googleCseId: readString(record.googleCseId ?? record.google_cse_id),
    steamgriddbApiKey: readString(record.steamgriddbApiKey ?? record.steamgriddb_api_key),
    // Empty means "use Orivo's default term", so a missing value is not a
    // missing feature — it is the tuned query the search already ships with.
    searchTermCover: readString(record.searchTermCover ?? record.search_term_cover),
    searchTermLandscape: readString(
      record.searchTermLandscape ?? record.search_term_landscape,
    ),
    searchTermBackground: readString(
      record.searchTermBackground ?? record.search_term_background,
    ),
    searchTermLogo: readString(record.searchTermLogo ?? record.search_term_logo),
  };
}

/**
 * Whether any artwork source has been given credentials.
 *
 * Only the keys count. The four search terms below them are tuning for the
 * keyword sources, not access to anything — treating a customised term as a
 * configured key is how the app would stop offering help it has not given yet.
 */
export function hasWallpaperKey(credentials: WallpaperCredentials): boolean {
  return [
    credentials.igdbClientId,
    credentials.igdbClientSecret,
    credentials.googleApiKey,
    credentials.googleCseId,
    credentials.steamgriddbApiKey,
  ].some((value) => value.trim() !== "");
}

export function normaliseDataUsage(value: unknown): DataUsage {
  const record = asRecord(value);
  const refreshedAt = record.refreshedAt ?? record.refreshed_at;
  const refreshedAtEpochMs = record.refreshedAtEpochMs ?? record.refreshed_at_epoch_ms;
  return {
    derivedCacheBytes: nonNegativeNumber(record.derivedCacheBytes ?? record.derived_cache_bytes),
    derivedCacheEntries: nonNegativeNumber(
      record.derivedCacheEntries ?? record.derived_cache_entries,
    ),
    refreshedAt:
      nullableString(refreshedAt) ??
      (typeof refreshedAtEpochMs === "number" && Number.isFinite(refreshedAtEpochMs)
        ? new Date(refreshedAtEpochMs).toISOString()
        : null),
  };
}

export function normaliseProviderStatuses(value: unknown): ProviderStatus[] {
  const record = asRecord(value);
  const candidates = Array.isArray(value)
    ? value
    : Array.isArray(record.providerStatuses)
      ? record.providerStatuses
      : Array.isArray(record.provider_statuses)
        ? record.provider_statuses
        : [];
  const result = new Map<StoreProvider, ProviderStatus>();
  for (const candidate of candidates) {
    const status = asRecord(candidate);
    const provider = readString(status.provider) as StoreProvider;
    const health = readString(status.health) as ProviderHealth;
    if (!providers.includes(provider) || !providerHealth.has(health)) continue;
    result.set(provider, {
      provider,
      label: readString(status.label) || providerLabels[provider],
      health,
      message: readString(status.message),
      refreshedAt: nullableString(status.refreshedAt ?? status.refreshed_at),
    });
  }
  return [...result.values()];
}

export function defaultProviderStatuses(): ProviderStatus[] {
  return providers.map((provider) => ({
    provider,
    label: providerLabels[provider],
    health: provider === "apple" ? "available" : "not-configured",
    message:
      provider === "apple"
        ? t("misc.provider.apple")
        : provider === "steam"
          ? t("misc.provider.steam")
          : t("misc.provider.other"),
    refreshedAt: null,
  }));
}

/**
 * What a store's price feed is called on screen.
 *
 * `unavailable` and `not-configured` are the same sentence to the person
 * reading the page — Orivo cannot show this store's prices yet — and differ
 * only in why: one has no authorised feed configured, the other has no public
 * feed to configure. Neither is a fault, and neither is anything the reader can
 * do something about, so a red "Unavailable" down half the list read as a row
 * of errors. Both say what is actually true instead: not yet. The feed's own
 * sentence is still one pointer away on the row.
 */
export function providerHealthLabel(health: ProviderHealth): string {
  switch (health) {
    case "available":
      return t("misc.provider.available");
    case "degraded":
      return t("misc.provider.degraded");
    default:
      return t("misc.provider.soon");
  }
}

export function formatDataSize(bytes: number): string {
  const safe = Math.max(0, Number.isFinite(bytes) ? bytes : 0);
  const decimal = (value: number): string => value.toFixed(1).replace(".", t("misc.size.decimal"));
  if (safe < 1_024) return t("misc.size.b", { value: safe.toFixed(0) });
  if (safe < 1_024 ** 2) return t("misc.size.kb", { value: decimal(safe / 1_024) });
  if (safe < 1_024 ** 3) return t("misc.size.mb", { value: decimal(safe / 1_024 ** 2) });
  return t("misc.size.gb", { value: decimal(safe / 1_024 ** 3) });
}

export function formatFreshness(value: string | null, now = Date.now()): string {
  if (!value) return t("misc.freshness.never");
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return t("misc.freshness.invalid");
  const elapsed = Math.max(0, now - timestamp);
  if (elapsed < 60_000) return t("misc.freshness.justNow");
  if (elapsed < 3_600_000) return t("misc.freshness.minutes", { count: Math.floor(elapsed / 60_000) });
  if (elapsed < 86_400_000) return t("misc.freshness.hours", { count: Math.floor(elapsed / 3_600_000) });
  return t("misc.freshness.days", { count: Math.floor(elapsed / 86_400_000) });
}

function asRecord(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
}

function readEnum<const T extends readonly string[]>(
  record: Record<string, unknown>,
  keys: string[],
  values: T,
  fallback: T[number],
): T[number] {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string" && values.includes(value as T[number])) return value as T[number];
  }
  return fallback;
}

function readString(value: unknown): string {
  return typeof value === "string" ? value : "";
}

function nullableString(value: unknown): string | null {
  return typeof value === "string" && value.length > 0 ? value : null;
}

function nonNegativeNumber(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0;
}
