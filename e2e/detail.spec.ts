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
    await openRoute(page, "#/store?category=short-sessions&provider=steam", "store");

    await page.locator("[data-focus-key='game-steam:1145350']").click();
    await waitForPage(page, "game");

    expect(await currentHash(page)).toBe("#/games/steam%3A1145350?from=store");
    await expect(page.locator(`${detailHost} .gd-back__label`)).toHaveText("Back to Store");
    await expect(page.locator("header.topbar [aria-current]")).toHaveText("Store");

    await page.locator(`${detailHost} .gd-back`).click();
    await waitForPage(page, "store");
    expect(await currentHash(page)).toBe("#/store?category=short-sessions&provider=steam");
    await expect(page.locator("#app-page-store:not([hidden]) .store-card")).toHaveCount(3);
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
  test("sections with no data do not render at all", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    // The browser fallback ships no friends, no activity and no related games.
    await expect(page.locator(`${detailHost} .gd-friends`)).toHaveCount(0);
    await expect(page.locator(`${detailHost} .gd-activity`)).toHaveCount(0);
    await expect(page.locator(`${detailHost} .gd-related`)).toHaveCount(0);
    // The wrapper is not emitted either, so there is no empty container left behind.
    await expect(page.locator(`${detailHost} .gd-social`)).toHaveCount(0);

    const headings = await page
      .locator(`${detailHost} .gd-panel__title`)
      .allTextContents();
    expect(headings).not.toContain("Friends who play");
    expect(headings).not.toContain("Activity feed");
    expect(headings).not.toContain("Related games");
    // The sections that do have data are still there.
    expect(headings).toEqual(["About this game", "Game info", "Features", "Achievements"]);
  });

  test("no placeholder or empty-state copy stands in for the missing sections", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");
    const text = await page.locator(detailHost).innerText();

    expect(text).not.toMatch(/friends/i);
    expect(text).not.toMatch(/activity/i);
    expect(text).not.toMatch(/coming soon|no data|placeholder/i);
  });
});

test.describe("game detail wallpaper dialog", () => {
  const HERO_MEDIA = "media-media_fallback_wallpaper_hero";
  const LANDSCAPE_MEDIA = "media-media_fallback_wallpaper_landscape";

  test("only wallpapers are offered in the hero rail", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    const thumbs = page.locator(`${detailHost} .gd-gallery__thumb`);
    await expect(thumbs).toHaveCount(2);
    await expect(thumbs.first().locator(".gd-gallery__thumb-image")).toBeVisible();
    // No media tabs, no icon or cover slots, no video.
    await expect(page.locator(`${detailHost} .gd-media__tab`)).toHaveCount(0);
    await expect(page.locator(`${detailHost} video`)).toHaveCount(0);
    await expect(page.locator(`${detailHost} .gd-gallery__search`)).toBeVisible();
  });

  test("a wallpaper click previews without persisting the selection", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    const heroImage = page.locator(`${detailHost} .gd-hero__image`);
    await expect(heroImage).toHaveAttribute("src", "/media/igdb/heroes/elden-ring-wallpaper.png");

    const landscapeThumb = page.locator(
      `${detailHost} .gd-gallery [data-focus-key='${LANDSCAPE_MEDIA}']`,
    );
    await landscapeThumb.click();

    // The preview swaps the hero art and the radio selection…
    await expect(heroImage).toHaveAttribute("src", "/media/igdb/landscapes/elden-ring.jpg");
    await expect(landscapeThumb).toHaveAttribute("aria-checked", "true");
    await expect(
      page.locator(`${detailHost} .gd-gallery [data-focus-key='${HERO_MEDIA}']`),
    ).toHaveAttribute("aria-checked", "false");

    // …but the applied badge — the persisted choice — never moves.
    await expect(
      page.locator(
        `${detailHost} .gd-gallery [data-focus-key='${HERO_MEDIA}'] .gd-gallery__thumb-applied`,
      ),
    ).toHaveCount(1);
    await expect(
      page.locator(
        `${detailHost} .gd-gallery [data-focus-key='${LANDSCAPE_MEDIA}'] .gd-gallery__thumb-applied`,
      ),
    ).toHaveCount(0);

    // A reload proves nothing was written anywhere.
    await page.reload({ waitUntil: "domcontentloaded" });
    await waitForPage(page, "game");

    await expect(page.locator(`${detailHost} .gd-hero__image`)).toHaveAttribute(
      "src",
      "/media/igdb/heroes/elden-ring-wallpaper.png",
    );
    await expect(
      page.locator(`${detailHost} .gd-gallery [data-focus-key='${HERO_MEDIA}']`),
    ).toHaveAttribute("aria-checked", "true");
  });

  test("the dialog opens on the previewed wallpaper and slides through existing art", async ({
    page,
  }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    await page
      .locator(`${detailHost} .gd-gallery [data-focus-key='${LANDSCAPE_MEDIA}']`)
      .click();
    await page.locator(`${detailHost} .gd-gallery__search`).click();

    const dialog = page.locator(`${detailHost} .gd-modal`);
    await expect(dialog).toBeVisible();
    await expect(page.locator(`${detailHost} [role='dialog']`)).toHaveCount(1);
    // Opens on the previewed landscape, so Use is armed and the counter says 2 of 2.
    await expect(dialog.locator(".gd-slide__title")).toHaveText("Landscape");
    await expect(dialog.locator(".gd-modal__counter")).toHaveText("2 of 2");
    await expect(page.locator(`${detailHost} [data-focus-key='wallpaper-use']`)).toBeEnabled();

    // Previous moves back to the hero art; Use then persists that selection.
    await page.locator(`${detailHost} [data-focus-key='wallpaper-slide-previous']`).click();
    await expect(dialog.locator(".gd-slide__title")).toHaveText("Key art");
    await expect(dialog.locator(".gd-modal__counter")).toHaveText("1 of 2");
    await page.locator(`${detailHost} [data-focus-key='wallpaper-use']`).click();

    // The dialog closes after the selection is committed.
    await expect(dialog).toHaveCount(0);
  });

  test("searches wallpapers, imports a chosen candidate and can fetch more", async ({ page }) => {
    await openRoute(page, DETAIL_ROUTE, "game");

    // The search toggle sits at the end of the wallpaper rail.
    const toggle = page.locator(`${detailHost} [data-focus-key='wallpaper-search-toggle']`);
    await expect(toggle).toBeVisible();
    await toggle.click();

    const form = page.locator(`${detailHost} .gd-search__form`);
    await expect(form).toBeVisible();
    // The fallback pre-fills the query with the game title.
    const input = page.locator(`${detailHost} [data-focus-key='wallpaper-search-input']`);
    await expect(input).toHaveValue("Elden Ring");

    await page.locator(`${detailHost} [data-focus-key='wallpaper-search-button']`).click();
    // The fresh results become slides, ahead of the existing wallpapers.
    await expect(page.locator(`${detailHost} .gd-slide__title`)).toHaveText("Key art");
    await expect(page.locator(`${detailHost} .gd-modal__counter`)).toHaveText("1 of 6");

    // Fetch more: 4 candidates, then 2 more, then none.
    await page.locator(`${detailHost} [data-focus-key='wallpaper-search-more']`).click();
    await expect(page.locator(`${detailHost} .gd-modal__counter`)).toHaveText("1 of 8");
    await page.locator(`${detailHost} [data-focus-key='wallpaper-search-more']`).click();
    await expect(page.locator(`${detailHost} .gd-search__notice`)).toHaveText(
      "No more wallpapers matched that search.",
    );
    await expect(page.locator(`${detailHost} .gd-modal__counter`)).toHaveText("1 of 8");

    // Import the first candidate: it is selected right away and the dialog closes.
    await page.locator(`${detailHost} [data-focus-key='wallpaper-use']`).click();
    const imported = page.locator(
      `${detailHost} [data-focus-key='media-media_fallback_wallpaper_searched']`,
    );
    await expect(imported).toBeVisible();
    await expect(imported.locator(".gd-gallery__thumb-applied")).toHaveCount(1);
    await expect(page.locator(`${detailHost} .gd-modal`)).toHaveCount(0);
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
