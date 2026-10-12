import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { inflateSync } from "node:zlib";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * The Android launcher icon is committed with the generated project, and
 * regenerating it with `tauri icon`'s defaults is what put it wrong in the
 * first place: a white background, and a foreground drawn to the edge of the
 * 108dp canvas, which every launcher mask then cuts into.
 */

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const RES = join(ROOT, "src-tauri/gen/android/app/src/main/res");
const DENSITIES = { mdpi: 1, hdpi: 1.5, xhdpi: 2, xxhdpi: 3, xxxhdpi: 4 } as const;

/** Decodes an 8-bit RGBA PNG far enough to find its opaque pixels' bounds. */
function opaqueBounds(bytes: Buffer): { width: number; height: number; box: [number, number, number, number] } {
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  expect([bytes[24], bytes[25]]).toEqual([8, 6]);
  const idat: Buffer[] = [];
  for (let offset = 8; offset < bytes.length; ) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString("ascii", offset + 4, offset + 8);
    if (type === "IDAT") idat.push(bytes.subarray(offset + 8, offset + 8 + length));
    offset += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * 4;
  const pixels = Buffer.alloc(height * stride);
  for (let y = 0; y < height; y += 1) {
    const filter = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x += 1) {
      const left = x >= 4 ? pixels[y * stride + x - 4] : 0;
      const up = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const upLeft = x >= 4 && y > 0 ? pixels[(y - 1) * stride + x - 4] : 0;
      const paeth = (() => {
        const p = left + up - upLeft;
        const [a, b, c] = [Math.abs(p - left), Math.abs(p - up), Math.abs(p - upLeft)];
        return a <= b && a <= c ? left : b <= c ? up : upLeft;
      })();
      const predictor = [0, left, up, (left + up) >> 1, paeth][filter];
      pixels[y * stride + x] = (line[x] + predictor) & 0xff;
    }
  }
  let box: [number, number, number, number] = [width, height, -1, -1];
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      if (pixels[y * stride + x * 4 + 3] > 8) {
        box = [Math.min(box[0], x), Math.min(box[1], y), Math.max(box[2], x), Math.max(box[3], y)];
      }
    }
  }
  return { width, height, box };
}

describe("Android launcher icon", () => {
  it("is an adaptive icon on Orivo's dark ground, not on white", () => {
    const adaptive = readFileSync(join(RES, "mipmap-anydpi-v26/ic_launcher.xml"), "utf8");
    expect(adaptive).toContain('android:drawable="@mipmap/ic_launcher_foreground"');
    expect(adaptive).toContain('android:drawable="@color/ic_launcher_background"');
    const colour = readFileSync(join(RES, "values/ic_launcher_background.xml"), "utf8");
    expect(colour).toMatch(/<color name="ic_launcher_background">#100F18<\/color>/i);
  });

  it("keeps the ring inside the 66dp circle every launcher mask leaves visible", () => {
    for (const [density, scale] of Object.entries(DENSITIES)) {
      const { width, height, box } = opaqueBounds(readFileSync(join(RES, `mipmap-${density}/ic_launcher_foreground.png`)));
      expect({ density, width, height }).toEqual({ density, width: 108 * scale, height: 108 * scale });
      const ringWidth = box[2] - box[0] + 1;
      expect(ringWidth, density).toBeLessThanOrEqual(66 * scale);
      // Centred: the mask is, so an off-centre ring is cut on one side first.
      expect(Math.abs(box[0] + box[2] + 1 - width), density).toBeLessThanOrEqual(2);
    }
  });

  it("has a legacy icon at every density for launchers older than Android 8", () => {
    for (const [density, scale] of Object.entries(DENSITIES)) {
      for (const name of ["ic_launcher.png", "ic_launcher_round.png"]) {
        const bytes = readFileSync(join(RES, `mipmap-${density}/${name}`));
        expect({ density, name, size: bytes.readUInt32BE(16) }).toEqual({ density, name, size: 48 * scale });
      }
    }
  });
});
