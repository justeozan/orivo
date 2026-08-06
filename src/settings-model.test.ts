import { describe, expect, it } from "vitest";
import {
  DEFAULT_PREFERENCES,
  EMPTY_WALLPAPER_CREDENTIALS,
  applyPreferencesUpdate,
  formatDataSize,
  formatFreshness,
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
        path: "/private/credentials",
      }),
    ).toEqual({
      igdbClientId: "twitch-client",
      igdbClientSecret: "secret",
      googleApiKey: "",
      googleCseId: "",
      steamgriddbApiKey: "",
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
});
