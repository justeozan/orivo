import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

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
});
