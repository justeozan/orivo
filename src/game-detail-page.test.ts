import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { GameSummary } from "./contracts";
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

function clientWithDetail(detail: ReturnType<typeof createFallbackGameDetail>): GameDetailPageClient {
  return {
    async getDetail() {
      return detail;
    },
    async setWishlist() {},
    async resetArtwork() {
      return { title: "Test", replaced: ["cover", "landscape", "background"] as const };
    },
    async selectMedia() {
      return [];
    },
    async importMedia() {
      return [];
    },
    async exportMedia() {},
    async cancelMediaDownload() {},
    async searchWallpapers() {
      return { phase: "ready", source: "igdb", query: "", message: "", candidates: [] };
    },
    async importWallpaper() {
      return [];
    },
    async openOffer() {},
    async searchArtwork() {},
    async removeGame() {},
    async setHomeImage() {},
  };
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
    client: { ...clientWithDetail(detail), ...overrides },
  });
  host = new PageLifecycleHost(container, page);
  await host.activate({ page: "game", gameId: GAME_ID, from: "library" });
  await flush();
  return host;
}

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

  it("shows the source logo and the more menu under the title, and no bookmark", async () => {
    await mountWith(createFallbackGameDetail(GAME_ID));

    // The fallback detail is a local game: the local-machine glyph renders as
    // a real logo (an inline SVG), not a text badge.
    const source = container.querySelector(".gd-hero__subline .gd-source");
    expect(source).not.toBeNull();
    expect(source?.getAttribute("aria-label")).toBe("Source: Local");
    expect(source?.querySelector("svg")).not.toBeNull();

    // The "…" actions control lives in the hero copy, below the title.
    expect(container.querySelector(".gd-hero__copy .gd-more")).not.toBeNull();

    // The bookmark/wishlist control is gone.
    expect(container.querySelector(".gd-wishlist")).toBeNull();
  });

  it("shows the Steam mark for a Steam game and no playtime when never played", async () => {
    const detail = createFallbackGameDetail(GAME_ID);
    detail.source = "steam";
    detail.playTimeSeconds = 0;
    detail.lastPlayedAt = null;

    await mountWith(detail);

    expect(container.querySelector(".gd-source")?.getAttribute("aria-label")).toBe("Source: Steam");
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

describe("resetting the covers", () => {
  it("refills all three roles through one action", async () => {
    const requested: string[] = [];
    const detail = createFallbackGameDetail(GAME_ID);
    await mountWith(detail, {
      async resetArtwork(gameId) {
        requested.push(gameId);
        return { title: "Test", replaced: ["cover", "landscape", "background"] };
      },
    });

    // The action replaced the old single-image search, which downloaded one
    // picture and used it as cover, landscape and background at once.
    const entry = container.querySelector<HTMLButtonElement>("[data-focus-key='menu-artwork']");
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

    container.querySelector<HTMLButtonElement>("[data-focus-key='menu-artwork']")!.click();
    await flush();

    // A game whose publisher never uploaded a wide capsule keeps the landscape
    // image it had, and the page says so rather than claiming a clean result.
    expect(container.textContent).toContain("no landscape cover was found");
  });
});
