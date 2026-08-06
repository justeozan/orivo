import { describe, expect, it } from "vitest";
import { brandIcon, hasBrandIcon, icon } from "./icons";
import {
  CONNECTED_SOURCES,
  defaultSourceAccounts,
  isConnectedSource,
  normaliseSourceAccounts,
  normaliseSourceSyncResult,
  sourceBadge,
  sourceStatusLine,
  sourceSyncSummary,
} from "./source-model";

describe("connected source registry", () => {
  it("names every store the backend can connect, once", () => {
    const providers = CONNECTED_SOURCES.map((source) => source.provider);
    expect(providers).toEqual([
      "epic",
      "gog",
      "ubisoft",
      "xbox",
      "microsoft-store",
      "instant-gaming",
    ]);
    expect(new Set(providers).size).toBe(providers.length);
    expect(providers.every(isConnectedSource)).toBe(true);
    expect(isConnectedSource("steam")).toBe(false);
  });

  it("gives every library source its own badge instead of falling back to local", () => {
    for (const source of CONNECTED_SOURCES) {
      const badge = sourceBadge(source.provider);
      expect(badge?.label).toBeTruthy();
      expect(badge?.icon).toBe(source.icon);
    }
    expect(sourceBadge("steam")?.label).toBe("Steam");
    expect(sourceBadge("wine")?.label).toBe("Windows");
    expect(sourceBadge("local")?.label).toBe("Local");
    // A showcase fixture has no provenance to claim.
    expect(sourceBadge("showcase")).toBeNull();
    expect(sourceBadge(undefined)).toBeNull();
  });
});

describe("brand marks", () => {
  it("keeps the library mark monochrome and gives Settings the brand colours", () => {
    // The library, the hero badge and the detail page inherit `currentColor`,
    // which is what makes a store mark read white over the artwork.
    const library = icon("microsoft");
    expect(library).toContain('fill="currentColor"');
    expect(library).not.toContain("#f25022");

    // Settings presents each store as itself. Microsoft is the case where the
    // colours are the identity, so all four squares must be there.
    const settings = brandIcon("microsoft");
    for (const colour of ["#f25022", "#7fba00", "#00a4ef", "#ffb900"]) {
      expect(settings).toContain(colour);
    }
    expect(brandIcon("xbox")).toContain("#107c10");
    expect(brandIcon("epic")).toContain("#2a2a2a");
  });

  it("falls back to the monochrome mark for a store with no colour variant", () => {
    expect(hasBrandIcon("gog")).toBe(true);
    // A glyph that is not a brand still renders rather than disappearing.
    expect(hasBrandIcon("folder")).toBe(false);
    expect(brandIcon("folder")).toContain("<svg");
  });

  it("has a mark for every connectable store, in both variants", () => {
    for (const source of CONNECTED_SOURCES) {
      expect(icon(source.icon)).toContain("<svg");
      expect(brandIcon(source.icon)).toContain("<svg");
    }
  });
});

describe("normaliseSourceAccounts", () => {
  it("keeps every store visible when the backend only answers for some", () => {
    const statuses = normaliseSourceAccounts([
      {
        provider: "gog",
        label: "GOG",
        description: "Your DRM-free GOG library.",
        connected: true,
        accountLabel: "player-one",
        style: "token",
        sharesSignInWith: [],
        launchable: true,
      },
    ]);

    expect(statuses).toHaveLength(CONNECTED_SOURCES.length);
    const gog = statuses.find((status) => status.provider === "gog");
    expect(gog?.connected).toBe(true);
    expect(gog?.accountLabel).toBe("player-one");
    // Epic was not in the answer, so it stays listed and disconnected rather
    // than vanishing from Settings.
    expect(statuses.find((status) => status.provider === "epic")).toMatchObject({
      connected: false,
      accountLabel: "",
    });
  });

  it("refuses a provider it does not know and a malformed record", () => {
    const statuses = normaliseSourceAccounts([
      { provider: "origin", connected: true },
      { provider: "epic", connected: "yes" },
      "not-a-record",
    ]);

    expect(statuses).toHaveLength(CONNECTED_SOURCES.length);
    expect(statuses.every((status) => status.connected === false)).toBe(true);
    expect(statuses.some((status) => status.provider === ("origin" as never))).toBe(false);
  });

  it("reads snake_case fields and falls back to the known style", () => {
    const statuses = normaliseSourceAccounts([
      {
        provider: "xbox",
        connected: true,
        account_label: "Gamertag",
        style: "nonsense",
        shares_sign_in_with: ["microsoft-store", "origin"],
        launchable: false,
      },
    ]);

    const xbox = statuses.find((status) => status.provider === "xbox");
    expect(xbox?.accountLabel).toBe("Gamertag");
    expect(xbox?.style).toBe("token");
    expect(xbox?.sharesSignInWith).toEqual(["microsoft-store"]);
    expect(xbox?.launchable).toBe(false);
  });

  it("defaults the two stores without an account API to a session sign-in", () => {
    const defaults = defaultSourceAccounts();
    const style = (provider: string): string =>
      defaults.find((status) => status.provider === provider)!.style;

    expect(style("ubisoft")).toBe("session");
    expect(style("instant-gaming")).toBe("session");
    expect(style("epic")).toBe("token");
    expect(normaliseSourceAccounts(undefined)).toEqual(defaults);
  });
});

describe("normaliseSourceSyncResult", () => {
  it("reads a sync result and refuses one from an unknown store", () => {
    expect(
      normaliseSourceSyncResult({
        provider: "epic",
        label: "Epic Games",
        totalGames: 42,
        importedGames: 40,
        updatedGames: 2,
        skippedGames: 0,
      }),
    ).toEqual({
      provider: "epic",
      label: "Epic Games",
      totalGames: 42,
      importedGames: 40,
      updatedGames: 2,
      skippedGames: 0,
    });
    expect(normaliseSourceSyncResult({ provider: "origin" })).toBeNull();
    expect(normaliseSourceSyncResult(null)).toBeNull();
  });

  it("clamps counts a backend could never mean", () => {
    const result = normaliseSourceSyncResult({
      provider: "gog",
      totalGames: -3,
      importedGames: 2.7,
      updatedGames: "many",
    });

    expect(result).toMatchObject({
      label: "GOG",
      totalGames: 0,
      importedGames: 2,
      updatedGames: 0,
      skippedGames: 0,
    });
  });
});

describe("sourceSyncSummary", () => {
  const result = {
    provider: "epic" as const,
    label: "Epic Games",
    totalGames: 0,
    importedGames: 0,
    updatedGames: 0,
    skippedGames: 0,
  };

  it("says what a sync actually did", () => {
    expect(sourceSyncSummary({ ...result, totalGames: 3, importedGames: 3 })).toBe(
      "Epic Games: 3 games added",
    );
    expect(sourceSyncSummary({ ...result, totalGames: 1, importedGames: 1 })).toBe(
      "Epic Games: 1 game added",
    );
    expect(
      sourceSyncSummary({ ...result, totalGames: 5, importedGames: 2, updatedGames: 3 }),
    ).toBe("Epic Games: 2 games added · 3 games refreshed");
  });

  it("never rounds a partial import up to a clean one", () => {
    expect(
      sourceSyncSummary({ ...result, totalGames: 4, importedGames: 4, skippedGames: 2 }),
    ).toBe("Epic Games: 4 games added · 2 games could not be read");
  });

  it("distinguishes an empty library from one that had nothing new", () => {
    expect(sourceSyncSummary(result)).toBe("Epic Games: no games found");
    expect(sourceSyncSummary({ ...result, totalGames: 12 })).toBe("Epic Games: nothing new");
  });
});

describe("sourceStatusLine", () => {
  it("names the account when there is one and explains a session sign-in", () => {
    const [epic, , ubisoft] = defaultSourceAccounts();

    expect(sourceStatusLine({ ...epic, connected: true, accountLabel: "player-one" })).toBe(
      "Connected as player-one",
    );
    expect(sourceStatusLine({ ...epic, connected: true, accountLabel: "  " })).toBe("Connected");
    expect(sourceStatusLine(epic)).toBe("Not connected");
    expect(sourceStatusLine(ubisoft)).toBe("Signs in through its own window");
  });
});
