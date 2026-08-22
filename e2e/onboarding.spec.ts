import { expect, test, type Page } from "@playwright/test";
import { documentOverflow, waitForImages } from "./helpers";

/**
 * The Library when it holds nothing.
 *
 * A browser build still serves `fallbackLibrary`, because every other spec and
 * all three goldens depend on it. `?library=empty` is the switch that asks for
 * an empty catalogue instead, which is the only way to reach this screen
 * without a desktop build.
 */
const WELCOME = "/?library=empty#/library";

async function openWelcome(page: Page): Promise<void> {
  await page.goto(WELCOME, { waitUntil: "domcontentloaded" });
  await page.waitForSelector("#library-onboarding:not([hidden])", { state: "visible" });
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
}

test.describe("the library welcome screen", () => {
  test("replaces the showcase library rather than standing beside it", async ({ page }) => {
    await openWelcome(page);

    await expect(page.locator("#library-onboarding")).toBeVisible();
    await expect(page.locator("#game-cards .game-card")).toHaveCount(0);
    // The Library's own furniture is out of the layout, not merely covered, so
    // nothing behind this screen can be tabbed into.
    for (const selector of [".hero-content", ".recently-played", ".browse-bar", ".scene-arrow"]) {
      await expect(page.locator(`#app-page-library ${selector}`).first()).toBeHidden();
    }
    await expect(page.locator("#onboarding-panel")).toContainText("Your library is empty");
  });

  test("wears the brand wordmark, and reads as a heading with or without it", async ({ page }) => {
    await openWelcome(page);
    const wordmark = page.locator(".onboarding__wordmark");

    // A missing file leaves an <img> that is still "visible" and completely
    // blank, so the decode is what gets asserted: this is the brand, and
    // either it arrives or the screen has a hole where the name should be.
    await expect(wordmark).toBeVisible();
    expect(
      await wordmark.evaluate((image: HTMLImageElement) => image.naturalWidth),
      "the wordmark did not decode — check /media/orivo-logo.png is served",
    ).toBeGreaterThan(0);

    // The alt text finishes the sentence the heading starts, so the accessible
    // name holds up whether or not the artwork ever loaded.
    await expect(
      page.getByRole("heading", { name: "Welcome to Orivo", level: 1 }),
    ).toBeVisible();
  });

  test("offers a local game and a store, and walks into the store two presses deep", async ({
    page,
  }) => {
    await openWelcome(page);
    const panel = page.locator("#onboarding-panel");

    await expect(panel.locator("[data-onboarding-action='local']")).toBeVisible();
    await panel.locator("[data-onboarding-action='sources']").click();
    await expect(panel.locator("[data-onboarding-action='choose-source']")).toHaveCount(7);

    await panel.locator("[data-onboarding-provider='epic'][data-onboarding-action='choose-source']").click();
    await expect(panel).toContainText("Connect Epic Games");

    await panel.locator("[data-onboarding-action='back']").click();
    await expect(panel.locator("[data-onboarding-action='choose-source']")).toHaveCount(7);

    await panel.locator("[data-onboarding-action='back']").click();
    await expect(panel).toContainText("Your library is empty");
  });

  test("never scrolls the document sideways", async ({ page }) => {
    await openWelcome(page);
    await waitForImages(page);

    const overflow = await documentOverflow(page);
    expect(
      overflow.scrollWidth,
      "welcome: documentElement.scrollWidth must not exceed clientWidth",
    ).toBeLessThanOrEqual(overflow.clientWidth);
  });

  test("keeps every control reachable and named", async ({ page }) => {
    await openWelcome(page);

    // Icon-only controls carry their own name; the rest are real buttons.
    await expect(page.locator("#notifications-button")).toHaveAttribute(
      "aria-label",
      "Notifications",
    );
    await expect(page.locator("#library-onboarding")).toHaveAttribute(
      "aria-label",
      "Welcome to Orivo",
    );

    const buttons = page.locator("#onboarding-panel button");
    expect(await buttons.count()).toBeGreaterThan(0);
    for (const button of await buttons.all()) {
      await expect(button).toHaveAttribute("type", "button");
    }
  });

  test("opens and closes the notification bell", async ({ page }) => {
    await openWelcome(page);

    await expect(page.locator("#notifications-panel")).toBeHidden();
    await page.locator("#notifications-button").click();
    await expect(page.locator("#notifications-panel")).toBeVisible();
    // Nothing has come due in a fresh session, and the panel says so rather
    // than opening empty.
    await expect(page.locator("#notifications-panel")).toContainText("Nothing to report");

    await page.keyboard.press("Escape");
    await expect(page.locator("#notifications-panel")).toBeHidden();
  });
});

test.describe("the library welcome screen under reduced motion", () => {
  test.beforeEach(async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
  });

  test("stages nothing, and leaves nothing part-way through an entrance", async ({ page }) => {
    await openWelcome(page);
    await waitForImages(page);
    await page.waitForTimeout(400);

    const motion = await page.evaluate(() => {
      const running = document
        .getAnimations()
        .filter((animation) => animation.playState === "running")
        .map((animation) => {
          const target = (animation.effect as KeyframeEffect | null)?.target as Element | null;
          return target ? `${target.tagName}.${target.className}` : "?";
        });
      // The staged arrival holds each piece at `opacity: 0` through its delay,
      // which the global reduce rules do not shorten — only removing the
      // animation outright does.
      const invisible = [...document.querySelectorAll<HTMLElement>("#library-onboarding *")]
        .filter((element) => Number.parseFloat(getComputedStyle(element).opacity) < 1)
        .map((element) => `${element.tagName}.${element.className}`);
      return { running, invisible };
    });

    expect(motion.running, "welcome: animations still running under reduced motion").toEqual([]);
    expect(motion.invisible, "welcome: an element stayed transparent under reduced motion").toEqual(
      [],
    );
  });
});
