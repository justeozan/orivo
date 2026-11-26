import { describe, expect, it } from "vitest";
import { aboutHighlights } from "./about-model";
import type { Highlight, ReleaseHighlights } from "./release-highlights";

const item = (titleKey: Highlight["titleKey"]): Highlight => ({
  icon: "sparkle",
  tone: "library",
  titleKey,
  lineKey: "highlights.switchPlugin.line",
});

const RELEASES: readonly ReleaseHighlights[] = [
  { version: "next", items: [item("highlights.general.title")] },
  { version: "0.4.9", items: [item("highlights.pluginStates.title"), item("highlights.storage.title")] },
  { version: "0.4.8", items: [] },
  { version: "0.4.3", items: [item("highlights.switchPlugin.title")] },
  { version: "0.4.10-rc.1", items: [item("highlights.accounts.title")] },
];

describe("aboutHighlights", () => {
  it("takes the running version's own highlights", () => {
    expect(aboutHighlights(RELEASES, "0.4.9")?.version).toBe("0.4.9");
    expect(aboutHighlights(RELEASES, "0.4.3")?.items[0].titleKey).toBe("highlights.switchPlugin.title");
    expect(aboutHighlights(RELEASES, "0.4.10-rc.1")?.version).toBe("0.4.10-rc.1");
  });

  it("falls back to the newest earlier release that has some, under its own version", () => {
    // 0.4.8 is listed but empty, and 0.4.10-rc.1 is a pre-release of 0.4.10, not older.
    expect(aboutHighlights(RELEASES, "0.4.10")?.version).toBe("0.4.9");
    expect(aboutHighlights(RELEASES, "0.4.8")?.version).toBe("0.4.3");
    expect(aboutHighlights(RELEASES, "v0.4.7")?.version).toBe("0.4.3");
  });

  it("never shows what is not released yet, nor anything newer than the running copy", () => {
    expect(aboutHighlights(RELEASES, "0.4.2")).toBeNull();
    expect(aboutHighlights(RELEASES, "0.4.9")?.items.map((entry) => entry.titleKey)).not.toContain(
      "highlights.general.title",
    );
    expect(aboutHighlights([{ version: "next", items: [item("highlights.general.title")] }], null)).toBeNull();
  });

  it("shows the newest release when the version is unknown", () => {
    expect(aboutHighlights(RELEASES, null)?.version).toBe("0.4.10-rc.1");
    expect(aboutHighlights(RELEASES, "")?.version).toBe("0.4.10-rc.1");
  });

  it("keeps it to four, however many a release lists", () => {
    const many = { version: "1.0.0", items: Array.from({ length: 7 }, () => item("highlights.about.title")) };
    expect(aboutHighlights([many], "1.0.0")?.items).toHaveLength(4);
  });
});
