import { expect, test } from "@playwright/test";
import {
  currentHash,
  host,
  libraryGameIds,
  openRoute,
  STORE_CATALOG_SIZE,
  STORE_FIRST_GAME_ID,
  STORE_SECOND_GAME_ID,
  waitForPage,
} from "./helpers";

const storeHost = "#app-page-store:not([hidden])";

/**
 * The Store rebuild (`53c0d96`, `311a91f`) replaced the provider-pill filter
 * bar with category/platform chips over the real, generated catalogue
 * (`store-catalog.generated.ts`, refreshed by `pnpm store:refresh`), and
 * dropped the synthetic "Price unavailable" / stale-offer / provider-notice
 * copy that only made sense over the old all-null-price editorial fixture.
 * Real prices are fetched from Steam, so the honesty rule now lives in
 * `formatPrice`/`selectBestOffer` (covered by `store-model.test.ts`): the
 * price slot is omitted rather than filled with a placeholder when no offer
 * carries one. These specs assert the current chip/rail UI and the omission
 * behaviour, not the retired provider-pill design.
 */
test.describe("Store filters", () => {
  test("category and platform filters combine and land in the URL", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(STORE_CATALOG_SIZE);

    await page.locator("[data-focus-key='category-short-sessions']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?category=short-sessions");
    const shortSessionsCount = await page.locator(`${storeHost} .store-card`).count();
    expect(shortSessionsCount).toBeGreaterThan(0);
    expect(shortSessionsCount).toBeLessThan(STORE_CATALOG_SIZE);

    await page.locator("[data-focus-key='platform-pc']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?category=short-sessions&platform=pc");
    const combinedCount = await page.locator(`${storeHost} .store-card`).count();
    expect(combinedCount).toBeGreaterThan(0);
    expect(combinedCount).toBeLessThanOrEqual(shortSessionsCount);

    await expect(page.locator("[data-focus-key='category-short-sessions']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator("[data-focus-key='category-all-games']")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
    await expect(page.locator("[data-focus-key='platform-pc']")).toHaveAttribute("aria-pressed", "true");
  });

  test("filter state survives a reload", async ({ page }) => {
    await openRoute(page, "#/store?category=short-sessions&platform=pc", "store");
    const count = await page.locator(`${storeHost} .store-card`).count();
    expect(count).toBeGreaterThan(0);

    await page.reload({ waitUntil: "domcontentloaded" });
    await waitForPage(page, "store");

    expect(await currentHash(page)).toBe("#/store?category=short-sessions&platform=pc");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(count);
    await expect(page.locator("[data-focus-key='category-short-sessions']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator("[data-focus-key='platform-pc']")).toHaveAttribute("aria-pressed", "true");
    // Exactly one category chip and one platform chip are pressed — no leaked state.
    await expect(
      page.locator(`${storeHost} .store-chipbar--categories .store-chip[aria-pressed='true']`),
    ).toHaveCount(1);
    await expect(
      page.locator(`${storeHost} .store-chipbar--platforms .store-chip--platform[aria-pressed='true']`),
    ).toHaveCount(1);
  });

  test("a search with no matches yields an explicit empty state, not a fabricated one", async ({
    page,
  }) => {
    await openRoute(page, "#/store?q=zzzznotfound", "store");

    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(0);
    await expect(page.locator(`${storeHost} .store-empty__title`)).toHaveText("Aucun jeu ne correspond");
    await expect(page.locator(`${storeHost} .store-card__price`)).toHaveCount(0);
  });
});

test.describe("Store price honesty", () => {
  test("every price shown is real, and no digits leak outside it", async ({ page }) => {
    await openRoute(page, "#/store", "store");

    const cards = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>("#app-page-store:not([hidden]) .store-card")].map(
        (card) => ({
          id: card.dataset.gameId ?? "",
          price: card.querySelector(".store-card__price")?.textContent ?? null,
        }),
      ),
    );

    expect(cards.length).toBe(STORE_CATALOG_SIZE);
    for (const card of cards) {
      // A price slot only ever exists when `formatPrice` actually produced one
      // (`selectBestOffer` + `formatPrice`, store-model.ts): "Gratuit" or a
      // "12,34 €"-shaped amount, never a placeholder or a bare number.
      if (card.price !== null) {
        expect(card.price, `${card.id}: unrecognised price format`).toMatch(/^(Gratuit|\d+,\d{2}\s?€)$/);
      }
    }
  });
});

test.describe("Store never mutates the Library", () => {
  test("browsing and wishlisting do not add a game to the Library", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const before = await libraryGameIds(page);
    expect(before.length).toBeGreaterThan(0);

    await page.locator("[data-nav-page='store']").click();
    await waitForPage(page, "store");

    const wishlist = page.locator(`[data-focus-key='wishlist-${STORE_FIRST_GAME_ID}']`);
    await expect(wishlist).toHaveAttribute("aria-pressed", "false");
    await wishlist.click();
    await expect(wishlist).toHaveAttribute("aria-pressed", "true");

    // Browse a detail page from the Store as well.
    await page.locator(`[data-focus-key='game-${STORE_SECOND_GAME_ID}']`).click();
    await waitForPage(page, "game");
    await page.locator(".gd-back").click();
    await waitForPage(page, "store");

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");

    const after = await libraryGameIds(page);
    expect(after).toEqual(before);
    expect(after, `store id ${STORE_FIRST_GAME_ID} must not appear in the Library`).not.toContain(
      STORE_FIRST_GAME_ID,
    );
    expect(after, `store id ${STORE_SECOND_GAME_ID} must not appear in the Library`).not.toContain(
      STORE_SECOND_GAME_ID,
    );
  });
});

test.describe("Store return state", () => {
  test("navigating away and back restores the filters", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await page.locator("[data-focus-key='category-short-sessions']").click();
    await page.locator("[data-focus-key='platform-pc']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?category=short-sessions&platform=pc");
    const count = await page.locator(`${storeHost} .store-card`).count();

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");

    await page.goBack();
    await waitForPage(page, "store");

    expect(await currentHash(page)).toBe("#/store?category=short-sessions&platform=pc");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(count);
    await expect(page.locator("[data-focus-key='category-short-sessions']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator("[data-focus-key='platform-pc']")).toHaveAttribute("aria-pressed", "true");
  });

  test("navigating away and back restores focus to the previously opened card", async ({ page }) => {
    // The Store rebuild replaced the vertical grid with a single-row
    // horizontal rail (`.store-rail__track`) that resets `scrollLeft` on every
    // re-render, so there is no scroll offset left to restore — the intent
    // that survives is focus, which `store-page.ts`'s `deactivate()` /
    // `restorePageState()` still capture and reapply by `data-focus-key`.
    await openRoute(page, "#/store", "store");

    const card = page.locator(`[data-focus-key='game-${STORE_SECOND_GAME_ID}']`);
    await card.click();
    await waitForPage(page, "game");

    await page.locator(".gd-back").click();
    await waitForPage(page, "store");

    // `activate()` schedules `restorePageState()` inside a `requestAnimationFrame`
    // (store-page.ts), one tick the click handler and `waitForPage` do not wait
    // out on their own, so the focus read polls rather than trusting a fixed
    // pause — the same race as the compact form-factor's `matchMedia` listener
    // (81ea15d) and the onboarding wordmark's image decode.
    await expect
      .poll(() =>
        page.evaluate(() => (document.activeElement as HTMLElement | null)?.dataset?.focusKey ?? null),
      )
      .toBe(`game-${STORE_SECOND_GAME_ID}`);
  });

  test("the topbar search is wired to the Store while the Store is open", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    const search = page.locator("#topbar-search");

    await expect(search).toBeEnabled();
    await expect(search).toHaveAttribute("placeholder", "Search the store…");

    await search.fill("hades");
    await search.press("Enter");
    await expect.poll(() => currentHash(page)).toBe("#/store?q=hades");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(1);
    // The card carries the store's own capsule, which is the art the game was
    // sold with and already has its name written across it, so the card prints
    // no title of its own. Its accessible name is what says which game it is.
    await expect(page.locator(`${storeHost} .store-card__open`)).toHaveAttribute(
      "aria-label",
      "Ouvrir Hades",
    );
    await expect(host(page, "store")).toBeVisible();
  });
});
