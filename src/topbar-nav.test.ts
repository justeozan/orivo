import { describe, expect, it } from "vitest";
import { navGlyph, navPlateEase } from "./topbar-nav";

describe("navPlateEase", () => {
  it("starts on the old link and ends exactly on the new one", () => {
    expect(navPlateEase(0)).toBe(0);
    expect(navPlateEase(1)).toBe(1);
    expect(navPlateEase(-0.2)).toBe(0);
    expect(navPlateEase(1.4)).toBe(1);
  });

  // The overshoot is the whole point of a spring, and too much of it reads as
  // a wobble: the plate passes its link by a few percent of the trip and
  // comes back.
  it("passes its target a little and settles back", () => {
    const samples = Array.from({ length: 101 }, (_, i) => navPlateEase(i / 100));
    const peak = Math.max(...samples);
    expect(peak).toBeGreaterThan(1.03);
    expect(peak).toBeLessThan(1.12);
    expect(Math.abs(navPlateEase(0.95) - 1)).toBeLessThan(0.02);
  });

  it("covers most of the trip early, so a press feels answered", () => {
    expect(navPlateEase(0.2)).toBeGreaterThan(0.6);
  });
});

describe("navGlyph", () => {
  it("draws an outline and a filled form for every page", () => {
    for (const page of ["library", "store", "me", "settings"] as const) {
      const markup = navGlyph(page);
      expect(markup).toContain("nav-glyph--line");
      expect(markup).toContain("nav-glyph--fill");
      expect(markup.match(/aria-hidden="true"/g)).toHaveLength(2);
    }
  });
});
