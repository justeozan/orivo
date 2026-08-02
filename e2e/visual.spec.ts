import { expect, test, type Locator, type Page } from "@playwright/test";
import {
  boxDelta,
  documentOverflow,
  expectVisibleFocusRing,
  openRoute,
  tabUntil,
  topbarBox,
  waitForImages,
  type PageName,
} from "./helpers";

interface GoldenCase {
  /** Golden file name: e2e/__screenshots__/<project>/<golden>.png */
  golden: string;
  hash: string;
  name: PageName;
  /** Regions whose content is time-derived and must not enter the golden. */
  masks: (page: Page) => Locator[];
  /** The control keyboard navigation has to be able to reach. */
  primaryFocusKey: string;
}

const GOLDENS: GoldenCase[] = [
  {
    golden: "store",
    hash: "#/store",
    name: "store",
    masks: () => [],
    primaryFocusKey: "game-steam:1245620",
  },
  {
    golden: "game-detail",
    hash: "#/games/steam%3A1245620?from=store",
    name: "game",
    // `.gd-stats` renders "Last played 2 days ago", derived from Date.now().
    masks: (page) => [page.locator("#app-page-game:not([hidden]) .gd-stats")],
    primaryFocusKey: "primary-action",
  },
  {
    golden: "settings",
    hash: "#/settings/general",
    name: "settings",
    masks: () => [],
    primaryFocusKey: "",
  },
];

async function prepare(page: Page, golden: GoldenCase): Promise<void> {
  await openRoute(page, golden.hash, golden.name);
  await waitForImages(page);
  // Nothing may be focused: a focus ring would leak into the golden.
  await page.evaluate(() => {
    const active = document.activeElement;
    if (active instanceof HTMLElement) active.blur();
  });
  await page.mouse.move(0, 0);
  await page.waitForTimeout(150);
}

for (const golden of GOLDENS) {
  test.describe(`${golden.golden} page`, () => {
    test(`matches its visual golden`, async ({ page }) => {
      await prepare(page, golden);
      await expect(page).toHaveScreenshot(`${golden.golden}.png`, {
        mask: golden.masks(page),
        maskColor: "#101014",
      });
    });

    test("does not overflow the document horizontally", async ({ page }) => {
      await prepare(page, golden);
      const overflow = await documentOverflow(page);
      expect(
        overflow.scrollWidth,
        `${golden.golden}: documentElement.scrollWidth must not exceed clientWidth`,
      ).toBeLessThanOrEqual(overflow.clientWidth);

      // Neither the body nor the page host may scroll sideways either.
      const widths = await page.evaluate(() => {
        const activePage = [...document.querySelectorAll<HTMLElement>(".app-page")].find((el) => !el.hidden)!;
        const root = activePage.firstElementChild as HTMLElement | null;
        return {
          body: { scroll: document.body.scrollWidth, client: document.body.clientWidth },
          host: { scroll: activePage.scrollWidth, client: activePage.clientWidth },
          root: root ? { scroll: root.scrollWidth, client: root.clientWidth } : null,
        };
      });
      expect(widths.body.scroll, `${golden.golden}: body`).toBeLessThanOrEqual(widths.body.client + 1);
      expect(widths.host.scroll, `${golden.golden}: page host`).toBeLessThanOrEqual(widths.host.client + 1);
      if (widths.root) {
        expect(widths.root.scroll, `${golden.golden}: page root`).toBeLessThanOrEqual(widths.root.client + 1);
      }

      // Sideways scrolling is allowed only in the three deliberate rails, each
      // of which advertises itself: the card rail leaves a card peeking past the
      // edge, and the two chip rows carry a `mask-image` fade. Anything else
      // scrolling sideways is content escaping its column.
      const DELIBERATE_RAILS = ["store-card-rail", "store-category-filters", "store-provider-filters"];
      const scrollers = await page.evaluate(() => {
        const activePage = [...document.querySelectorAll<HTMLElement>(".app-page")].find((el) => !el.hidden)!;
        return [activePage, ...activePage.querySelectorAll<HTMLElement>("*")]
          .filter((el) => {
            const overflowX = getComputedStyle(el).overflowX;
            return (overflowX === "auto" || overflowX === "scroll") && el.scrollWidth > el.clientWidth + 1;
          })
          .map((el) => `${el.tagName}.${typeof el.className === "string" ? el.className : ""}`);
      });
      for (const scroller of scrollers) {
        expect(
          DELIBERATE_RAILS.some((rail) => scroller.includes(rail)),
          `${golden.golden}: unexpected horizontal scroller ${scroller}`,
        ).toBe(true);
      }
    });

    test("keyboard navigation reaches the primary controls with a visible focus ring", async ({
      page,
    }) => {
      await prepare(page, golden);

      // The shell topbar is always first in the tab order.
      const nav = await tabUntil(page, (report) => report.className.includes("nav-link"));
      expect(nav.found, `${golden.golden}: Tab never reached the primary nav`).not.toBeNull();
      expectVisibleFocusRing(nav.found!);

      const target =
        golden.primaryFocusKey === ""
          ? await tabUntil(page, (report) => report.className.includes("settings-section-link"))
          : await tabUntil(page, (report) => report.focusKey === golden.primaryFocusKey);

      expect(
        target.found,
        `${golden.golden}: Tab never reached the primary control. Tab order was:\n` +
          target.visited.map((stop) => `  ${stop.tag}.${stop.className} (${stop.focusKey ?? stop.label})`).join("\n"),
      ).not.toBeNull();
      expectVisibleFocusRing(target.found!);
    });
  });
}

test.describe("topbar geometry", () => {
  test("stays stable within 4px across Store, Detail and Settings", async ({ page }, testInfo) => {
    const boxes: Record<string, Awaited<ReturnType<typeof topbarBox>>> = {};

    for (const golden of GOLDENS) {
      await openRoute(page, golden.hash, golden.name);
      boxes[golden.golden] = await topbarBox(page);
    }

    const names = Object.keys(boxes);
    let worst = 0;
    let worstPair = "";
    for (const left of names) {
      for (const right of names) {
        const delta = boxDelta(boxes[left], boxes[right]);
        if (delta > worst) {
          worst = delta;
          worstPair = `${left} vs ${right}`;
        }
      }
    }

    testInfo.annotations.push({
      type: "topbar-delta",
      description: `${testInfo.project.name}: max ${worst}px (${worstPair || "identical"}) — ${JSON.stringify(boxes)}`,
    });

    expect(worst, `topbar moved ${worst}px between pages (${worstPair})`).toBeLessThanOrEqual(4);
    // The topbar also has to keep its declared height budget on every page.
    for (const [name, box] of Object.entries(boxes)) {
      expect(box.y, `${name}: topbar top`).toBeGreaterThanOrEqual(0);
      expect(box.height, `${name}: topbar height`).toBeGreaterThan(0);
    }
  });

  test("the Library page keeps the same topbar as the three golden pages", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    const store = await topbarBox(page);

    await openRoute(page, "#/library", "library");
    const library = await topbarBox(page);

    expect(boxDelta(store, library)).toBeLessThanOrEqual(4);
  });
});

test.describe("reduced motion", () => {
  // Emulated before the first navigation so the pages read the preference on
  // their very first render (`prefersReducedMotion()` is checked at render time).
  test.beforeEach(async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
  });

  for (const golden of GOLDENS) {
    test(`${golden.golden}: nothing animates and no video autoplays`, async ({ page }) => {
      await openRoute(page, golden.hash, golden.name);
      await waitForImages(page);
      await page.waitForTimeout(400);

      const motion = await page.evaluate(() => {
        const running = document
          .getAnimations()
          .filter((animation) => animation.playState === "running")
          .map((animation) => {
            const effect = animation.effect as KeyframeEffect | null;
            const target = effect?.target as Element | null;
            return `${target ? `${target.tagName}.${target.className}` : "?"} ${animation.constructor.name}`;
          });
        const videos = [...document.querySelectorAll("video")].map((video) => ({
          autoplay: video.autoplay,
          paused: video.paused,
          currentTime: video.currentTime,
        }));
        const animatedHero = document.querySelectorAll(".gd-hero__image--animated").length;
        return { running, videos, animatedHero };
      });

      expect(motion.running, `${golden.golden}: animations still running under reduced motion`).toEqual([]);
      expect(motion.animatedHero, `${golden.golden}: hero ken-burns class under reduced motion`).toBe(0);
      for (const video of motion.videos) {
        expect(video.autoplay, `${golden.golden}: a video declared autoplay`).toBe(false);
        expect(video.paused, `${golden.golden}: a video was playing`).toBe(true);
        expect(video.currentTime, `${golden.golden}: a video advanced on its own`).toBe(0);
      }
    });
  }

  test("the Appearance preference forces reduced motion on top of the system setting", async ({
    page,
  }) => {
    await openRoute(page, "#/settings/appearance", "settings");
    await page.locator("input[name='motion-preference'][value='reduced']").check();
    await expect(page.locator("#app")).toHaveAttribute("data-motion", "reduced");
  });
});
