import { expect, test } from "@playwright/test";
import { mainLandmarks, openRoute, rotatingLoaders, topbarSearchProbe, type PageName } from "./helpers";

/**
 * The persistent shell: its landmark shape, its always-live search, and the
 * house rule that progress is a bar and never a disc.
 *
 * Every route is checked, because the shell outlives navigation — a page host
 * that has been visited once keeps its DOM (and its `<main>`) around while
 * hidden, which is exactly how a second landmark sneaks in.
 */

const ROUTES: Array<{ hash: string; name: PageName; label: string }> = [
  { hash: "#/library", name: "library", label: "Library" },
  { hash: "#/store", name: "store", label: "Store" },
  { hash: "#/games/steam%3A1245620?from=store", name: "game", label: "Game detail" },
  { hash: "#/settings/general", name: "settings", label: "Settings" },
  { hash: "#/nowhere-at-all", name: "not-found", label: "Not found" },
];

/** The routes whose page root is a `<main>` today. */
const ROUTES_WITH_MAIN = ROUTES.filter((route) => route.name === "store" || route.name === "game");

test.describe("landmark structure", () => {
  test("the topbar is a banner outside every main, and no route shows two", async ({ page }) => {
    // Walk the whole set in one context so the already-mounted, hidden page
    // hosts are in the DOM while each later route is measured.
    for (const route of ROUTES) {
      await openRoute(page, route.hash, route.name);
      const report = await mainLandmarks(page);

      expect(
        report.headerInsideMain,
        `${route.label}: header.topbar is nested in a <main> (${report.headerAncestors})`,
      ).toBe(false);
      expect(
        report.visible.length,
        `${route.label}: ${report.visible.length} visible <main> landmarks — ${report.visible.join(", ")}`,
      ).toBeLessThanOrEqual(1);
    }
  });

  test("the Store and the detail page each own exactly one visible main", async ({ page }) => {
    for (const route of ROUTES_WITH_MAIN) {
      // Visit the other page first so its `<main>` is mounted but hidden.
      const other = ROUTES_WITH_MAIN.find((candidate) => candidate !== route)!;
      await openRoute(page, other.hash, other.name);
      await openRoute(page, route.hash, route.name);

      const report = await mainLandmarks(page);
      expect(report.total, `${route.label}: mounted <main> elements`).toBeGreaterThanOrEqual(2);
      expect(
        report.visible,
        `${route.label}: exactly one <main> may be visible`,
      ).toHaveLength(1);
      expect(report.headerInsideMain).toBe(false);
    }
  });

  test("every route exposes a main landmark", async ({ page }) => {
    // KNOWN PRODUCT DEFECT — only the Store (`main.store-page`) and the game
    // detail (`main.gd-page`) build their page root as a `<main>`. The Library
    // host, the Settings host (`div.settings-layout`) and the 404 host
    // (`section.not-found`) render zero `<main>` elements, so on those three
    // routes a screen reader has no main landmark to jump to at all.
    // Repro: open #/settings/general and run
    // `[...document.querySelectorAll("main")].filter(m => !m.closest("[hidden]")).length` → 0.
    test.fail();

    for (const route of ROUTES) {
      await openRoute(page, route.hash, route.name);
      const report = await mainLandmarks(page);
      expect(report.visible, `${route.label}: exactly one visible <main>`).toHaveLength(1);
    }
  });
});

test.describe("topbar search", () => {
  test("is present, enabled and focusable on every route", async ({ page }) => {
    for (const route of ROUTES) {
      await openRoute(page, route.hash, route.name);

      let probe = await topbarSearchProbe(page);
      expect(probe.present, `${route.label}: #topbar-search is missing`).toBe(true);
      expect(probe.visible, `${route.label}: #topbar-search is not rendered`).toBe(true);
      expect(probe.disabled, `${route.label}: #topbar-search is disabled`).toBe(false);
      expect(probe.readOnly, `${route.label}: #topbar-search is read-only`).toBe(false);
      expect(probe.placeholder, `${route.label}: the field must say what it searches`).not.toBe("");
      expect(probe.ariaLabel, `${route.label}: the field must be labelled`).not.toBeNull();

      // The `/` shortcut reaches it from every page, including the two that
      // used to blank the field out.
      await page.evaluate(() => {
        const active = document.activeElement;
        if (active instanceof HTMLElement) active.blur();
      });
      await page.keyboard.press("/");
      probe = await topbarSearchProbe(page);
      expect(probe.focused, `${route.label}: "/" did not focus the search`).toBe(true);
    }
  });

  test(":focus-within paints a 2px ring around the search pill", async ({ page }) => {
    for (const route of ROUTES) {
      await openRoute(page, route.hash, route.name);

      // A hash navigation is same-document, so focus survives it: blur first or
      // the "unfocused" baseline is whatever the previous route left behind.
      await page.evaluate(() => {
        const active = document.activeElement;
        if (active instanceof HTMLElement) active.blur();
      });
      const blurred = await topbarSearchProbe(page);
      expect(blurred.focusWithin, `${route.label}: search should start unfocused`).toBe(false);
      expect(blurred.ringWidth, `${route.label}: no ring while unfocused`).not.toBeCloseTo(2, 1);

      await page.locator("#topbar-search").focus();
      const focused = await topbarSearchProbe(page);
      expect(focused.focusWithin, `${route.label}: .search-control :focus-within`).toBe(true);
      expect(
        focused.ringWidth,
        `${route.label}: the focus ring must be a visible 2px, got ${focused.ringWidth}px`,
      ).toBeCloseTo(2, 1);
    }
  });

  test("typing on the Store filters it, and on other routes the field still accepts input", async ({
    page,
  }) => {
    await openRoute(page, "#/store", "store");
    await page.locator("#topbar-search").fill("hades");
    await page.locator("#topbar-search").press("Enter");
    await expect(page.locator("#app-page-store:not([hidden]) .store-card")).toHaveCount(1);

    // A detail page cannot show library results, but the field is still live —
    // it is never a dead 352px hole in the middle of the bar.
    await openRoute(page, "#/games/steam%3A1245620?from=store", "game");
    await page.locator("#topbar-search").fill("elden");
    await expect(page.locator("#topbar-search")).toHaveValue("elden");
  });
});

test.describe("no circular spinners", () => {
  // House style (DESIGN.md 13): work in progress is an indeterminate bar.
  // `store-spin` and `gd-spin` were deleted; nothing may bring a disc back.
  for (const route of ROUTES_WITH_MAIN) {
    test(`${route.label} renders no rotating loader`, async ({ page }) => {
      await openRoute(page, route.hash, route.name);
      const loaders = await rotatingLoaders(page);

      expect(
        loaders.animations,
        `${route.label}: a rotating animation is running — ${loaders.animations.join(", ")}`,
      ).toEqual([]);
      expect(
        loaders.classNames,
        `${route.label}: a spinner-shaped class name survived — ${loaders.classNames.join(", ")}`,
      ).toEqual([]);
      await expect(page.locator(".store-spin, .gd-spin")).toHaveCount(0);
      // The progress affordance that replaced them is a 2px bar.
      const bars = await page.evaluate(() => {
        const active = [...document.querySelectorAll<HTMLElement>(".app-page")].find((el) => !el.hidden)!;
        return [...active.querySelectorAll<HTMLElement>("[role='progressbar']")].map((el) =>
          Math.round(el.getBoundingClientRect().height),
        );
      });
      for (const height of bars) {
        expect(height, `${route.label}: a progress indicator is disc-shaped`).toBeLessThanOrEqual(8);
      }
    });
  }
});
