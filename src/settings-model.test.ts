import { describe, expect, it } from "vitest";
import {
  DEFAULT_PREFERENCES,
  EMPTY_WALLPAPER_CREDENTIALS,
  applyPreferencesUpdate,
  formatDataSize,
  formatFreshness,
  hasWallpaperKey,
  normaliseDataUsage,
  normalisePreferences,
  normaliseProviderStatuses,
  normaliseWallpaperCredentials,
} from "./settings-model";

describe("settings model", () => {
  it("accepts only closed preference values", () => {
    expect(
      normalisePreferences({
        startPage: "store",
        storeRegion: "fr",
        motion: "reduced",
        showShowcaseGames: true,
        debugSampleSocial: true,
      }),
    ).toEqual({
      startPage: "store",
      storeRegion: "fr",
      motion: "reduced",
      showShowcaseGames: true,
      debugSampleSocial: true,
      // Absent from the record and so false: a beta surface is opt-in.
      betaFeatures: false,
    });
    expect(normalisePreferences({ startPage: "downloads", storeRegion: "zz", motion: "fast" })).toEqual(
      DEFAULT_PREFERENCES,
    );
  });

  it("resets preferences without carrying unrelated update fields", () => {
    const current = {
      startPage: "store",
      storeRegion: "jp",
      motion: "reduced",
      showShowcaseGames: true,
      debugSampleSocial: true,
      betaFeatures: true,
    } as const;
    expect(applyPreferencesUpdate(current, { reset: true, storeRegion: "de" })).toEqual(
      DEFAULT_PREFERENCES,
    );
  });

  it("normalises data usage without exposing paths", () => {
    expect(
      normaliseDataUsage({
        derivedCacheBytes: 2_048,
        derivedCacheEntries: 4,
        refreshedAt: "2026-08-01T08:00:00Z",
        path: "/private/cache",
      }),
    ).toEqual({
      derivedCacheBytes: 2_048,
      derivedCacheEntries: 4,
      refreshedAt: "2026-08-01T08:00:00Z",
    });
    expect(formatDataSize(2_048)).toBe("2.0 KB");
    expect(formatFreshness("2026-08-01T08:00:00Z", Date.parse("2026-08-01T08:05:00Z"))).toBe(
      "Refreshed 5 min ago",
    );
  });

  it("normalises wallpaper credentials and strips non-string values", () => {
    expect(
      normaliseWallpaperCredentials({
        igdbClientId: "twitch-client",
        igdb_client_secret: "secret",
        googleApiKey: 42,
        googleCseId: "",
        searchTermCover: '"{name}" cover',
        search_term_logo: 7,
        path: "/private/credentials",
      }),
    ).toEqual({
      ...EMPTY_WALLPAPER_CREDENTIALS,
      igdbClientId: "twitch-client",
      igdbClientSecret: "secret",
      // A term the user saved survives; a non-string one falls back to empty,
      // which means "use Orivo's default term" rather than "search for nothing".
      searchTermCover: '"{name}" cover',
      searchTermLogo: "",
    });
    expect(normaliseWallpaperCredentials(null)).toEqual(EMPTY_WALLPAPER_CREDENTIALS);
  });

  it("deduplicates and validates provider status records", () => {
    expect(
      normaliseProviderStatuses([
        { provider: "steam", label: "Steam", health: "available", message: "Ready" },
        { provider: "steam", label: "Steam", health: "degraded", message: "Later value" },
        { provider: "unknown", health: "available" },
      ]),
    ).toEqual([
      {
        provider: "steam",
        label: "Steam",
        health: "degraded",
        message: "Later value",
        refreshedAt: null,
      },
    ]);
  });

  it("counts a wallpaper key only when a key is actually set", () => {
    expect(hasWallpaperKey(EMPTY_WALLPAPER_CREDENTIALS)).toBe(false);
    // A customised search term is tuning, not access: treating it as a key
    // would retire the offer of help before any help was given.
    expect(
      hasWallpaperKey({ ...EMPTY_WALLPAPER_CREDENTIALS, searchTermCover: '"{name}" cover' }),
    ).toBe(false);
    expect(hasWallpaperKey({ ...EMPTY_WALLPAPER_CREDENTIALS, steamgriddbApiKey: "  " })).toBe(
      false,
    );
    expect(hasWallpaperKey({ ...EMPTY_WALLPAPER_CREDENTIALS, steamgriddbApiKey: "key" })).toBe(
      true,
    );
    expect(hasWallpaperKey({ ...EMPTY_WALLPAPER_CREDENTIALS, igdbClientId: "id" })).toBe(true);
  });
});
