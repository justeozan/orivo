#!/usr/bin/env node
// The release side of src/release-highlights.ts.
//
//   node scripts/release-highlights.mjs stamp 0.4.11   # tag.yml, in the bump commit
//   node scripts/release-highlights.mjs notes v0.4.11  # release-notes.sh
//
// Both load the TypeScript files with Node's own type stripping (Node 22.18 or
// newer), so the data has a single source and no build step or dependency.
//
// `stamp` renames the "next" bucket to the version being cut and opens a new,
// empty one above it. A release made of PRs without highlights stamps nothing,
// so the About page falls back to the last release that had some.
//
// `notes` prints the English highlights of one version as Markdown, or nothing.
//
// Optional last arguments point at other files, for the tests:
//   stamp <version> [data.ts]   notes <version> [data.ts] [locale.ts]
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const DATA = resolve(root, "src/release-highlights.ts");
const LOCALE = resolve(root, "src/locales/highlights.ts");
const NEXT = "next";
const VERSION = /^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/;
// The bucket's opening, as the data file lays it out. Matching the layout
// rather than parsing TypeScript keeps the rest of the file byte for byte.
const NEXT_BUCKET = /^( *)\{\n( *)version: "next",\n/m;

const fail = (message) => {
  console.error(`release-highlights: ${message}`);
  process.exit(1);
};

// A query string per load, so a file read again after a rewrite is not served
// from the module cache.
let loads = 0;
async function load(path) {
  loads += 1;
  return import(`${pathToFileURL(path).href}?load=${loads}`);
}

function plainVersion(value) {
  const version = (value ?? "").replace(/^v/, "");
  if (!VERSION.test(version)) fail(`'${value ?? ""}' is not a version (MAJOR.MINOR.PATCH[-pre]).`);
  return version;
}

async function stamp(version, path = DATA) {
  const { RELEASE_HIGHLIGHTS: releases } = await load(path);
  const next = releases.find((release) => release.version === NEXT);
  if (!next) fail(`${path} has no "${NEXT}" bucket to stamp.`);
  // Checked even with nothing to stamp, so the tests catch a layout that
  // drifted in the PR that changed it, not at a later release.
  const source = readFileSync(path, "utf8");
  const openings = source.match(new RegExp(NEXT_BUCKET.source, "gm")) ?? [];
  if (openings.length !== 1) {
    fail(`expected one '{ version: "${NEXT}",' opening in ${path}, found ${openings.length}.`);
  }
  if (next.items.length === 0) {
    console.log(`No highlights waiting; nothing to stamp for ${version}.`);
    return;
  }
  if (releases.some((release) => release.version === version)) {
    fail(`${path} already has highlights for ${version}.`);
  }

  writeFileSync(
    path,
    source.replace(
      NEXT_BUCKET,
      (_, outer, inner) =>
        `${outer}{\n${inner}version: "${NEXT}",\n${inner}items: [],\n${outer}},\n` +
        `${outer}{\n${inner}version: "${version}",\n`,
    ),
  );

  // The rewrite is textual, so it is read back the way the app will read it.
  const after = (await load(path)).RELEASE_HIGHLIGHTS;
  const stamped = after.find((release) => release.version === version);
  const empty = after.find((release) => release.version === NEXT);
  if (!stamped || stamped.items.length !== next.items.length || !empty || empty.items.length !== 0) {
    fail(`the stamped ${path} does not read back as expected.`);
  }
  console.log(`Stamped ${next.items.length} highlight(s) as ${version}.`);
}

async function notes(version, path = DATA, localePath = LOCALE) {
  const { RELEASE_HIGHLIGHTS: releases } = await load(path);
  const { en } = await load(localePath);
  const release = releases.find((candidate) => candidate.version === version);
  if (!release || release.items.length === 0) return;
  const lines = release.items.map(({ titleKey, lineKey }) => {
    if (!en[titleKey] || !en[lineKey]) fail(`no English text for ${titleKey} or ${lineKey}.`);
    return `- **${en[titleKey]}**: ${en[lineKey]}`;
  });
  process.stdout.write(`## Highlights\n\n${lines.join("\n")}\n`);
}

const [command, version, ...files] = process.argv.slice(2);
switch (command) {
  case "stamp":
    await stamp(plainVersion(version), ...files);
    break;
  case "notes":
    await notes(plainVersion(version), ...files);
    break;
  default:
    fail("usage: release-highlights.mjs stamp|notes <version> [files…]");
}
