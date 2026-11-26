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
    await page.locator("[data-segmented='startPage'] [aria-pressed='true']").focus();
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
    // The Store nav link always requests the PC platform by default.
    expect(await currentHash(page)).toBe("#/store?platform=pc");
  });
});

test.describe("Settings navigation", () => {
  test("every section is reachable by URL", async ({ page }) => {
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

  test("every section is reachable from the sidebar", async ({ page }) => {
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
    // Five sections must not cost five Tab presses to walk past. The selected
    // tab is the only one in the tab order; the others are reached with arrows.
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
    // Nothing is open yet: the detail column asks for a pick.
    await expect(page.locator("#plugins-detail-empty")).toBeVisible();
    // Runner profiles live in the detail of the plugin that provides them.
    await expect(page.locator("#runners-panel")).toBeHidden();

    // The built-ins are cards, each with its category pill. Four "coming
    // soon" cards (Spotify, Moonlight/Sunshine, Playnite, Ludusavi) sit in
    // the catalogue, shown only with Beta features on.
    const installed = page.locator("#plugins-installed-list .plugin-card");
    await expect(installed).toHaveCount(2);
    await expect(installed.nth(0)).toContainText("Wine");
    await expect(installed.nth(0).locator(".plugin-pill")).toHaveText("Compatibility");
    await expect(installed.nth(1)).toContainText("Wallpaper Searcher");
    await expect(installed.nth(1).locator(".plugin-pill")).toHaveText("Visuals");
    await expect(page.locator(".plugin-card--soon")).toHaveCount(4);
    await expect(page.locator("[data-plugin-open='runners']")).toHaveCount(0);

    // This suite runs in a plain browser with no Tauri, so the live registry
    // answers nothing and no real catalogue card is ever rendered — but the
    // "coming soon" teasers fill the panel, so the empty-registry notice
    // (`refs.pluginsCatalogEmpty`, gated on `matches.length + soon.length`)
    // has nothing to say and stays hidden.
    await expect(page.locator(".plugin-card--catalog:not(.plugin-card--soon)")).toHaveCount(0);
    await expect(page.locator("#plugins-catalog-empty")).toBeHidden();
    // Install from file is a debug control: it only shows with Beta features on.
    await expect(page.locator("[data-plugin-install-file]")).toBeHidden();
  });

  test("a card opens its plugin's settings beside the list", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");

    await page.locator("[data-plugin-open='wine']").click();
    await expect(page.locator("#wine-settings-panel")).toBeVisible();
    // The list and the catalogue stay: only the detail column changes.
    await expect(page.locator("#plugins-installed-list")).toBeVisible();
    await expect(page.locator("#plugins-catalog-group")).toBeVisible();
    await expect(page.locator("[data-plugin-card='wine']")).toHaveClass(/is-selected/);

    await page.locator("[data-plugin-open='wallpaper-searcher']").click();
    await expect(page.locator("#wallpaper-plugin-panel")).toBeVisible();
    await expect(page.locator("#wine-settings-panel")).toBeHidden();
    await expect(page.locator("[data-plugin-card='wallpaper-searcher']")).toHaveClass(/is-selected/);
    await expect(page.locator("[data-plugin-card='wine']")).not.toHaveClass(/is-selected/);

    await page.locator("#wallpaper-igdb-client-id").fill("twitch-client");
    await page.locator("#wallpaper-igdb-client-secret").fill("secret");
    await page.locator("#wallpaper-google-api-key").fill("api-key");
    await page.locator("#wallpaper-google-cse-id").fill("cse-id");
    await page.locator("#wallpaper-credentials-save").click();
    await expect(page.locator("#toast")).toHaveText(/Wallpaper keys are saved in the Orivo desktop app/);
    await expect(page.locator("#wallpaper-igdb-client-id")).toHaveValue("twitch-client");
  });

  test("in a narrow window a plugin's detail takes the list's place, with a way back", async ({ page }) => {
    await page.setViewportSize({ width: 620, height: 820 });
    await openRoute(page, "#/settings/plugins", "settings");
    await expect(page.locator("#plugins-detail-empty")).toBeHidden();

    await page.locator("[data-plugin-open='wallpaper-searcher']").click();
    await expect(page.locator("#wallpaper-plugin-panel")).toBeVisible();
    await expect(page.locator("#plugins-installed-list")).toBeHidden();
    await expect(page.locator("#plugins-catalog-group")).toBeHidden();

    await page.locator("#wallpaper-plugin-panel [data-plugin-back]").click();
    await expect(page.locator("#plugins-installed-list")).toBeVisible();
    await expect(page.locator("#wallpaper-plugin-panel")).toBeHidden();
  });

  test("Plugins & Runners offers automatic updates and a registry check, both no-ops without Tauri", async ({
    page,
  }) => {
    await openRoute(page, "#/settings/plugins", "settings");

    const automaticUpdates = page.locator("#plugins-automatic-updates");
    await expect(automaticUpdates).not.toBeChecked();
    await automaticUpdates.click();
    // Without a desktop host the command simply cannot run; the panel must
    // not crash or get stuck offering a promise it cannot keep.
    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();

    await page.locator("[data-plugin-refresh-registry]").click();
    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();
    await expect(page.locator(".plugin-card--catalog:not(.plugin-card--soon)")).toHaveCount(0);
  });

  test("the catalogue search survives an empty registry", async ({ page }) => {
    await openRoute(page, "#/settings/plugins", "settings");

    // Searching an empty catalogue is a no-op, not a crash, and it must not
    // start blaming the query for a registry that answered nothing.
    await page.locator("#plugins-catalog-search").fill("dolphin");
    await expect(page.locator(".plugin-card--catalog")).toHaveCount(0);
    await expect(page.locator("#plugins-catalog-empty")).toBeVisible();
    await expect(page.locator("#plugins-catalog-empty")).toHaveText(
      "There are no plugins to install right now.",
    );

    await page.locator("#plugins-catalog-search").fill("");
    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();
    await expect(page.locator("#plugins-catalog-search")).toHaveValue("");
  });

  test("installing from a file needs the desktop app and stays on the browser", async ({
    page,
  }) => {
    // Install from file is a debug control, offered with Beta features on.
    await openRoute(page, "#/settings/general", "settings");
    await page.locator(".settings-disclosure > summary").click();
    await page.locator("#preference-beta").check();
    await openRoute(page, "#/settings/plugins", "settings");

    await page.locator("[data-plugin-install-file]").click();
    await expect(page.locator("#plugins-catalog-panel")).toBeVisible();
    await expect(page.locator("#toast")).toHaveText(/desktop app/i);
  });
});

test.describe("Settings ships no fake controls", () => {
  const FORBIDDEN = [
    { label: "autostart", pattern: /auto[- ]?start|start (?:at|on) (?:login|startup)|launch at login|open at login/i },
    { label: "tray", pattern: /\btray\b|menu ?bar icon/i },
    { label: "updater", pattern: /check for updates?|auto[- ]?updat|update channel|install updates?/i },
    { label: "screenshot folder", pattern: /screenshots?/i },
  ];

  test("no control offers autostart, tray, updater or screenshot folders", async ({
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
    const lit = (preference: string) =>
      page.locator(`[data-segmented='${preference}'] [aria-pressed='true']`);
    await expect(lit("startPage")).toHaveAttribute("data-segment-value", "library");
    await expect(page.locator("#preference-store-region")).toHaveValue("automatic");
    await expect(page.locator("#reset-preferences")).toBeVisible();

    // "full" joined "system"/"reduced" when the Windows animation fix landed
    // (`b5809d2`), letting a player opt back into motion its OS says to skip;
    // it is the default since Appearance folded into General.
    await expect(page.locator("[data-segmented='motion'] button")).toHaveCount(3);
    await expect(lit("motion")).toHaveAttribute("data-segment-value", "full");

    await openRoute(page, "#/settings/data", "settings");
    await expect(page.locator("#derived-cache-size")).toHaveText("0 B");
    await expect(page.locator("#derived-cache-freshness")).toHaveText("Not refreshed yet");

    await openRoute(page, "#/settings/about", "settings");
    await expect(page.locator("#about-app-version")).toHaveText("Development build");
    await expect(page.locator("#about-tauri-version")).toHaveText("Browser preview");
    await expect(page.locator(`${settingsHost} .settings-attributions li`)).toHaveCount(4);
  });
});
