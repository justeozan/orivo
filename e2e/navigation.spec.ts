import { expect, test, type Page } from "@playwright/test";
import { blurEverything, currentHash, openRoute } from "./helpers";

/**
 * The keys read a page the way a console does: up and down go from row to row,
 * left and right move along the row focus is on — and on the Library, left and
 * right change the game. Everything below is behaviour `src/spatial-nav.ts`
 * owns, so these specs read focus and selection only: no screenshots, no
 * goldens, nothing that depends on how a row looks.
 */

const selectedGame = (page: Page): Promise<string | null> =>
  page.locator("#game-cards .game-card.is-selected").getAttribute("data-game-id");

const focusIsOnBody = (page: Page): Promise<boolean> =>
  page.evaluate(() => document.activeElement === document.body);

test.describe("the library reads as rows: topbar, Play, games, browse bar", () => {
  test("up climbs from the game to Play, then to the topbar's Library link", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);
    const before = await selectedGame(page);

    await page.keyboard.press("ArrowUp");
    await expect(page.locator("#play-button")).toBeFocused();

    await page.keyboard.press("ArrowUp");
    await expect(page.locator("[data-nav-page='library']")).toBeFocused();

    // The top of the page is the top: another press stays in the topbar.
    await page.keyboard.press("ArrowUp");
    await expect(page.locator("[data-nav-page='library']")).toBeFocused();

    // Climbing never touched the game on screen.
    expect(await selectedGame(page)).toBe(before);
  });

  // A console tab bar: reaching a page's link opens the page, with no second
  // press, and focus stays in the bar so the next press goes on along it.
  test("the topbar walks sideways, opening each page as it reaches it", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    await page.locator("[data-nav-page='library']").focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("[data-nav-page='store']")).toBeFocused();
    await expect(page).toHaveURL(/#\/store/);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("[data-nav-page='settings']")).toBeFocused();
    await expect(page).toHaveURL(/#\/settings/);
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("#topbar-search")).toBeFocused();
    // With no notice there is no bell: the search ends the bar.
    await expect(page.locator("#notifications-button")).toBeHidden();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("#topbar-search")).toBeFocused();

    await page.keyboard.press("ArrowLeft");
    await expect(page.locator("[data-nav-page='settings']")).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(page.locator("[data-nav-page='store']")).toBeFocused();
    await expect(page).toHaveURL(/#\/store/);
  });

  test("coming up into the topbar lands on the page on screen", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await blurEverything(page);
    // Whatever the keys stood on last, up into the bar lands on the current
    // page, so entering the bar never changes page.
    await page.locator("[data-nav-page='store']").focus();
    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowUp");
    await expect(page.locator("[data-nav-page='store']")).toBeFocused();
    await expect(page).toHaveURL(/#\/store/);
  });

  test("down comes back from any topbar entry through Play to the game on screen", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);
    const before = await selectedGame(page);

    // The search sits over the rail, so geometry alone would drop straight
    // onto whatever card lies under it.
    await page.locator("#topbar-search").focus();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#play-button")).toBeFocused();

    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#game-cards .game-card.is-selected")).toBeFocused();
    expect(await selectedGame(page)).toBe(before);

    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#browse-segments .browse-bar__segment.is-active")).toBeFocused();
  });

  test("left and right on Play change the game, and focus stays on Play", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);
    const cards = page.locator("#game-cards .game-card");
    const first = await cards.nth(0).getAttribute("data-game-id");
    const second = await cards.nth(1).getAttribute("data-game-id");
    expect(await selectedGame(page)).toBe(first);

    await page.locator("#play-button").focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("#play-button")).toBeFocused();
    expect(await selectedGame(page)).toBe(second);

    await page.keyboard.press("ArrowLeft");
    await expect(page.locator("#play-button")).toBeFocused();
    expect(await selectedGame(page)).toBe(first);
  });

  test("left and right change the game with nothing focused, and focus follows it", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);
    const second = await page.locator("#game-cards .game-card").nth(1).getAttribute("data-game-id");

    await page.keyboard.press("ArrowRight");

    expect(await selectedGame(page)).toBe(second);
    await expect(page.locator("#game-cards .game-card.is-selected")).toBeFocused();
    expect(await focusIsOnBody(page)).toBe(false);
  });

  test("the rail walks sideways and wraps at both ends", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    const cards = page.locator("#game-cards .game-card");
    const count = await cards.count();
    expect(count, "the fixture library needs a rail to walk").toBeGreaterThan(2);

    await cards.first().focus();
    await page.keyboard.press("ArrowRight");
    await expect(cards.nth(1)).toBeFocused();

    await page.keyboard.press("ArrowLeft");
    await expect(cards.first()).toBeFocused();

    // The end of the rail is a circle, not a wall.
    await page.keyboard.press("ArrowLeft");
    await expect(cards.nth(count - 1)).toBeFocused();

    await page.keyboard.press("ArrowRight");
    await expect(cards.first()).toBeFocused();
  });

  test("walking the rail keeps the next game in sight, both ways", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    const cards = page.locator("#game-cards .game-card");
    const count = await cards.count();

    /** How much of the card beside the selected one, that way, the rail shows. */
    const neighbourShown = (step: 1 | -1): Promise<number | null> =>
      page.evaluate((delta) => {
        const rail = document.querySelector<HTMLElement>("#game-cards")!;
        const all = Array.from(rail.querySelectorAll<HTMLElement>(".game-card"));
        const next = all[all.findIndex((card) => card.classList.contains("is-selected")) + delta];
        if (!next) return null;
        const view = rail.getBoundingClientRect();
        const box = next.getBoundingClientRect();
        const shown = Math.min(box.right, view.right) - Math.max(box.left, view.left);
        return Math.round((Math.max(0, shown) / box.width) * 100);
      }, step);

    // The shelf has to move before the selection reaches its edge, never after:
    // whichever card is selected, the one it leads to is already whole.
    for (let step = 1; step < count; step += 1) {
      await page.keyboard.press("ArrowRight");
      if (step === count - 1) break;
      await expect.poll(() => neighbourShown(1), { message: `right, card ${step}` }).toBe(100);
    }
    for (let step = count - 2; step > 0; step -= 1) {
      await page.keyboard.press("ArrowLeft");
      await expect.poll(() => neighbourShown(-1), { message: `left, card ${step}` }).toBe(100);
    }
  });

  test("the browse bar steps sideways, and up returns to the game on screen", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);
    const before = await selectedGame(page);

    const segments = page.locator("#browse-segments .browse-bar__segment");
    await segments.first().focus();

    await page.keyboard.press("ArrowRight");
    await expect(segments.nth(1)).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("#browse-mode")).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(segments.nth(1)).toBeFocused();

    await page.keyboard.press("ArrowUp");
    await expect(page.locator("#game-cards .game-card.is-selected")).toBeFocused();
    expect(await selectedGame(page)).toBe(before);
  });

  test("down from a search reaches the page", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    await page.locator("#topbar-search").fill("hades");
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#play-button")).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#game-cards .game-card.is-selected")).toBeFocused();
  });

  test("a control the keys reach shows it, and the pointer never does", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    await page.keyboard.press("ArrowUp");
    const play = page.locator("#play-button");
    await expect(play).toBeFocused();
    await expect(play).toHaveAttribute("data-nav-focus", "");
    // Lifted, not ringed.
    expect(await play.evaluate((node) => getComputedStyle(node).outlineStyle)).toBe("none");
    await expect
      .poll(() => play.evaluate((node) => getComputedStyle(node).transform))
      .not.toBe("none");

    await page.locator("[data-nav-page='store']").hover();
    await page.mouse.down();
    await expect(page.locator("[data-nav-focus]")).toHaveCount(0);
    await page.mouse.up();
  });

  test("a lifted Play is never shaved by the scene around it", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    await page.keyboard.press("ArrowUp");
    await expect(page.locator("#play-button")).toBeFocused();
    await page.waitForTimeout(400);

    // Every ancestor that clips must still hold the whole grown button.
    const clipped = await page.locator("#play-button").evaluate((play) => {
      const rect = play.getBoundingClientRect();
      const cuts: string[] = [];
      for (let node = play.parentElement; node; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (style.overflowX === "visible" && style.overflowY === "visible") continue;
        const box = node.getBoundingClientRect();
        if (
          rect.left < box.left - 0.5 ||
          rect.right > box.right + 0.5 ||
          rect.top < box.top - 0.5 ||
          rect.bottom > box.bottom + 0.5
        ) {
          cuts.push(node.className || node.tagName);
        }
      }
      return cuts;
    });
    expect(clipped).toEqual([]);
  });
});

test.describe("the store and settings read the same way", () => {
  test("the store climbs from a card to the topbar, and the chevrons are on the way", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await blurEverything(page);

    const featured = page.locator(".store-section--featured .store-card__open");
    await featured.first().focus();
    let climbed = false;
    for (let press = 0; press < 3 && !climbed; press += 1) {
      await page.keyboard.press("ArrowUp");
      climbed = await page.locator("[data-nav-page='store']").evaluate((node) => node === document.activeElement);
    }
    expect(climbed, "up reaches the topbar's Store link").toBe(true);

    await page.locator("[data-focus-key='store-next']").focus();
    await page.keyboard.press("Enter");
    await expect.poll(() => currentHash(page)).toBe("#/store?store=playstation");
  });

  test("a row of three walks sideways and stops at its ends, and down goes to the next row", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await blurEverything(page);

    const featured = page.locator(".store-section--featured .store-card__open");
    await featured.first().focus();
    await page.keyboard.press("ArrowRight");
    await expect(featured.nth(1)).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowLeft");
    await expect(featured.first()).toBeFocused();

    await page.keyboard.press("ArrowDown");
    await expect(page.locator(".store-section--top-rated .store-card__open").first()).toBeFocused();
  });

  test("the detail keeps the keys inside it until it is closed", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await blurEverything(page);

    const first = page.locator(".store-section--featured .store-card__open").first();
    await first.focus();
    await page.keyboard.press("Enter");
    const dialog = page.locator("#app-page-store .store-detail");
    await expect(dialog).toBeVisible();

    for (const key of ["ArrowUp", "ArrowLeft", "ArrowDown", "ArrowRight", "Tab"]) {
      await page.keyboard.press(key);
      const inside = await page.evaluate(
        () => document.querySelector("#app-page-store .store-detail")?.contains(document.activeElement) ?? false,
      );
      expect(inside, `${key} must not leave the detail`).toBe(true);
    }

    await page.keyboard.press("Escape");
    await expect(dialog).toBeHidden();
    await expect(first).toBeFocused();
  });

  test("the settings column walks with up and down, takes the section with it, and climbs to the topbar", async ({
    page,
  }) => {
    await openRoute(page, "#/settings/general", "settings");
    await blurEverything(page);

    await page.locator("#settings-tab-general").focus();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#settings-tab-libraries")).toBeFocused();
    expect(await currentHash(page)).toBe("#/settings/libraries");

    await page.keyboard.press("ArrowUp");
    await expect(page.locator("#settings-tab-general")).toBeFocused();
    expect(await currentHash(page)).toBe("#/settings/general");

    await page.keyboard.press("ArrowUp");
    await expect(page.locator("[data-nav-page='settings']")).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#settings-tab-general")).toBeFocused();
  });

  test("the settings column keeps up when the page hears of the URL late", async ({ page }) => {
    // `hashchange` is a queued task, and a page busy with keys — a CI runner,
    // a key held down, a controller — can run it long after the URL changed.
    // Hold every one back by 300ms: the tab a key comes back to has to be the
    // one the URL names, not the one the page was showing before.
    await page.addInitScript(() => {
      const listen = window.addEventListener.bind(window);
      window.addEventListener = ((type: string, listener: EventListener, options?: AddEventListenerOptions) => {
        if (type !== "hashchange") return listen(type, listener, options);
        return listen(type, (event: Event) => window.setTimeout(() => listener(event), 300), options);
      }) as typeof window.addEventListener;
    });
    await openRoute(page, "#/settings/general", "settings");
    await blurEverything(page);

    await page.locator("#settings-tab-general").focus();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#settings-tab-libraries")).toBeFocused();
    // The CI run that failed: this section had been shown...
    await expect(page.locator("#settings-tab-libraries")).toHaveAttribute("aria-selected", "true");
    // ...and the next three keys came before the page heard of the one after.
    await page.keyboard.press("ArrowUp");
    await expect(page.locator("#settings-tab-general")).toBeFocused();
    await page.keyboard.press("ArrowUp");
    await expect(page.locator("[data-nav-page='settings']")).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(page.locator("#settings-tab-general")).toBeFocused();
    expect(await currentHash(page)).toBe("#/settings/general");
  });

  test("right leaves the settings column for the section, and left comes back to its tab", async ({ page }) => {
    await openRoute(page, "#/settings/general", "settings");
    await blurEverything(page);

    await page.locator("#settings-tab-general").focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("#settings-panel-general :focus")).toHaveCount(1);
    expect(await currentHash(page)).toBe("#/settings/general");

    await page.keyboard.press("ArrowLeft");
    await expect(page.locator("#settings-tab-general")).toBeFocused();
  });
});

/**
 * A pad goes through the same engine as the arrow keys. The page cannot plug a
 * real controller in, so a stand-in answers `navigator.getGamepads()` and the
 * spec presses its d-pad one frame at a time.
 */
test.describe("a controller walks the same rows", () => {
  test("the d-pad climbs to Play and the topbar, and walks the topbar", async ({ page }) => {
    await page.addInitScript(() => {
      const buttons = Array.from({ length: 17 }, () => ({ pressed: false, touched: false, value: 0 }));
      const pad = {
        id: "stand-in",
        index: 0,
        connected: true,
        mapping: "standard",
        timestamp: 0,
        axes: [0, 0, 0, 0],
        buttons,
      };
      Object.defineProperty(navigator, "getGamepads", { value: () => [pad] });
      (window as unknown as { __pad: typeof pad }).__pad = pad;
    });
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    // A tap is one frame long, timed inside the page. The bridge reads the pad
    // once per animation frame and turns a direction held past 420ms into a
    // repeat; a press timed from the test, 80ms plus two round trips, could
    // outlast that on a loaded machine and climb two rows instead of one. The
    // bridge's poll for the next frame is already queued when this runs, and
    // frame callbacks run in the order they were asked for, so it sees the
    // press before the release, and the release before the promise settles.
    const tap = (button: number): Promise<void> =>
      page.evaluate(
        (index) =>
          new Promise<void>((resolve) => {
            const pad = (window as unknown as { __pad: { buttons: { pressed: boolean }[] } }).__pad;
            pad.buttons[index]!.pressed = true;
            requestAnimationFrame(() => {
              pad.buttons[index]!.pressed = false;
              requestAnimationFrame(() => resolve());
            });
          }),
        button,
      );
    const [UP, RIGHT] = [12, 15];

    await tap(UP);
    await expect(page.locator("#play-button")).toBeFocused();
    await tap(UP);
    await expect(page.locator("[data-nav-page='library']")).toBeFocused();
    await tap(RIGHT);
    await expect(page.locator("[data-nav-page='store']")).toBeFocused();
    await expect(page.locator("[data-nav-page='store']")).toHaveAttribute("data-nav-focus", "");
  });
});
