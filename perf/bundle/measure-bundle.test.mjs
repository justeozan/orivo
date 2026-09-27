/**
 * `node --test perf/bundle/measure-bundle.test.mjs`
 *
 * Not wired into `pnpm test` (vitest.config.ts only includes `src/**\/*.test.ts`
 * — a perf-tooling test has no reason to run on every unit-test invocation),
 * so this uses Node's built-in runner instead of adding a dependency for it.
 */
import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { summarizeDist } from "./measure-bundle.mjs";

test("categorises files by extension and sums their real byte size", async () => {
  const dir = await mkdtemp(join(tmpdir(), "orivo-bundle-test-"));
  try {
    await mkdir(join(dir, "assets"));
    await mkdir(join(dir, "media", "store"), { recursive: true });
    await writeFile(join(dir, "assets", "index.js"), "x".repeat(1000));
    await writeFile(join(dir, "assets", "index.css"), "y".repeat(500));
    await writeFile(join(dir, "media", "store", "cover.jpg"), "z".repeat(2000));
    await writeFile(join(dir, "index.html"), "<html></html>");

    const { totals, fileCount } = await summarizeDist(dir);

    assert.equal(fileCount, 4);
    assert.equal(totals.js, 1000);
    assert.equal(totals.css, 500);
    assert.equal(totals.media, 2000);
    assert.equal(totals.other, "<html></html>".length);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("gzip totals only cover JS and CSS, and are smaller than the raw size for compressible text", async () => {
  const dir = await mkdtemp(join(tmpdir(), "orivo-bundle-test-"));
  try {
    // Repetitive text compresses well; this only needs to be smaller, not any
    // particular ratio.
    await writeFile(join(dir, "index.js"), "const x = 1;\n".repeat(500));

    const { totals, gzip } = await summarizeDist(dir);

    assert.ok(gzip.js > 0);
    assert.ok(gzip.js < totals.js);
    assert.equal(gzip.css, 0);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
