import { describe, expect, it } from "vitest";
import { navGlyph } from "./topbar-nav";

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
