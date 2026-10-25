/**
 * Wordmarks arrive in every shape a publisher ever exported: a 4:1 banner, a
 * near-square crest, and often either one floating in a canvas of transparent
 * margin. Boxed by height alone, the crest fills the box, the banner shrinks
 * to a strip and the padded one looks a size smaller than its neighbours —
 * the same piece of furniture at three different sizes.
 *
 * Two corrections make them read alike:
 *  - the transparent margin is cut away, so the box measures the mark rather
 *    than the canvas it was exported on;
 *  - the height is scaled by the mark's proportions, so marks of different
 *    shapes cover a similar area instead of a similar height.
 */

export interface PreparedLogo {
  /** What the <img> loads: the trimmed copy when there was margin to cut. */
  src: string;
  /** Width over height of the visible mark. */
  aspect: number;
}

/** The proportions a mark has when it fills the whole box height. */
const REFERENCE_ASPECT = 1.8;
/** The smallest share of the box a very wide mark is allowed to shrink to. */
const MIN_SCALE = 0.62;
/** A pixel this transparent or more is margin, not mark (anti-aliasing fringe). */
const ALPHA_THRESHOLD = 12;
/** Margins thinner than this share of a side are not worth a re-encode. */
const MIN_TRIM = 0.03;
/** The analysis runs on a reduced copy; the bounds are scaled back up. */
const ANALYSIS_SIZE = 320;

/**
 * How much of the box height a mark of `aspect` should use. Equal area: a mark
 * twice as wide as the reference is drawn at 1/√2 of its height, never more
 * than the box allows and never below `MIN_SCALE`.
 */
export function logoScale(aspect: number): number {
  if (!Number.isFinite(aspect) || aspect <= 0) return 1;
  return Math.min(1, Math.max(MIN_SCALE, Math.sqrt(REFERENCE_ASPECT / aspect)));
}

/** The visible bounds of an RGBA buffer, or null when every pixel is margin. */
export function opaqueBounds(
  data: Uint8ClampedArray,
  width: number,
  height: number,
): { left: number; top: number; right: number; bottom: number } | null {
  let left = width;
  let top = height;
  let right = -1;
  let bottom = -1;
  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      if (data[(y * width + x) * 4 + 3] < ALPHA_THRESHOLD) continue;
      if (x < left) left = x;
      if (x > right) right = x;
      if (y < top) top = y;
      if (y > bottom) bottom = y;
    }
  }
  return right < 0 ? null : { left, top, right: right + 1, bottom: bottom + 1 };
}

const prepared = new Map<string, Promise<PreparedLogo | null>>();

const load = (source: string, cors: boolean): Promise<HTMLImageElement> =>
  new Promise((resolve, reject) => {
    const image = new Image();
    image.decoding = "async";
    if (cors) image.crossOrigin = "anonymous";
    image.onload = () => resolve(image);
    image.onerror = () => reject(new Error("logo failed to load"));
    image.src = source;
  });

/**
 * Trim the margin when the pixels can be read. A cached file comes over the
 * asset protocol, which answers CORS; a CDN that does not simply keeps its
 * margin, and the mark is still sized by its full proportions.
 */
const trim = async (image: HTMLImageElement, source: string): Promise<PreparedLogo> => {
  const width = image.naturalWidth;
  const height = image.naturalHeight;
  const untouched: PreparedLogo = { src: source, aspect: height > 0 ? width / height : 1 };
  try {
    const ratio = Math.min(1, ANALYSIS_SIZE / Math.max(width, height));
    const probe = document.createElement("canvas");
    probe.width = Math.max(1, Math.round(width * ratio));
    probe.height = Math.max(1, Math.round(height * ratio));
    const context = probe.getContext("2d", { willReadFrequently: true });
    if (!context) return untouched;
    context.drawImage(image, 0, 0, probe.width, probe.height);
    const bounds = opaqueBounds(context.getImageData(0, 0, probe.width, probe.height).data, probe.width, probe.height);
    if (!bounds) return untouched;

    // Back to full resolution, one analysis pixel of slack on every side so
    // the fringe the threshold ignored is not shaved off the real mark.
    const scale = 1 / ratio;
    const left = Math.max(0, Math.floor((bounds.left - 1) * scale));
    const top = Math.max(0, Math.floor((bounds.top - 1) * scale));
    const right = Math.min(width, Math.ceil((bounds.right + 1) * scale));
    const bottom = Math.min(height, Math.ceil((bounds.bottom + 1) * scale));
    const cropWidth = right - left;
    const cropHeight = bottom - top;
    const aspect = cropWidth / cropHeight;
    if (cropWidth >= width * (1 - MIN_TRIM) && cropHeight >= height * (1 - MIN_TRIM)) {
      return { src: source, aspect };
    }

    const canvas = document.createElement("canvas");
    canvas.width = cropWidth;
    canvas.height = cropHeight;
    canvas.getContext("2d")?.drawImage(image, left, top, cropWidth, cropHeight, 0, 0, cropWidth, cropHeight);
    const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
    return blob ? { src: URL.createObjectURL(blob), aspect } : { src: source, aspect };
  } catch {
    // A tainted canvas: the pixels are not ours to read.
    return untouched;
  }
};

/**
 * Decode a wordmark once and say how to show it, or null when it cannot be
 * shown at all (a 404, or an image that decodes to nothing). Every caller for
 * the same source shares the one answer.
 */
export function prepareLogo(source: string): Promise<PreparedLogo | null> {
  const known = prepared.get(source);
  if (known) return known;
  const pending = load(source, true)
    .then((image) => (image.naturalWidth > 0 ? trim(image, source) : null))
    // Without CORS the picture may still load; it just cannot be measured
    // beyond its own proportions.
    .catch(() =>
      load(source, false).then(
        (image): PreparedLogo | null =>
          image.naturalWidth > 0
            ? { src: source, aspect: image.naturalHeight > 0 ? image.naturalWidth / image.naturalHeight : 1 }
            : null,
      ),
    )
    .catch(() => null);
  prepared.set(source, pending);
  return pending;
}

/** Size an <img> already showing a prepared mark. */
export function applyLogoFit(image: HTMLImageElement, logo: PreparedLogo): void {
  image.style.setProperty("--logo-scale", logoScale(logo.aspect).toFixed(3));
}
