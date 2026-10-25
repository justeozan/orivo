/**
 * The picture a game wears before, or instead of, its own artwork (#59).
 *
 * It used to be the Orivo ring: correct, but three cards in a row with the
 * same mark read as a loading error, and nothing told one game apart from the
 * next. This is a ground of its own instead — two soft lights on a dark field
 * under a fixed grain (`.art-ground` in styles.css) — whose colours and light
 * positions are derived from the game, so the same game always looks the same
 * and two games rarely look alike. Nothing here is a file: the variety costs
 * no bytes, and the darkest stops keep a light title readable on any hue.
 */

export interface ArtSeed {
  /** The main light's hue, 0–359. */
  hue: number;
  /** The second light's hue: a neighbour on the wheel, never a clash. */
  hue2: number;
  /** Where the main light sits, in percent of the frame. */
  x: number;
  y: number;
}

/** FNV-1a: tiny, stable across runs and platforms, and well spread. */
const hash = (value: string): number => {
  let result = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    result ^= value.charCodeAt(index);
    result = Math.imul(result, 0x01000193);
  }
  return result >>> 0;
};

export function artSeed(key: string): ArtSeed {
  const value = hash(key);
  const hue = value % 360;
  // 30–90° away, either side: an analogous pair, which is what keeps every
  // ground looking composed rather than random.
  const offset = 30 + ((value >>> 9) % 61);
  const hue2 = (hue + ((value >>> 15) & 1 ? offset : 360 - offset)) % 360;
  return {
    hue,
    hue2,
    x: 12 + ((value >>> 16) % 46),
    y: 8 + ((value >>> 22) % 40),
  };
}

/** Paint `element`'s `.art-ground` for the game identified by `key`. */
export function applyArtSeed(element: HTMLElement, key: string): void {
  if (element.dataset.artSeed === key) return;
  element.dataset.artSeed = key;
  const seed = artSeed(key);
  element.style.setProperty("--art-hue", String(seed.hue));
  element.style.setProperty("--art-hue-2", String(seed.hue2));
  element.style.setProperty("--art-x", `${seed.x}%`);
  element.style.setProperty("--art-y", `${seed.y}%`);
}
