import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * The iOS build is uploaded to an App Store Connect record that already exists,
 * and App Store Connect checks two things nothing else here would catch before
 * an upload fails: that the bundle identifier is the record's, and that the App
 * Store icon has no alpha channel. Both live in the gitignored Xcode project,
 * which scripts/sync-ios-project.sh fills from the files checked here.
 */

// Not `new URL(`../${path}`, import.meta.url)`: Vite rewrites that pattern into
// an asset lookup, which resolves a path it was not built with to `undefined`.
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const repoFile = (path: string): string => join(ROOT, path);
const readJson = (path: string): { identifier: string } => JSON.parse(readFileSync(repoFile(path), "utf8"));
const ICONS = "src-tauri/icons/ios";

/** Width, height, and whether the PNG can carry any transparency at all. */
function pngInfo(bytes: Buffer): { width: number; height: number; alpha: boolean } {
  expect(bytes.subarray(0, 8)).toEqual(Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]));
  // IHDR is always the first chunk: width, height, bit depth, colour type.
  const width = bytes.readUInt32BE(16);
  const height = bytes.readUInt32BE(20);
  const colourType = bytes[25];
  let alpha = colourType === 4 || colourType === 6;
  // A palette or greyscale image can still be transparent through tRNS.
  for (let offset = 8; offset < bytes.length && !alpha; ) {
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString("ascii", offset + 4, offset + 8);
    if (type === "tRNS") alpha = true;
    if (type === "IEND") break;
    offset += 12 + length;
  }
  return { width, height, alpha };
}

describe("iOS App Store build", () => {
  it("builds iOS under the bundle identifier of the App Store Connect record", () => {
    expect(readJson("src-tauri/tauri.ios.conf.json").identifier).toBe("run.evey.orivo");
  });

  it("keeps the identifier existing desktop and Android installs keep their data under", () => {
    expect(readJson("src-tauri/tauri.conf.json").identifier).toBe("io.orivo.desktop");
  });

  it("fills every slot of the asset catalog `tauri ios init` writes", () => {
    // A slot missing here keeps Tauri's logo in the build.
    expect(readdirSync(repoFile(ICONS)).sort()).toEqual([
      "AppIcon-20x20@1x.png",
      "AppIcon-20x20@2x-1.png",
      "AppIcon-20x20@2x.png",
      "AppIcon-20x20@3x.png",
      "AppIcon-29x29@1x.png",
      "AppIcon-29x29@2x-1.png",
      "AppIcon-29x29@2x.png",
      "AppIcon-29x29@3x.png",
      "AppIcon-40x40@1x.png",
      "AppIcon-40x40@2x-1.png",
      "AppIcon-40x40@2x.png",
      "AppIcon-40x40@3x.png",
      "AppIcon-512@2x.png",
      "AppIcon-60x60@2x.png",
      "AppIcon-60x60@3x.png",
      "AppIcon-76x76@1x.png",
      "AppIcon-76x76@2x.png",
      "AppIcon-83.5x83.5@2x.png",
    ]);
  });

  it("ships a 1024-pixel App Store icon App Store Connect accepts", () => {
    const icon = pngInfo(readFileSync(repoFile(`${ICONS}/AppIcon-512@2x.png`)));
    expect(icon).toEqual({ width: 1024, height: 1024, alpha: false });
  });

  it("has no alpha channel in any of the home screen icons either", () => {
    // `tauri icon` writes RGBA even from an opaque source, and ITMS-90717
    // refuses the channel itself, not only transparent pixels.
    for (const name of readdirSync(repoFile(ICONS))) {
      const { width, height, alpha } = pngInfo(readFileSync(repoFile(`${ICONS}/${name}`)));
      expect({ name, square: width === height, alpha }).toEqual({ name, square: true, alpha: false });
    }
  });
});
