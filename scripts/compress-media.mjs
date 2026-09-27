#!/usr/bin/env node
/**
 * Re-encodes the bundled demo artwork under `public/media`.
 *
 * Every JPEG in there arrived straight from a store's CDN at roughly quality
 * 93, which is about three times the bytes a screenshot needs on screen. That
 * matters more here than on a normal web page: `tauri::generate_context!()`
 * embeds the whole of `dist/` into the native binary, so a megabyte of store
 * artwork is a megabyte inside the Android `.so` and inside the macOS app.
 *
 *   node scripts/compress-media.mjs            re-encode in place
 *   node scripts/compress-media.mjs --check    report, write nothing
 *
 * Needs `cjpeg`/`djpeg` from libjpeg-turbo (`brew install jpeg-turbo`). Only a
 * maintainer refreshing the artwork needs them — the results are committed, so
 * building, testing and running Orivo do not.
 */
import { readdir, readFile, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { join, resolve } from "node:path";
import { dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MEDIA_ROOT = resolve(ROOT, "public/media");

/**
 * Quality 82, progressive, optimised Huffman tables.
 *
 * 82 is the point where these particular images stop losing bytes without
 * starting to lose detail: the six golden screenshots still match at the
 * suite's own tolerance, and the saving is about 40%. Below ~76 the gradients
 * in the store heroes begin to band.
 */
const QUALITY = 82;

/**
 * Keep the re-encode only when it saves at least this much.
 *
 * This is what makes the script safe to run twice. Re-encoding an image that is
 * already at quality 82 saves almost nothing but throws away a little more
 * detail each time, so anything under the floor is left exactly as it was.
 */
const MIN_SAVING = 0.1;

const check = process.argv.includes("--check");

function run(command, args, stdin) {
  return new Promise((done, fail) => {
    const child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"] });
    const out = [];
    const err = [];
    child.stdout.on("data", (chunk) => out.push(chunk));
    child.stderr.on("data", (chunk) => err.push(chunk));
    child.on("error", (error) => {
      fail(
        error.code === "ENOENT"
          ? new Error(
              `${command} is not on PATH. Install libjpeg-turbo (brew install jpeg-turbo), or skip this step — the artwork in the repository is already compressed.`,
            )
          : error,
      );
    });
    child.on("close", (code) => {
      if (code === 0) done(Buffer.concat(out));
      else fail(new Error(`${command} exited ${code}: ${Buffer.concat(err).toString().trim()}`));
    });
    child.stdin.end(stdin);
  });
}

async function* jpegs(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) yield* jpegs(path);
    else if (/\.jpe?g$/i.test(entry.name)) yield path;
  }
}

async function main() {
  let before = 0;
  let after = 0;
  let rewritten = 0;
  let left = 0;

  for await (const path of jpegs(MEDIA_ROOT)) {
    const original = await readFile(path);
    // djpeg to PPM then cjpeg: cjpeg cannot read JPEG, and going through the
    // pixels is the only way to change the quantisation tables at all.
    const pixels = await run("djpeg", ["-outfile", "/dev/stdout", path]);
    const encoded = await run(
      "cjpeg",
      ["-quality", String(QUALITY), "-optimize", "-progressive"],
      pixels,
    );

    before += original.byteLength;
    const saving = 1 - encoded.byteLength / original.byteLength;
    if (saving < MIN_SAVING) {
      after += original.byteLength;
      left += 1;
      continue;
    }

    after += encoded.byteLength;
    rewritten += 1;
    if (!check) await writeFile(path, encoded);
  }

  const mb = (bytes) => (bytes / 1e6).toFixed(2);
  console.log(
    `${check ? "would rewrite" : "rewrote"} ${rewritten} JPEGs, left ${left} alone`,
  );
  console.log(`${mb(before)} MB -> ${mb(after)} MB (${((1 - after / before) * 100).toFixed(1)}%)`);
}

await main().catch((error) => {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
});
