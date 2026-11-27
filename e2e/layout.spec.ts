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

/** `--topbar-height` on a normal window. */
const TOPBAR_HEIGHT = 76;
/** …and below 860px: 18px from the top, the 52px capsule, 12 of air. */
const NARROW_TOPBAR_HEIGHT = 82;

/**
 * Where the capsule's bottom edge falls on a desktop window: 16px from the
 * top, then its round controls (2.75vw, from 40 to 48px), 5px of padding on
 * each side and a 1px rim.
 */
const capsuleBottom = (width: number): number =>
  16 + Math.min(48, Math.max(40, width * 0.0275)) + 12;
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

  test("below 860px the topbar keeps to one row and the Store hero still clears it", async ({ page }) => {
    // The capsule holds the search on its own row, so a narrow window only
    // moves the bar down a little. `--topbar-height` is the single source of
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
    // The capsule floats inside its 76px band, and grows with the window.
    expect(box.y + box.height, "the bar must stay inside the band it declares").toBeLessThanOrEqual(
      declared,
    );
    expect(box.y + box.height).toBeCloseTo(capsuleBottom(page.viewportSize()!.width), 0);
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

test.describe("large screens and televisions", () => {
  const STEPS = [
    { width: 1536, height: 1024 },
    { width: 1920, height: 1080 },
    { width: 2560, height: 1440 },
    { width: 3840, height: 2160 },
  ];

  test("the rail's cards grow at every step, and the scene stays aligned", async ({ page }) => {
    const original = page.viewportSize()!;
    test.skip(original.width !== 1536, "measured once, from the desktop project");
    let previous = 0;
    for (const size of STEPS) {
      await page.setViewportSize(size);
      await openRoute(page, "#/library", "library");
      await waitForImages(page);
      const probe = await page.evaluate(() => {
        const card = document.querySelector<HTMLElement>(".game-card:not(.is-selected)");
        const play = document.querySelector<HTMLElement>(".play-button");
        const first = document.querySelector<HTMLElement>(".game-card");
        return {
          width: card?.getBoundingClientRect().width ?? 0,
          height: card?.getBoundingClientRect().height ?? 0,
          playLeft: play?.getBoundingClientRect().left ?? 0,
          firstLeft: first?.getBoundingClientRect().left ?? 0,
          scrollWidth: document.documentElement.scrollWidth,
        };
      });
      const label = `${size.width}x${size.height}`;
      expect(probe.width, `${label} card width`).toBeGreaterThan(previous);
      expect(probe.scrollWidth, `${label} horizontal overflow`).toBeLessThanOrEqual(size.width);
      expect(Math.abs(probe.playLeft - probe.firstLeft), `${label} Play and first card share a left edge`)
        .toBeLessThan(16 * (size.width / 1536));
      expect((await selectorHeight(page)).height, `${label} .selector`).toBeCloseTo(size.height, 0);
      previous = probe.width;
    }
    await page.setViewportSize(original);
  });

  test("a wide but short window keeps the base card size", async ({ page }) => {
    test.skip(page.viewportSize()!.width !== 1536, "measured once, from the desktop project");
    await page.setViewportSize({ width: 2560, height: 800 });
    await openRoute(page, "#/library", "library");
    const width = await page.evaluate(() =>
      parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--rail-card-width")),
    );
    expect(width).toBe(148);
  });

  test("the Store grows its cards and its grid gains columns on a television", async ({ page }) => {
    test.skip(page.viewportSize()!.width !== 1536, "measured once, from the desktop project");
    const probes: { card: number; columns: number }[] = [];
    for (const size of [STEPS[0], STEPS[3]]) {
      await page.setViewportSize(size);
      await openRoute(page, "#/store", "store");
      await waitForImages(page);
      const rail = await storeCardVisibility(page);
      probes.push({
        card: rail.cards[0].width,
        columns: await page.evaluate(
          () => getComputedStyle(document.querySelector(".store-grid--grid")!).gridTemplateColumns.split(" ").length,
        ),
      });
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(size.width);
    }
    expect(probes[1].card).toBeGreaterThan(probes[0].card);
    expect(probes[1].columns).toBeGreaterThan(probes[0].columns);
  });
});
