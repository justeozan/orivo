#!/usr/bin/env node
/**
 * Merges the fetched Steam facts with Orivo's hand-written French curation and
 * emits `src/store-catalog.generated.ts`.
 *
 *   node scripts/fetch-store-catalog.mjs && node scripts/build-store-catalog.mjs
 *
 * Prices are copied through, never invented: a title Steam quoted no price for
 * keeps `priceMinor: null`, and the CheapShark rows keep their own USD currency
 * rather than being converted into euros behind the shopper's back.
 */
import { readdir, readFile, writeFile, access } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DATA = resolve(ROOT, ".context/redesign/steam-data.json");
const CURATED_DIR = resolve(ROOT, ".context/redesign");
const OUT = resolve(ROOT, "src/store-catalog.generated.ts");
// The same catalogue, as plain data, for the Rust shell to embed with
// `include_str!`. One catalogue, written once, read by both sides.
const OUT_JSON = resolve(ROOT, "src-tauri/resources/store-catalog.json");

const exists = async (path) => {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
};

/** Which shop sells on which machine. Kept in one place, mirrored in Rust. */
const PROVIDER_PLATFORM = {
  steam: "pc",
  "instant-gaming": "pc",
  epic: "pc",
  gog: "pc",
  humble: "pc",
  fanatical: "pc",
  "green-man-gaming": "pc",
  ubisoft: "pc",
  microsoft: "xbox",
  playstation: "playstation",
  nintendo: "switch",
};

const PROVIDER_LABEL = {
  steam: "Steam",
  "instant-gaming": "Instant Gaming",
  epic: "Epic Games Store",
  gog: "GOG",
  humble: "Humble Store",
  fanatical: "Fanatical",
  "green-man-gaming": "Green Man Gaming",
  ubisoft: "Ubisoft Store",
  microsoft: "Microsoft Store",
  playstation: "PlayStation Store",
  nintendo: "Nintendo eShop",
  apple: "App Store",
  "google-play": "Google Play",
};

const PLATFORM_PROVIDER = { playstation: "playstation", xbox: "microsoft", switch: "nintendo" };

function offer(gameId, provider, values) {
  return {
    id: `offer_${provider}_${gameId.replace(/[^a-z0-9]+/gi, "")}`,
    gameId,
    provider,
    providerLabel: PROVIDER_LABEL[provider] ?? provider,
    priceMinor: null,
    currency: null,
    region: "FR",
    verifiedAt: null,
    availability: "unknown",
    stale: false,
    ...values,
  };
}

function buildOffers(game, curation, fetchedAt) {
  const gameId = `steam:${game.appId}`;
  const offers = [];

  if (game.price) {
    offers.push(
      offer(gameId, "steam", {
        priceMinor: game.price.finalMinor,
        currency: game.price.currency,
        verifiedAt: fetchedAt,
        availability: "available",
        stale: false,
      }),
    );
  } else if (game.isFree) {
    offers.push(
      offer(gameId, "steam", {
        priceMinor: 0,
        currency: "EUR",
        verifiedAt: fetchedAt,
        availability: "available",
      }),
    );
  } else {
    offers.push(offer(gameId, "steam", { availability: "available", stale: true }));
  }

  // Real cross-shop PC prices. CheapShark quotes US dollars, so they are kept
  // in USD and compared only against other USD rows.
  for (const deal of game.deals ?? []) {
    if (deal.provider === "steam" || !PROVIDER_PLATFORM[deal.provider]) continue;
    if (offers.some((existing) => existing.provider === deal.provider)) continue;
    offers.push(
      offer(gameId, deal.provider, {
        priceMinor: Math.round(deal.salePriceUsd * 100),
        currency: "USD",
        region: "US",
        verifiedAt: fetchedAt,
        availability: "available",
      }),
    );
  }

  // Console availability is a fact Orivo knows; the price on those storefronts
  // is not, so it stays null rather than being guessed.
  for (const platform of curation.platforms ?? []) {
    const provider = PLATFORM_PROVIDER[platform];
    if (!provider || offers.some((existing) => existing.provider === provider)) continue;
    offers.push(offer(gameId, provider, { availability: "available", stale: true }));
  }

  return offers;
}

const CATEGORY_TAG = {
  "good-for-brain": "Bon pour le cerveau",
  "short-sessions": "Courte durée",
  "strong-stories": "Récits forts",
  relaxing: "Relaxant",
};

/**
 * The five cards the approved design opens on are pinned to exactly the copy
 * and strengths it shows, so the shelf at rest is the design.
 */
const DESIGN_EXACT = {
  "planet-of-lana": {
    genresFr: "Aventure, Réflexion",
    duration: "3-4h",
    mode: "Solo",
    tagline: "Histoire émouvante sans violence.",
    stats: [
      { label: "Réflexion", value: 5 },
      { label: "Créativité", value: 4 },
      { label: "Relaxation", value: 4 },
    ],
  },
  firewatch: {
    genresFr: "Aventure, Exploration",
    duration: "4-5h",
    mode: "Solo",
    tagline: "Une histoire humaine et contemplative.",
    stats: [
      { label: "Réflexion", value: 4 },
      { label: "Immersion", value: 4 },
      { label: "Déconnexion", value: 4 },
    ],
  },
  inscryption: {
    genresFr: "Stratégie, Cartes",
    duration: "6-8h",
    mode: "Solo",
    tagline: "Un jeu intelligent qui challenge ton esprit.",
    stats: [
      { label: "Réflexion", value: 4 },
      { label: "Mémoire", value: 4 },
      { label: "Stratégie", value: 4 },
    ],
  },
  dorfromantik: {
    genresFr: "Stratégie, Créatif",
    duration: "2-3h",
    mode: "Solo",
    tagline: "Construis, détends-toi, recommence.",
    stats: [
      { label: "Créativité", value: 4 },
      { label: "Relaxation", value: 4 },
      { label: "Focus", value: 3 },
    ],
  },
  "a-short-hike": {
    genresFr: "Aventure, Exploration",
    duration: "1-2h",
    mode: "Solo",
    tagline: "Petit jeu, grand bol d'air.",
    stats: [
      { label: "Bien-être", value: 3 },
      { label: "Exploration", value: 4 },
      { label: "Sérénité", value: 3 },
    ],
  },
};

/**
 * The shelf shows the name a person would say, not the legal one: trademark
 * marks and edition suffixes are dropped so a card never has to ellipsise
 * ("SPIRITFARER®: ÉDITION FA…").
 */
function displayTitle(value) {
  return String(value ?? "")
    .replace(/[®™©]/g, "")
    .replace(/\s*[:\u2013-]\s*(the\s+)?(final cut|definitive|farewell|complete|deluxe|game of the year|goty|remastered|enhanced|anniversary|panoramic)\b.*$/i, "")
    .replace(/\s*[:\u2013-]\s*(pc\s+)?(édition|edition|les plus beaux paysages)\b.*$/i, "")
    .replace(/\s{2,}/g, " ")
    .trim();
}

/** The curators wrote genres as one phrase; the card renders them as a list. */
function genreList(value) {
  if (Array.isArray(value)) return value.slice(0, 2);
  return String(value ?? "")
    .split(/\s*[,/]\s*/)
    .filter(Boolean)
    .slice(0, 2);
}

/** Steam descriptions run long; cut on a sentence, never mid-word. */
function trimDescription(value, limit = 190) {
  const text = String(value ?? "").trim();
  if (text.length <= limit) return text;
  const window = text.slice(0, limit);
  const sentence = Math.max(window.lastIndexOf(". "), window.lastIndexOf("! "), window.lastIndexOf("? "));
  if (sentence > limit * 0.55) return window.slice(0, sentence + 1);
  return `${window.slice(0, window.lastIndexOf(" "))}…`;
}

async function media(slug, file, fallbacks) {
  for (const candidate of [file, ...fallbacks]) {
    if (await exists(resolve(ROOT, "public/media/store", slug, candidate))) {
      return `/media/store/${slug}/${candidate}`;
    }
  }
  return "";
}

async function main() {
  const { fetchedAt, games } = JSON.parse(await readFile(DATA, "utf8"));
  const curatedFiles = (await readdir(CURATED_DIR)).filter((name) => /^curated-\d+\.json$/.test(name));
  if (curatedFiles.length === 0) throw new Error("no curated-*.json files found");

  const curation = new Map();
  for (const name of curatedFiles) {
    const parsed = JSON.parse(await readFile(resolve(CURATED_DIR, name), "utf8"));
    const rows = Array.isArray(parsed) ? parsed : (parsed.games ?? []);
    for (const row of rows) curation.set(row.slug, row);
  }

  // Which screenshot reads best inside the card's 273×201 window, chosen by
  // looking at every candidate rather than trusting the store's own ordering.
  const sceneFiles = (await readdir(CURATED_DIR)).filter((name) => /^scenes-\d+\.json$/.test(name));
  const scenes = new Map();
  for (const name of sceneFiles) {
    const parsed = JSON.parse(await readFile(resolve(CURATED_DIR, name), "utf8"));
    const rows = Array.isArray(parsed) ? parsed : (parsed.picks ?? []);
    for (const row of rows) scenes.set(row.slug, row);
  }

  const entries = [];
  const missing = [];
  for (const game of games) {
    const base = curation.get(game.slug);
    if (!base) {
      missing.push(game.slug);
      continue;
    }
    const curated = { ...base, ...(DESIGN_EXACT[game.slug] ?? {}) };
    const genres = genreList(curated.genresFr);
    const gameId = `steam:${game.appId}`;
    const heroUrl = await media(game.slug, "hero.jpg", ["capsule.jpg", "cover.jpg"]);
    // `landscapeUrl` is what the card shows. A hand-picked scene beats the
    // capsule, which is mostly wordmark once it is cropped this small.
    const pick = scenes.get(game.slug);
    const sceneFile = pick && pick.scene >= 0 ? `scene-${pick.scene}.jpg` : null;
    const landscapeUrl = sceneFile
      ? await media(game.slug, sceneFile, ["capsule.jpg", "hero.jpg", "cover.jpg"])
      : await media(game.slug, "capsule.jpg", ["hero.jpg", "cover.jpg"]);
    const coverUrl = await media(game.slug, "cover.jpg", ["capsule.jpg", "hero.jpg"]);
    if (!heroUrl) {
      missing.push(`${game.slug} (no artwork)`);
      continue;
    }

    entries.push({
      id: gameId,
      title: displayTitle(game.title),
      source: "store",
      shortDescription: trimDescription(game.shortDescription),
      coverUrl,
      heroUrl,
      landscapeUrl,
      genres,
      tags: [
        ...(curated.categories ?? []).map((category) => CATEGORY_TAG[category]).filter(Boolean),
        ...game.steamGenres.slice(0, 3),
      ],
      supportedPlatforms: game.supportedPlatforms,
      owned: false,
      launchable: false,
      wishlisted: false,
      playTimeSeconds: 0,
      lastPlayedAt: null,
      recommendationReasons: curated.reasons ?? [],
      offers: buildOffers(game, curated, fetchedAt),
      curation: {
        genres,
        duration: curated.duration,
        mode: curated.mode,
        stats: curated.stats,
        tagline: curated.tagline,
        heroTitle: curated.heroTitle,
        heroLead: curated.heroLead,
        highlights: curated.highlights,
        categories: curated.categories ?? [],
        platforms: curated.platforms ?? ["pc"],
      },
      meta: {
        appId: game.appId,
        developer: game.developer,
        publisher: game.publisher,
        releaseDate: game.releaseDate,
        metacritic: game.metacritic,
      },
    });
  }

  // The design opens on Planet of Lana, Firewatch, Inscryption, Dorfromantik and
  // A Short Hike, in that order; the rest keeps the curated order.
  const opening = ["steam:1608230", "steam:383870", "steam:1092790", "steam:1455840", "steam:1055540"];
  entries.sort((left, right) => {
    const leftRank = opening.indexOf(left.id);
    const rightRank = opening.indexOf(right.id);
    if (leftRank !== -1 || rightRank !== -1) {
      return (leftRank === -1 ? 99 : leftRank) - (rightRank === -1 ? 99 : rightRank);
    }
    return 0;
  });

  const body = entries
    .map((entry) => {
      const { meta, ...summary } = entry;
      return `  {\n    ...${JSON.stringify(summary, null, 4).slice(1, -1).trim().replace(/\n/g, "\n  ")}\n  }`;
    })
    .join(",\n");

  const source = `// GENERATED FILE — run \`node scripts/fetch-store-catalog.mjs && node scripts/build-store-catalog.mjs\`.
// Facts (title, description, genres, platform support, prices) come from the
// Steam store listing for the French region; the French editorial copy in
// \`curation\` is Orivo's own. Nothing here is invented: a title with no quoted
// price keeps \`priceMinor: null\`.
import type { GameSummary } from "./contracts";

export const STORE_CATALOG_FETCHED_AT = ${JSON.stringify(fetchedAt)};

export const STORE_CATALOG_META: Record<string, { appId: number; developer: string | null; publisher: string | null; releaseDate: string | null; metacritic: number | null }> = ${JSON.stringify(
    Object.fromEntries(entries.map((entry) => [entry.id, entry.meta])),
    null,
    2,
  )};

export const STORE_CATALOG: GameSummary[] = ${JSON.stringify(
    entries.map(({ meta, ...summary }) => summary),
    null,
    2,
  )};
`;

  await writeFile(OUT, source);
  // The Rust shell embeds this sibling at compile time, so the desktop store
  // and the WebView can never drift onto two different catalogues.
  await writeFile(
    OUT_JSON,
    `${JSON.stringify(
      entries.map(({ meta, ...summary }) => summary),
      null,
      2,
    )}\n`,
  );
  console.log(`wrote ${entries.length} games → ${OUT}`);
  console.log(`wrote ${entries.length} games → ${OUT_JSON}`);
  if (missing.length) console.log(`skipped: ${missing.join(", ")}`);
}

await main();
