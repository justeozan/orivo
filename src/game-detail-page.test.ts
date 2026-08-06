import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { GameSummary, WallpaperCandidateView, WallpaperCategory } from "./contracts";
import { createFallbackGameDetail } from "./game-detail-model";
import { createGameDetailPage, type GameDetailPageClient } from "./game-detail-page";
import { PageLifecycleHost } from "./page-lifecycle";

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));
const GAME_ID = "steam:1245620";

function related(overrides: Partial<GameSummary> = {}): GameSummary {
  return {
    id: "steam:1",
    title: "Related Game",
    source: "store",
    shortDescription: "A related entry.",
    coverUrl: "/media/related.jpg",
    heroUrl: "/media/related-hero.jpg",
    landscapeUrl: "/media/related-landscape.jpg",
    genres: ["RPG"],
    tags: [],
    supportedPlatforms: ["windows"],
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: [],
    offers: [],
    ...overrides,
  };
}

function clientWithDetail(
  detail: ReturnType<typeof createFallbackGameDetail>,
  overrides: Partial<GameDetailPageClient> = {},
): GameDetailPageClient {
  return {
    async getDetail() {
      return detail;
    },
    async setWishlist() {},
    async selectMedia() {
      return [];
    },
    async importMedia() {
      return [];
    },
    async exportMedia() {},
    async cancelMediaDownload() {},
    async searchWallpapers(source, category, query) {
      return { phase: "ready", source, category, query, message: "", candidates: [] };
    },
    async importWallpaper() {
      return [];
    },
    async openOffer() {},
    async searchArtwork() {},
    async removeGame() {},
    async setHomeImage() {},
    ...overrides,
  };
}

/** `count` results already scoped to one category, as the backend now returns. */
function candidatesFor(category: WallpaperCategory, count: number): WallpaperCandidateView[] {
  return Array.from({ length: count }, (_, index) => ({
    id: `${category}-${index}`,
    title: `${category} ${index}`,
    thumbnailUrl: `/media/${category}-${index}.jpg`,
    category,
  }));
}

let container: HTMLElement;
let host: PageLifecycleHost | null = null;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
});

afterEach(() => {
  host?.deactivate();
  host = null;
  container.remove();
});

async function mountWith(
  detail: ReturnType<typeof createFallbackGameDetail>,
  overrides: Partial<GameDetailPageClient> = {},
): Promise<PageLifecycleHost> {
  const page = createGameDetailPage({
    navigate: () => {},
    back: () => {},
    play: () => {},
    client: clientWithDetail(detail, overrides),
  });
  host = new PageLifecycleHost(container, page);
  await host.activate({ page: "game", gameId: GAME_ID, from: "library" });
  await flush();
  return host;
}

/**
 * Opens the wallpaper dialog and waits for all three category requests, which
 * the page fires together rather than one after another.
 */
async function openWallpaperDialog(
  overrides: Partial<GameDetailPageClient> = {},
): Promise<void> {
  const detail = createFallbackGameDetail(GAME_ID);
  // No saved wallpapers, so each row shows exactly what its own search returned.
  detail.media = [];
  await mountWith(detail, overrides);
  // The dialog opens from the "…" menu now: open the menu, then the item.
  container.querySelector<HTMLElement>("[data-focus-key='more-actions']")?.click();
  container.querySelector<HTMLElement>("[data-focus-key='menu-wallpaper']")?.click();
  await flush();
}

const rowTiles = (category: WallpaperCategory): HTMLElement[] => [
  ...container.querySelectorAll<HTMLElement>(
    `.gd-wallrow[data-category='${category}'] .gd-wallgrid__tile`,
  ),
];

describe("game detail page social rendering", () => {
  it("renders the friends, activity and related row when the backend ships the data", async () => {
    const detail = createFallbackGameDetail(GAME_ID);
    detail.friends = Array.from({ length: 6 }, (_, i) => ({
      id: `f${i}`,
      name: `Friend ${i}`,
      avatarUrl: `/media/avatar-${i}.jpg`,
      status: "In game",
    }));
    detail.activity = Array.from({ length: 3 }, (_, i) => ({
      id: `a${i}`,
      actorName: `Actor ${i}`,
      summary: "Reached a new level",
      detail: "42",
      avatarUrl: `/media/avatar-${i}.jpg`,
      occurredAt: new Date().toISOString(),
    }));
    detail.relatedGames = Array.from({ length: 4 }, (_, i) =>
      related({ id: `steam:${i}`, title: `Related ${i}` }),
    );

    await mountWith(detail);

    const social = container.querySelector(".gd-social");
    expect(social).not.toBeNull();
    expect(social?.querySelector(".gd-friends")).not.toBeNull();
    expect(social?.querySelector(".gd-activity")).not.toBeNull();
    expect(social?.querySelector(".gd-related")).not.toBeNull();

    // Friends panel caps the stack at four avatars and reports the remainder as
    // an inline "+N" pill, matching the reference.
    expect(container.querySelectorAll(".gd-friends__item")).toHaveLength(4);
    expect(container.querySelector(".gd-friends__count")?.textContent).toBe("+2");

    // Activity caps at three entries.
    expect(container.querySelectorAll(".gd-activity__item")).toHaveLength(3);

    // Related cards render the covers, capped at eight.
    expect(container.querySelectorAll(".gd-related__cell")).toHaveLength(4);
    expect(container.querySelector(".gd-related__media")).not.toBeNull();

    // The media rail lives inside the hero (top-right), not as a page-level row.
    const gallery = container.querySelector(".gd-gallery");
    expect(gallery).not.toBeNull();
    expect(gallery?.closest(".gd-hero")).not.toBeNull();

    // Section order: hero, panels, social row.
    const roots = [...container.querySelectorAll<HTMLElement>(".gd-page > *")];
    const order = roots.map((node) => node.className);
    expect(order.indexOf("gd-hero")).toBeLessThan(order.indexOf("gd-panels"));
    expect(order.indexOf("gd-panels")).toBeLessThan(order.indexOf("gd-social"));
  });

  it("gives the meta row glyphs and an accent-toned score, and the stats row icons", async () => {
    await mountWith(createFallbackGameDetail(GAME_ID));

    // The reference meta row carries a clock, a star and a thumbs-up glyph.
    expect(container.querySelector(".gd-meta__item .gd-icon")).not.toBeNull();
    // Dot separators still join the facts.
    expect(container.querySelector(".gd-meta__item")?.getAttribute("data-separator")).toBe("dot");
    // The review score is accent-toned.
    expect(container.querySelector(".gd-meta__item[data-tone='accent']")).not.toBeNull();
    expect(container.querySelector(".gd-meta__item[data-fact-id='review']")?.textContent).toBe("97%");
    expect(container.querySelector(".gd-stats__item .gd-icon")).not.toBeNull();
  });

  it("renders the social row from the browser fallback data", async () => {
    // No `client` passed: in a non-Tauri environment the page falls back to the
    // editorial detail, which now ships the reference's social content.
    const page = createGameDetailPage({ navigate: () => {}, back: () => {}, play: () => {} });
    host = new PageLifecycleHost(container, page);
    await host.activate({ page: "game", gameId: GAME_ID, from: "store" });
    await flush();

    expect(container.querySelector(".gd-social")).not.toBeNull();
    expect(container.querySelector(".gd-friends")).not.toBeNull();
    expect(container.querySelector(".gd-activity")).not.toBeNull();
    expect(container.querySelector(".gd-related")).not.toBeNull();
    // The media rail sits inside the hero.
    expect(container.querySelector(".gd-hero .gd-gallery")).not.toBeNull();
    expect(container.querySelector(".gd-hero__title")?.textContent).toBe("Elden Ring");
  });

  it("puts Wishlist and the more menu under the title, with no source logo", async () => {
    await mountWith(createFallbackGameDetail(GAME_ID));

    // The approved design's meta row opens straight on the developer name —
    // the old Steam/local source logo is gone.
    expect(container.querySelector(".gd-hero__subline .gd-source")).toBeNull();

    // Play, Wishlist and the "…" actions control share the row below the copy.
    const wishlist = container.querySelector<HTMLButtonElement>("[data-focus-key='wishlist']");
    expect(wishlist).not.toBeNull();
    expect(wishlist?.textContent).toContain("Wishlist");
    expect(container.querySelector(".gd-hero__copy .gd-more")).not.toBeNull();
  });

  it("shows no playtime when never played", async () => {
    const detail = createFallbackGameDetail(GAME_ID);
    detail.source = "steam";
    detail.playTimeSeconds = 0;
    detail.lastPlayedAt = null;

    await mountWith(detail);

    // A never-played game shows no "0h" and no empty label at all.
    expect(container.querySelector(".gd-stats__item[data-fact-id='playtime']")).toBeNull();
    expect(container.querySelector(".gd-stats__item[data-fact-id='last-played']")).toBeNull();
  });

  it("omits the social row and its sections when the detail ships no social data", async () => {
    const detail = createFallbackGameDetail(GAME_ID);
    detail.friends = [];
    detail.activity = [];
    detail.relatedGames = [];

    await mountWith(detail);

    expect(container.querySelector(".gd-social")).toBeNull();
    expect(container.querySelector(".gd-friends")).toBeNull();
    expect(container.querySelector(".gd-activity")).toBeNull();
    expect(container.querySelector(".gd-related")).toBeNull();
  });
});

describe("wallpaper dialog category rows", () => {
  it("renders one row per shape, each holding only its own category's tiles", async () => {
    const counts: Record<WallpaperCategory, number> = { cover: 5, landscape: 3, background: 2 };
    const asked: WallpaperCategory[] = [];

    await openWallpaperDialog({
      async searchWallpapers(source, category, query) {
        asked.push(category);
        return {
          phase: "ready",
          source,
          category,
          query,
          message: "",
          candidates: candidatesFor(category, counts[category]),
        };
      },
    });

    // One scoped request per row — never one flat search that is sorted after.
    expect([...asked].sort()).toEqual(["background", "cover", "landscape"]);

    const rows = [...container.querySelectorAll<HTMLElement>(".gd-wallrow")];
    expect(rows.map((row) => row.dataset.category)).toEqual(["cover", "landscape", "background"]);
    expect(rows.map((row) => row.querySelector(".gd-wallrow__title")?.textContent)).toEqual([
      "Cover",
      "Landscape cover",
      "Background",
    ]);

    expect(rowTiles("cover")).toHaveLength(5);
    expect(rowTiles("landscape")).toHaveLength(3);
    expect(rowTiles("background")).toHaveLength(2);

    // A short row pads to five slots so the dialog keeps a steady width.
    expect(
      container.querySelectorAll(".gd-wallrow[data-category='landscape'] .gd-wallgrid__ghost"),
    ).toHaveLength(2);

    // The portrait row only ever holds portrait results.
    expect(rowTiles("cover").map((tile) => tile.getAttribute("aria-label"))).toEqual([
      "cover 0",
      "cover 1",
      "cover 2",
      "cover 3",
      "cover 4",
    ]);

    // Each grid carries its category so the CSS can give Cover the 2:3 shape.
    expect(container.querySelector(".gd-wallgrid[data-category='cover']")).not.toBeNull();

    // Every row offers its own "Voir tout" link.
    expect(container.querySelectorAll("[data-focus-key^='wallpaper-seeall-']")).toHaveLength(3);
  });

  it("caps an unfiltered row at five tiles and shows the rest once it is expanded", async () => {
    await openWallpaperDialog({
      async searchWallpapers(source, category, query) {
        return {
          phase: "ready",
          source,
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 8),
        };
      },
    });

    expect(rowTiles("background")).toHaveLength(5);

    container.querySelector<HTMLElement>("[data-focus-key='wallpaper-seeall-background']")?.click();

    expect(container.querySelectorAll(".gd-wallrow")).toHaveLength(1);
    expect(rowTiles("background")).toHaveLength(8);
  });

  it("narrows to a single row when a chip is picked, and restores all three on a second click", async () => {
    await openWallpaperDialog({
      async searchWallpapers(source, category, query) {
        return {
          phase: "ready",
          source,
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 4),
        };
      },
    });

    expect(container.querySelectorAll(".gd-chip")).toHaveLength(3);
    expect(container.querySelectorAll(".gd-wallrow")).toHaveLength(3);

    container.querySelector<HTMLElement>("[data-focus-key='wallpaper-chip-landscape']")?.click();

    const rows = [...container.querySelectorAll<HTMLElement>(".gd-wallrow")];
    expect(rows).toHaveLength(1);
    expect(rows[0].dataset.category).toBe("landscape");
    expect(rowTiles("landscape")).toHaveLength(4);

    const chip = container.querySelector<HTMLElement>("[data-focus-key='wallpaper-chip-landscape']");
    expect(chip?.getAttribute("aria-pressed")).toBe("true");
    expect(chip?.classList.contains("gd-chip--active")).toBe(true);
    // The tick badge only rides the selected chip.
    expect(container.querySelectorAll(".gd-chip--active")).toHaveLength(1);

    // Clicking the lit chip again clears the filter.
    container.querySelector<HTMLElement>("[data-focus-key='wallpaper-chip-landscape']")?.click();
    expect(container.querySelectorAll(".gd-wallrow")).toHaveLength(3);
    expect(container.querySelectorAll(".gd-chip--active")).toHaveLength(0);
  });

  it("keeps a failing row's error inside that row while its neighbours still render", async () => {
    await openWallpaperDialog({
      async searchWallpapers(source, category, query) {
        if (category === "cover") throw new Error("Cover art search failed.");
        return {
          phase: "ready",
          source,
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 3),
        };
      },
    });

    const coverRow = container.querySelector<HTMLElement>(".gd-wallrow[data-category='cover']");
    expect(coverRow).not.toBeNull();
    expect(coverRow?.querySelector(".gd-search__notice--error")?.textContent).toBe(
      "Cover art search failed.",
    );
    expect(rowTiles("cover")).toHaveLength(0);

    // The other two rows are untouched, and carry no error of their own.
    expect(rowTiles("landscape")).toHaveLength(3);
    expect(rowTiles("background")).toHaveLength(3);
    expect(container.querySelectorAll(".gd-search__notice--error")).toHaveLength(1);
  });

  it("ticks a tile with the violet check and never asks which slot it fills", async () => {
    await openWallpaperDialog({
      async searchWallpapers(source, category, query) {
        return {
          phase: "ready",
          source,
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 2),
        };
      },
    });

    expect(container.querySelector(".gd-roles")).toBeNull();

    container.querySelector<HTMLElement>("[data-focus-key='wall-cover-0']")?.click();

    const tile = container.querySelector<HTMLElement>("[data-focus-key='wall-cover-0']");
    expect(tile?.classList.contains("gd-wallgrid__tile--selected")).toBe(true);
    expect(tile?.querySelector(".gd-wallgrid__check")).not.toBeNull();
    // The row already named the slot, so no "Apply as" picker ever appears.
    expect(container.querySelector(".gd-roles")).toBeNull();
    expect(
      container.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']")?.disabled,
    ).toBe(false);
  });
});
