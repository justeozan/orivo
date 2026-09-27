import { expect, test } from "@playwright/test";
import { currentHash, host, openRoute, waitForPage } from "./helpers";

const detailHost = "#app-page-game:not([hidden])";
const DETAIL_ROUTE = "#/games/steam%3A1245620?from=store";

test.describe("game detail origins", () => {
  test("opens from the Library with from=library and returns there", async ({ page }) => {
    await openRoute(page, "#/library", "library");

    const firstCard = page.locator("#game-cards .game-card").first();
    const gameId = await firstCard.getAttribute("data-game-id");
    await firstCard.click();
    await waitForPage(page, "game");

    expect(await currentHash(page)).toBe(`#/games/${encodeURIComponent(gameId!)}?from=library`);
    await expect(page.locator(`${detailHost} .gd-back__label`)).toHaveText("Back to Library");
    await expect(page.locator("header.topbar [aria-current]")).toHaveText("Library");

    await page.locator(`${detailHost} .gd-back`).click();
    await waitForPage(page, "library");
    expect(await currentHash(page)).toBe("#/library");
  });

  test("opens from the Store with from=store and returns to the filtered Store", async ({ page }) => {
    // The provider-pill filter bar (`provider=steam`) was retired with the
    // Store rebuild (`311a91f`); category + platform chips replaced it.
    await openRoute(page, "#/store?category=short-sessions&platform=pc", "store");
    const filteredCount = await page.locator("#app-page-store:not([hidden]) .store-card").count();

    await page.locator("[data-focus-key='game-steam:1608230']").click();
    await waitForPage(page, "game");

    expect(await currentHash(page)).toBe("#/games/steam%3A1608230?from=store");
    await expect(page.locator(`${detailHost} .gd-back__label`)).toHaveText("Back to Store");
    await expect(page.locator("header.topbar [aria-current]")).toHaveText("Store");

    await page.locator(`${detailHost} .gd-back`).click();
    await waitForPage(page, "store");
    expect(await currentHash(page)).toBe("#/store?category=short-sessions&platform=pc");
    await expect(page.locator("#app-page-store:not([hidden]) .store-card")).toHaveCount(filteredCount);
  });

  test("the origin comes from the URL, not from the last visited page", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await openRoute(page, "#/games/steam%3A1245620?from=library", "game");
    await expect(page.locator(`${detailHost} .gd-back__label`)).toHaveText("Back to Library");

    await openRoute(page, "#/games/steam%3A1245620?from=store", "game");
    await expect(page.locator(`${detailHost} .gd-back__label`)).toHaveText("Back to Store");
  });
});

test.describe("game detail sections", () => {
  test("every section the fallback carries data for renders, in order", async ({ page }) => {
    // The game detail rework (`01c8a8c`) gave the browser fallback its own
    // friends, activity feed and related games instead of shipping none, so
    // the panels these used to omit now render like every other section.
    await openRoute(page, DETAIL_ROUTE, "game");

    await expect(page.locator(`${detailHost} .gd-friends`)).toHaveCount(1);
    await expect(page.locator(`${detailHost} .gd-activity`)).toHaveCount(1);
    await expect(page.locator(`${detailHost} .gd-related`)).toHaveCount(1);

    const headings = await page.locator(`${detailHost} .gd-panel__title`).allTextContents();
    expect(headings).toEqual([
      "About this game",
      "Game info",
      "Features",
      "Achievements",
      "Friends who play",
      "Activity feed",
      "Related games",
    ]);
  });

  test("no generic placeholder copy stands in for a section's real content", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");
    const text = await page.locator(detailHost).innerText();

    expect(text).not.toMatch(/coming soon|no data|placeholder/i);
  });
});

test.describe("game detail wallpaper rail", () => {
  const HERO_MEDIA = "media-media_fallback_wallpaper_hero";
  const LANDSCAPE_MEDIA = "media-media_fallback_wallpaper_landscape";

  test("only wallpapers are offered, and adding one lives in the … menu", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    // The fallback now ships fourteen bundled wallpapers (two named plates
    // plus twelve filler tiles), not two.
    const tiles = page.locator(`${detailHost} .gd-gallery__tile`);
    await expect(tiles).toHaveCount(14);
    await expect(tiles.first().locator(".gd-gallery__tile-image")).toBeVisible();
    // No media tabs, no icon or cover slots, no video.
    await expect(page.locator(`${detailHost} .gd-media__tab`)).toHaveCount(0);
    await expect(page.locator(`${detailHost} video`)).toHaveCount(0);
    // "Search cover & images" is gone from the rail itself: changing a
    // wallpaper is a "…" menu action now (`renderMoreButton`), so the group
    // never fetched art alongside the cover and the logo can end up wearing
    // each other's images.
    await expect(page.locator(`${detailHost} .gd-gallery__search`)).toHaveCount(0);
    await expect(page.locator(`${detailHost} [data-focus-key='more-actions']`)).toBeVisible();
  });

  test("a rail click previews the wallpaper without persisting it", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    const heroImage = page.locator(`${detailHost} .gd-hero__image`);
    await expect(heroImage).toHaveAttribute("src", "/media/igdb/heroes/elden-ring-wallpaper.png");

    const landscapeTile = page.locator(`${detailHost} .gd-gallery [data-focus-key='${LANDSCAPE_MEDIA}']`);
    await landscapeTile.click();

    // The click swaps the hero art and the rail's own selection…
    await expect(heroImage).toHaveAttribute("src", "/media/igdb/landscapes/elden-ring.jpg");
    await expect(landscapeTile).toHaveClass(/gd-gallery__tile--selected/);
    await expect(
      page.locator(`${detailHost} .gd-gallery [data-focus-key='${HERO_MEDIA}']`),
    ).not.toHaveClass(/gd-gallery__tile--selected/);

    // …but a reload proves the browser fallback never wrote it anywhere.
    await page.reload({ waitUntil: "domcontentloaded" });
    await waitForPage(page, "game");

    await expect(page.locator(`${detailHost} .gd-hero__image`)).toHaveAttribute(
      "src",
      "/media/igdb/heroes/elden-ring-wallpaper.png",
    );
    await expect(page.locator(`${detailHost} .gd-gallery [data-focus-key='${HERO_MEDIA}']`)).toHaveClass(
      /gd-gallery__tile--selected/,
    );
  });
});

test.describe("game detail change-wallpaper dialog", () => {
  // The old slideshow ("N of M", Previous/Next through the wallpapers already
  // on the game) was replaced by a category grid fed by a live search
  // (`01c8a8c`), opened from the "…" menu instead of a rail button.
  async function openWallpaperDialog(page: import("@playwright/test").Page): Promise<void> {
    await page.locator(`${detailHost} [data-focus-key='more-actions']`).click();
    await page.locator(`${detailHost} [data-focus-key='menu-wallpaper']`).click();
    await expect(page.locator(`${detailHost} .gd-modal`)).toBeVisible();
  }

  test("opens from the … menu with the shape chips and a search field", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");
    await openWallpaperDialog(page);

    await expect(page.locator(`${detailHost} [role='dialog']`)).toHaveCount(1);
    await expect(page.locator(`${detailHost} [data-focus-key='wallpaper-chip-cover']`)).toBeVisible();
    await expect(page.locator(`${detailHost} [data-focus-key='wallpaper-chip-landscape']`)).toBeVisible();
    await expect(page.locator(`${detailHost} [data-focus-key='wallpaper-chip-background']`)).toBeVisible();
    await expect(page.locator(`${detailHost} [data-focus-key='wallpaper-chip-logo']`)).toBeVisible();

    const input = page.locator(`${detailHost} [data-focus-key='wallpaper-search-input']`);
    await expect(input).toBeVisible();
    // Pre-filled with the game's own title, same as the retired dialog.
    await expect(input).toHaveValue("Elden Ring");
    // Nothing is ticked yet, so applying has nothing to do.
    await expect(page.locator(`${detailHost} [data-focus-key='wallpaper-apply']`)).toBeDisabled();

    await page.keyboard.press("Escape");
    await expect(page.locator(`${detailHost} .gd-modal`)).toHaveCount(0);
  });

  test("searches, ticks one candidate per row and applies it in one pass", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");
    await openWallpaperDialog(page);

    const input = page.locator(`${detailHost} [data-focus-key='wallpaper-search-input']`);
    await input.fill("elden ring");
    await page.locator(`${detailHost} [data-focus-key='wallpaper-search-button']`).click();

    const firstBackground = page.locator(`${detailHost} [data-focus-key='wall-candidate-background-1']`);
    await expect(firstBackground).toBeVisible();
    await firstBackground.click();
    await expect(firstBackground).toHaveAttribute("aria-pressed", "true");

    // Ticking a second tile in the same row swaps the pick rather than adding
    // to it — one row is one card slot, and picking across rows is what fills
    // more than one slot in a single Apply.
    const secondBackground = page.locator(`${detailHost} [data-focus-key='wall-candidate-background-2']`);
    await secondBackground.click();
    await expect(firstBackground).toHaveAttribute("aria-pressed", "false");
    await expect(secondBackground).toHaveAttribute("aria-pressed", "true");

    const apply = page.locator(`${detailHost} [data-focus-key='wallpaper-apply']`);
    await expect(apply).toBeEnabled();
    await expect(apply).toHaveText("Apply wallpaper");
    await apply.click();

    // The browser fallback's importer always answers with the same stand-in
    // media (`createFallbackImportedWallpaper()`), which becomes the rail's
    // only tile and the new hero art; the dialog closes on commit.
    await expect(page.locator(`${detailHost} .gd-modal`)).toHaveCount(0);
    const imported = page.locator(`${detailHost} [data-focus-key='media-media_fallback_wallpaper_searched']`);
    await expect(imported).toBeVisible();
    await expect(imported).toHaveClass(/gd-gallery__tile--selected/);
  });
});

test.describe("game detail shell integration", () => {
  test("the detail page keeps the shell topbar and never opens a dialog", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    await expect(page.locator("header.topbar")).toBeVisible();
    await expect(page.locator("[role='dialog']")).toHaveCount(0);
    await expect(page.locator("[aria-modal]")).toHaveCount(0);
    await expect(host(page, "game").locator(".gd-page")).toHaveAttribute("aria-label", "Game details");
  });
});
