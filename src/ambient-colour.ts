/**
 * The colour a scene is lit by, so the Play button can wear it.
 *
 * A fixed palette put a blue button on Cult of the Lamb's red. Instead the
 * wallpaper is read once at a few dozen pixels, its dominant hue is found by
 * a saturation-weighted histogram (a big grey sky must not outvote a red
 * banner), and the button's lights are built around that hue.
 */

export interface AmbientPalette {
  a: string;
  b: string;
  c: string;
  spark: string;
  rim: string;
}

const SAMPLE_SIZE = 32;
const HUE_BUCKETS = 24;

type Rgb = [number, number, number];

function rgbToHsl([r, g, b]: Rgb): [number, number, number] {
  const red = r / 255;
  const green = g / 255;
  const blue = b / 255;
  const max = Math.max(red, green, blue);
  const min = Math.min(red, green, blue);
  const lightness = (max + min) / 2;
  if (max === min) return [0, 0, lightness];
  const delta = max - min;
  const saturation = lightness > 0.5 ? delta / (2 - max - min) : delta / (max + min);
  const hue =
    max === red
      ? ((green - blue) / delta + (green < blue ? 6 : 0)) * 60
      : max === green
        ? ((blue - red) / delta + 2) * 60
        : ((red - green) / delta + 4) * 60;
  return [hue, saturation, lightness];
}

/**
 * The dominant hue of an RGBA buffer and how colourful it is, or null when
 * there is nothing to read. Pure, so it is tested without a canvas.
 */
export function dominantHue(data: Uint8ClampedArray): { hue: number; saturation: number } | null {
  const weights = new Array<number>(HUE_BUCKETS).fill(0);
  const hues = new Array<number>(HUE_BUCKETS).fill(0);
  let saturationSum = 0;
  let total = 0;
  for (let index = 0; index + 3 < data.length; index += 4) {
    if (data[index + 3] < 128) continue;
    const [hue, saturation, lightness] = rgbToHsl([data[index], data[index + 1], data[index + 2]]);
    // Near-black and near-white pixels carry no hue worth following.
    const weight = saturation * (1 - Math.abs(lightness - 0.5) * 2);
    total += 1;
    saturationSum += weight;
    if (weight <= 0.04) continue;
    const bucket = Math.floor(hue / (360 / HUE_BUCKETS)) % HUE_BUCKETS;
    weights[bucket] += weight;
    hues[bucket] += hue * weight;
  }
  if (total === 0) return null;
  let best = 0;
  for (let bucket = 1; bucket < HUE_BUCKETS; bucket += 1) {
    if (weights[best] < weights[bucket]) best = bucket;
  }
  if (weights[best] === 0) return { hue: 0, saturation: 0 };
  return { hue: hues[best] / weights[best], saturation: Math.min(1, (saturationSum / total) * 2.4) };
}

/**
 * The button's lights for a hue. A colourless scene gets a silver palette
 * rather than a forced tint; a colourful one gets its own hue and two
 * neighbours, so the field still moves between colours.
 */
export function paletteFor(hue: number, saturation: number): AmbientPalette {
  const s = Math.round(saturation < 0.12 ? 8 : 55 + Math.min(1, saturation) * 35);
  const h = Math.round(hue);
  const hsl = (shift: number, sat: number, light: number) => `hsl(${(h + shift + 360) % 360} ${sat}% ${light}%)`;
  return {
    a: hsl(-18, s, 66),
    b: hsl(0, s, 52),
    c: hsl(24, s, 46),
    spark: hsl(-36, Math.max(8, s - 15), 80),
    rim: [hsl(0, Math.max(8, s - 30), 94), hsl(-18, s, 76), hsl(0, s, 56), hsl(24, s, 72), hsl(0, Math.max(8, s - 30), 94)].join(", "),
  };
}

const known = new Map<string, Promise<AmbientPalette | null>>();

/**
 * The palette for a picture, read once per source. A picture whose pixels
 * cannot be read (a CDN that does not answer CORS) gives null, and the button
 * keeps its own palette.
 */
export function ambientPalette(source: string): Promise<AmbientPalette | null> {
  const cached = known.get(source);
  if (cached) return cached;
  const pending = new Promise<AmbientPalette | null>((resolve) => {
    const image = new Image();
    image.crossOrigin = "anonymous";
    image.decoding = "async";
    image.onerror = () => resolve(null);
    image.onload = () => {
      try {
        const canvas = document.createElement("canvas");
        canvas.width = SAMPLE_SIZE;
        canvas.height = SAMPLE_SIZE;
        const context = canvas.getContext("2d", { willReadFrequently: true });
        if (!context || image.naturalWidth === 0) return resolve(null);
        context.drawImage(image, 0, 0, SAMPLE_SIZE, SAMPLE_SIZE);
        const found = dominantHue(context.getImageData(0, 0, SAMPLE_SIZE, SAMPLE_SIZE).data);
        resolve(found ? paletteFor(found.hue, found.saturation) : null);
      } catch {
        resolve(null);
      }
    };
    image.src = source;
  });
  known.set(source, pending);
  return pending;
}
