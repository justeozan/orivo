import { expect, test } from "@playwright/test";
import {
  currentHash,
  EDITORIAL_GAME_IDS,
  host,
  libraryGameIds,
  openRoute,
  SHORT_SESSION_IDS,
  waitForPage,
} from "./helpers";

const storeHost = "#app-page-store:not([hidden])";

test.describe("Store filters", () => {
  test("category and provider filters combine and land in the URL", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(EDITORIAL_GAME_IDS.length);

    await page.locator("[data-focus-key='category-short-sessions']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?category=short-sessions");

    await page.locator("[data-focus-key='provider-steam']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?category=short-sessions&provider=steam");

    const ids = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>("#app-page-store:not([hidden]) .store-card")].map(
        (card) => card.dataset.gameId,
      ),
    );
    expect(ids).toEqual([...SHORT_SESSION_IDS]);

    await expect(page.locator("[data-focus-key='category-short-sessions']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator("[data-focus-key='provider-steam']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator(`${storeHost} .store-catalog__count`)).toHaveText("3 games shown");
    await expect(page.locator(`${storeHost} .store-catalog__title`)).toHaveText("Short Sessions");
  });

  test("filter state survives a reload", async ({ page }) => {
    await openRoute(page, "#/store?category=short-sessions&provider=steam", "store");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(3);

    await page.reload({ waitUntil: "domcontentloaded" });
    await waitForPage(page, "store");

    expect(await currentHash(page)).toBe("#/store?category=short-sessions&provider=steam");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(3);
    await expect(page.locator("[data-focus-key='category-short-sessions']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator("[data-focus-key='provider-steam']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    // Exactly one category and one provider are pressed — no leaked state.
    await expect(page.locator(`${storeHost} .store-filter-pill[aria-pressed='true']`)).toHaveCount(1);
    await expect(page.locator(`${storeHost} .store-provider-pill[aria-pressed='true']`)).toHaveCount(1);
  });

  test("a provider with no offers yields an explicit empty state, not a fabricated one", async ({
    page,
  }) => {
    await openRoute(page, "#/store", "store");
    await page.locator("[data-focus-key='provider-instant-gaming']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?provider=instant-gaming");

    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(0);
    await expect(page.locator(`${storeHost} .store-empty__title`)).toHaveText(
      "No matching games in the saved catalog",
    );
    await expect(page.locator(`${storeHost} .store-card__price`)).toHaveCount(0);
    await expect(page.locator("[data-focus-key='provider-instant-gaming']")).toHaveAttribute(
      "data-health",
      "unavailable",
    );
    await expect(page.locator("[data-focus-key='provider-instant-gaming']")).toHaveAttribute(
      "title",
      "No authorized commercial feed is configured.",
    );
  });
});

test.describe("Store offer honesty", () => {
  test("an offer with no price shows no digits anywhere on the card", async ({ page }) => {
    await openRoute(page, "#/store", "store");

    const cards = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>("#app-page-store:not([hidden]) .store-card")].map(
        (card) => ({
          id: card.dataset.gameId ?? "",
          price: card.querySelector(".store-card__price")?.textContent ?? "",
          detail: card.querySelector(".store-card__offer-detail")?.textContent ?? "",
          offerText: card.querySelector(".store-card__offer")?.textContent ?? "",
        }),
      ),
    );

    expect(cards.length).toBe(EDITORIAL_GAME_IDS.length);
    for (const card of cards) {
      expect(card.price, `${card.id} must not invent a price`).toBe("Price unavailable");
      expect(
        card.offerText,
        `${card.id} offer block must contain no digits and no currency symbol`,
      ).not.toMatch(/[0-9]|[$€£¥₹]/);
    }
  });

  test("Instant Gaming never renders a price", async ({ page }) => {
    // The editorial catalog carries a single Steam offer per game; Instant
    // Gaming is declared "unavailable". No card, filtered or not, may ever show
    // an Instant Gaming price — and no card may show any price at all, since
    // every editorial offer has `priceMinor: null`.
    for (const hash of ["#/store", "#/store?provider=instant-gaming", "#/store?category=all-games"]) {
      await openRoute(page, hash, "store");

      const offers = await page.evaluate(() =>
        [...document.querySelectorAll<HTMLElement>("#app-page-store:not([hidden]) .store-card")].map((card) => ({
          id: card.dataset.gameId ?? "",
          offer: (card.querySelector<HTMLElement>(".store-card__offer")?.innerText ?? "").replace(/\s+/g, " "),
        })),
      );
      for (const offer of offers) {
        expect(offer.offer, `${hash} / ${offer.id}: offer block shows a number`).not.toMatch(/[0-9]/);
        expect(offer.offer, `${hash} / ${offer.id}: an Instant Gaming offer appeared`).not.toMatch(
          /Instant Gaming/,
        );
      }
      await expect(page.locator(`${storeHost} .store-card__price`).filter({ hasText: /[0-9]/ })).toHaveCount(0);
    }
  });

  test("a stale verifiedAt is visibly flagged", async ({ page }) => {
    await openRoute(page, "#/store", "store");

    const total = await page.locator(`${storeHost} .store-card`).count();
    // Every editorial offer has `verifiedAt: null`, so every card is stale.
    await expect(page.locator(`${storeHost} .store-card__offer--stale`)).toHaveCount(total);
    await expect(page.locator(`${storeHost} .store-card__offer-detail`).first()).toHaveText(
      "Steam · not recently verified",
    );

    const colors = await page.evaluate(() => {
      const card = document.querySelector("#app-page-store:not([hidden]) .store-card")!;
      return {
        staleDetail: getComputedStyle(card.querySelector(".store-card__offer-detail")!).color,
        description: getComputedStyle(card.querySelector(".store-card__description")!).color,
      };
    });
    // The stale amber from store-page.css, distinct from ordinary body copy.
    expect(colors.staleDetail).toBe("rgb(195, 162, 122)");
    expect(colors.staleDetail).not.toBe(colors.description);
  });

  test("provider source notices are disclosed rather than hidden", async ({ page }) => {
    await openRoute(page, "#/store", "store");

    const summary = page.locator(`${storeHost} .store-provider-statuses__summary`);
    await expect(summary).toHaveText("6 source notices");
    await summary.click();
    await expect(page.locator(`${storeHost} .store-provider-statuses__item`)).toHaveCount(6);
    await expect(
      page.locator(`${storeHost} .store-provider-statuses__item[data-health='available']`),
    ).toHaveCount(0);
  });
});

test.describe("Store never mutates the Library", () => {
  test("browsing and wishlisting do not add a game to the Library", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const before = await libraryGameIds(page);
    expect(before.length).toBeGreaterThan(0);

    await page.locator("[data-nav-page='store']").click();
    await waitForPage(page, "store");

    const wishlist = page.locator("[data-focus-key='wishlist-steam:1091500']");
    await expect(wishlist).toHaveAttribute("aria-pressed", "false");
    await wishlist.click();
    await expect(wishlist).toHaveAttribute("aria-pressed", "true");

    // Browse a detail page from the Store as well.
    await page.locator("[data-focus-key='game-steam:1086940']").click();
    await waitForPage(page, "game");
    await page.locator(".gd-back").click();
    await waitForPage(page, "store");

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");

    const after = await libraryGameIds(page);
    expect(after).toEqual(before);
    for (const id of EDITORIAL_GAME_IDS) {
      expect(after, `store id ${id} must not appear in the Library`).not.toContain(id);
    }
  });
});

test.describe("Store return state", () => {
  test("navigating away and back restores the filters", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await page.locator("[data-focus-key='category-short-sessions']").click();
    await page.locator("[data-focus-key='provider-steam']").click();
    await expect.poll(() => currentHash(page)).toBe("#/store?category=short-sessions&provider=steam");

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");

    await page.goBack();
    await waitForPage(page, "store");

    expect(await currentHash(page)).toBe("#/store?category=short-sessions&provider=steam");
    await expect(page.locator(`${storeHost} .store-card`)).toHaveCount(3);
    await expect(page.locator("[data-focus-key='category-short-sessions']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator("[data-focus-key='provider-steam']")).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  test("navigating away and back restores scroll position and focus", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    // `.app-page--scroll` is the one scroll container: `.store-page` computes
    // `overflow-y: visible` and cannot hold a scroll offset at all, so the host
    // is both what gets scrolled and what has to be restored. How far it can
    // scroll depends on the viewport, so aim for 300px but settle for the end.
    const scrolled = await page.evaluate(() => {
      const host = document.getElementById("app-page-store")!;
      const target = Math.min(300, host.scrollHeight - host.clientHeight);
      host.scrollTop = target;
      return {
        target,
        host: host.scrollTop,
        root: document.querySelector<HTMLElement>(".store-page")!.scrollTop,
      };
    });
    expect(scrolled.target, "the Store must have somewhere to scroll to").toBeGreaterThan(0);
    expect(scrolled.host, "the Store host must be the scroll container").toBe(scrolled.target);
    expect(scrolled.root, ".store-page must not scroll independently").toBe(0);
    await page.waitForTimeout(200);

    const card = page.locator("[data-focus-key='game-steam:1145350']");
    await card.click();
    await waitForPage(page, "game");

    await page.locator(".gd-back").click();
    await waitForPage(page, "store");
    await page.waitForTimeout(400);

    const restored = await page.evaluate(() => ({
      scrollTop: document.getElementById("app-page-store")!.scrollTop,
      focusKey: (document.activeElement as HTMLElement | null)?.dataset?.focusKey ?? null,
    }));

    expect(
      restored.scrollTop,
      `the Store returned at ${restored.scrollTop}px instead of ${scrolled.target}px`,
    ).toBeCloseTo(scrolled.target, 0);
    expect(restored.focusKey).toBe("game-steam:1145350");
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
    await expect(page.locator(`${storeHost} .store-card__title`)).toHaveText("Hades II");
    await expect(host(page, "store")).toBeVisible();
  });
});
