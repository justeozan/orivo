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
    for (const [hash, name, ownsClearance] of [
      ["#/store", "store", true],
      ["#/games/steam%3A1245620?from=store", "game", true],
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

  test("the Store page is not double-padded", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    const probe = await measureTopPadding(page);

    expect(probe.innerTag, "the Store page root").toBe("MAIN");
    expect(probe.innerClass).toContain("store-page");
    // The two numbers that used to be 92 and ~80.
    expect(probe.hostPaddingTop, "host clearance").toBeCloseTo(TOPBAR_HEIGHT, 0);
    expect(probe.innerPaddingTop, ".store-page must add no top padding of its own").toBe(0);
    expect(probe.innerOverflowY, ".store-page must not be a scroll container").toBe("visible");
    expect(
      probe.hostPaddingTop + probe.innerPaddingTop,
      `Store top offset: host ${probe.hostPaddingTop}px + ${probe.innerClass} ${probe.innerPaddingTop}px`,
    ).toBeLessThanOrEqual(probe.topbarHeight + GUTTER_BUDGET);
  });

  test("the game detail page is not double-padded", async ({ page }) => {
    await openRoute(page, "#/games/steam%3A1245620?from=store", "game");
    const probe = await measureTopPadding(page);

    expect(probe.innerTag, "the detail page root").toBe("MAIN");
    expect(probe.innerClass).toContain("gd-page");
    expect(probe.hostPaddingTop, "host clearance").toBeCloseTo(TOPBAR_HEIGHT, 0);
    expect(probe.innerPaddingTop, ".gd-page must add no top padding of its own").toBe(0);
    expect(probe.innerOverflowY, ".gd-page must not be a scroll container").toBe("visible");
    expect(
      probe.hostPaddingTop + probe.innerPaddingTop,
      `Detail top offset: host ${probe.hostPaddingTop}px + ${probe.innerClass} ${probe.innerPaddingTop}px`,
    ).toBeLessThanOrEqual(probe.topbarHeight + GUTTER_BUDGET);
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

  test("below 860px the clearance grows with the topbar, and only there", async ({ page }) => {
    // The search control leaves the bar and takes a row of its own, so the
    // hosts owe the bar more room. The variable is the single source of both.
    const original = page.viewportSize()!;
    try {
      await page.setViewportSize({ width: 820, height: 900 });
      await openRoute(page, "#/store", "store");
      const narrow = await measureTopPadding(page);
      expect(narrow.topbarHeight, "820px: --topbar-height").toBeCloseTo(NARROW_TOPBAR_HEIGHT, 0);
      expect(narrow.hostPaddingTop, "820px: host padding-top").toBeCloseTo(NARROW_TOPBAR_HEIGHT, 0);
      expect(narrow.innerPaddingTop, "820px: .store-page still adds nothing").toBe(0);

      await page.setViewportSize({ width: 900, height: 900 });
      await openRoute(page, "#/store", "store");
      const wide = await measureTopPadding(page);
      expect(wide.topbarHeight, "900px: --topbar-height").toBeCloseTo(TOPBAR_HEIGHT, 0);
      expect(wide.hostPaddingTop, "900px: host padding-top").toBeCloseTo(TOPBAR_HEIGHT, 0);
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

  test("the Store host scrolls real content, not an empty band", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await waitForImages(page);
    const probe = await measureTopPadding(page);

    // 1536x1024 measures 1461/1024 — the second card row, not padding.
    expect(probe.hostScrollHeight).toBeGreaterThan(probe.hostClientHeight);
    const overshoot = probe.hostScrollHeight - probe.hostClientHeight;
    expect(
      Math.abs(overshoot - probe.topbarHeight),
      `Store host overshoot ${overshoot}px vs topbar ${probe.topbarHeight}px`,
    ).toBeGreaterThan(TOL);

    // Scrolling to the bottom must reveal card content, not blank space.
    const bottom = await page.evaluate(() => {
      const host = document.getElementById("app-page-store")!;
      host.scrollTop = host.scrollHeight;
      const cards = [...host.querySelectorAll<HTMLElement>(".store-card:not(.store-card--skeleton)")];
      const lowest = cards.reduce(
        (best, card) => Math.max(best, card.getBoundingClientRect().bottom),
        Number.NEGATIVE_INFINITY,
      );
      return { lowest: Math.round(lowest), viewportHeight: window.innerHeight, scrollTop: host.scrollTop };
    });
    expect(
      bottom.viewportHeight - bottom.lowest,
      `scrolled to the end, ${bottom.viewportHeight - bottom.lowest}px below the last card is empty`,
    ).toBeLessThan(TOPBAR_HEIGHT);
  });

  test("the hero and the first card start above the fold", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await waitForImages(page);

    const probe = await measureTopPadding(page);
    const rail = await storeCardVisibility(page);

    expect(probe.contentTop, "the Store hero must be visible without scrolling").toBeLessThan(
      rail.viewport.height * 0.35,
    );
    expect(rail.cards.length).toBeGreaterThan(0);
    expect(rail.cards[0].top, "the first Store card must start above the fold").toBeLessThan(
      rail.viewport.height,
    );
  });
});

test.describe("Store card grid at the acceptance sizes", () => {
  test("the cards are on screen, measured against the viewport", async ({ page }, testInfo) => {
    await openRoute(page, "#/store", "store");
    await waitForImages(page);

    const rail = await storeCardVisibility(page);
    expect(rail.cards.length).toBeGreaterThan(0);

    const firstRowTop = rail.cards[0].top;
    const firstRow = rail.cards.filter((card) => Math.abs(card.top - firstRowTop) <= TOL);
    const fullyVisible = rail.cards.filter((card) => card.fullyVisible);

    testInfo.annotations.push({
      type: "store-cards",
      description:
        `${testInfo.project.name}: row=${firstRow.length} fullyVisible=${fullyVisible.length} ` +
        `card=${firstRow[0].width}x${firstRow[0].height} top=${firstRow[0].top} bottom=${firstRow[0].bottom}`,
    });

    for (const card of firstRow) {
      expect(card.height, `card ${card.id} must be portrait`).toBeGreaterThan(card.width);
    }

    if (rail.viewport.width >= 1536) {
      // 1536x1024 — five across, and all five whole inside the window.
      expect(firstRow[0].top, "first card top").toBeGreaterThanOrEqual(481 - TOL);
      expect(firstRow[0].top, "first card top").toBeLessThanOrEqual(481 + TOL);
      expect(firstRow[0].bottom, "first card bottom").toBeGreaterThanOrEqual(944 - TOL);
      expect(firstRow[0].bottom, "first card bottom").toBeLessThanOrEqual(944 + TOL);
      expect(firstRow[0].height, "first card height").toBeGreaterThanOrEqual(463 - TOL);
      expect(firstRow[0].height, "first card height").toBeLessThanOrEqual(463 + TOL);

      expect(firstRow.length, "1536x1024 must lay out five cards across").toBe(5);
      expect(
        fullyVisible.length,
        `1536x1024 must show five whole cards inside the viewport, got ${fullyVisible.length}:\n` +
          describeCards(rail),
      ).toBe(5);
      for (const card of fullyVisible) {
        expect(card.top, `${card.id} top`).toBeGreaterThanOrEqual(0);
        expect(card.bottom, `${card.id} bottom`).toBeLessThanOrEqual(rail.viewport.height);
      }
    } else {
      // 1040x700 — a 463px-tall card cannot be whole in a 700px window under a
      // 76px topbar, so the bar is "essentially all of it": >=90% of each card's
      // own height above the fold, for at least three cards, plus a peek.
      expect(firstRow[0].top, "first card top").toBeGreaterThanOrEqual(361 - TOL);
      expect(firstRow[0].top, "first card top").toBeLessThanOrEqual(361 + TOL);
      expect(firstRow[0].bottom, "first card bottom").toBeGreaterThanOrEqual(713 - TOL);
      expect(firstRow[0].bottom, "first card bottom").toBeLessThanOrEqual(713 + TOL);
      expect(firstRow[0].height, "first card height").toBeGreaterThanOrEqual(352 - TOL);
      expect(firstRow[0].height, "first card height").toBeLessThanOrEqual(352 + TOL);
      expect(
        firstRow[0].verticalVisibleRatio,
        `first card is only ${Math.round(firstRow[0].verticalVisibleRatio * 100)}% above the fold`,
      ).toBeGreaterThanOrEqual(0.9);

      const mostlyVisible = firstRow.filter(
        (card) => card.horizontallyInside && card.verticalVisibleRatio >= 0.9,
      );
      expect(
        mostlyVisible.length,
        `1040x700 must show at least three cards with >=90% of their height above the fold:\n` +
          describeCards(rail),
      ).toBeGreaterThanOrEqual(3);

      const peek = firstRow.filter((card) => card.peeking);
      expect(
        peek.length,
        `1040x700 must reveal a horizontal peek of the next card so the rail reads as scrollable:\n` +
          describeCards(rail),
      ).toBeGreaterThan(0);
      expect(peek[0].id, "the peek is the card after the three whole ones").toBe(
        firstRow[mostlyVisible.length].id,
      );
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
