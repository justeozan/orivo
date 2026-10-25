import { describe, expect, it } from "vitest";
import { logoScale, opaqueBounds } from "./logo-fit";

describe("logoScale", () => {
  it("lets a compact mark use the whole box and shrinks a banner", () => {
    expect(logoScale(1)).toBe(1);
    expect(logoScale(1.8)).toBe(1);
    expect(logoScale(3.6)).toBeCloseTo(Math.SQRT1_2, 3);
  });

  it("never shrinks a very wide mark past its floor", () => {
    expect(logoScale(40)).toBeCloseTo(0.62, 3);
    expect(logoScale(Number.NaN)).toBe(1);
  });
});

describe("opaqueBounds", () => {
  const image = (width: number, height: number, opaque: Array<[number, number]>) => {
    const data = new Uint8ClampedArray(width * height * 4);
    for (const [x, y] of opaque) data[(y * width + x) * 4 + 3] = 255;
    return data;
  };

  it("finds the mark inside its transparent margin", () => {
    expect(opaqueBounds(image(10, 6, [[2, 1], [7, 4]]), 10, 6)).toEqual({ left: 2, top: 1, right: 8, bottom: 5 });
  });

  it("says when there is no mark at all", () => {
    expect(opaqueBounds(image(4, 4, []), 4, 4)).toBeNull();
  });
});
