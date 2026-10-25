import type { WallpaperCategory } from "./contracts";

/**
 * The little badge on a wallpaper tile: "4K", "1080p" and so on, or the raw
 * "W×H" when the picture is smaller than any of those.
 *
 * Either edge may qualify an image, because the rows are not all 16:9 — a
 * 3840x1240 hero is 4K by width though it is nowhere near 2160 tall. A portrait
 * image is turned on its side first, so a cover is judged as its landscape
 * twin would be. A wordmark is not "1080p"
 * or "4K" in any useful sense, only as wide as its words, so a logo always
 * shows its pixel size.
 *
 * Returns `null` when the size is unknown (or not a real size): the tile then
 * wears no badge rather than a made-up one.
 */
export function imageQualityLabel(
  width: number | undefined,
  height: number | undefined,
  category?: WallpaperCategory,
): string | null {
  if (
    typeof width !== "number" ||
    typeof height !== "number" ||
    !Number.isFinite(width) ||
    !Number.isFinite(height) ||
    width <= 0 ||
    height <= 0
  ) {
    return null;
  }
  const w = Math.round(width);
  const h = Math.round(height);
  if (category !== "logo") {
    // A portrait image is read sideways, so a 600x900 cover is not "720p" just
    // because its long edge passes 720.
    const long = Math.max(w, h);
    const short = Math.min(w, h);
    const [wide, tall] = h > w ? [long, short] : [w, h];
    if (wide >= 3840 || tall >= 2160) return "4K";
    if (wide >= 2560 || tall >= 1440) return "1440p";
    if (wide >= 1920 || tall >= 1080) return "1080p";
    if (wide >= 1280 || tall >= 720) return "720p";
  }
  return `${w}×${h}`;
}
