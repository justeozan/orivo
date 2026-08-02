import { expect, type Locator, type Page } from "@playwright/test";

/**
 * Shared vocabulary for the Orivo end-to-end suite.
 *
 * Run it with `pnpm test:e2e` (see playwright.config.ts for the flags).
 *
 * Every spec runs against `pnpm dev`, i.e. a plain browser with no Tauri
 * runtime. `isTauriRuntime()` is false there, so the Store serves
 * `EDITORIAL_STORE_HOME`, the game detail serves `createFallbackGameDetail()`,
 * and Settings serves `DEFAULT_PREFERENCES`. All of that is deterministic,
 * which is what makes the assertions and the goldens stable.
 */

/** The library ids that `fallbackLibrary` renders in browser mode. */
export const FALLBACK_LIBRARY_IDS = [
  "showcase-elden-ring",
  "showcase-cyberpunk-2077",
  "showcase-baldurs-gate-3",
  "showcase-hades-2",
  "showcase-red-dead-redemption-2",
  "showcase-the-witcher-3",
  "showcase-horizon-forbidden-west",
  "showcase-god-of-war",
  "showcase-unrailed",
  "showcase-astro-duel-2",
] as const;

/** Editorial Store ids, in `EDITORIAL_GAMES` order. */
export const EDITORIAL_GAME_IDS = [
  "steam:1245620",
  "steam:1091500",
  "steam:1086940",
  "steam:1145350",
  "steam:1174180",
  "steam:292030",
  "steam:2420110",
  "steam:1593500",
  "steam:1016920",
  "steam:655350",
] as const;

/** Games tagged "Short Sessions" — the combined-filter fixture. */
export const SHORT_SESSION_IDS = ["steam:1145350", "steam:1016920", "steam:655350"] as const;

export const SETTINGS_SECTIONS = [
  { id: "general", title: "General" },
  { id: "libraries", title: "Libraries & Sources" },
  { id: "plugins", title: "Plugins & Runners" },
  { id: "appearance", title: "Appearance" },
  { id: "data", title: "Data" },
  { id: "about", title: "About" },
] as const;

export type PageName = "library" | "store" | "game" | "settings" | "not-found";

const HOST_ID: Record<PageName, string> = {
  library: "#app-page-library",
  store: "#app-page-store",
  game: "#app-page-game",
  settings: "#app-page-settings",
  "not-found": "#app-page-not-found",
};

/** The one page host that is not `hidden`. */
export function activeHost(page: Page): Locator {
  return page.locator(".app-page:not([hidden])");
}

export function host(page: Page, name: PageName): Locator {
  return page.locator(HOST_ID[name]);
}

/** Waits until the named page host is visible and has painted real content. */
export async function waitForPage(page: Page, name: PageName): Promise<void> {
  const ready: Record<PageName, string> = {
    library: `${HOST_ID.library}:not([hidden]) #hero-title`,
    // `.store-catalog__title` is present for both the populated rail and the
    // empty state, so a filter that matches nothing still counts as "ready".
    store: `${HOST_ID.store}:not([hidden]) .store-catalog__title`,
    game: `${HOST_ID.game}:not([hidden]) .gd-hero__title`,
    settings: `${HOST_ID.settings}:not([hidden]) .settings-layout`,
    "not-found": `${HOST_ID["not-found"]}:not([hidden]) .not-found__title`,
  };
  await page.waitForSelector(ready[name], { state: "visible" });
  // One frame so the page's own `requestAnimationFrame` restore work has run.
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
}

/** Full document load of a hash route (never a same-document hash swap). */
export async function openRoute(page: Page, hash: string, name: PageName): Promise<void> {
  await page.goto(`/${hash}`, { waitUntil: "domcontentloaded" });
  await waitForPage(page, name);
}

/** The route currently in the address bar, without the leading `#`. */
export async function currentHash(page: Page): Promise<string> {
  return page.evaluate(() => window.location.hash);
}

/** Moves keyboard focus out of every control so key specs test the shell. */
export async function blurEverything(page: Page): Promise<void> {
  await page.evaluate(() => {
    const active = document.activeElement;
    if (active instanceof HTMLElement) active.blur();
  });
}

export interface Box {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The shell topbar's bounding box, rounded to 1/100 px. */
export async function topbarBox(page: Page): Promise<Box> {
  return page.evaluate(() => {
    const rect = document.querySelector("header.topbar")!.getBoundingClientRect();
    const round = (value: number): number => Math.round(value * 100) / 100;
    return { x: round(rect.x), y: round(rect.y), width: round(rect.width), height: round(rect.height) };
  });
}

export function boxDelta(a: Box, b: Box): number {
  return Math.max(
    Math.abs(a.x - b.x),
    Math.abs(a.y - b.y),
    Math.abs(a.width - b.width),
    Math.abs(a.height - b.height),
  );
}

/** `--topbar-height` as resolved on `.selector`. */
export async function topbarHeightVar(page: Page): Promise<number> {
  return page.evaluate(() => {
    const shell = document.querySelector(".selector")!;
    return Number.parseFloat(getComputedStyle(shell).getPropertyValue("--topbar-height")) || 0;
  });
}

export interface PaddingProbe {
  hostId: string;
  hostPaddingTop: number;
  hostOverflowY: string;
  innerTag: string;
  innerClass: string;
  innerPaddingTop: number;
  innerOverflowY: string;
  contentTop: number;
  topbarHeight: number;
  topbarBottom: number;
  hostScrollHeight: number;
  hostClientHeight: number;
}

/**
 * Measures the top offset the active page spends before its first content
 * block, split between the shell host (`.app-page--scroll`) and the page's own
 * root element.
 *
 * `hostOverflowY` / `innerOverflowY` are part of the probe because the split of
 * responsibilities is the actual invariant: the host owns the clearance *and*
 * the scrolling, the page root owns neither. Two elements each adding half of
 * one is what produced the double-padding and phantom-scroll-band defects.
 */
export async function measureTopPadding(page: Page): Promise<PaddingProbe> {
  return page.evaluate(() => {
    const shell = document.querySelector(".selector")!;
    const activePage = [...document.querySelectorAll<HTMLElement>(".app-page")].find((el) => !el.hidden)!;
    const inner = activePage.firstElementChild as HTMLElement | null;
    const innerStyle = inner ? getComputedStyle(inner) : null;
    const content = activePage.querySelector<HTMLElement>(
      ".store-hero, .gd-topline, .settings-layout, .not-found",
    );
    const topbar = document.querySelector("header.topbar")!.getBoundingClientRect();
    return {
      hostId: activePage.id,
      hostPaddingTop: Number.parseFloat(getComputedStyle(activePage).paddingTop) || 0,
      hostOverflowY: getComputedStyle(activePage).overflowY,
      innerTag: inner ? inner.tagName : "",
      innerClass: inner ? inner.className : "",
      innerPaddingTop: innerStyle ? Number.parseFloat(innerStyle.paddingTop) || 0 : 0,
      innerOverflowY: innerStyle ? innerStyle.overflowY : "",
      contentTop: content ? Math.round(content.getBoundingClientRect().top * 100) / 100 : Number.NaN,
      topbarHeight: Number.parseFloat(getComputedStyle(shell).getPropertyValue("--topbar-height")) || 0,
      topbarBottom: Math.round(topbar.bottom * 100) / 100,
      hostScrollHeight: activePage.scrollHeight,
      hostClientHeight: activePage.clientHeight,
    };
  });
}

/**
 * `.selector` is the shell canvas every page host is `inset: 0` of, so anything
 * shorter than the window strands the bottom of the screen and shortens every
 * internal scroll container with it.
 */
export async function selectorHeight(page: Page): Promise<{ height: number; viewportHeight: number }> {
  return page.evaluate(() => ({
    height: Math.round(document.querySelector(".selector")!.getBoundingClientRect().height * 100) / 100,
    viewportHeight: window.innerHeight,
  }));
}

/** `true` when the document itself scrolls sideways. */
export async function documentOverflow(page: Page): Promise<{ scrollWidth: number; clientWidth: number }> {
  return page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    clientWidth: document.documentElement.clientWidth,
  }));
}

/**
 * Waits until every `<img>` that overlaps the viewport has settled. Off-screen
 * cards use `loading="lazy"` and never resolve, so they are deliberately
 * excluded — they are not in the screenshot either.
 */
export async function waitForImages(page: Page): Promise<void> {
  await page.waitForFunction(
    () => {
      const vw = document.documentElement.clientWidth;
      const vh = document.documentElement.clientHeight;
      const visible = [...document.querySelectorAll("img")].filter((img) => {
        const rect = img.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && rect.top < vh && rect.bottom > 0 && rect.left < vw && rect.right > 0;
      });
      return visible.length > 0 && visible.every((img) => img.complete);
    },
    undefined,
    { timeout: 15_000 },
  );
  // Give the compositor one paint after the last decode.
  await page.evaluate(
    () => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))),
  );
}

export interface FocusReport {
  focusKey: string | null;
  tag: string;
  className: string;
  label: string;
  focusVisible: boolean;
  outlineWidth: number;
  outlineStyle: string;
}

async function describeFocus(page: Page): Promise<FocusReport | null> {
  return page.evaluate(() => {
    const active = document.activeElement;
    if (!(active instanceof HTMLElement) || active === document.body) return null;
    const style = getComputedStyle(active);
    return {
      focusKey: active.dataset.focusKey ?? null,
      tag: active.tagName,
      className: typeof active.className === "string" ? active.className : "",
      label:
        active.getAttribute("aria-label") ??
        active.id ??
        (active.textContent ?? "").trim().slice(0, 40),
      focusVisible: active.matches(":focus-visible"),
      outlineWidth: Number.parseFloat(style.outlineWidth) || 0,
      outlineStyle: style.outlineStyle,
    };
  });
}

/**
 * Tabs forward until `match` accepts the focused element. Returns every stop on
 * the way so a failure can show the real tab order.
 */
export async function tabUntil(
  page: Page,
  match: (report: FocusReport) => boolean,
  maxSteps = 60,
): Promise<{ found: FocusReport | null; visited: FocusReport[] }> {
  const visited: FocusReport[] = [];
  for (let step = 0; step < maxSteps; step += 1) {
    await page.keyboard.press("Tab");
    const report = await describeFocus(page);
    if (!report) continue;
    visited.push(report);
    if (match(report)) return { found: report, visited };
  }
  return { found: null, visited };
}

/** Asserts the focused element renders a visible focus ring. */
export function expectVisibleFocusRing(report: FocusReport): void {
  expect(report.focusVisible, `${report.tag}.${report.className} should match :focus-visible`).toBe(true);
  expect(
    report.outlineWidth,
    `${report.tag}.${report.className} should paint an outline (got ${report.outlineWidth}px ${report.outlineStyle})`,
  ).toBeGreaterThan(0);
  expect(report.outlineStyle).not.toBe("none");
}

export interface CardVisibility {
  id: string;
  left: number;
  right: number;
  top: number;
  bottom: number;
  width: number;
  height: number;
  /**
   * The only definition of "visible" this suite accepts: the card's border box
   * lies entirely inside the visual viewport. An earlier revision compared each
   * card against the *rail's* box instead, which happily certified a rail that
   * had been pushed below the fold — every card "fit", none was on screen.
   */
  fullyVisible: boolean;
  /** Fraction of the card's own height that is above the fold, 0…1. */
  verticalVisibleRatio: number;
  /** The card's full width lies inside the viewport horizontally. */
  horizontallyInside: boolean;
  /** The card straddles the right edge — the "peek" that advertises the rail. */
  peeking: boolean;
}

export interface RailVisibility {
  viewport: { width: number; height: number };
  cards: CardVisibility[];
}

/**
 * Every Store card measured against the visual viewport (`window.innerWidth` /
 * `window.innerHeight`), never against its scroll parent.
 */
export async function storeCardVisibility(page: Page): Promise<RailVisibility> {
  return page.evaluate(() => {
    const viewport = { width: window.innerWidth, height: window.innerHeight };
    const rail = document.querySelector("#app-page-store:not([hidden]) .store-card-rail");
    if (!rail) return { viewport, cards: [] };
    const cards = [...rail.querySelectorAll<HTMLElement>(".store-card:not(.store-card--skeleton)")].map(
      (card) => {
        const rect = card.getBoundingClientRect();
        const aboveFold = Math.max(0, Math.min(rect.bottom, viewport.height) - Math.max(rect.top, 0));
        return {
          id: card.dataset.gameId ?? "",
          left: Math.round(rect.left),
          right: Math.round(rect.right),
          top: Math.round(rect.top),
          bottom: Math.round(rect.bottom),
          width: Math.round(rect.width),
          height: Math.round(rect.height),
          fullyVisible:
            rect.top >= 0 &&
            rect.bottom <= viewport.height &&
            rect.left >= 0 &&
            rect.right <= viewport.width,
          verticalVisibleRatio: rect.height > 0 ? aboveFold / rect.height : 0,
          horizontallyInside: rect.left >= 0 && rect.right <= viewport.width,
          peeking: rect.left < viewport.width && rect.right > viewport.width,
        };
      },
    );
    return { viewport, cards };
  });
}

/** Human-readable dump of a rail, used in every card-visibility failure message. */
export function describeCards(rail: RailVisibility): string {
  return rail.cards
    .map(
      (card) =>
        `  ${card.id} x:${card.left}→${card.right} y:${card.top}→${card.bottom} ` +
        `${card.width}x${card.height} full=${card.fullyVisible} ` +
        `vert=${Math.round(card.verticalVisibleRatio * 100)}%`,
    )
    .join("\n");
}

export interface MainLandmarkReport {
  total: number;
  visible: string[];
  headerInsideMain: boolean;
  headerAncestors: string;
}

/**
 * Landmark shape of the shell. The topbar is the document banner, so it must
 * sit outside every `<main>`; and no route may ever show two `<main>`s at once,
 * because a mounted-but-hidden page host keeps its own root in the DOM.
 */
export async function mainLandmarks(page: Page): Promise<MainLandmarkReport> {
  return page.evaluate(() => {
    const mains = [...document.querySelectorAll<HTMLElement>("main")];
    const visible = mains.filter((main) => {
      if (main.closest("[hidden]")) return false;
      const style = getComputedStyle(main);
      if (style.display === "none" || style.visibility === "hidden") return false;
      const rect = main.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
    const header = document.querySelector<HTMLElement>("header.topbar")!;
    const ancestors: string[] = [];
    for (let node = header.parentElement; node; node = node.parentElement) {
      ancestors.push(node.tagName.toLowerCase());
    }
    return {
      total: mains.length,
      visible: visible.map((main) => `main.${main.className}`),
      headerInsideMain: header.closest("main") !== null,
      headerAncestors: ancestors.join(" < "),
    };
  });
}

export interface SearchProbe {
  present: boolean;
  disabled: boolean;
  readOnly: boolean;
  visible: boolean;
  placeholder: string;
  ariaLabel: string | null;
  focused: boolean;
  focusWithin: boolean;
  ringWidth: number;
}

/** State of the shell search field and the ring its wrapper paints. */
export async function topbarSearchProbe(page: Page): Promise<SearchProbe> {
  return page.evaluate(() => {
    const input = document.querySelector<HTMLInputElement>("#topbar-search");
    const control = document.querySelector<HTMLElement>("#topbar-search-control");
    if (!input || !control) {
      return {
        present: false,
        disabled: true,
        readOnly: true,
        visible: false,
        placeholder: "",
        ariaLabel: null,
        focused: false,
        focusWithin: false,
        ringWidth: 0,
      };
    }
    const rect = input.getBoundingClientRect();
    // `.search-control:focus-within` paints the indicator as the first layer of
    // `box-shadow`: "rgb(195, 181, 255) 0px 0px 0px 2px". The spread of that
    // first layer is the ring width.
    const shadow = getComputedStyle(control).boxShadow;
    const firstLayer = shadow.split(/,(?![^(]*\))/)[0] ?? "";
    const lengths = firstLayer.match(/-?\d+(?:\.\d+)?px/g) ?? [];
    return {
      present: true,
      disabled: input.disabled,
      readOnly: input.readOnly,
      visible: rect.width > 0 && rect.height > 0,
      placeholder: input.placeholder,
      ariaLabel: input.getAttribute("aria-label"),
      focused: document.activeElement === input,
      focusWithin: control.matches(":focus-within"),
      ringWidth: lengths.length >= 4 ? Number.parseFloat(lengths[3]) : 0,
    };
  });
}

/**
 * Anything on the active page that spins. `store-spin` / `gd-spin` were deleted
 * as a house-style violation (DESIGN.md 13 — progress is a bar, never a disc),
 * so both a rotating animation and a spinner-shaped class name are failures.
 */
export async function rotatingLoaders(
  page: Page,
): Promise<{ animations: string[]; classNames: string[] }> {
  return page.evaluate(() => {
    const activePage = [...document.querySelectorAll<HTMLElement>(".app-page")].find((el) => !el.hidden)!;
    const nodes = [activePage, ...activePage.querySelectorAll<HTMLElement>("*")];
    const animations: string[] = [];
    for (const node of nodes) {
      for (const pseudo of ["", "::before", "::after"]) {
        const style = getComputedStyle(node, pseudo || undefined);
        const name = style.animationName;
        if (!name || name === "none") continue;
        if (/spin|rot(?:ate|ation)|circle/i.test(name)) {
          animations.push(`${node.tagName}.${node.className}${pseudo} → ${name}`);
        }
      }
    }
    const classNames = nodes
      .map((node) => (typeof node.className === "string" ? node.className : ""))
      .filter((name) => /\bspin(?:ner)?\b|\bloader\b|\bthrobber\b/i.test(name));
    return { animations, classNames };
  });
}

export async function libraryGameIds(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>("#game-cards .game-card")].map(
      (card) => card.dataset.gameId ?? "",
    ),
  );
}
