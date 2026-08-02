import { expect, test } from "@playwright/test";
import {
  blurEverything,
  currentHash,
  FALLBACK_LIBRARY_IDS,
  host,
  openRoute,
  waitForPage,
} from "./helpers";

test.describe("hash router", () => {
  test("unicode and percent-encoded game ids round-trip", async ({ page }) => {
    const ids = ["steam:1245620", "local:Café Über", "游戏 Ω", "id with spaces & symbols"];

    for (const id of ids) {
      await openRoute(page, `#/games/${encodeURIComponent(id)}?from=library`, "game");

      await expect(host(page, "not-found")).toBeHidden();
      await expect(host(page, "game").locator(".gd-hero__title")).toBeVisible();

      const hash = await currentHash(page);
      const path = hash.slice(1).split("?", 1)[0];
      const encodedId = path.replace("/games/", "");
      expect(decodeURIComponent(encodedId), `id ${id} should survive the address bar`).toBe(id);
    }
  });

  test("in-app navigation encodes the id the same way a deep link does", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await page.locator("[data-focus-key='game-steam:1091500']").click();
    await waitForPage(page, "game");

    expect(await currentHash(page)).toBe("#/games/steam%3A1091500?from=store");
  });

  test("a game id containing a slash is rejected rather than half-decoded", async ({ page }) => {
    // `decodeSegment()` in src/router.ts refuses ids that decode to a path
    // separator, so `%2F` must land on the not-found page, not on a detail page
    // for the id "a".
    await openRoute(page, "#/games/a%2Fb", "not-found");

    await expect(host(page, "game")).toBeHidden();
    await expect(page.locator("#not-found-detail")).toContainText("/games/a%2Fb");
  });

  test("an invalid route renders not-found with a working Back to Library", async ({ page }) => {
    await openRoute(page, "#/definitely-not-a-page", "not-found");

    await expect(page.locator(".not-found__code")).toHaveText("404");
    await expect(page.locator(".not-found__title")).toHaveText("This page does not exist");
    await expect(page.locator("#not-found-detail")).toContainText("/definitely-not-a-page");

    await page.locator("[data-app-action='go-library']").click();
    await waitForPage(page, "library");
    expect(await currentHash(page)).toBe("#/library");
  });

  test("an unknown settings section is not-found, a known one is not", async ({ page }) => {
    await openRoute(page, "#/settings/nope", "not-found");
    await expect(host(page, "settings")).toBeHidden();

    await openRoute(page, "#/settings/appearance", "settings");
    await expect(page.locator("#settings-page-title")).toHaveText("Appearance");
  });

  test("reload preserves the current route", async ({ page }) => {
    const cases = [
      { hash: "#/store?category=relaxing&provider=steam", name: "store" },
      { hash: "#/games/steam%3A1245620?from=store", name: "game" },
      { hash: "#/settings/data", name: "settings" },
    ] as const;

    for (const routeCase of cases) {
      await openRoute(page, routeCase.hash, routeCase.name);
      await page.reload({ waitUntil: "domcontentloaded" });
      await waitForPage(page, routeCase.name);
      expect(await currentHash(page)).toBe(routeCase.hash);
    }
  });

  test("Back and Forward move between Library, Store and Detail", async ({ page }) => {
    await openRoute(page, "#/library", "library");

    await page.locator("[data-nav-page='store']").click();
    await waitForPage(page, "store");

    await page.locator("[data-focus-key='game-steam:1091500']").click();
    await waitForPage(page, "game");

    await page.goBack();
    await waitForPage(page, "store");
    expect(await currentHash(page)).toBe("#/store");

    await page.goBack();
    await waitForPage(page, "library");
    expect(await currentHash(page)).toBe("#/library");

    await page.goForward();
    await waitForPage(page, "store");
    expect(await currentHash(page)).toBe("#/store");

    await page.goForward();
    await waitForPage(page, "game");
    expect(await currentHash(page)).toBe("#/games/steam%3A1091500?from=store");

    // Exactly one host is ever mounted and visible.
    await expect(page.locator(".app-page:not([hidden])")).toHaveCount(1);
  });

  test("deep-linked detail with no in-app history falls back to Library", async ({ page }) => {
    // `HashRouter.back()` counts its own pushes (`#pushDepth`) instead of
    // trusting `window.history.length`, which also counts the entries a fresh
    // tab already had. With no push of its own it replaces the route rather
    // than calling `history.back()`, so Back can never leave the app.
    await openRoute(page, "#/games/steam%3A1245620", "game");
    // A fresh context still reports a history length > 1 — the exact signal the
    // old implementation trusted.
    expect(await page.evaluate(() => window.history.length)).toBeGreaterThan(1);

    await page.locator(".gd-back").click();
    await waitForPage(page, "library");

    expect(page.url(), "Back from a deep link must stay inside Orivo").toContain("#/library");
    expect(page.url()).not.toContain("about:blank");
    expect(await currentHash(page)).toBe("#/library");
    await expect(host(page, "library")).toBeVisible();
  });

  test("deep-linked Back never leaves the app, from any origin and any id", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const origin = new URL(page.url()).origin;

    for (const hash of [
      "#/games/steam%3A1245620",
      "#/games/steam%3A1091500?from=store",
      "#/games/local%3ACaf%C3%A9%20%C3%9Cber?from=library",
    ]) {
      // A brand-new context each time, so there is genuinely no in-app history.
      const context = await page.context().browser()!.newContext({
        viewport: page.viewportSize()!,
        colorScheme: "dark",
      });
      const fresh = await context.newPage();
      try {
        await fresh.goto(`${origin}/${hash}`, { waitUntil: "domcontentloaded" });
        await fresh.waitForSelector("#app-page-game:not([hidden]) .gd-hero__title", { state: "visible" });
        await fresh.locator(".gd-back").click();
        await fresh.waitForSelector("#app-page-library:not([hidden]) #hero-title", { state: "visible" });

        expect(fresh.url(), `${hash}: Back left the application`).toContain("#/library");
        expect(fresh.url()).not.toContain("about:blank");
      } finally {
        await context.close();
      }
    }
  });

  test("Back from a detail opened in-app returns to its origin", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await page.evaluate(() => {
      window.location.hash = "#/games/steam%3A1245620?from=library";
    });
    await waitForPage(page, "game");

    await page.locator(".gd-back").click();
    await waitForPage(page, "library");
    expect(await currentHash(page)).toBe("#/library");
  });

  test("exactly one topbar element carries aria-current", async ({ page }) => {
    const cases = [
      { hash: "#/library", name: "library", current: "Library" },
      { hash: "#/store", name: "store", current: "Store" },
      { hash: "#/store?category=relaxing", name: "store", current: "Store" },
      { hash: "#/games/steam%3A1245620?from=library", name: "game", current: "Library" },
      { hash: "#/games/steam%3A1245620?from=store", name: "game", current: "Store" },
      { hash: "#/settings/general", name: "settings", current: "Settings" },
      { hash: "#/settings/about", name: "settings", current: "Settings" },
      { hash: "#/nowhere", name: "not-found", current: "Library" },
    ] as const;

    for (const routeCase of cases) {
      await openRoute(page, routeCase.hash, routeCase.name);

      const marked = page.locator("header.topbar [aria-current]");
      await expect(marked, `${routeCase.hash} should mark one nav item`).toHaveCount(1);
      await expect(marked).toHaveText(routeCase.current);
      await expect(marked).toHaveAttribute("aria-current", "page");
      // The whole document must not carry a second current marker either.
      await expect(page.locator("[aria-current]")).toHaveCount(1);
    }
  });
});

test.describe("library keyboard shortcuts are scoped to the Library page", () => {
  const libraryKeys = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "i", "I", "Enter"];

  async function pressLibraryKeys(page: import("@playwright/test").Page): Promise<void> {
    await blurEverything(page);
    for (const key of libraryKeys) {
      await page.keyboard.press(key);
    }
    await page.keyboard.press("Shift+I");
    await page.waitForTimeout(250);
  }

  test("they do not fire on the Store page", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const heroBefore = await page.locator("#hero-title").textContent();
    const selectedBefore = await page.locator("#game-cards .game-card.is-selected").getAttribute("data-game-id");

    await page.locator("[data-nav-page='store']").click();
    await waitForPage(page, "store");
    await pressLibraryKeys(page);

    expect(await currentHash(page)).toBe("#/store");
    await expect(host(page, "store")).toBeVisible();
    await expect(page.locator("#toast.is-visible")).toHaveCount(0);

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");
    await expect(page.locator("#hero-title")).toHaveText(heroBefore ?? "");
    expect(
      await page.locator("#game-cards .game-card.is-selected").getAttribute("data-game-id"),
    ).toBe(selectedBefore);
  });

  test("they do not fire on the Settings page", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const heroBefore = await page.locator("#hero-title").textContent();

    await openRoute(page, "#/settings/appearance", "settings");
    await pressLibraryKeys(page);

    expect(await currentHash(page)).toBe("#/settings/appearance");
    await expect(page.locator("#settings-page-title")).toHaveText("Appearance");

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");
    await expect(page.locator("#hero-title")).toHaveText(heroBefore ?? "");
  });

  test("they do not fire on a game detail page", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const heroBefore = await page.locator("#hero-title").textContent();

    await page.evaluate(() => {
      window.location.hash = "#/games/steam%3A1245620?from=library";
    });
    await waitForPage(page, "game");
    await pressLibraryKeys(page);

    expect(await currentHash(page)).toBe("#/games/steam%3A1245620?from=library");
    await expect(host(page, "game")).toBeVisible();

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");
    await expect(page.locator("#hero-title")).toHaveText(heroBefore ?? "");
  });

  test("they do not fire on the not-found page", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const heroBefore = await page.locator("#hero-title").textContent();

    await openRoute(page, "#/nowhere-at-all", "not-found");
    await pressLibraryKeys(page);

    expect(await currentHash(page)).toBe("#/nowhere-at-all");
    await expect(host(page, "not-found")).toBeVisible();

    await page.locator("[data-nav-page='library']").click();
    await waitForPage(page, "library");
    await expect(page.locator("#hero-title")).toHaveText(heroBefore ?? "");
  });

  test("the shared search shortcut works on every route", async ({ page }) => {
    // The bar is visually identical everywhere, so the field is never blanked
    // out or disabled — a 352px hole mid-bar reads as a broken layout. Only the
    // placeholder, i.e. what the field searches, changes.
    const cases = [
      { hash: "#/library", name: "library", placeholder: "Search games…" },
      { hash: "#/store", name: "store", placeholder: "Search the store…" },
      { hash: "#/games/steam%3A1245620?from=store", name: "game", placeholder: "Search games…" },
      { hash: "#/settings/general", name: "settings", placeholder: "Search settings…" },
      { hash: "#/nowhere-at-all", name: "not-found", placeholder: "Search games…" },
    ] as const;

    for (const routeCase of cases) {
      await openRoute(page, routeCase.hash, routeCase.name);
      await blurEverything(page);
      await page.keyboard.press("/");

      await expect(page.locator("#topbar-search"), `${routeCase.hash}: "/" must focus the search`).toBeFocused();
      await expect(page.locator("#topbar-search")).toBeEnabled();
      await expect(page.locator("#topbar-search")).toHaveAttribute("placeholder", routeCase.placeholder);
    }
  });

  test("the fallback library renders its ten showcase games", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    const ids = await page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>("#game-cards .game-card")].map((c) => c.dataset.gameId),
    );
    expect(ids).toEqual([...FALLBACK_LIBRARY_IDS]);
  });
});
