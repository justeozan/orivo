import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import type { StoreBrand, StoreProvider } from "./contracts";
import { STORE_CATALOG } from "./generated/store-catalog";
import { STORE_EXCLUSIVES } from "./generated/store-exclusives";
import { BUNDLED_STORE_GAMES, bundledExclusives } from "./store-catalog-all";
import { STORE_BRANDS } from "./store-tabs";

// Not `new URL(…, import.meta.url)`: Vite rewrites that pattern into an asset
// lookup (see generated-files.test.ts).
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const readJson = (path: string) => JSON.parse(readFileSync(join(ROOT, path), "utf8"));

const PROVIDER: Record<StoreBrand, StoreProvider> = {
  playstation: "playstation",
  xbox: "microsoft",
  nintendo: "nintendo",
  steam: "steam",
  epic: "epic",
};

const PAGE_HOST: Record<StoreBrand, string> = {
  playstation: "store.playstation.com",
  xbox: "apps.microsoft.com",
  nintendo: "www.nintendo.com",
  steam: "store.steampowered.com",
  epic: "store.epicgames.com",
};

interface SourceRow {
  slug: string;
  title: string;
  brand: StoreBrand;
  steamAppId?: number;
  sources: string[];
  price: { minor: number; currency: string; verifiedAt: string } | null;
  criticScore: { score: number; source: string } | null;
}

describe("bundled Store games", () => {
  // `store.rs` embeds the JSON; the WebView bundles the TypeScript. Both come
  // from one generator run, and the catalogue next door drifted once already.
  it("give the WebView and the Rust host the same exclusives", () => {
    expect(STORE_EXCLUSIVES).toEqual(readJson("src-tauri/resources/store-exclusives.json"));
  });

  it("are generated from the sourced list as it stands", () => {
    const rows: SourceRow[] = readJson("scripts/store-exclusives.json").games;
    expect(STORE_EXCLUSIVES.map((game) => game.id)).toEqual(
      rows.map((row) => (row.brand === "steam" ? `steam:${row.steamAppId}` : `store:${row.slug}`)),
    );
    for (const [index, row] of rows.entries()) {
      const game = STORE_EXCLUSIVES[index];
      expect(row.sources.length, row.slug).toBeGreaterThan(0);
      expect(game.title).toBe(row.title);
      expect(game.exclusiveTo).toBe(row.brand);
      expect(game.criticScore).toBe(row.criticScore?.score);
      expect(game.offers[0].priceMinor).toBe(row.price?.minor ?? null);
      expect(game.offers[0].currency).toBe(row.price?.currency ?? null);
      expect(game.offers[0].verifiedAt).toBe(row.price?.verifiedAt ?? null);
    }
  });

  it("fill every brand tab: three recommended and three ranked cards", () => {
    for (const brand of STORE_BRANDS) {
      expect(bundledExclusives(brand).length, brand).toBeGreaterThanOrEqual(6);
      expect(bundledExclusives(brand).every((game) => game.exclusiveTo === brand)).toBe(true);
    }
  });

  it("are each sold through their own brand's shop, whose page the host holds", () => {
    const pages: Record<string, string> = readJson("src-tauri/resources/store-exclusive-pages.json");
    for (const game of STORE_EXCLUSIVES) {
      const brand = game.exclusiveTo as StoreBrand;
      expect(game.offers, game.id).toHaveLength(1);
      expect(game.offers[0].provider, game.id).toBe(PROVIDER[brand]);
      const page = new URL(pages[game.offers[0].id]);
      expect(page.protocol).toBe("https:");
      expect(page.host, game.id).toBe(PAGE_HOST[brand]);
    }
    // The pages are the host's: no URL is bundled into the WebView's rows.
    expect(JSON.stringify(STORE_EXCLUSIVES)).not.toMatch(/https?:/);
  });

  it("only ever show artwork bundled under public/", () => {
    for (const game of STORE_EXCLUSIVES) {
      expect(game.gallery?.length, game.id).toBeGreaterThan(0);
      for (const url of [game.coverUrl, game.heroUrl, game.landscapeUrl, ...(game.gallery ?? [])]) {
        expect(url, game.id).toMatch(/^\/media\/store\/[a-z0-9-]+\/[a-z0-9-]+\.jpg$/);
        expect(existsSync(join(ROOT, "public", url)), url).toBe(true);
      }
      // The card shows the capsule, which is the art that carries the wordmark.
      expect(game.landscapeUrl).toMatch(/\/capsule\.jpg$/);
    }
  });

  it("put the exclusives first, then the catalogue, with no game twice", () => {
    expect(BUNDLED_STORE_GAMES).toEqual([...STORE_EXCLUSIVES, ...STORE_CATALOG]);
    const ids = BUNDLED_STORE_GAMES.map((game) => game.id);
    expect(new Set(ids).size).toBe(ids.length);
    // Exclusivity is a sourced fact: the Steam-built catalogue never claims it.
    expect(STORE_CATALOG.some((game) => game.exclusiveTo !== undefined)).toBe(false);
  });
});
