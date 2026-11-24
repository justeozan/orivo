import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { STORE_CATALOG } from "./generated/store-catalog";

/**
 * Files a script writes live in `src/generated/`, and only there, so that a
 * reader — or an agent — can tell from the path alone that editing one by hand
 * is wasted: the next run of its script overwrites it. These hold that line.
 */

// Not `new URL(`../${path}`, import.meta.url)`: Vite rewrites that pattern into
// an asset lookup, which resolves a path it was not built with to `undefined`.
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

describe("generated files", () => {
  const generated = readdirSync(join(ROOT, "src/generated")).filter(
    (name) => !name.endsWith(".test.ts"),
  );

  it("open with a DO NOT EDIT header that names how to regenerate them", () => {
    expect(generated.length).toBeGreaterThan(0);
    for (const name of generated) {
      const firstLine = readFileSync(join(ROOT, "src/generated", name), "utf8").split("\n")[0];
      expect(firstLine, name).toMatch(/^\/\/ GENERATED FILE — DO NOT EDIT\. Run `[^`]+`/);
    }
  });

  it("live in src/generated and nowhere else in src", () => {
    const strays = readdirSync(join(ROOT, "src"), { recursive: true, encoding: "utf8" }).filter(
      (path) => !path.startsWith("generated") && /\.generated\.\w+$/.test(path),
    );
    expect(strays).toEqual([]);
  });

  // `scripts/build-store-catalog.mjs` writes the catalogue twice: as
  // TypeScript for the WebView, and as JSON that `store.rs` embeds with
  // `include_str!`. They drifted once already — the TypeScript was regenerated
  // after the cards moved to the store capsule, the JSON was not, and the
  // desktop Store kept showing the old scenes.
  it("give the WebView and the Rust host the same Store catalogue", () => {
    const embedded = JSON.parse(
      readFileSync(join(ROOT, "src-tauri/resources/store-catalog.json"), "utf8"),
    );
    expect(STORE_CATALOG).toEqual(embedded);
  });
});
