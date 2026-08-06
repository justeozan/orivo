import type {
  ProviderHealth,
  ProviderStatus,
  SettingsSection,
  StoreProvider,
  WallpaperCredentials,
} from "./contracts";

export const START_PAGES = ["library", "store"] as const;
export const STORE_REGIONS = ["automatic", "us", "ca", "gb", "fr", "de", "jp", "au"] as const;
export const MOTION_PREFERENCES = ["system", "reduced"] as const;

export type StartPage = (typeof START_PAGES)[number];
export type StoreRegion = (typeof STORE_REGIONS)[number];
export type MotionPreference = (typeof MOTION_PREFERENCES)[number];

export interface Preferences {
  startPage: StartPage;
  storeRegion: StoreRegion;
  motion: MotionPreference;
  /** Debug-only: seed the library with the bundled demo (showcase) games. */
  showShowcaseGames: boolean;
  /** Debug-only: fill detail pages with sample achievements, friends and activity. */
  debugSampleSocial: boolean;
}

export interface PreferencesUpdate {
  startPage?: StartPage;
  storeRegion?: StoreRegion;
  motion?: MotionPreference;
  showShowcaseGames?: boolean;
  debugSampleSocial?: boolean;
  reset?: boolean;
}

export interface DataUsage {
  derivedCacheBytes: number;
  derivedCacheEntries: number;
  refreshedAt: string | null;
}

export const DEFAULT_PREFERENCES: Readonly<Preferences> = Object.freeze({
  startPage: "library",
  storeRegion: "automatic",
  motion: "system",
  showShowcaseGames: false,
  debugSampleSocial: false,
});

export const SETTINGS_SECTIONS: ReadonlyArray<{
  id: SettingsSection;
  label: string;
  description: string;
}> = [
  { id: "general", label: "General", description: "Startup and store defaults" },
  { id: "libraries", label: "Libraries & Sources", description: "Accounts, imports, and providers" },
  { id: "plugins", label: "Plugins & Runners", description: "Wine profiles and plugin health" },
  { id: "appearance", label: "Appearance", description: "Motion preferences" },
  { id: "data", label: "Data", description: "Derived cache usage" },
  { id: "about", label: "About", description: "Versions and attributions" },
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
    storeRegion: readEnum(
      record,
      ["storeRegion", "store_region"],
      STORE_REGIONS,
      DEFAULT_PREFERENCES.storeRegion,
    ),
    motion: readEnum(record, ["motion"], MOTION_PREFERENCES, DEFAULT_PREFERENCES.motion),
    showShowcaseGames: record.showShowcaseGames === true || record.show_showcase_games === true,
    debugSampleSocial: record.debugSampleSocial === true || record.debug_sample_social === true,
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
};

export function normaliseWallpaperCredentials(value: unknown): WallpaperCredentials {
  const record = asRecord(value);
  return {
    igdbClientId: readString(record.igdbClientId ?? record.igdb_client_id),
    igdbClientSecret: readString(record.igdbClientSecret ?? record.igdb_client_secret),
    googleApiKey: readString(record.googleApiKey ?? record.google_api_key),
    googleCseId: readString(record.googleCseId ?? record.google_cse_id),
    steamgriddbApiKey: readString(record.steamgriddbApiKey ?? record.steamgriddb_api_key),
  };
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
        ? "Public search is available."
        : provider === "steam"
          ? "Connect Steam to enable account-backed data."
          : "An authorised provider feed is not configured.",
    refreshedAt: null,
  }));
}

export function formatDataSize(bytes: number): string {
  const safe = Math.max(0, Number.isFinite(bytes) ? bytes : 0);
  if (safe < 1_024) return `${safe.toFixed(0)} B`;
  if (safe < 1_024 ** 2) return `${(safe / 1_024).toFixed(1)} KB`;
  if (safe < 1_024 ** 3) return `${(safe / 1_024 ** 2).toFixed(1)} MB`;
  return `${(safe / 1_024 ** 3).toFixed(1)} GB`;
}

export function formatFreshness(value: string | null, now = Date.now()): string {
  if (!value) return "Not refreshed yet";
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "Refresh time unavailable";
  const elapsed = Math.max(0, now - timestamp);
  if (elapsed < 60_000) return "Refreshed just now";
  if (elapsed < 3_600_000) return `Refreshed ${Math.floor(elapsed / 60_000)} min ago`;
  if (elapsed < 86_400_000) return `Refreshed ${Math.floor(elapsed / 3_600_000)} hr ago`;
  return `Refreshed ${Math.floor(elapsed / 86_400_000)} d ago`;
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
