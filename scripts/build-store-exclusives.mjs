#!/usr/bin/env node
/**
 * Turns the hand-checked list of Store exclusives into the rows both sides of
 * the app read:
 *
 *   node scripts/build-store-exclusives.mjs      (or `pnpm store:exclusives`)
 *
 * Input, committed: `scripts/store-exclusives.json` (the games, their sources
 * and their prices) and the artwork under `public/media/store/<slug>/`, which
 * `scripts/fetch-store-exclusives-media.mjs` downloads from the URLs that same
 * file records. Nothing is fetched here, so this runs offline, on any machine,
 * and writes the same bytes every time.
 *
 * Output:
 *   src/generated/store-exclusives.ts           `STORE_EXCLUSIVES` for the WebView
 *   src-tauri/resources/store-exclusives.json    the same rows, for `store.rs`
 *   src-tauri/resources/store-exclusive-pages.json
 *                                                offer id → the store page, for
 *                                                the host alone: the WebView
 *                                                names an offer, never a URL
 *
 * The rule a game has to meet is in `docs/store-exclusives.md`. A row that
 * breaks the file's own contract (no source, an invented-looking price, a
 * missing picture) stops the build instead of reaching a shelf.
 */
import { access, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DATA = resolve(ROOT, "scripts/store-exclusives.json");
const MEDIA_ROOT = resolve(ROOT, "public/media/store");
const OUT = resolve(ROOT, "src/generated/store-exclusives.ts");
const OUT_JSON = resolve(ROOT, "src-tauri/resources/store-exclusives.json");
const OUT_PAGES = resolve(ROOT, "src-tauri/resources/store-exclusive-pages.json");

/** The shop each brand's exclusives are sold through: its own. */
const BRAND_PROVIDER = {
  playstation: "playstation",
  xbox: "microsoft",
  nintendo: "nintendo",
  steam: "steam",
  epic: "epic",
};

/** Mirrors `PROVIDER_LABEL` in build-store-catalog.mjs. */
const PROVIDER_LABEL = {
  playstation: "PlayStation Store",
  microsoft: "Microsoft Store",
  nintendo: "Nintendo eShop",
  steam: "Steam",
  epic: "Epic Games Store",
};

/**
 * Where a brand's store pages live. A page outside its brand's host is a typo
 * or a copy-paste from another row, and the host would refuse to open it
 * anyway (`ALLOWED_OFFER_HOSTS` in store.rs).
 */
const BRAND_PAGE_HOST = {
  playstation: "store.playstation.com",
  xbox: "apps.microsoft.com",
  nintendo: "www.nintendo.com",
  steam: "store.steampowered.com",
  epic: "store.epicgames.com",
};

/** In the order `store.rs` sorts them, so both sides list them alike. */
const STORE_PLATFORMS = ["pc", "playstation", "xbox", "switch"];
/** The machine a brand's own store sells for. */
const BRAND_PLATFORM = { playstation: "playstation", xbox: "xbox", nintendo: "switch", steam: "pc", epic: "pc" };
const DESKTOP_PLATFORMS = new Set(["windows", "macos", "linux"]);
const SLUG = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const DAY = /^\d{4}-\d{2}-\d{2}$/;
const INSTANT = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z$/;
const CURRENCY = /^[A-Z]{3}$/;

const exists = async (path) => {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
};

function fail(slug, message) {
  throw new Error(`store-exclusives.json: ${slug}: ${message}`);
}

function httpsUrl(slug, value, what) {
  let url;
  try {
    url = new URL(value);
  } catch {
    fail(slug, `${what} is not a URL: ${value}`);
  }
  if (url.protocol !== "https:") fail(slug, `${what} is not https: ${value}`);
  return url;
}

/** The store's own words run long; cut on a sentence, never mid-word. */
function trimDescription(value, limit = 220) {
  const text = String(value ?? "").replace(/\s+/g, " ").trim();
  if (text.length <= limit) return text;
  const window = text.slice(0, limit);
  const sentence = Math.max(window.lastIndexOf(". "), window.lastIndexOf("! "), window.lastIndexOf("? "));
  if (sentence > limit * 0.55) return window.slice(0, sentence + 1);
  return `${window.slice(0, window.lastIndexOf(" "))}…`;
}

async function mediaUrl(slug, file) {
  if (!(await exists(resolve(MEDIA_ROOT, slug, file)))) {
    fail(slug, `public/media/store/${slug}/${file} is missing (run scripts/fetch-store-exclusives-media.mjs)`);
  }
  return `/media/store/${slug}/${file}`;
}

function gameId(row) {
  // A Steam exclusive keeps its Steam id: it is the same game the Steam refresh
  // prices and the same id a Steam library import carries, so ownership and
  // live prices both line up. Anything else is addressed by its slug.
  return row.brand === "steam" ? `steam:${row.steamAppId}` : `store:${row.slug}`;
}

function validate(row, seen) {
  const slug = row?.slug;
  if (typeof slug !== "string" || !SLUG.test(slug)) fail(String(slug), "slug must be kebab-case");
  if (seen.has(slug)) fail(slug, "appears twice");
  seen.add(slug);
  if (typeof row.title !== "string" || !row.title.trim()) fail(slug, "no title");
  if (!(row.brand in BRAND_PROVIDER)) fail(slug, `unknown brand ${row.brand}`);
  if (row.brand === "steam" && !Number.isInteger(row.steamAppId)) fail(slug, "a Steam exclusive needs its steamAppId");
  if (!Array.isArray(row.sources) || row.sources.length === 0) fail(slug, "an exclusive needs a source");
  for (const source of row.sources) httpsUrl(slug, source, "source");
  if (typeof row.exclusivity !== "string" || !row.exclusivity.trim()) fail(slug, "say why it is exclusive");
  if (!DAY.test(row.verifiedAt ?? "")) fail(slug, "verifiedAt must be YYYY-MM-DD");
  const page = httpsUrl(slug, row.storeUrl, "storeUrl");
  if (page.host !== BRAND_PAGE_HOST[row.brand]) fail(slug, `storeUrl is not on ${BRAND_PAGE_HOST[row.brand]}`);
  if (typeof row.shortDescription !== "string" || !row.shortDescription.trim()) fail(slug, "no description");
  if (!Array.isArray(row.genres) || row.genres.length === 0) fail(slug, "no genres");
  if (!Array.isArray(row.platforms) || !row.platforms.every((platform) => STORE_PLATFORMS.includes(platform))) {
    fail(slug, "platforms must be Store platforms (pc, playstation, xbox, switch)");
  }
  if (!row.platforms.includes(BRAND_PLATFORM[row.brand])) fail(slug, `a ${row.brand} game is sold on ${BRAND_PLATFORM[row.brand]}`);
  if (!(row.supportedPlatforms ?? []).every((platform) => DESKTOP_PLATFORMS.has(platform))) {
    fail(slug, "supportedPlatforms are desktop builds (windows, macos, linux)");
  }
  if ((row.supportedPlatforms ?? []).length > 0 && !row.platforms.includes("pc")) {
    fail(slug, "a game with a desktop build is sold on pc");
  }
  if (row.price !== null) {
    const price = row.price ?? {};
    if (!Number.isInteger(price.minor) || price.minor < 0) fail(slug, "price.minor must be a whole number of cents");
    if (!CURRENCY.test(price.currency ?? "")) fail(slug, "price.currency must be ISO 4217");
    if (!INSTANT.test(price.verifiedAt ?? "")) fail(slug, "price.verifiedAt must be YYYY-MM-DDTHH:MM:SSZ");
    httpsUrl(slug, price.source, "price.source");
  }
  if (row.criticScore !== null) {
    const critic = row.criticScore ?? {};
    if (!Number.isInteger(critic.score) || critic.score < 0 || critic.score > 100) {
      fail(slug, "criticScore.score must be 0-100");
    }
    httpsUrl(slug, critic.source, "criticScore.source");
  }
  if (!row.media?.capsule) fail(slug, "no capsule: the card needs the art with the wordmark");
  if (!Array.isArray(row.media?.scenes) || row.media.scenes.length === 0) fail(slug, "no screenshots");
}

async function toSummary(row) {
  const id = gameId(row);
  const provider = BRAND_PROVIDER[row.brand];
  const landscapeUrl = await mediaUrl(row.slug, "capsule.jpg");
  const gallery = [];
  for (const [index] of row.media.scenes.entries()) {
    gallery.push(await mediaUrl(row.slug, `scene-${index + 1}.jpg`));
  }
  // A store that publishes no wide art without the wordmark gets its first
  // screenshot as the hero, rather than the capsule printing the title twice
  // under the page's own. No portrait art: the card's capsule stands in.
  const heroUrl = row.media.hero ? await mediaUrl(row.slug, "hero.jpg") : gallery[0];
  const coverUrl = row.media.cover ? await mediaUrl(row.slug, "cover.jpg") : landscapeUrl;
  const price = row.price;
  const offer = {
    id: `offer_${provider}_${id.replace(/[^a-z0-9]+/gi, "")}`,
    gameId: id,
    provider,
    providerLabel: PROVIDER_LABEL[provider],
    // Copied from the store's own page, never converted or guessed: a store
    // that would not quote one leaves it null.
    priceMinor: price ? price.minor : null,
    currency: price ? price.currency : null,
    region: "FR",
    verifiedAt: price ? price.verifiedAt : null,
    availability: "available",
    stale: price === null,
  };
  const summary = {
    id,
    title: row.title,
    source: "store",
    shortDescription: trimDescription(row.shortDescription),
    coverUrl,
    heroUrl,
    landscapeUrl,
    genres: row.genres.slice(0, 3),
    tags: [],
    supportedPlatforms: row.supportedPlatforms ?? [],
    availablePlatforms: STORE_PLATFORMS.filter((platform) => row.platforms.includes(platform)),
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: [],
    offers: [offer],
    exclusiveTo: row.brand,
    gallery,
  };
  if (row.criticScore) summary.criticScore = row.criticScore.score;
  return { summary, page: [offer.id, row.storeUrl] };
}

async function main() {
  const data = JSON.parse(await readFile(DATA, "utf8"));
  const rows = data.games ?? [];
  const seen = new Set();
  for (const row of rows) validate(row, seen);

  const built = [];
  for (const row of rows) built.push(await toSummary(row));
  const ids = new Set();
  for (const { summary } of built) {
    if (ids.has(summary.id)) fail(summary.id, "two rows share an id");
    ids.add(summary.id);
  }

  const games = built.map(({ summary }) => summary);
  const pages = Object.fromEntries(built.map(({ page }) => page));

  const source = `// GENERATED FILE — DO NOT EDIT. Run \`pnpm store:exclusives\` after editing scripts/store-exclusives.json.
// One row per game sold on a single brand's store, by the rule in
// docs/store-exclusives.md. Every row is sourced in that JSON file; prices are
// the brand's own French store price on the day it was checked, or null.
import type { GameSummary } from "../contracts";

export const STORE_EXCLUSIVES: GameSummary[] = ${JSON.stringify(games, null, 2)};
`;

  await writeFile(OUT, source);
  // The Rust shell embeds this sibling, so the desktop Store and the WebView
  // read the very same rows.
  await writeFile(OUT_JSON, `${JSON.stringify(games, null, 2)}\n`);
  await writeFile(OUT_PAGES, `${JSON.stringify(pages, null, 2)}\n`);

  const perBrand = Object.keys(BRAND_PROVIDER)
    .map((brand) => `${brand} ${games.filter((game) => game.exclusiveTo === brand).length}`)
    .join(", ");
  console.log(`wrote ${games.length} exclusives (${perBrand}) → ${OUT}`);
  console.log(`wrote ${games.length} exclusives → ${OUT_JSON}`);
  console.log(`wrote ${Object.keys(pages).length} store pages → ${OUT_PAGES}`);
}

await main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
