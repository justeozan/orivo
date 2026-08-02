import { expect, test } from "@playwright/test";
import { currentHash, host, openRoute, SETTINGS_SECTIONS, waitForPage } from "./helpers";

const settingsHost = "#app-page-settings:not([hidden])";

test.describe("Settings is a page, not a modal", () => {
  test("there is no dialog role, no aria-modal and no backdrop", async ({ page }) => {
    await openRoute(page, "#/settings/general", "settings");

    await expect(page.locator("[role='dialog']")).toHaveCount(0);
    await expect(page.locator("[role='alertdialog']")).toHaveCount(0);
    await expect(page.locator("[aria-modal]")).toHaveCount(0);
    await expect(page.locator("dialog")).toHaveCount(0);

    const overlays = await page.evaluate(() => {
      const settings = document.getElementById("app-page-settings")!;
      const vw = document.documentElement.clientWidth;
      const vh = document.documentElement.clientHeight;
      return [...settings.querySelectorAll<HTMLElement>("*")]
        .filter((el) => {
          const style = getComputedStyle(el);
          if (style.position !== "fixed" && style.position !== "absolute") return false;
          const rect = el.getBoundingClientRect();
          return rect.width >= vw * 0.98 && rect.height >= vh * 0.98;
        })
        .map((el) => `${el.tagName}.${el.className}`);
    });
    expect(overlays, "no element inside Settings covers the viewport like a scrim").toEqual([]);

    const classNames = await page.evaluate(() =>
      [...document.getElementById("app-page-settings")!.querySelectorAll<HTMLElement>("*")]
        .map((el) => (typeof el.className === "string" ? el.className : ""))
        .filter((name) => /backdrop|scrim|overlay|modal/i.test(name)),
    );
    expect(classNames).toEqual([]);
  });

  test("Escape does not close Settings", async ({ page }) => {
    await openRoute(page, "#/settings/libraries", "settings");

    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);
    expect(await currentHash(page)).toBe("#/settings/libraries");
    await expect(host(page, "settings")).toBeVisible();

    // Also from inside a control, and repeatedly.
    await page.locator("[data-settings-section='general']").click();
    await waitForPage(page, "settings");
    await page.locator("#preference-start-page").focus();
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await page.waitForTimeout(250);

    expect(await currentHash(page)).toBe("#/settings/general");
    await expect(host(page, "settings")).toBeVisible();
    await expect(page.locator("#settings-page-title")).toHaveText("General");
  });

  test("the shell topbar stays live while Settings is open", async ({ page }) => {
    await openRoute(page, "#/settings/general", "settings");

    await expect(page.locator("header.topbar")).toBeVisible();
    await expect(page.locator("#topbar-search")).toBeEnabled();
    await expect(page.locator("#topbar-search")).toHaveAttribute("placeholder", "Search settings…");

    await page.locator("[data-nav-page='store']").click();
    await waitForPage(page, "store");
    expect(await currentHash(page)).toBe("#/store");
  });
});

test.describe("Settings navigation", () => {
  test("all six sections are reachable by URL", async ({ page }) => {
    for (const section of SETTINGS_SECTIONS) {
      await openRoute(page, `#/settings/${section.id}`, "settings");

      await expect(page.locator("#settings-page-title")).toHaveText(section.title);
      await expect(page.locator(`${settingsHost} [data-settings-panel]:not([hidden])`)).toHaveCount(1);
      await expect(
        page.locator(`${settingsHost} [data-settings-panel='${section.id}']`),
      ).toBeVisible();
      await expect(page.locator(`[data-settings-section='${section.id}']`)).toHaveAttribute(
        "aria-selected",
        "true",
      );
      await expect(page.locator(`${settingsHost} [aria-selected='true']`)).toHaveCount(1);
    }
  });

  test("all six sections are reachable from the sidebar", async ({ page }) => {
    await openRoute(page, "#/settings/general", "settings");

    for (const section of SETTINGS_SECTIONS) {
      await page.locator(`[data-settings-section='${section.id}']`).click();
      await expect.poll(() => currentHash(page)).toBe(`#/settings/${section.id}`);
      await expect(page.locator("#settings-page-title")).toHaveText(section.title);
      await expect(
        page.locator(`${settingsHost} [data-settings-panel='${section.id}']`),
      ).toBeVisible();
    }

    // Sidebar entries are tabs of one tablist, not links that reload the shell.
    await expect(page.locator(`${settingsHost} [role='tablist']`)).toHaveCount(1);
    await expect(page.locator(`${settingsHost} [role='tab']`)).toHaveCount(SETTINGS_SECTIONS.length);
  });

  test("the tablist is a single Tab stop (roving tabindex)", async ({ page }) => {
    // Six sections must not cost six Tab presses to walk past. The selected tab
    // is the only one in the tab order; the other five are reached with arrows.
    for (const section of SETTINGS_SECTIONS) {
      await openRoute(page, `#/settings/${section.id}`, "settings");

      const tabs = await page.evaluate(() =>
        [...document.querySelectorAll<HTMLElement>("[role='tab']")].map((tab) => ({
          id: tab.id,
          tabindex: tab.getAttribute("tabindex"),
          selected: tab.getAttribute("aria-selected"),
        })),
      );

      expect(tabs, `${section.id}: the tablist must render every section`).toHaveLength(
        SETTINGS_SECTIONS.length,
      );
      const stops = tabs.filter((tab) => tab.tabindex !== "-1");
      expect(
        stops.map((tab) => tab.id),
        `${section.id}: exactly one tab may be in the tab order, got ${JSON.stringify(tabs)}`,
      ).toEqual([`settings-tab-${section.id}`]);
      expect(stops[0].tabindex).toBe("0");
      expect(stops[0].selected, "the single Tab stop is the selected tab").toBe("true");
    }

    // And the tab order proves it: from the selected tab, one Tab press leaves
    // the tablist entirely rather than stepping to the next section.
    await openRoute(page, "#/settings/general", "settings");
    await page.locator("#settings-tab-general").focus();
    await page.keyboard.press("Tab");
    const afterTab = await page.evaluate(() => {
      const active = document.activeElement as HTMLElement | null;
      return { id: active?.id ?? "", role: active?.getAttribute("role") ?? "" };
    });
    expect(afterTab.role, `Tab moved to another tab (${afterTab.id})`).not.toBe("tab");
  });

  test("a section reached by URL survives a reload", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");
    await page.reload({ waitUntil: "domcontentloaded" });
    await waitForPage(page, "settings");

    expect(await currentHash(page)).toBe("#/settings/plugins");
    await expect(page.locator("#settings-page-title")).toHaveText("Plugins & Runners");
  });
});

test.describe("Plugins & Runners browser", () => {
  test("the section opens the plugin browser, not the Wine panel", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");

    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();
    await expect(page.locator("#wine-settings-panel")).toBeHidden();
    await expect(page.locator("#wallpaper-plugin-panel")).toBeHidden();

    const installed = page.locator(".plugin-row");
    await expect(installed).toHaveCount(2);
    await expect(installed.nth(0)).toContainText("Wine");
    await expect(installed.nth(1)).toContainText("Wallpaper Searcher");
    await expect(page.locator("[data-plugin-open='wine']")).toBeVisible();
    await expect(page.locator("[data-plugin-open='wallpaper-searcher']")).toBeVisible();

    const catalogue = page.locator(".plugin-catalog-row");
    await expect(catalogue).toHaveCount(6);
    await expect(page.locator("[data-plugin-install='astris']")).toHaveAttribute(
      "aria-label",
      "Install Astris Emulator",
    );
    await expect(page.locator("[data-plugin-install='ps2']")).toHaveAttribute(
      "aria-label",
      "Install PlayStation 2 Emulator",
    );
    await expect(page.locator("[data-plugin-install='dolphin']")).toHaveAttribute(
      "aria-label",
      "Install Dolphin Emulator",
    );
  });

  test("the chevron opens a plugin's settings and back returns to the browser", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");

    await page.locator("[data-plugin-open='wine']").click();
    await expect(page.locator("#wine-settings-panel")).toBeVisible();
    await expect(page.locator("#plugins-catalog-panel")).toBeHidden();

    await page.locator("#wine-settings-panel [data-plugin-back]").click();
    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();
    await expect(page.locator("#wine-settings-panel")).toBeHidden();

    await page.locator("[data-plugin-open='wallpaper-searcher']").click();
    await expect(page.locator("#wallpaper-plugin-panel")).toBeVisible();
    await expect(page.locator("#plugins-catalog-panel")).toBeHidden();
    await expect(page.locator("#wallpaper-plugin-panel")).toContainText(
      "Steam Store, Wikimedia Commons and Openverse work without any keys",
    );

    await page.locator("#wallpaper-igdb-client-id").fill("twitch-client");
    await page.locator("#wallpaper-igdb-client-secret").fill("secret");
    await page.locator("#wallpaper-google-api-key").fill("api-key");
    await page.locator("#wallpaper-google-cse-id").fill("cse-id");
    await page.locator("#wallpaper-credentials-save").click();
    await expect(page.locator("#toast")).toHaveText(/Wallpaper keys are saved in the Orivo desktop app/);
    await expect(page.locator("#wallpaper-igdb-client-id")).toHaveValue("twitch-client");

    await page.locator("#wallpaper-plugin-panel [data-plugin-back]").click();
    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();
  });

  test("the catalogue search filters the available plugins", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");

    await page.locator("#plugins-catalog-search").fill("dolphin");
    await expect(page.locator(".plugin-catalog-row")).toHaveCount(1);
    await expect(page.locator(".plugin-catalog-row")).toContainText("Dolphin Emulator");
    await expect(page.locator("#plugins-catalog-empty")).toBeHidden();

    await page.locator("#plugins-catalog-search").fill("xbox");
    await expect(page.locator("#plugins-catalog-empty")).toBeVisible();
    await expect(page.locator(".plugin-catalog-row")).toHaveCount(0);

    await page.locator("#plugins-catalog-search").fill("");
    await expect(page.locator(".plugin-catalog-row")).toHaveCount(6);
  });

  test("an install button is a placeholder that stays on the browser", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");

    await page.locator("[data-plugin-install='astris']").click();
    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();
    await expect(page.locator("#toast")).toHaveText(/install/i);
  });
});

test.describe("Settings ships no fake controls", () => {
  const FORBIDDEN = [
    { label: "language", pattern: /\blanguages?\b|\blocale\b/i },
    { label: "autostart", pattern: /auto[- ]?start|start (?:at|on) (?:login|startup)|launch at login|open at login/i },
    { label: "tray", pattern: /\btray\b|menu ?bar icon/i },
    { label: "updater", pattern: /check for updates?|auto[- ]?updat|update channel|install updates?/i },
    { label: "screenshot folder", pattern: /screenshots?/i },
  ];

  test("no control offers language, autostart, tray, updater or screenshot folders", async ({
    page,
  }) => {
    const seen: string[] = [];

    for (const section of SETTINGS_SECTIONS) {
      await openRoute(page, `#/settings/${section.id}`, "settings");

      const labels = await page.evaluate(() => {
        const panel = [...document.querySelectorAll<HTMLElement>("[data-settings-panel]")].find(
          (candidate) => !candidate.hidden,
        );
        if (!panel) return [] as string[];
        const controlNames = [...panel.querySelectorAll<HTMLElement>("button, input, select, textarea, a[href]")].map(
          (control) =>
            control.getAttribute("aria-label") ??
            control.id ??
            (control.textContent ?? "").trim(),
        );
        const rowHeadings = [
          ...panel.querySelectorAll<HTMLElement>(
            ".settings-row__copy strong, .settings-card__copy strong, legend, label",
          ),
        ].map((node) => (node.textContent ?? "").trim());
        return [...controlNames, ...rowHeadings];
      });

      seen.push(...labels.map((label) => `${section.id}: ${label}`));
    }

    expect(seen.length, "the settings sections must expose some controls to check").toBeGreaterThan(5);
    for (const forbidden of FORBIDDEN) {
      const offenders = seen.filter((entry) => forbidden.pattern.test(entry));
      expect(offenders, `Settings must not offer a ${forbidden.label} control`).toEqual([]);
    }
  });

  test("the controls that do exist are the real ones", async ({ page }) => {
    await openRoute(page, "#/settings/general", "settings");
    await expect(page.locator("#preference-start-page")).toBeVisible();
    await expect(page.locator("#preference-start-page")).toHaveValue("library");
    await expect(page.locator("#preference-store-region")).toHaveValue("automatic");
    await expect(page.locator("#reset-preferences")).toBeVisible();

    await openRoute(page, "#/settings/appearance", "settings");
    await expect(page.locator("input[name='motion-preference']")).toHaveCount(2);
    await expect(page.locator("input[name='motion-preference'][value='system']")).toBeChecked();

    await openRoute(page, "#/settings/data", "settings");
    await expect(page.locator("#derived-cache-size")).toHaveText("0 B");
    await expect(page.locator("#derived-cache-freshness")).toHaveText("Not refreshed yet");

    await openRoute(page, "#/settings/about", "settings");
    await expect(page.locator("#about-app-version")).toHaveText("Development build");
    await expect(page.locator("#about-tauri-version")).toHaveText("Browser preview");
    await expect(page.locator(`${settingsHost} .settings-attributions li`)).toHaveCount(4);
  });
});
