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
 * The Store is one brand at a time: two chevrons walk the stores ("Tout",
 * PlayStation, Xbox, Nintendo, Steam, Epic) routed as `?store=`, the brand's
 * masthead names the one on screen, then its exclusives in rows of three. The bundled catalogue carries no exclusive
 * yet, so "Tout" keeps the whole shelf and every brand shows its empty state;
 * these specs assert exactly that, and the detail a card opens.
 */
test.describe("Store chevrons", () => {
  test("a chevron walks to the next store and lands in the URL", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(STORE_CATALOG_SIZE);
    await expect(page.locator(`${storeHost} .store-tab`)).toHaveCount(0);

    await page.locator("[data-focus-key='store-next']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?store=playstation");
    await expect(page.locator(`${storeHost} .store-page`)).toHaveAttribute("data-store", "playstation");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(0);
    await expect(page.locator(`${storeHost} .store-empty__title`)).toHaveText("No exclusives yet");

    await page.locator("[data-focus-key='store-previous']").click();
    await page.locator("[data-focus-key='store-previous']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?store=epic");
  });

  test("the store survives a reload", async ({ page }) => {
    await openRoute(page, "#/store?store=nintendo", "store");
    await page.reload({ waitUntil: "domcontentloaded" });
    await waitForPage(page, "store");

    expect(await currentHash(page)).toBe("#/store?store=nintendo");
    await expect(page.locator(`${storeHost} .store-page`)).toHaveAttribute("data-store", "nintendo");
  });

  test("a search with no matches yields an explicit empty state, not a fabricated one", async ({
    page,
  }) => {
    await openRoute(page, "#/store?q=zzzznotfound", "store");

    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(0);
    await expect(page.locator(`${storeHost} .store-empty__title`)).toHaveText("No game matches");
    await expect(page.locator(`${storeHost} .store-card__price`)).toHaveCount(0);
  });
});

test.describe("Store detail", () => {
  test("a card opens its game in place, and Escape puts it back", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await page.locator(`[data-focus-key='game-${STORE_FIRST_GAME_ID}']`).click();

    const dialog = page.locator(`${storeHost} .store-detail`);
    await expect(dialog).toBeVisible();
    await expect(dialog).toHaveAttribute("aria-modal", "true");
    await expect(dialog.locator(".store-detail__title")).toHaveText("Planet of Lana");
    await expect(dialog.locator(".store-detail__amount")).toHaveText(/^\d+,\d{2}\s?€$/);
    await expect(page.locator("[data-focus-key='detail-buy']")).toBeFocused();

    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(page.locator(`[data-focus-key='game-${STORE_FIRST_GAME_ID}']`)).toBeFocused();
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
        expect(card.price, `${card.id}: unrecognised price format`).toMatch(/^(Gratuit|Free|\d+,\d{2}\s?€)$/);
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

    // Browse a detail page from the Store as well, through the card's detail.
    await page.locator(`[data-focus-key='game-${STORE_SECOND_GAME_ID}']`).click();
    await page.locator("[data-focus-key='detail-page']").click();
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
  test("navigating away and back restores the store", async ({ page }) => {
    await openRoute(page, "#/store?store=playstation", "store");
    await page.locator("[data-focus-key='store-next']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?store=xbox");

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");

    await page.goBack();
    await waitForPage(page, "store");

    expect(await currentHash(page)).toBe("#/store?store=xbox");
    await expect(page.locator(`${storeHost} .store-page`)).toHaveAttribute("data-store", "xbox");
  });

  test("navigating away and back restores focus to the game that was opened", async ({ page }) => {
    await openRoute(page, "#/store", "store");

    await page.locator(`[data-focus-key='game-${STORE_SECOND_GAME_ID}']`).click();
    await page.locator("[data-focus-key='detail-page']").click();
    await waitForPage(page, "game");

    await page.locator(".gd-back").click();
    await waitForPage(page, "store");

    // `activate()` schedules `restorePageState()` inside a `requestAnimationFrame`
    // (store-page.ts), one tick `waitForPage` does not wait out on its own, so
    // the focus read polls rather than trusting a fixed pause.
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
    // The card carries the store's own capsule, which already has the game's
    // name written across it, so the card prints no title of its own. Its
    // accessible name is what says which game it is.
    await expect(page.locator(`${storeHost} .store-card__open`)).toHaveAttribute(
      "aria-label",
      "Open Hades",
    );
    await expect(host(page, "store")).toBeVisible();
  });
});
