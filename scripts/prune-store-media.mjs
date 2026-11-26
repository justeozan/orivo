#!/usr/bin/env node
/**
 * Deletes every downloaded Store image the generated catalogue and exclusives
 * do not point at. `fetch-store-scenes.mjs` pulls six candidates per game so the artwork can
 * be chosen by eye; only the winner ships.
 *
 *   node scripts/prune-store-media.mjs          # report only
 *   node scripts/prune-store-media.mjs --apply  # delete
 *
 * Re-running the fetch scripts brings the candidates back, so this is always
 * safe to apply.
 */
import { readdir, readFile, rm, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MEDIA_ROOT = resolve(ROOT, "public/media/store");
// The exclusives share the media folder, so what they show is kept as well.
const CATALOGS = [
  resolve(ROOT, "src/generated/store-catalog.ts"),
  resolve(ROOT, "src/generated/store-exclusives.ts"),
];
const APPLY = process.argv.includes("--apply");

const referenced = new Set();
for (const catalog of CATALOGS) {
  const source = await readFile(catalog, "utf8");
  for (const match of source.matchAll(/\/media\/store\/([^"]+)/g)) referenced.add(match[1]);
}

let kept = 0;
let removed = 0;
let freed = 0;

for (const slug of await readdir(MEDIA_ROOT)) {
  const directory = resolve(MEDIA_ROOT, slug);
  if (!(await stat(directory)).isDirectory()) continue;
  for (const file of await readdir(directory)) {
    if (referenced.has(`${slug}/${file}`)) {
      kept += 1;
      continue;
    }
    freed += (await stat(resolve(directory, file))).size;
    removed += 1;
    if (APPLY) await rm(resolve(directory, file));
  }
}

console.log(
  `${kept} kept, ${removed} ${APPLY ? "removed" : "removable"}, ${(freed / 1_048_576).toFixed(1)} MB${APPLY ? " freed" : " recoverable"}`,
);
if (!APPLY) console.log("re-run with --apply to delete");
