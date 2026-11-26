import { expect, test } from "@playwright/test";
import {
  describeCards,
  measureTopPadding,
  openRoute,
  selectorHeight,
  storeCardVisibility,
  topbarBox,
  topbarHeightVar,
  waitForImages,
} from "./helpers";

/**
 * Shell geometry: who owns the clearance under the floating topbar, who owns
 * the scrolling, and whether the Store rail actually lands on screen.
 *
 * The division of labour is the invariant. `.app-page--scroll` (src/styles.css)
 * owns *both* the `padding-top: var(--topbar-height)` clearance and the
 * `overflow-y: auto` scrolling; `.store-page` / `.gd-page` own neither. When
 * both halves added padding the pages started ~170px down, and when both
 * declared a height the host scrolled by exactly the topbar height with nothing
 * in that band. These specs measure each half separately so a regression names
 * the offender.
 */

/** How much room a page may spend under the topbar before content starts. */
const GUTTER_BUDGET = 24;

/** `--topbar-height` on a normal window. The bar's real bottom edge is 65px. */
const TOPBAR_HEIGHT = 76;
/** …and below 860px, where the search drops to a row of its own. */
const NARROW_TOPBAR_HEIGHT = 124;
/** Pixel slack for every geometry assertion, so the suite is not brittle. */
const TOL = 3;

test.describe("shell / page padding integration", () => {
  test("the shell owns the topbar clearance and no page adds a second one", async ({ page }) => {
    // Store and the game detail page opted out of this model when they moved
    // to a full-bleed hero under a floating topbar (see the dedicated
    // "full-bleed hero" describe block below) — only Settings and the 404
    // still clear the bar with host-side `padding-top`.
    for (const [hash, name, ownsClearance] of [
      ["#/settings/general", "settings", true],
      // The 404 is a short, centred empty state: its `clamp(56px, 13vh, 150px)`
      // is optical centring, not a second helping of topbar clearance, so only
      // the shell-side half of the invariant applies to it.
      ["#/nowhere", "not-found", false],
    ] as const) {
      await openRoute(page, hash, name);
      const probe = await measureTopPadding(page);
      const topbar = await topbarBox(page);

      expect(probe.topbarHeight, `${hash}: --topbar-height`).toBeCloseTo(TOPBAR_HEIGHT, 0);
      expect(probe.hostPaddingTop, `${hash}: ${probe.hostId} padding-top`).toBeCloseTo(
        TOPBAR_HEIGHT,
        0,
      );
      // The host is the scroll container, so the clearance and the scrolling
      // live on the same element.
      expect(probe.hostOverflowY, `${hash}: ${probe.hostId} overflow-y`).toBe("auto");
      // Content clears the bar's real bottom edge (65px).
      expect(probe.contentTop, `${hash}: content must clear the topbar`).toBeGreaterThanOrEqual(
        topbar.y + topbar.height,
      );
      if (!ownsClearance) continue;
      // …without stacking a second full topbar's worth of padding on the first.
      expect(
        probe.hostPaddingTop + probe.innerPaddingTop,
        `${hash} top offset: host ${probe.hostPaddingTop}px + ${probe.innerClass} ` +
          `${probe.innerPaddingTop}px for a ${probe.topbarHeight}px topbar`,
      ).toBeLessThanOrEqual(probe.topbarHeight + GUTTER_BUDGET);
    }
  });

  /**
   * The Store rebuild (`311a91f`) and the game detail rework (`01c8a8c`) both
   * moved to a "fit" layout: the host no longer scrolls or pads itself
   * (`overflow-y: hidden`, `padding-top: 0`), and the floating topbar sits
   * over a full-bleed hero instead of being cleared by it. The invariant that
   * survives is readability, not a padding value: the topbar never covers
   * anything a player needs to read, and nothing after the hero gets a second
   * helping of clearance.
   */
  test("the Store and the detail page float the topbar over a full-bleed hero", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    const storeTopbar = await topbarBox(page);
    const storeGeometry = await page.evaluate(() => {
      const host = document.getElementById("app-page-store")!;
      const hero = document.querySelector(".store-mast")!.getBoundingClientRect();
      return {
        hostPaddingTop: Number.parseFloat(getComputedStyle(host).paddingTop) || 0,
        hostOverflowY: getComputedStyle(host).overflowY,
        heroTop: hero.top,
      };
    });
    expect(storeGeometry.hostPaddingTop, "the Store host no longer owns a padding clearance").toBe(0);
    expect(storeGeometry.hostOverflowY, "the Store host is its page's scroll container").toBe("auto");
    expect(
      storeGeometry.heroTop,
      "the Store's masthead must still clear the topbar's real bottom edge",
    ).toBeGreaterThanOrEqual(storeTopbar.y + storeTopbar.height);

    await openRoute(page, "#/games/steam%3A1245620?from=store", "game");
    const detailTopbar = await topbarBox(page);
    const detailGeometry = await page.evaluate(() => {
      const host = document.getElementById("app-page-game")!;
      const hero = document.querySelector(".gd-hero")!.getBoundingClientRect();
      const title = document.querySelector(".gd-hero__title")!.getBoundingClientRect();
      const body = document.querySelector(".gd-body")!.getBoundingClientRect();
      return {
        hostOverflowY: getComputedStyle(host).overflowY,
        heroTop: hero.top,
        heroBottom: hero.bottom,
        titleTop: title.top,
        bodyTop: body.top,
      };
    });
    expect(detailGeometry.hostOverflowY, "the detail host no longer scrolls vertically").toBe("hidden");
    // The hero art is deliberately full-bleed under the floating bar…
    expect(detailGeometry.heroTop, "the detail hero art starts at the very top of the page").toBe(0);
    // …but the title it carries is placed low enough to never sit under it.
    expect(
      detailGeometry.titleTop,
      "the game title must clear the topbar's real bottom edge",
    ).toBeGreaterThanOrEqual(detailTopbar.y + detailTopbar.height);
    // And whatever follows the hero starts exactly where it ends — no gap, no
    // second padding stacked on top of the full-bleed art.
    expect(detailGeometry.bodyTop, "the game body must not add its own clearance below the hero").toBe(
      detailGeometry.heroBottom,
    );
  });

  test("Settings is not double-padded", async ({ page }) => {
    await openRoute(page, "#/settings/general", "settings");
    const probe = await measureTopPadding(page);
    const topbar = await topbarBox(page);

    expect(probe.hostPaddingTop).toBeCloseTo(probe.topbarHeight, 0);
    expect(
      probe.hostPaddingTop + probe.innerPaddingTop,
      `Settings top offset: host ${probe.hostPaddingTop}px + ${probe.innerClass} ${probe.innerPaddingTop}px`,
    ).toBeLessThanOrEqual(probe.topbarHeight + GUTTER_BUDGET);
    expect(probe.contentTop).toBeGreaterThanOrEqual(topbar.y + topbar.height);
  });

  test("below 860px the topbar grows and the Store hero still clears it", async ({ page }) => {
    // The search control leaves the bar and takes a row of its own, so the
    // hosts owe the bar more room. `--topbar-height` is the single source of
    // both; the Store no longer clears it with host padding (see the
    // full-bleed-hero test above), so the invariant that survives is that the
    // hero copy's own top position tracks the bar's real bottom edge.
    const original = page.viewportSize()!;
    try {
      await page.setViewportSize({ width: 820, height: 900 });
      await openRoute(page, "#/store", "store");
      let topbar = await topbarBox(page);
      let heroTop = await page.evaluate(() => document.querySelector(".store-mast")!.getBoundingClientRect().top);
      expect(await topbarHeightVar(page), "820px: --topbar-height").toBeCloseTo(NARROW_TOPBAR_HEIGHT, 0);
      expect(heroTop, "820px: the Store hero must clear the taller topbar").toBeGreaterThanOrEqual(
        topbar.y + topbar.height,
      );

      await page.setViewportSize({ width: 900, height: 900 });
      await openRoute(page, "#/store", "store");
      topbar = await topbarBox(page);
      heroTop = await page.evaluate(() => document.querySelector(".store-mast")!.getBoundingClientRect().top);
      expect(await topbarHeightVar(page), "900px: --topbar-height").toBeCloseTo(TOPBAR_HEIGHT, 0);
      expect(heroTop, "900px: the Store hero must clear the topbar").toBeGreaterThanOrEqual(
        topbar.y + topbar.height,
      );
    } finally {
      await page.setViewportSize(original);
    }
  });

  test("the scroll hosts do not add a phantom scroll band", async ({ page }) => {
    // The host is allowed — required, even — to scroll real content. What it may
    // never do is overshoot by exactly `--topbar-height`, which is the signature
    // of the page root sizing itself to `100svh` *inside* a host that is already
    // inset by that much: the last bandful of scroll is empty.
    const offenders: string[] = [];
    for (const [hash, name] of [
      ["#/store", "store"],
      ["#/games/steam%3A1245620?from=store", "game"],
      ["#/settings/general", "settings"],
      ["#/nowhere", "not-found"],
    ] as const) {
      await openRoute(page, hash, name);
      const probe = await measureTopPadding(page);
      const overshoot = probe.hostScrollHeight - probe.hostClientHeight;
      if (Math.abs(overshoot - probe.topbarHeight) <= TOL) {
        offenders.push(
          `${probe.hostId} scrolls ${overshoot}px — exactly its ${probe.topbarHeight}px topbar band ` +
            `(${probe.hostScrollHeight}/${probe.hostClientHeight})`,
        );
      }
    }

    expect(offenders, "a host that scrolls by exactly the topbar height scrolls nothing").toEqual([]);
  });

  test("the Store scrolls its shelf inside its own page, never sideways", async ({ page }) => {
    // The Store is a page of rows now (docs/design/store/): its host is the
    // one scroll container, as every page host is.
    await openRoute(page, "#/store", "store");
    await waitForImages(page);

    const geometry = await page.evaluate(() => {
      const root = document.querySelector<HTMLElement>("#app-page-store")!;
      const shelf = document.querySelector<HTMLElement>("#app-page-store .store-shelf")!;
      return {
        overflowX: getComputedStyle(root).overflowX,
        overflowY: getComputedStyle(root).overflowY,
        scrollHeight: root.scrollHeight,
        clientHeight: root.clientHeight,
        shelfScrollWidth: shelf.scrollWidth,
        shelfClientWidth: shelf.clientWidth,
      };
    });
    expect(geometry.overflowY).toBe("auto");
    expect(geometry.scrollHeight, "the shelf runs past one screen").toBeGreaterThan(geometry.clientHeight);
    // Nothing scrolls sideways, and the shelf itself fits its column.
    expect(geometry.overflowX).toBe("hidden");
    expect(geometry.shelfScrollWidth, "no row runs past its column").toBeLessThanOrEqual(geometry.shelfClientWidth + TOL);
  });

  test("the masthead and the first row start above the fold", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await waitForImages(page);

    const mastTop = await page.evaluate(() => document.querySelector(".store-mast")!.getBoundingClientRect().top);
    const rail = await storeCardVisibility(page);

    expect(mastTop, "the Store masthead must be visible without scrolling").toBeLessThan(
      rail.viewport.height * 0.35,
    );
    expect(rail.cards.length).toBeGreaterThan(0);
    expect(rail.cards[0].top, "the first Store card must start above the fold").toBeLessThan(
      rail.viewport.height,
    );
  });
});

test.describe("Store card grid at the acceptance sizes", () => {
  test("the first row is whole and on screen, measured against the viewport", async ({ page }, testInfo) => {
    // Rows of three large cards, two below 900px: the cards grow with the
    // window instead of the row gaining columns, which is what a television
    // needs. Every card of the first row is inside the viewport's width.
    await openRoute(page, "#/store", "store");
    await waitForImages(page);

    const rail = await storeCardVisibility(page);
    expect(rail.cards.length).toBeGreaterThan(0);

    const firstRowTop = rail.cards[0].top;
    const row = rail.cards.filter((card) => Math.abs(card.top - firstRowTop) <= TOL);
    const columns = rail.viewport.width > 900 ? 3 : rail.viewport.width > 600 ? 2 : 1;

    testInfo.annotations.push({
      type: "store-cards",
      description:
        `${testInfo.project.name}: row=${row.length} card=${row[0].width}x${row[0].height} ` +
        `top=${row[0].top} bottom=${row[0].bottom}`,
    });

    expect(row.length, `the first row holds ${columns} cards:\n${describeCards(rail)}`).toBe(columns);
    for (const card of row) {
      expect(card.left, `${card.id} left`).toBeGreaterThanOrEqual(0);
      expect(card.right, `${card.id} right`).toBeLessThanOrEqual(rail.viewport.width);
      expect(card.top, `${card.id} must start above the fold`).toBeLessThan(rail.viewport.height);
    }
  });

  test("the shell topbar height variable matches the rendered topbar", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    const declared = await topbarHeightVar(page);
    const box = await topbarBox(page);

    expect(declared).toBeCloseTo(TOPBAR_HEIGHT, 0);
    // The bar is inset inside its 76px band; its real bottom edge is 65px.
    expect(box.y + box.height, "the bar must stay inside the band it declares").toBeLessThanOrEqual(
      declared,
    );
    expect(box.y + box.height).toBeCloseTo(65, 0);
  });
});

test.describe("the shell canvas fills the window", () => {
  // REGRESSION — `.selector` used to be capped at `height: 760px` below 860px
  // (styles.css:1183). Every page host is `inset: 0` of that box, so a taller
  // window stranded everything past 760px and shortened each internal scroll
  // container by the same amount. The cap is now `min-height: max(760px, 100svh)`.
  const SIZES = [
    { width: 760, height: 900 },
    { width: 900, height: 900 },
    { width: 1536, height: 1024 },
    { width: 1040, height: 700 },
  ];

  test("`.selector` is exactly as tall as the viewport at every size", async ({ page }) => {
    const original = page.viewportSize()!;
    try {
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await openRoute(page, "#/library", "library");
        const shell = await selectorHeight(page);
        expect(
          shell.height,
          `${size.width}x${size.height}: .selector is ${shell.height}px in a ${shell.viewportHeight}px window`,
        ).toBeCloseTo(shell.viewportHeight, 0);
        expect(shell.viewportHeight).toBe(size.height);
      }
    } finally {
      await page.setViewportSize(original);
    }
  });

  test("the page hosts inherit the full canvas height", async ({ page }) => {
    const original = page.viewportSize()!;
    try {
      for (const size of SIZES) {
        await page.setViewportSize(size);
        await openRoute(page, "#/settings/general", "settings");
        const probe = await page.evaluate(() => {
          const host = document.getElementById("app-page-settings")!;
          return { height: Math.round(host.getBoundingClientRect().height), viewport: window.innerHeight };
        });
        expect(
          probe.height,
          `${size.width}x${size.height}: #app-page-settings is ${probe.height}px of ${probe.viewport}px`,
        ).toBeCloseTo(probe.viewport, 0);
      }
    } finally {
      await page.setViewportSize(original);
    }
  });
});
