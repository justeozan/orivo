import { expect, test, type TestInfo } from "@playwright/test";
import {
  documentOverflow,
  openRoute,
  tabUntil,
  topbarBox,
  waitForPage,
} from "./helpers";

/**
 * The scene on a phone held sideways — 914x411, the viewport an Android build
 * reports on a Pixel 8; 874x402, an iPhone 18 Pro with its landscape safe
 * area; 667x375, an iPhone SE.
 *
 * This file runs under the `android-914`, `iphone-874` and `iphone-667`
 * projects and nowhere else (playwright.config.ts), which is what keeps the
 * desktop goldens out of it. Everything here asks the same question in
 * different places: can you still reach it? A desktop-shaped scene does not
 * shrink on a short screen, it resolves to a negative height and silently
 * drops what was inside.
 */

interface SafeArea {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

const NO_SAFE_AREA: SafeArea = { top: 0, right: 0, bottom: 0, left: 0 };

/** The insets a project stands for — an iPhone's — or none. */
function safeAreaOf(testInfo: TestInfo): SafeArea {
  return (testInfo.project.metadata.safeAreaInsets as SafeArea | undefined) ?? NO_SAFE_AREA;
}

/**
 * Chromium has no device descriptor that carries a safe area, so a project
 * that stands for an iPhone names its insets and they are applied here, before
 * any page loads. `env(safe-area-inset-*)` then resolves as it does in
 * WKWebView, and the Pixel and the SE keep the zeros they really have.
 */
test.beforeEach(async ({ page }, testInfo) => {
  const insets = safeAreaOf(testInfo);
  if (insets === NO_SAFE_AREA) return;
  const session = await page.context().newCDPSession(page);
  await session.send("Emulation.setSafeAreaInsetsOverride", { insets });
});

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

/** Waits out every transition in flight, so a box is measured where it lands. */
async function settle(page: import("@playwright/test").Page): Promise<void> {
  await page.evaluate(() =>
    Promise.all(
      document
        .getAnimations()
        .filter((animation) => animation.effect?.getComputedTiming().iterations !== Infinity)
        .map((animation) => animation.finished.catch(() => undefined)),
    ),
  );
}

test.describe("the compact form factor", () => {
  test("is decided by height, and follows the device through a rotation", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    expect(await page.evaluate(() => document.documentElement.dataset.formFactor)).toBe("compact");

    // The two desktop viewports the suite already covers must keep the desktop
    // scene: this is the assertion that says "nothing above 560px changed".
    // `applyFormFactor()` (src/form-factor.ts) reflects the attribute from a
    // `matchMedia` "change" event, which fires asynchronously relative to
    // `setViewportSize` — under load (a busy CI runner, several suites
    // sharing one machine) the read can outrace the listener, so each check
    // polls instead of reading the attribute the instant the resize resolves.
    await page.setViewportSize({ width: 1040, height: 700 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.dataset.formFactor))
      .toBeUndefined();
    await page.setViewportSize({ width: 1536, height: 1024 });
    await expect
      .poll(() => page.evaluate(() => document.documentElement.dataset.formFactor))
      .toBeUndefined();

    // Back to the phone: Android hands the app a new viewport on rotation, and
    // an attribute set once at boot would be stale by now.
    await page.setViewportSize(test.info().project.use.viewport!);
    await expect
      .poll(() => page.evaluate(() => document.documentElement.dataset.formFactor))
      .toBe("compact");
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

  test("reads top to bottom with no two bands overlapping", async ({ page }, testInfo) => {
    const topbar = await topbarBox(page);
    const rail = await rectOf(page, ".recently-played");
    const bar = await rectOf(page, ".browse-bar");
    const { height } = await viewport(page);

    expect(topbar.y + topbar.height).toBeLessThanOrEqual(rail!.top);
    // On an iPhone the bar stands on the home indicator, 21px up, and the
    // rail has to rise with it rather than keep a fixed 40px.
    expect(rail!.bottom).toBeLessThanOrEqual(bar!.top);
    expect(bar!.bottom).toBeLessThanOrEqual(height - safeAreaOf(testInfo).bottom);
  });

  test("keeps the scene inside the safe area", async ({ page }, testInfo) => {
    const insets = safeAreaOf(testInfo);
    const { width, height } = await viewport(page);
    for (const selector of [".topbar", ".hero-content", ".browse-bar"]) {
      const rect = await rectOf(page, selector);
      expect(rect!.left, selector).toBeGreaterThanOrEqual(insets.left);
      expect(rect!.right, selector).toBeLessThanOrEqual(width - insets.right);
      expect(rect!.bottom, selector).toBeLessThanOrEqual(height - insets.bottom);
    }
    // The rail runs off the right edge on purpose; it starts inside the left.
    expect((await rectOf(page, ".recently-played"))!.left).toBeGreaterThanOrEqual(insets.left);
  });

  test("shows the whole title, on the shortest screen too", async ({ page }) => {
    const titleFits = () =>
      page.evaluate(() => {
        const title = document.querySelector("#hero-title")!;
        const fontSize = Number.parseFloat(getComputedStyle(title).fontSize);
        return title.getBoundingClientRect().height >= fontSize * 0.9;
      });
    expect(await titleFits()).toBe(true);

    // A small Android phone. A 128px rail left its hero 74px for a title and
    // Play, and `.hero-identity` clipped the title to a 6px sliver: the cards
    // are what give way now.
    await page.setViewportSize({ width: 800, height: 360 });
    await expect.poll(titleFits).toBe(true);
    const rail = await rectOf(page, ".recently-played");
    const bar = await rectOf(page, ".browse-bar");
    expect(rail!.bottom).toBeLessThanOrEqual(bar!.top);
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

  test("brings the cards down a size without changing their shape", async ({ page }) => {
    const card = await rectOf(page, "#game-cards .game-card:not(.is-selected)");
    expect(card!.height).toBeLessThanOrEqual(100);
    // The desktop card's proportions: .672 upright, 2.5:1 once selected.
    expect(card!.width / card!.height).toBeCloseTo(0.672, 2);
    await expect
      .poll(async () => (await rectOf(page, "#game-cards .game-card.is-selected"))!.width)
      .toBeCloseTo(card!.height * 2.5, 0);
  });

  test("offers finger-sized targets", async ({ page }) => {
    // The navigation waits behind the Orivo mark on a phone, so it is measured
    // once the mark has brought it out. `me` is behind the beta flag and hidden
    // here, so it has no box to measure.
    await page.click("#library-menu-button");
    await settle(page);
    const targets = [
      "#library-menu-button",
      "[data-nav-page='library']",
      "[data-nav-page='store']",
      "[data-nav-page='settings']",
      "#notifications-button",
    ];
    for (const selector of targets) {
      const rect = await rectOf(page, selector);
      expect(rect, selector).not.toBeNull();
      expect(Math.min(rect!.width, rect!.height), selector).toBeGreaterThanOrEqual(44);
    }
    // Play comes down a size on a phone and stays a wide pill for a thumb.
    const play = await rectOf(page, "#play-button");
    expect(play!.height).toBeGreaterThanOrEqual(36);
    expect(play!.width).toBeGreaterThanOrEqual(play!.height * 2);
  });

  test("still names the navigation once the labels are icons", async ({ page }) => {
    await page.click("#library-menu-button");
    for (const name of ["Library", "Store", "Settings"]) {
      await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
    }
  });

  test("keeps the navigation behind the Orivo mark, and drops the search and the arrows", async ({ page }) => {
    expect(await page.evaluate(() => document.documentElement.dataset.device)).toBe("phone");
    const pills = ["Library", "Store", "Settings"].map((name) =>
      page.getByRole("button", { name, exact: true }),
    );
    for (const pill of pills) await expect(pill).toBeHidden();
    await expect(page.locator("#topbar-search-control")).toBeHidden();
    await expect(page.locator("#previous-game")).toBeHidden();
    await expect(page.locator("#next-game")).toBeHidden();

    // The mark brings them out with its menu, and a press on the wallpaper
    // puts them all away again.
    await page.click("#library-menu-button");
    await expect(page.locator("#library-source-menu")).toBeVisible();
    for (const pill of pills) await expect(pill).toBeVisible();
    const { width, height } = await viewport(page);
    await page.mouse.click(width * 0.75, height * 0.3);
    await expect(page.locator("#library-source-menu")).toBeHidden();
    for (const pill of pills) await expect(pill).toBeHidden();

    // A pill is pressed like any other: the finger landing on it does not
    // close the menu out from under it, and the page it opens starts clean.
    await page.click("#library-menu-button");
    await pills[1].click();
    await expect(page).toHaveURL(/#\/store/);
    await expect(page.locator("#library-source-menu")).toBeHidden();
    for (const pill of pills) await expect(pill).toBeHidden();
  });

  test("keeps the notification panel inside the window", async ({ page }) => {
    await page.click("#notifications-button");
    const panel = await rectOf(page, "#notifications-panel");
    const { width, height } = await viewport(page);
    expect(panel!.bottom).toBeLessThanOrEqual(height);
    expect(panel!.right).toBeLessThanOrEqual(width);
  });
});

test.describe("a desktop window dragged as short", () => {
  // The same height with a mouse in hand: compact, but not a phone. It keeps
  // the bar a keyboard walks and the search.
  test("keeps its navigation and its search, in tab order", async ({ browser }, testInfo) => {
    // A context made here inherits the project's phone emulation unless told
    // otherwise, touch included.
    const context = await browser.newContext({
      viewport: testInfo.project.use.viewport!,
      deviceScaleFactor: 1,
      isMobile: false,
      hasTouch: false,
    });
    const page = await context.newPage();
    try {
      await openRoute(page, "#/library", "library");
      const root = await page.evaluate(() => ({ ...document.documentElement.dataset }));
      expect(root.formFactor).toBe("compact");
      expect(root.device).toBeUndefined();
      for (const name of ["Library", "Store", "Settings"]) {
        await expect(page.getByRole("button", { name, exact: true })).toBeVisible();
      }
      await expect(page.locator("#topbar-search-control")).toBeVisible();

      const nav = await tabUntil(page, (report) => report.className.includes("nav-link"));
      expect(nav.found, "Tab never reached the navigation").not.toBeNull();
      // Play is disabled in browser mode (nothing to launch), so the next proof
      // that the compact bar is still a tab order is the search field.
      const search = await tabUntil(page, (report) => report.tag === "INPUT" && report.label.startsWith("Search"));
      expect(search.found, "Tab never reached the search field").not.toBeNull();
    } finally {
      await context.close();
    }
  });
});

test.describe("every route", () => {
  const routes = [
    { hash: "#/library", name: "library" as const },
    { hash: "#/store", name: "store" as const },
    { hash: "#/games/steam%3A1245620", name: "game" as const },
    { hash: "#/me", name: "library" as const },
    { hash: "#/settings/general", name: "settings" as const },
    { hash: "#/settings/plugins", name: "settings" as const },
  ];

  for (const route of routes) {
    test(`${route.hash} never scrolls sideways`, async ({ page }) => {
      await page.goto(`/${route.hash}`, { waitUntil: "domcontentloaded" });
      if (route.hash !== "#/me") await waitForPage(page, route.name);
      const overflow = await documentOverflow(page);
      expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth + 1);
    });
  }

  test("the Third-party runners panel stays inside the screen", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");
    // Reached with no click at all: the panel is inline with the browser.
    await expect(page.locator("#runners-panel")).toBeVisible();
    await expect(page.locator("#runners-panel-body")).toContainText(
      "Install a runner plugin to add an emulator",
    );
    const overflow = await documentOverflow(page);
    expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth + 1);
  });
});

test.describe("the other pages", () => {
  test("a deep link to #/me with the beta off lands on the Library alone", async ({ page }) => {
    await page.goto("/#/me");
    // Me is a beta surface: the gate walks the deep link back to the Library,
    // and Me's own mount must not put it back on top once it finishes.
    await expect(page).toHaveURL(/#\/library$/);
    await expect(page.locator("#app-page-me .me-page")).toBeAttached();
    await expect(page.locator("#app-page-me")).toBeHidden();
    await expect(page.locator("#app-page-library")).toBeVisible();
  });

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

  test("the welcome screen offers its first step without a scroll", async ({ page }) => {
    await page.goto("/?library=empty#/library", { waitUntil: "domcontentloaded" });
    await page.waitForSelector("#onboarding-panel", { state: "visible" });
    // The arrival slides the panel in; measure where it lands, not mid-flight.
    await page.evaluate(() =>
      Promise.all(
        document
          .getAnimations()
          .filter((animation) => animation.effect?.getComputedTiming().iterations !== Infinity)
          .map((animation) => animation.finished.catch(() => undefined)),
      ),
    );
    // Stacked under the copy below 880px of width, the panel started below
    // the fold of every phone narrower than that.
    const cta = await rectOf(page, "#onboarding-panel .onboarding__rows button");
    const { height } = await viewport(page);
    expect(cta!.bottom).toBeLessThanOrEqual(height);
  });

  test("the Store and the game page keep their copy off the screen's edges", async ({ page }, testInfo) => {
    const insets = safeAreaOf(testInfo);
    await openRoute(page, "#/store", "store");
    for (const selector of [".store-hero__title", ".store-card"]) {
      expect((await rectOf(page, selector))!.left, selector).toBeGreaterThanOrEqual(insets.left);
    }
    await openRoute(page, "#/games/steam%3A1245620", "game");
    expect((await rectOf(page, ".gd-hero__title"))!.left).toBeGreaterThanOrEqual(insets.left);
  });

  test("the Store's price line stays clear of the shelf", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    // Any card that carries a real price previews it under the hero's action.
    const priced = page.locator(".store-card:has(.store-card__price)").first();
    test.skip((await priced.count()) === 0, "the catalogue carries no price to preview");
    await priced.hover();
    await expect(page.locator(".store-hero__offer")).not.toHaveAttribute("hidden", "");
    const offer = await rectOf(page, ".store-hero__offer");
    const shelf = await rectOf(page, ".store-shelf");
    expect(offer!.bottom).toBeLessThanOrEqual(shelf!.top);
  });

  test("the Store's chips scroll inside their bars, on the widest phone too", async ({ page }) => {
    // An iPhone Pro Max: wider than the 940px at which the bars start
    // scrolling, still too narrow for both of them, and the page was pushed
    // sideways instead.
    await page.setViewportSize({ width: 956, height: 440 });
    await openRoute(page, "#/store", "store");
    const overflow = await page.evaluate(() => {
      const host = document.querySelector<HTMLElement>("#app-page-store")!;
      return { scrollWidth: host.scrollWidth, clientWidth: host.clientWidth };
    });
    expect(overflow.scrollWidth).toBeLessThanOrEqual(overflow.clientWidth + 1);
  });

  test("game detail keeps its back pill above the title", async ({ page }) => {
    await openRoute(page, "#/games/steam%3A1245620", "game");
    const back = await rectOf(page, ".gd-hero .gd-back");
    const title = await rectOf(page, ".gd-hero__title");
    expect(back!.bottom).toBeLessThanOrEqual(title!.top + 1);
  });

  test("game detail opens its menu below the button, clear of the topbar", async ({ page }) => {
    await openRoute(page, "#/games/steam%3A1245620", "game");
    await page.click(".gd-more");
    const menu = await rectOf(page, ".gd-menu");
    const more = await rectOf(page, ".gd-more");
    const topbar = await topbarBox(page);
    expect(menu!.top).toBeGreaterThanOrEqual(more!.bottom);
    expect(menu!.top).toBeGreaterThanOrEqual(topbar.y + topbar.height);
  });

  test("the Store and the game page put a scrim under the topbar once they scroll", async ({ page }) => {
    const scrim = () =>
      page.evaluate(() => getComputedStyle(document.querySelector("header.topbar")!, "::before").opacity);
    const pages = [
      { hash: "#/store", name: "store", host: "#app-page-store" },
      { hash: "#/games/steam%3A1245620", name: "game", host: "#app-page-game" },
    ] as const;
    for (const { hash, name, host } of pages) {
      await openRoute(page, hash, name);
      // At rest the art still reaches the top edge. Polled: arriving from a
      // scrolled Store, the scrim is still fading out over its 160ms.
      await expect.poll(scrim, { message: name }).toBe("0");
      await page.evaluate((selector) => document.querySelector(selector)!.scrollBy(0, 160), host);
      await expect.poll(scrim, { message: name }).toBe("1");
    }
  });
});
