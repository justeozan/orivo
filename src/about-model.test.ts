import { describe, expect, it } from "vitest";
import { aboutNews, changelogSection, releaseNews } from "./about-model";
import { INITIAL_UPDATE_STATE, type UpdateState } from "./updater-model";

const CHANGELOG = `# Changelog

## [Unreleased]

### Added

- Something nobody has yet.

## [0.4.2] - 2026-11-24

### Added

- A local game that was moved on disk is found again instead of breaking. Before each games-folder scan, Orivo looks for its program.
- A game's page shows the game's logo.

### Fixed

- On Windows, a local game that needs administrator rights starts.

## [0.4.0] - 2026-11-16

### Added

- Orivo speaks English and French. Settings › General › Language follows the system.
`;

describe("changelogSection", () => {
  it("takes the running version's own section", () => {
    expect(changelogSection(CHANGELOG, "0.4.0")?.version).toBe("0.4.0");
    expect(changelogSection(CHANGELOG, "0.4.2")?.version).toBe("0.4.2");
  });

  it("falls back to the newest released section before a version without one", () => {
    expect(changelogSection(CHANGELOG, "0.4.4")?.version).toBe("0.4.2");
    expect(changelogSection(CHANGELOG, "0.4.1")?.version).toBe("0.4.0");
  });

  it("never takes Unreleased, nor a version newer than the one running", () => {
    expect(changelogSection(CHANGELOG, "0.3.0")).toBeNull();
    expect(changelogSection(CHANGELOG, null)?.version).toBe("0.4.2");
    expect(changelogSection(CHANGELOG, "0.4.2")?.body).not.toContain("nobody has yet");
  });

  it("reads nothing out of nothing", () => {
    expect(changelogSection("", "0.4.2")).toBeNull();
    expect(changelogSection(null, "0.4.2")).toBeNull();
  });
});

describe("releaseNews", () => {
  it("makes a title of the first sentence and a line of the next, leaving fixes out", () => {
    const items = releaseNews(changelogSection(CHANGELOG, "0.4.2")!.body);
    expect(items.map(({ title, detail }) => ({ title, detail }))).toEqual([
      {
        title: "A local game that was moved on disk is found again instead of breaking",
        detail: "Before each games-folder scan, Orivo looks for its program",
      },
      { title: "A game's page shows the game's logo", detail: "" },
    ]);
  });

  it("draws a tile after what the line is about, and varies the others", () => {
    const items = releaseNews(
      "### Added\n- Streaming from your PC.\n- Something else.\n- Another thing.",
    );
    expect(items[0]).toMatchObject({ category: "streaming", icon: "cast" });
    expect(items[1].icon).toBe("sparkle");
    expect(items[1].category).not.toBe(items[2].category);
  });

  it("keeps it to a few lines, each one short", () => {
    const long = `A ${"very ".repeat(40)}long line.`;
    const items = releaseNews(`### Changed\n${Array.from({ length: 8 }, () => `- ${long}`).join("\n")}`);
    expect(items).toHaveLength(4);
    expect(items[0].title.length).toBeLessThanOrEqual(91);
    expect(items[0].title.endsWith("…")).toBe(true);
  });
});

describe("aboutNews", () => {
  const pending = (notes: string | null): UpdateState => ({
    ...INITIAL_UPDATE_STATE,
    status: "available",
    currentVersion: "0.4.0",
    availableVersion: "0.4.5",
    notes,
  });

  it("lets a pending update speak first, under its own version", () => {
    const news = aboutNews(pending("### Added\n- Faster store browsing."), CHANGELOG, "0.4.0");
    expect(news?.version).toBe("0.4.5");
    expect(news?.items.map((item) => item.title)).toEqual(["Faster store browsing"]);
  });

  it("falls back to the running version's notes when the update says nothing readable", () => {
    const news = aboutNews(pending("- #105 : fix: something"), CHANGELOG, "0.4.0");
    expect(news?.version).toBe("0.4.0");
    expect(news?.items[0].title).toBe("Orivo speaks English and French");
  });

  it("shows the running version's notes when nothing is pending", () => {
    expect(aboutNews(INITIAL_UPDATE_STATE, CHANGELOG, "0.4.4")?.version).toBe("0.4.2");
  });

  it("is nothing when no notes can be read", () => {
    expect(aboutNews(INITIAL_UPDATE_STATE, null, "0.4.4")).toBeNull();
  });
});
