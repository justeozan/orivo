import { describe, expect, it } from "vitest";
import { sourceMark } from "./source-marks";
import type { SourceKind } from "./library-browse";

const segments: Array<{ id: string; kind: SourceKind }> = [
  { id: "all", kind: "all" },
  { id: "steam", kind: "store" },
  { id: "epic", kind: "store" },
  { id: "gog", kind: "store" },
  { id: "ubisoft", kind: "store" },
  { id: "xbox", kind: "store" },
  { id: "microsoft-store", kind: "store" },
  { id: "instant-gaming", kind: "store" },
  { id: "android", kind: "store" },
  { id: "switch", kind: "console" },
  { id: "psp", kind: "console" },
  { id: "snes", kind: "console" },
  { id: "runner:com.example.mame", kind: "plugin" },
  { id: "local", kind: "local" },
  { id: "stream", kind: "stream" },
];

describe("sourceMark", () => {
  it("gives every segment a vector mark, hidden from assistive tech", () => {
    for (const segment of segments) {
      const markup = sourceMark(segment);
      expect(markup, segment.id).toMatch(/^<svg class="icon [^"]*source-mark/);
      expect(markup, segment.id).toContain('aria-hidden="true"');
      // Inline and self-contained: no image, nothing fetched.
      expect(markup, segment.id).not.toMatch(/<image|href=/);
    }
  });

  it("draws a brand by its own mark, in the colour of the word beside it", () => {
    for (const id of ["android", "psp", "switch"]) {
      const markup = sourceMark({ id, kind: id === "android" ? "store" : "console" });
      expect(markup).toContain("source-mark--brand");
      expect(markup).toContain('fill="currentColor"');
    }
  });

  it("falls back to a glyph for a console or plugin it has no mark for", () => {
    expect(sourceMark({ id: "megadrive", kind: "console" })).toBe(sourceMark({ id: "nes", kind: "console" }));
    expect(sourceMark({ id: "runner:a", kind: "plugin" })).toBe(sourceMark({ id: "runner:b", kind: "plugin" }));
  });
});
