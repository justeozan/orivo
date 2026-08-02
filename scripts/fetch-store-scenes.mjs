#!/usr/bin/env node
/**
 * Downloads candidate scene artwork for every catalogue game.
 *
 * The card frame is a small landscape window (273×201). Steam's capsule art is
 * mostly wordmark at that size and the library hero is a 3:1 banner that
 * centre-crops to a sliver, so the card uses a real screenshot instead. This
 * pulls the first six screenshots per game; a later pass picks the one that
 * reads best.
 *
 *   node scripts/fetch-store-scenes.mjs
 *
 * Output: public/media/store/<slug>/scene-<n>.jpg  (+ scenes written back into
 * .context/redesign/steam-data.json)
 */
import { mkdir, readFile, writeFile, access } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DATA = resolve(ROOT, ".context/redesign/steam-data.json");
const MEDIA_ROOT = resolve(ROOT, "public/media/store");
const UA = "Orivo/0.3 (+https://orivo.io; contact@oneiby.com)";
const CANDIDATES = 6;

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

async function exists(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

async function download(url, destination) {
  if (await exists(destination)) return true;
  const response = await fetch(url, { headers: { "user-agent": UA } });
  if (!response.ok) return false;
  const body = Buffer.from(await response.arrayBuffer());
  if (body.byteLength < 4_000) return false;
  await mkdir(dirname(destination), { recursive: true });
  await writeFile(destination, body);
  return true;
}

async function main() {
  const payload = JSON.parse(await readFile(DATA, "utf8"));
  for (const [index, game] of payload.games.entries()) {
    process.stdout.write(`[${index + 1}/${payload.games.length}] ${game.slug} … `);
    try {
      const details = await getJson(
        `https://store.steampowered.com/api/appdetails?appids=${game.appId}&cc=fr&l=french&filters=screenshots`,
      );
      const shots = details?.[String(game.appId)]?.data?.screenshots ?? [];
      const saved = [];
      for (const [shotIndex, shot] of shots.slice(0, CANDIDATES).entries()) {
        const url = String(shot.path_full ?? "").split("?")[0];
        if (!url) continue;
        const file = `scene-${shotIndex}.jpg`;
        if (await download(url, resolve(MEDIA_ROOT, game.slug, file))) saved.push(file);
      }
      game.scenes = saved;
      console.log(`${saved.length} scene(s)`);
    } catch (error) {
      game.scenes = [];
      console.log(`failed — ${error.message ?? error}`);
    }
    await sleep(320);
  }
  await writeFile(DATA, `${JSON.stringify(payload, null, 2)}\n`);
  console.log("updated", DATA);
}

await main();
