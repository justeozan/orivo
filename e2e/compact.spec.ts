import { expect, test } from "@playwright/test";
import {
  documentOverflow,
  expectVisibleFocusRing,
  openRoute,
  tabUntil,
  topbarBox,
  waitForPage,
} from "./helpers";

/**
 * The scene on a phone held sideways — 914x411, the viewport an Android build
 * reports on a Pixel 8.
 *
 * This file runs under the `android-914` project and nowhere else
 * (playwright.config.ts), which is what keeps the desktop goldens out of it.
 * Everything here asks the same question in different places: can you still
 * reach it? A desktop-shaped scene does not shrink on a short screen, it
 * resolves to a negative height and silently drops what was inside.
 */

interface Rect {
  top: number;
  bottom: number;
  left: number;
  right: number;
  width: number;
  height: number;
}

async function rectOf(page: import("@playwright/test").Page, selector: string): Promise<Rect | null> {
  return page.evaluate((target) => {
    const node = document.querySelector(target);
    if (!node) return null;
    const { top, bottom, left, right, width, height } = node.getBoundingClientRect();
    return { top, bottom, left, right, width, height };
  }, selector);
}

async function viewport(page: import("@playwright/test").Page): Promise<{ width: number; height: number }> {
  return page.evaluate(() => ({ width: window.innerWidth, height: window.innerHeight }));
}

test.describe("the compact form factor", () => {
  test("is decided by height, and follows the device through a rotation", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    expect(await page.evaluate(() => document.documentElement.dataset.formFactor)).toBe("compact");

    // The two desktop viewports the suite already covers must keep the desktop
    // scene: this is the assertion that says "nothing above 560px changed".
    await page.setViewportSize({ width: 1040, height: 700 });
    expect(await page.evaluate(() => document.documentElement.dataset.formFactor)).toBeUndefined();
    await page.setViewportSize({ width: 1536, height: 1024 });
    expect(await page.evaluate(() => document.documentElement.dataset.formFactor)).toBeUndefined();

    // Back to the phone: Android hands the app a new viewport on rotation, and
    // an attribute set once at boot would be stale by now.
    await page.setViewportSize({ width: 914, height: 411 });
    expect(await page.evaluate(() => document.documentElement.dataset.formFactor)).toBe("compact");
  });
});

test.describe("the Library scene", () => {
  test.beforeEach(async ({ page }) => {
    await openRoute(page, "#/library", "library");
  });

  test("Play is on screen, under the topbar, and nothing covers it", async ({ page }) => {
    const hero = await rectOf(page, ".hero-content");
    const play = await rectOf(page, "#play-button");
    const topbar = await topbarBox(page);
    const { height } = await viewport(page);

    // The desktop scene resolves `.hero-content` to a negative height here, so
    // this is the check that the whole compact layout exists for.
    expect(hero!.height).toBeGreaterThan(0);
    expect(play!.height).toBeGreaterThan(0);
    expect(play!.top).toBeGreaterThanOrEqual(topbar.y + topbar.height);
    expect(play!.bottom).toBeLessThanOrEqual(height);

    // A box in the right place is not the same as a button you can press.
    const hit = await page.evaluate(() => {
      const button = document.querySelector("#play-button")!;
      const rect = button.getBoundingClientRect();
      const at = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
      return at === button || button.contains(at);
    });
    expect(hit).toBe(true);
  });

  test("reads top to bottom with no two bands overlapping", async ({ page }) => {
    const topbar = await topbarBox(page);
    const rail = await rectOf(page, ".recently-played");
    const bar = await rectOf(page, ".browse-bar");
    const { height } = await viewport(page);

    expect(topbar.y + topbar.height).toBeLessThanOrEqual(rail!.top);
    expect(rail!.bottom).toBeLessThanOrEqual(bar!.top);
    expect(bar!.bottom).toBeLessThanOrEqual(height);
  });

  test("keeps every part of the scene above the fold", async ({ page }) => {
    const { height } = await viewport(page);
    for (const selector of [
      ".hero-content",
      "#play-button",
      ".recently-played",
      ".browse-bar",
      "#game-cards .game-card",
    ]) {
      const rect = await rectOf(page, selector);
      expect(rect, selector).not.toBeNull();
      expect(rect!.bottom, selector).toBeLessThanOrEqual(height + 1);
    }
  });

  test("keeps the resting rail cards portrait rather than squashing them", async ({ page }) => {
    // The selected card is deliberately wide — it shows the landscape art.
    const card = await rectOf(page, "#game-cards .game-card:not(.is-selected)");
    expect(card!.height).toBeGreaterThan(card!.width);
  });

  test("offers finger-sized targets", async ({ page }) => {
    // `me` is behind the beta flag and hidden here, so it has no box to measure.
    const targets = [
      "[data-nav-page='library']",
      "[data-nav-page='store']",
      "[data-nav-page='settings']",
      "#notifications-button",
      "#library-menu-button",
      "#play-button",
      ".scene-arrow--next",
    ];
    for (const selector of targets) {
      const rect = await rectOf(page, selector);
      expect(rect, selector).not.toBeNull();
      expect(Math.min(rect!.width, rect!.height), selector).toBeGreaterThanOrEqual(44);
    }
  });

  test("still names the navigation once the labels are icons", async ({ page }) => {
    for (const name of ["Library", "Store", "Settings"]) {
      await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
    }
  });

  test("keeps the notification panel inside the window", async ({ page }) => {
    await page.click("#notifications-button");
    const panel = await rectOf(page, "#notifications-panel");
    const { width, height } = await viewport(page);
    expect(panel!.bottom).toBeLessThanOrEqual(height);
    expect(panel!.right).toBeLessThanOrEqual(width);
  });

  test("reaches the navigation and then the search by keyboard", async ({ page }) => {
    const nav = await tabUntil(page, (report) => report.className.includes("nav-link"));
    expect(nav.found, "Tab never reached the navigation").not.toBeNull();
    expectVisibleFocusRing(nav.found!);

    // Play is disabled in browser mode (nothing to launch), so the next proof
    // that the compact bar is still a tab order is the search field.
    const search = await tabUntil(page, (report) => report.tag === "INPUT" && report.label.startsWith("Search"));
    expect(search.found, "Tab never reached the search field").not.toBeNull();
  });
});

test.describe("every route", () => {
  const routes = [
    { hash: "#/library", name: "library" as const },
    { hash: "#/store", name: "store" as const },
    { hash: "#/games/steam%3A1245620", name: "game" as const },
    { hash: "#/me", name: "library" as const },
    { hash: "#/settings/general", name: "settings" as const },
  ];

  for (const route of routes) {
    test(`${route.hash} never scrolls sideways`, async ({ page }) => {
      await page.goto(`/${route.hash}`, { waitUntil: "domcontentloaded" });
      if (route.hash !== "#/me") await waitForPage(page, route.name);
      const overflow = await documentOverflow(page);
      expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth + 1);
    });
  }
});

test.describe("the other pages", () => {
  test("the Store shows portrait cards that start on screen", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    const card = await rectOf(page, ".store-card");
    const { height } = await viewport(page);
    expect(card!.height).toBeGreaterThan(card!.width);
    expect(card!.top).toBeLessThan(height);
  });

  test("game detail drops to a single column", async ({ page }) => {
    await openRoute(page, "#/games/steam%3A1245620", "game");
    const tracks = await page.evaluate(() => {
      const body = document.querySelector(".gd-body");
      return body ? getComputedStyle(body).gridTemplateColumns.split(/\s+/).length : 0;
    });
    expect(tracks).toBeLessThanOrEqual(2);
  });

  test("the welcome screen can be read to the end", async ({ page }) => {
    await page.goto("/?library=empty#/library", { waitUntil: "domcontentloaded" });
    await page.waitForSelector(".onboarding", { state: "visible" });
    const reachable = await page.evaluate(() => {
      const cta = document.querySelector<HTMLElement>(".onboarding__rows button, .onboarding button");
      if (!cta) return false;
      cta.scrollIntoView({ block: "nearest" });
      const rect = cta.getBoundingClientRect();
      return rect.bottom <= window.innerHeight + 1 && rect.top >= 0;
    });
    expect(reachable).toBe(true);
  });
});
