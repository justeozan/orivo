import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type {
  GameSummary,
  WallpaperCandidateView,
  WallpaperCategory,
} from "./contracts";
import { createFallbackGameDetail } from "./game-detail-model";
import {
  createGameDetailPage,
  type GameDetailPageClient,
} from "./game-detail-page";
import { PageLifecycleHost } from "./page-lifecycle";

const flush = (): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, 0));
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
    async resetArtwork() {
      return {
        title: "Test",
        replaced: ["cover", "landscape", "background"] as const,
      };
    },
    async selectMedia() {
      return [];
    },
    async importMedia() {
      return [];
    },
    async exportMedia() {},
    async cancelMediaDownload() {},
    async searchWallpapers(category, query) {
      return {
        phase: "ready",
        category,
        query,
        message: "",
        candidates: [],
      };
    },
    async importWallpaper() {
      return [];
    },
    async openOffer() {},
    async installEpicGame() {},
    async uninstallEpicGame() {},
    async epicInstallStatus() {
      return {
        appName: "Sugar",
        state: "not-installed" as const,
        percent: 0,
        installedBytes: 0,
        totalBytes: 0,
        installPath: null,
      };
    },
    async searchArtwork() {},
    async removeGame() {},
    async setGameHidden() {},
    async setHomeImage() {},
    ...overrides,
  };
}

/** `count` results already scoped to one category, as the backend now returns. */
function candidatesFor(
  category: WallpaperCategory,
  count: number,
): WallpaperCandidateView[] {
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
  container
    .querySelector<HTMLElement>("[data-focus-key='more-actions']")
    ?.click();
  container
    .querySelector<HTMLElement>("[data-focus-key='menu-wallpaper']")
    ?.click();
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

    // Every panel shares one grid now, so the page fits a viewport without
    // hiding a section below a fold.
    const body = container.querySelector(".gd-body");
    expect(body).not.toBeNull();
    expect(body?.querySelector(".gd-friends")).not.toBeNull();
    expect(body?.querySelector(".gd-activity")).not.toBeNull();
    expect(body?.querySelector(".gd-related")).not.toBeNull();

    // Friends panel caps the stack at four avatars and reports the remainder as
    // an inline "+N" pill, matching the reference.
    expect(container.querySelectorAll(".gd-friends__item")).toHaveLength(4);
    expect(container.querySelector(".gd-friends__count")?.textContent).toBe(
      "+2",
    );

    // Activity caps at three entries.
    expect(container.querySelectorAll(".gd-activity__item")).toHaveLength(3);

    // Related cards render the covers, capped at eight.
    expect(container.querySelectorAll(".gd-related__cell")).toHaveLength(4);
    expect(container.querySelector(".gd-related__media")).not.toBeNull();

    // The media rail lives inside the hero (top-right), not as a page-level row.
    const gallery = container.querySelector(".gd-gallery");
    expect(gallery).not.toBeNull();
    expect(gallery?.closest(".gd-hero")).not.toBeNull();

    // Two rows only: the hero band, then the grid holding every panel.
    const roots = [...container.querySelectorAll<HTMLElement>(".gd-page > *")];
    const order = roots.map((node) => node.className);
    expect(order.indexOf("gd-hero")).toBeLessThan(order.indexOf("gd-body"));
    // Reading order inside the grid still runs About → facts → achievements →
    // friends → activity → related.
    const panels = [...(body?.children ?? [])].map((node) => node.className);
    expect(panels[0]).toContain("gd-about");
    expect(panels.at(-1)).toContain("gd-related");
  });

  it("gives the meta row glyphs and an accent-toned score, and the stats row icons", async () => {
    await mountWith(createFallbackGameDetail(GAME_ID));

    // The reference meta row carries a clock, a star and a thumbs-up glyph.
    expect(container.querySelector(".gd-meta__item .gd-icon")).not.toBeNull();
    // Dot separators still join the facts.
    expect(
      container.querySelector(".gd-meta__item")?.getAttribute("data-separator"),
    ).toBe("dot");
    // The review score is accent-toned.
    expect(
      container.querySelector(".gd-meta__item[data-tone='accent']"),
    ).not.toBeNull();
    expect(
      container.querySelector(".gd-meta__item[data-fact-id='review']")
        ?.textContent,
    ).toBe("97%");
    expect(container.querySelector(".gd-stats__item .gd-icon")).not.toBeNull();
  });

  it("renders the social row from the browser fallback data", async () => {
    // No `client` passed: in a non-Tauri environment the page falls back to the
    // editorial detail, which now ships the reference's social content.
    const page = createGameDetailPage({
      navigate: () => {},
      back: () => {},
      play: () => {},
    });
    host = new PageLifecycleHost(container, page);
    await host.activate({ page: "game", gameId: GAME_ID, from: "store" });
    await flush();

    expect(container.querySelector(".gd-body")).not.toBeNull();
    expect(container.querySelector(".gd-friends")).not.toBeNull();
    expect(container.querySelector(".gd-activity")).not.toBeNull();
    expect(container.querySelector(".gd-related")).not.toBeNull();
    // The media rail sits inside the hero.
    expect(container.querySelector(".gd-hero .gd-gallery")).not.toBeNull();
    expect(container.querySelector(".gd-hero__title")?.textContent).toBe(
      "Elden Ring",
    );
  });

  it("puts Favourite and the more menu under the title, behind the source mark", async () => {
    await mountWith(createFallbackGameDetail(GAME_ID));

    // The meta row opens on the store a game came from. Now that a library can
    // be stitched together from six connected accounts, which one owns a game
    // is the first thing the row has to answer — the small Steam/local glyph
    // this design dropped was replaced by each store's own mark.
    expect(
      container.querySelector(".gd-hero__subline .gd-source"),
    ).not.toBeNull();

    // Play, Favourite and the "…" actions control share the row below the copy.
    // The library is what you already own, so nothing user-facing says
    // "wishlist" — only the stored flag still carries the old name.
    const favourite = container.querySelector<HTMLButtonElement>(
      "[data-focus-key='wishlist']",
    );
    expect(favourite).not.toBeNull();
    expect(favourite?.textContent).toContain("Favourite");
    expect(favourite?.textContent).not.toContain("Wishlist");
    expect(container.querySelector(".gd-hero__copy .gd-more")).not.toBeNull();
  });

  it("offers Uninstall only for an installed Epic game, and arms it first", async () => {
    const calls: string[] = [];
    const installed = createFallbackGameDetail(GAME_ID);
    installed.source = "epic";
    installed.installState = "installed";
    await mountWith(installed, {
      async uninstallEpicGame(gameId) {
        calls.push(gameId);
      },
    });
    container
      .querySelector<HTMLElement>("[data-focus-key='more-actions']")
      ?.click();
    const uninstall = container.querySelector<HTMLButtonElement>(
      "[data-focus-key='menu-uninstall']",
    );
    expect(uninstall).not.toBeNull();
    // It deletes the game's files, so one click only arms it.
    uninstall!.click();
    expect(uninstall!.textContent).toContain("Delete the game files");
    expect(calls).toEqual([]);
    uninstall!.click();
    await flush();
    expect(calls).toEqual([GAME_ID]);

    // Nothing on disk, nothing to remove: the entry must not appear at all
    // rather than open the launcher on a game it never downloaded.
    host?.deactivate();
    host = null;
    const absent = createFallbackGameDetail(GAME_ID);
    absent.source = "epic";
    absent.installState = "not-installed";
    await mountWith(absent);
    container
      .querySelector<HTMLElement>("[data-focus-key='more-actions']")
      ?.click();
    expect(container.querySelector("[data-focus-key='menu-uninstall']")).toBeNull();
  });

  it("shows no playtime when never played", async () => {
    const detail = createFallbackGameDetail(GAME_ID);
    detail.source = "steam";
    detail.playTimeSeconds = 0;
    detail.lastPlayedAt = null;

    await mountWith(detail);

    // A never-played game shows no "0h" and no empty label at all.
    expect(
      container.querySelector(".gd-stats__item[data-fact-id='playtime']"),
    ).toBeNull();
    expect(
      container.querySelector(".gd-stats__item[data-fact-id='last-played']"),
    ).toBeNull();
  });

  it("omits the social row and its sections when the detail ships no social data", async () => {
    const detail = createFallbackGameDetail(GAME_ID);
    detail.friends = [];
    detail.activity = [];
    detail.relatedGames = [];

    await mountWith(detail);

    expect(container.querySelector(".gd-friends")).toBeNull();
    expect(container.querySelector(".gd-activity")).toBeNull();
    expect(container.querySelector(".gd-related")).toBeNull();
  });
});

describe("wallpaper dialog category rows", () => {
  it("renders one row per shape, each holding only its own category's tiles", async () => {
    const counts: Record<WallpaperCategory, number> = {
      cover: 5,
      landscape: 3,
      background: 2,
      logo: 1,
    };
    const asked: WallpaperCategory[] = [];

    await openWallpaperDialog({
      async searchWallpapers(category, query) {
        asked.push(category);
        return {
          phase: "ready",
          category,
          query,
          message: "",
          candidates: candidatesFor(category, counts[category]),
        };
      },
    });

    // One scoped request per row — never one flat search that is sorted after.
    expect([...asked].sort()).toEqual([
      "background",
      "cover",
      "landscape",
      "logo",
    ]);

    // Two columns: the card shapes stack on the left as sideways rails, the
    // background runs down the right where a tile is big enough to judge.
    const cards = [
      ...container.querySelectorAll<HTMLElement>(
        ".gd-wallpanes__cards .gd-wallrow",
      ),
    ];
    expect(cards.map((row) => row.dataset.category)).toEqual([
      "cover",
      "landscape",
      "logo",
    ]);
    const feed = [
      ...container.querySelectorAll<HTMLElement>(
        ".gd-wallpanes__feed .gd-wallrow",
      ),
    ];
    expect(feed.map((row) => row.dataset.category)).toEqual(["background"]);
    const rows = [...container.querySelectorAll<HTMLElement>(".gd-wallrow")];
    expect(
      rows.map((row) => row.querySelector(".gd-wallrow__title")?.textContent),
    ).toEqual(["Cover", "Landscape cover", "Logo", "Background"]);

    expect(rowTiles("cover")).toHaveLength(5);
    expect(rowTiles("landscape")).toHaveLength(3);
    expect(rowTiles("background")).toHaveLength(2);
    expect(rowTiles("logo")).toHaveLength(1);

    // A row that answered shows everything it holds and pads nothing: it is a
    // scroller, so there is no fixed number of slots to fill.
    expect(
      container.querySelectorAll(".gd-wallgrid__ghost"),
    ).toHaveLength(0);

    // The portrait row only ever holds portrait results.
    expect(
      rowTiles("cover").map((tile) => tile.getAttribute("aria-label")),
    ).toEqual(["cover 0", "cover 1", "cover 2", "cover 3", "cover 4"]);

    // Each grid carries its category so the CSS can give Cover the 2:3 shape.
    expect(
      container.querySelector(".gd-wallgrid[data-category='cover']"),
    ).not.toBeNull();

  });

  it("shows every tile a row holds, because the row is a scroller", async () => {
    await openWallpaperDialog({
      async searchWallpapers(category, query) {
        return {
          phase: "ready",
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 8),
        };
      },
    });

    // No five-tile cap and no "see all": a scroller already reaches the eighth
    // tile, so a control whose only job was revealing it had nothing to do.
    expect(rowTiles("background")).toHaveLength(8);
    expect(
      container.querySelectorAll("[data-focus-key^='wallpaper-seeall-']"),
    ).toHaveLength(0);
    // Paging survives, because the tiles *it* would reveal have not been
    // fetched yet — that is the one thing scrolling cannot do.
    expect(
      container.querySelector("[data-focus-key='wallpaper-more-background']"),
    ).not.toBeNull();
  });

  it("asks every source at once instead of making the user pick one", async () => {
    const asked: WallpaperCategory[] = [];
    await openWallpaperDialog({
      async searchWallpapers(category, query) {
        asked.push(category);
        return {
          phase: "ready",
          category,
          query,
          message: "",
          candidates: [],
        };
      },
    });

    // Choosing a provider was a question with no good answer — the person
    // asking wants the best picture, not a source — so the picker is gone and
    // the backend merges all of them.
    expect(container.querySelector(".gd-search__source")).toBeNull();
    // The request carries the row and the query, and nothing else.
    expect(asked.sort()).toEqual(["background", "cover", "landscape", "logo"]);
    // The query box stays: it is the only way to correct a wrong auto-match.
    expect(
      container.querySelector("[data-focus-key='wallpaper-search-input']"),
    ).not.toBeNull();
  });

  it("narrows to a single row when a chip is picked, and restores every row on a second click", async () => {
    await openWallpaperDialog({
      async searchWallpapers(category, query) {
        return {
          phase: "ready",
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 4),
        };
      },
    });

    expect(container.querySelectorAll(".gd-chip")).toHaveLength(4);
    expect(container.querySelectorAll(".gd-wallrow")).toHaveLength(4);

    container
      .querySelector<HTMLElement>("[data-focus-key='wallpaper-chip-landscape']")
      ?.click();

    const rows = [...container.querySelectorAll<HTMLElement>(".gd-wallrow")];
    expect(rows).toHaveLength(1);
    expect(rows[0].dataset.category).toBe("landscape");
    expect(rowTiles("landscape")).toHaveLength(4);

    const chip = container.querySelector<HTMLElement>(
      "[data-focus-key='wallpaper-chip-landscape']",
    );
    expect(chip?.getAttribute("aria-pressed")).toBe("true");
    expect(chip?.classList.contains("gd-chip--active")).toBe(true);
    // The tick badge only rides the selected chip.
    expect(container.querySelectorAll(".gd-chip--active")).toHaveLength(1);

    // Clicking the lit chip again clears the filter.
    container
      .querySelector<HTMLElement>("[data-focus-key='wallpaper-chip-landscape']")
      ?.click();
    expect(container.querySelectorAll(".gd-wallrow")).toHaveLength(4);
    expect(container.querySelectorAll(".gd-chip--active")).toHaveLength(0);
  });

  it("keeps a failing row's error inside that row while its neighbours still render", async () => {
    await openWallpaperDialog({
      async searchWallpapers(category, query) {
        if (category === "cover") throw new Error("Cover art search failed.");
        return {
          phase: "ready",
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 3),
        };
      },
    });

    const coverRow = container.querySelector<HTMLElement>(
      ".gd-wallrow[data-category='cover']",
    );
    expect(coverRow).not.toBeNull();
    expect(
      coverRow?.querySelector(".gd-search__notice--error")?.textContent,
    ).toBe("Cover art search failed.");
    expect(rowTiles("cover")).toHaveLength(0);

    // The other two rows are untouched, and carry no error of their own.
    expect(rowTiles("landscape")).toHaveLength(3);
    expect(rowTiles("background")).toHaveLength(3);
    expect(
      container.querySelectorAll(".gd-search__notice--error"),
    ).toHaveLength(1);
  });

  it("ticks a tile with the violet check and never asks which slot it fills", async () => {
    await openWallpaperDialog({
      async searchWallpapers(category, query) {
        return {
          phase: "ready",
          category,
          query,
          message: "",
          candidates: candidatesFor(category, 2),
        };
      },
    });

    expect(container.querySelector(".gd-roles")).toBeNull();

    container
      .querySelector<HTMLElement>("[data-focus-key='wall-cover-0']")
      ?.click();

    const tile = container.querySelector<HTMLElement>(
      "[data-focus-key='wall-cover-0']",
    );
    expect(tile?.classList.contains("gd-wallgrid__tile--selected")).toBe(true);
    expect(tile?.querySelector(".gd-wallgrid__check")).not.toBeNull();
    // The row already named the slot, so no "Apply as" picker ever appears.
    expect(container.querySelector(".gd-roles")).toBeNull();
    expect(
      container.querySelector<HTMLButtonElement>(
        "[data-focus-key='wallpaper-apply']",
      )?.disabled,
    ).toBe(false);
  });
});

describe("resetting the covers", () => {
  it("refills all three roles through one action", async () => {
    const requested: string[] = [];
    const detail = createFallbackGameDetail(GAME_ID);
    await mountWith(detail, {
      async resetArtwork(gameId) {
        requested.push(gameId);
        return {
          title: "Test",
          replaced: ["cover", "landscape", "background"],
        };
      },
    });

    // The action replaced the old single-image search, which downloaded one
    // picture and used it as cover, landscape and background at once.
    const entry = container.querySelector<HTMLButtonElement>(
      "[data-focus-key='menu-artwork']",
    );
    expect(entry?.textContent).toContain("Reset the covers");
    entry!.click();
    await flush();
    expect(requested).toEqual([GAME_ID]);
  });

  it("says which role it could not find art for instead of reporting success", async () => {
    const detail = createFallbackGameDetail(GAME_ID);
    await mountWith(detail, {
      async resetArtwork() {
        return { title: "Test", replaced: ["cover", "background"] };
      },
    });

    container
      .querySelector<HTMLButtonElement>("[data-focus-key='menu-artwork']")!
      .click();
    await flush();

    // A game whose publisher never uploaded a wide capsule keeps the landscape
    // image it had, and the page says so rather than claiming a clean result.
    expect(container.textContent).toContain("no landscape cover was found");
  });
});
