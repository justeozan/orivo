import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { brandLogoFor } from "./brand-logos";
import { BRAND_LOGOS } from "./brand-logos.generated";

// Not `new URL(`../${path}`, import.meta.url)`: Vite rewrites that pattern into
// an asset lookup, which resolves a path it was not built with to `undefined`.
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Width and height from a PNG's IHDR chunk. */
function pngSize(path: string): { width: number; height: number } {
  const bytes = readFileSync(path);
  expect(bytes.subarray(1, 4).toString("ascii")).toBe("PNG");
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

describe("brand logos", () => {
  it("ships every logo it names, at the size it says, all one height", () => {
    for (const [id, logo] of Object.entries(BRAND_LOGOS)) {
      expect(logo.src, id).toBe(`/media/brands/${id}.png`);
      expect(pngSize(join(ROOT, "public", logo.src)), id).toEqual({ width: logo.width, height: logo.height });
      expect(logo.height, id).toBe(72);
    }
  });

  it("covers what the Library files games by", () => {
    const sources = ["steam", "epic", "gog", "ubisoft", "xbox", "microsoft-store"];
    const runners = ["ryujinx", "retroarch", "ppsspp"];
    const platforms = ["macos", "windows", "linux", "switch", "xbox", "ps5", "psp", "nes", "snes", "gba"];
    for (const id of [...sources, ...runners]) expect(brandLogoFor("source", id), id).not.toBeNull();
    for (const id of platforms) expect(brandLogoFor("platform", id), id).not.toBeNull();
    // No public-domain or CC0 logo to build these from, so they keep their word.
    for (const id of ["instant-gaming", "winlator"]) expect(brandLogoFor("source", id), id).toBeNull();
    // No wordmark of their own: plain segments with a standard glyph instead.
    for (const id of ["local", "wine", "moonlight"]) expect(brandLogoFor("source", id), id).toBeNull();
    for (const id of ["gb", "megadrive", "other"]) expect(brandLogoFor("platform", id), id).toBeNull();
  });

  it("is a store or a platform only where the bar is sorting by one", () => {
    expect(brandLogoFor("genre", "steam")).toBeNull();
    expect(brandLogoFor("activity", "windows")).toBeNull();
    expect(brandLogoFor("source", "constructor")).toBeNull();
    expect(brandLogoFor("source", "runner:org.example.dolphin")).toBeNull();
  });
});
