#!/usr/bin/env node
/**
 * Downloads the artwork `scripts/store-exclusives.json` names for each Store
 * exclusive, from the store's or publisher's own listing, and writes it to
 * `public/media/store/<slug>/` at the size the Store shows it:
 *
 *   capsule.jpg   landscape key art with the game's wordmark   1280 wide
 *   hero.jpg      wide art, ideally with no wordmark           1600 wide
 *   cover.jpg     portrait box art                              600 wide
 *   scene-N.jpg   screenshots, best first                      1280 wide
 *
 *   node scripts/fetch-store-exclusives-media.mjs            fetch what is missing
 *   node scripts/fetch-store-exclusives-media.mjs --force    fetch everything again
 *   node scripts/fetch-store-exclusives-media.mjs <slug>…    only these games
 *
 * The files are committed, so only a maintainer changing the list runs this.
 * macOS only: it resizes with `sips` and encodes with `cjpeg` from libjpeg-turbo
 * (`brew install jpeg-turbo`) at the settings `compress-media.mjs` uses, so a
 * later pass of that script leaves these files alone.
 *
 * A picture can be given as `{ "url", "crop": "16:9", "anchor": "center" | "top" }`
 * when the store has no 16:9 cut of it: PlayStation publishes some key art
 * with the wordmark at 4:3 only, and `anchor` keeps a wordmark near the top.
 *
 * Every download is untrusted bytes: it lands in a fresh temporary directory,
 * must start like a JPEG, PNG or WebP image, and is only ever read by `sips`.
 */
import { mkdir, mkdtemp, readFile, rm, stat, writeFile, access } from "node:fs/promises";
import { spawn } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DATA = resolve(ROOT, "scripts/store-exclusives.json");
const MEDIA_ROOT = resolve(ROOT, "public/media/store");
const UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const QUALITY = 82;
const MAX_BYTES = 40 * 1024 * 1024;

/** Store and publisher CDNs the listing artwork is served from. */
const ALLOWED_HOSTS = new Set([
  "image.api.playstation.com",
  "store-images.s-microsoft.com",
  "assets.nintendo.com",
  "www.nintendo.com",
  "fs-prod-cdn.nintendo-europe.com",
  "shared.akamai.steamstatic.com",
  "shared.fastly.steamstatic.com",
  "cdn.cloudflare.steamstatic.com",
  "cdn.akamai.steamstatic.com",
  "cdn2.unrealengine.com",
]);

const WIDTH = { capsule: 1280, hero: 1600, cover: 600, scene: 1280 };

const force = process.argv.includes("--force");
const only = new Set(process.argv.slice(2).filter((arg) => !arg.startsWith("--")));

function run(command, args, stdin) {
  return new Promise((done, fail) => {
    const child = spawn(command, args, { stdio: [stdin ? "pipe" : "ignore", "pipe", "pipe"] });
    const out = [];
    const err = [];
    child.stdout.on("data", (chunk) => out.push(chunk));
    child.stderr.on("data", (chunk) => err.push(chunk));
    child.on("error", (error) =>
      fail(error.code === "ENOENT" ? new Error(`${command} is not on PATH (see the header of this script)`) : error),
    );
    child.on("close", (code) => {
      if (code === 0) done(Buffer.concat(out));
      else fail(new Error(`${command} exited ${code}: ${Buffer.concat(err).toString().trim()}`));
    });
    if (stdin) child.stdin.end(stdin);
  });
}

const exists = async (path) => {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
};

function looksLikeImage(bytes) {
  const jpeg = bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff;
  const png = bytes.subarray(0, 8).equals(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
  const webp = bytes.subarray(0, 4).toString("latin1") === "RIFF" && bytes.subarray(8, 12).toString("latin1") === "WEBP";
  return jpeg || png || webp;
}

async function download(url, scratch) {
  const parsed = new URL(url);
  if (parsed.protocol !== "https:" || !ALLOWED_HOSTS.has(parsed.host)) {
    throw new Error(`refusing ${url}: not a store CDN this script knows`);
  }
  const response = await fetch(url, { headers: { "user-agent": UA }, redirect: "error" });
  if (!response.ok) throw new Error(`${response.status} ${url}`);
  const bytes = Buffer.from(await response.arrayBuffer());
  if (bytes.byteLength > MAX_BYTES) throw new Error(`${url} is larger than ${MAX_BYTES} bytes`);
  if (!looksLikeImage(bytes)) throw new Error(`${url} is not an image`);
  const path = join(scratch, "source");
  await writeFile(path, bytes);
  return path;
}

async function pixelWidth(path) {
  const report = (await run("sips", ["-g", "pixelWidth", path])).toString();
  const match = /pixelWidth:\s*(\d+)/.exec(report);
  if (!match) throw new Error(`sips could not read ${path}`);
  return Number(match[1]);
}

async function pixelHeight(path) {
  const report = (await run("sips", ["-g", "pixelHeight", path])).toString();
  const match = /pixelHeight:\s*(\d+)/.exec(report);
  if (!match) throw new Error(`sips could not read ${path}`);
  return Number(match[1]);
}

/** Cuts a picture taller than 16:9 down to it, around its centre or its top. */
async function crop(source, scratch, anchor) {
  const width = await pixelWidth(source);
  const height = await pixelHeight(source);
  const target = Math.round((width * 9) / 16);
  if (height <= target) return source;
  const cropped = join(scratch, "cropped.png");
  const offset = anchor === "top" ? ["--cropOffset", "0", "0"] : [];
  await run("sips", ["-s", "format", "png", "--cropToHeightWidth", String(target), String(width), ...offset, source, "--out", cropped]);
  return cropped;
}

async function encode(source, scratch, width) {
  // sips resizes and decodes every format the stores serve, but its own JPEG
  // encoder has no progressive mode; a near-lossless intermediate goes through
  // djpeg/cjpeg exactly the way compress-media.mjs re-encodes.
  const resized = join(scratch, "resized.jpg");
  const args = ["-s", "format", "jpeg", "-s", "formatOptions", "100"];
  if ((await pixelWidth(source)) > width) args.push("--resampleWidth", String(width));
  await run("sips", [...args, source, "--out", resized]);
  const pixels = await run("djpeg", ["-outfile", "/dev/stdout", resized]);
  return run("cjpeg", ["-quality", String(QUALITY), "-optimize", "-progressive"], pixels);
}

async function fetchOne(slug, file, picture, width) {
  const destination = resolve(MEDIA_ROOT, slug, file);
  if (!force && (await exists(destination))) return "kept";
  const { url, crop: ratio, anchor } = typeof picture === "string" ? { url: picture } : picture;
  const scratch = await mkdtemp(join(tmpdir(), "orivo-exclusive-"));
  try {
    let source = await download(url, scratch);
    if (ratio === "16:9") source = await crop(source, scratch, anchor);
    else if (ratio) throw new Error(`unknown crop ${ratio}`);
    const encoded = await encode(source, scratch, width);
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, encoded);
    return `${Math.round(encoded.byteLength / 1024)} KB`;
  } finally {
    await rm(scratch, { recursive: true, force: true });
  }
}

async function main() {
  const { games } = JSON.parse(await readFile(DATA, "utf8"));
  let failures = 0;
  for (const game of games) {
    if (only.size && !only.has(game.slug)) continue;
    const jobs = [
      ["capsule.jpg", game.media.capsule, WIDTH.capsule],
      ...(game.media.hero ? [["hero.jpg", game.media.hero, WIDTH.hero]] : []),
      ...(game.media.cover ? [["cover.jpg", game.media.cover, WIDTH.cover]] : []),
      ...game.media.scenes.map((url, index) => [`scene-${index + 1}.jpg`, url, WIDTH.scene]),
    ];
    const results = [];
    for (const [file, url, width] of jobs) {
      try {
        results.push(`${file} ${await fetchOne(game.slug, file, url, width)}`);
      } catch (error) {
        failures += 1;
        results.push(`${file} FAILED (${error.message})`);
      }
    }
    console.log(`${game.slug}: ${results.join(", ")}`);
  }
  let total = 0;
  for (const game of games) {
    for (const file of ["capsule.jpg", "hero.jpg", "cover.jpg", ...game.media.scenes.map((_, i) => `scene-${i + 1}.jpg`)]) {
      const path = resolve(MEDIA_ROOT, game.slug, file);
      if (await exists(path)) total += (await stat(path)).size;
    }
  }
  console.log(`exclusives artwork on disk: ${(total / 1e6).toFixed(1)} MB`);
  if (failures) {
    console.error(`${failures} download(s) failed`);
    process.exitCode = 1;
  }
}

await main();
