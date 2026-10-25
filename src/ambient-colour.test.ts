import { describe, expect, it } from "vitest";
import { dominantHue, paletteFor } from "./ambient-colour";

const pixels = (...colours: Array<[number, number, number, number]>) => {
  const data = new Uint8ClampedArray(colours.length * 4);
  colours.forEach((colour, index) => data.set(colour, index * 4));
  return data;
};

describe("dominantHue", () => {
  it("follows the colour, not the grey that covers more of the picture", () => {
    const grey: [number, number, number, number] = [128, 128, 128, 255];
    const red: [number, number, number, number] = [200, 40, 40, 255];
    const found = dominantHue(pixels(grey, grey, grey, grey, red, red));
    expect(found).not.toBeNull();
    expect(found!.hue).toBeLessThan(15);
    expect(found!.saturation).toBeGreaterThan(0.2);
  });

  it("calls a colourless picture colourless", () => {
    expect(dominantHue(pixels([30, 30, 30, 255], [220, 220, 220, 255]))?.saturation).toBe(0);
  });
});

describe("paletteFor", () => {
  it("builds the lights around the scene's hue", () => {
    expect(paletteFor(0, 0.8).b).toMatch(/^hsl\(0 /);
    expect(paletteFor(0, 0).b).toBe("hsl(0 8% 52%)");
  });
});
