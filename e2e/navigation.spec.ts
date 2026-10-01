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

  test("the topbar walks sideways between its entries", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);

    await page.locator("[data-nav-page='library']").focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("[data-nav-page='store']")).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("[data-nav-page='settings']")).toBeFocused();
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("#topbar-search")).toBeFocused();
    // An empty field hands left and right straight on.
    await page.keyboard.press("ArrowRight");
    await expect(page.locator("#notifications-button")).toBeFocused();

    await page.keyboard.press("ArrowLeft");
    await expect(page.locator("#topbar-search")).toBeFocused();
    await page.keyboard.press("ArrowLeft");
    await expect(page.locator("[data-nav-page='settings']")).toBeFocused();
  });

  test("down comes back from any topbar entry through Play to the game on screen", async ({ page }) => {
    await openRoute(page, "#/library", "library");
    await blurEverything(page);
    const before = await selectedGame(page);

    // The bell sits over the far end of the rail, so geometry alone would drop
    // straight onto whatever card lies under it.
    await page.locator("#notifications-button").focus();
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
  test("the store climbs from the featured card through the hero's action to the topbar", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await blurEverything(page);

    await page.keyboard.press("ArrowUp");
    await expect(page.locator(".store-hero__action")).toBeFocused();
    await page.keyboard.press("ArrowUp");
    await expect(page.locator("[data-nav-page='store']")).toBeFocused();

    await page.keyboard.press("ArrowDown");
    await page.keyboard.press("ArrowDown");
    await expect(page.locator(".store-rail .store-card__open").first()).toBeFocused();

    // Below the shelf, the chips: the one that is on.
    await page.keyboard.press("ArrowDown");
    await expect(page.locator(".store-chipbar--categories .store-chip[aria-pressed='true']")).toBeFocused();
  });

  test("a key held down never leaves the rail behind", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await blurEverything(page);

    const cards = page.locator(".store-rail .store-card__open");
    await cards.first().focus();

    /** How much of the focused card the shelf is showing, right now. */
    const shown = (): Promise<number> =>
      page.evaluate(() => {
        const track = document.querySelector<HTMLElement>(".store-rail__track")!;
        const card = document.activeElement as HTMLElement | null;
        if (!card || !track.contains(card)) return -1;
        const view = track.getBoundingClientRect();
        const box = card.getBoundingClientRect();
        const visible = Math.min(box.right, view.right) - Math.max(box.left, view.left);
        return Math.round((Math.max(0, visible) / box.width) * 100);
      });

    // A held key repeats about every 45ms, faster than one eased scroll can
    // run. Restarting that ease on every press left the shelf crawling a few
    // pixels a frame while the focus ran thousands of pixels ahead of it — so
    // what matters is where the shelf is *during* the burst, not where it
    // catches up to once the key is let go.
    let leastShown = 100;
    for (let press = 0; press < 14; press += 1) {
      await page.keyboard.press("ArrowRight");
      await page.waitForTimeout(45);
      leastShown = Math.min(leastShown, await shown());
    }
    expect(leastShown, "the focused card never ran off the shelf").toBeGreaterThan(50);

    // And once the key is let go, it sits fully in view.
    await expect.poll(shown, { timeout: 4_000 }).toBe(100);
  });

  test("the store rail walks sideways and wraps", async ({ page }) => {
    await openRoute(page, "#/store", "store");
    await blurEverything(page);

    const cards = page.locator(".store-rail .store-card__open");
    await cards.first().focus();
    await page.keyboard.press("ArrowRight");
    await expect(cards.nth(1)).toBeFocused();

    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowLeft");
    await expect(cards.last()).toBeFocused();
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

    const tap = async (button: number): Promise<void> => {
      await page.evaluate((index) => {
        (window as unknown as { __pad: { buttons: { pressed: boolean }[] } }).__pad.buttons[index]!.pressed = true;
      }, button);
      await page.waitForTimeout(80);
      await page.evaluate((index) => {
        (window as unknown as { __pad: { buttons: { pressed: boolean }[] } }).__pad.buttons[index]!.pressed = false;
      }, button);
      await page.waitForTimeout(80);
    };
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
