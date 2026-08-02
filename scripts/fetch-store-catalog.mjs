#!/usr/bin/env node
/**
 * Builds the Store's real catalogue.
 *
 * For every curated title it resolves the real Steam app id, reads the French
 * store listing (real EUR price, real genres, real release date, real platform
 * support), asks CheapShark for the other PC storefronts that sell it, and
 * downloads the real artwork next to the app so the page never depends on a
 * remote image at render time.
 *
 *   node scripts/fetch-store-catalog.mjs            # everything
 *   node scripts/fetch-store-catalog.mjs --no-art   # metadata only
 *
 * Output:
 *   public/media/store/<slug>/{hero.jpg,capsule.jpg,cover.jpg,logo.png}
 *   .context/redesign/steam-data.json
 */
import { mkdir, readFile, writeFile, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MEDIA_ROOT = resolve(ROOT, "public/media/store");
const OUT = resolve(ROOT, ".context/redesign/steam-data.json");
const UA = "Orivo/0.3 (+https://orivo.io; contact@oneiby.com)";
const SKIP_ART = process.argv.includes("--no-art");

const sleep = (ms) => new Promise((done) => setTimeout(done, ms));

async function getJson(url, attempt = 0) {
  const response = await fetch(url, { headers: { "user-agent": UA, accept: "application/json" } });
  if (response.status === 429 && attempt < 4) {
    await sleep(4_000 * (attempt + 1));
    return getJson(url, attempt + 1);
  }
  if (!response.ok) throw new Error(`${response.status} ${url}`);
  return response.json();
}

async function download(url, destination) {
  const response = await fetch(url, { headers: { "user-agent": UA } });
  if (!response.ok) return false;
  const body = Buffer.from(await response.arrayBuffer());
  if (body.byteLength < 2_000) return false;
  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, body);
  return true;
}

async function alreadyThere(path) {
  try {
    return (await stat(path)).size > 2_000;
  } catch {
    return false;
  }
}

/** Steam serves several artwork shapes per app; each has a documented fallback. */
const ARTWORK = [
  { file: "hero.jpg", candidates: ["library_hero.jpg", "page_bg_generated_v6b.jpg", "header.jpg"] },
  { file: "capsule.jpg", candidates: ["capsule_616x353.jpg", "header.jpg"] },
  { file: "cover.jpg", candidates: ["library_600x900.jpg", "capsule_616x353.jpg"] },
  { file: "logo.png", candidates: ["logo.png"] },
];

async function fetchArtwork(appId, slug) {
  const saved = {};
  for (const { file, candidates } of ARTWORK) {
    const destination = resolve(MEDIA_ROOT, slug, file);
    if (await alreadyThere(destination)) {
      saved[file] = true;
      continue;
    }
    for (const candidate of candidates) {
      const url = `https://cdn.cloudflare.steamstatic.com/steam/apps/${appId}/${candidate}`;
      if (await download(url, destination)) {
        saved[file] = true;
        break;
      }
    }
    saved[file] ??= false;
  }
  return saved;
}

async function resolveAppId(title) {
  const url = `https://store.steampowered.com/api/storesearch/?term=${encodeURIComponent(title)}&cc=fr&l=french`;
  const payload = await getJson(url);
  const items = payload?.items ?? [];
  if (items.length === 0) return null;
  const wanted = title.toLowerCase();
  const exact = items.find((item) => String(item.name).toLowerCase() === wanted);
  const prefix = items.find((item) => String(item.name).toLowerCase().startsWith(wanted));
  return (exact ?? prefix ?? items[0]).id ?? null;
}

async function fetchDetails(appId) {
  const url = `https://store.steampowered.com/api/appdetails?appids=${appId}&cc=fr&l=french`;
  const payload = await getJson(url);
  const entry = payload?.[String(appId)];
  return entry?.success ? entry.data : null;
}

/** Real prices on the other PC storefronts. CheapShark quotes USD. */
async function fetchDeals(steamAppId) {
  try {
    const url = `https://www.cheapshark.com/api/1.0/deals?steamAppID=${steamAppId}&limit=12`;
    const deals = await getJson(url);
    return Array.isArray(deals) ? deals : [];
  } catch {
    return [];
  }
}

const STORE_NAMES = {
  1: "steam",
  3: "green-man-gaming",
  7: "gog",
  11: "humble",
  13: "ubisoft",
  15: "fanatical",
  25: "epic",
  27: "gamesplanet",
};

function stripHtml(value) {
  return String(value ?? "")
    .replace(/<[^>]*>/g, " ")
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .replace(/&#39;/g, "'")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

async function main() {
  const groups = JSON.parse(await readFile(resolve(ROOT, ".context/redesign/games.json"), "utf8"));
  const wanted = Object.values(groups).flat();
  const results = [];
  const failures = [];

  for (const [index, game] of wanted.entries()) {
    process.stdout.write(`[${index + 1}/${wanted.length}] ${game.title} … `);
    try {
      const appId = await resolveAppId(game.title);
      if (!appId) throw new Error("no app id");
      await sleep(350);
      const details = await fetchDetails(appId);
      if (!details) throw new Error("no store listing");
      const deals = await fetchDeals(appId);
      const artwork = SKIP_ART ? {} : await fetchArtwork(appId, game.slug);

      const price = details.price_overview ?? null;
      results.push({
        slug: game.slug,
        appId,
        title: details.name ?? game.title,
        shortDescription: stripHtml(details.short_description).slice(0, 220),
        developer: details.developers?.[0] ?? null,
        publisher: details.publishers?.[0] ?? null,
        releaseDate: details.release_date?.date ?? null,
        metacritic: details.metacritic?.score ?? null,
        steamGenres: (details.genres ?? []).map((genre) => genre.description),
        steamCategories: (details.categories ?? []).map((category) => category.description),
        supportedPlatforms: [
          details.platforms?.windows ? "windows" : null,
          details.platforms?.mac ? "macos" : null,
          details.platforms?.linux ? "linux" : null,
        ].filter(Boolean),
        isFree: Boolean(details.is_free),
        price: price
          ? {
              currency: price.currency,
              initialMinor: price.initial,
              finalMinor: price.final,
              discountPercent: price.discount_percent,
            }
          : null,
        deals: deals
          .filter((deal) => STORE_NAMES[Number(deal.storeID)])
          .map((deal) => ({
            provider: STORE_NAMES[Number(deal.storeID)],
            salePriceUsd: Number(deal.salePrice),
            normalPriceUsd: Number(deal.normalPrice),
            onSale: deal.isOnSale === "1",
          }))
          .sort((left, right) => left.salePriceUsd - right.salePriceUsd),
        artwork,
      });
      console.log(`ok (app ${appId}${price ? `, ${(price.final / 100).toFixed(2)} ${price.currency}` : ", no price"})`);
    } catch (error) {
      failures.push({ slug: game.slug, title: game.title, error: String(error.message ?? error) });
      console.log(`FAILED — ${error.message ?? error}`);
    }
    await sleep(450);
  }

  await mkdir(dirname(OUT), { recursive: true });
  await writeFile(OUT, `${JSON.stringify({ fetchedAt: new Date().toISOString(), games: results, failures }, null, 2)}\n`);
  console.log(`\n${results.length} resolved, ${failures.length} failed → ${OUT}`);
  if (failures.length) console.log(failures.map((failure) => `  · ${failure.title}: ${failure.error}`).join("\n"));
}

await main();
