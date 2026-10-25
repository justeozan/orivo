import { describe, expect, it } from "vitest";
import { artSeed } from "./art-fallback";

describe("artSeed", () => {
  it("gives a game the same ground every time", () => {
    expect(artSeed("runner:abc")).toEqual(artSeed("runner:abc"));
  });

  it("tells neighbouring games apart", () => {
    const hues = new Set(["runner:1", "runner:2", "runner:3", "runner:4", "runner:5"].map((id) => artSeed(id).hue));
    expect(hues.size).toBeGreaterThan(3);
  });

  it("keeps the second light a neighbour of the first", () => {
    for (const id of ["a", "b", "steam:620", "local:hozy"]) {
      const { hue, hue2, x, y } = artSeed(id);
      const distance = Math.min(Math.abs(hue - hue2), 360 - Math.abs(hue - hue2));
      expect(distance).toBeGreaterThanOrEqual(30);
      expect(distance).toBeLessThanOrEqual(90);
      expect(x).toBeGreaterThanOrEqual(12);
      expect(y).toBeLessThan(50);
    }
  });
});
