import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { NavStepDetail, SpatialNav } from "./spatial-nav";
import { NAV_STEP_EVENT, createSpatialNav } from "./spatial-nav";

/**
 * jsdom has no layout engine, so every rect would come back as 0x0 and the
 * engine would see nothing focusable. Tests place their elements by hand.
 */
const place = (element: HTMLElement, left: number, top: number, width = 100, height = 40): void => {
  element.getBoundingClientRect = () =>
    ({
      left,
      top,
      right: left + width,
      bottom: top + height,
      width,
      height,
      x: left,
      y: top,
      toJSON: () => ({}),
    }) as DOMRect;
};

const button = (label: string, left: number, top: number): HTMLButtonElement => {
  const node = document.createElement("button");
  node.type = "button";
  node.textContent = label;
  node.dataset.testLabel = label;
  place(node, left, top);
  return node;
};

const press = (key: string): void => {
  window.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
};

const focusedLabel = (): string | undefined =>
  (document.activeElement as HTMLElement | null)?.dataset.testLabel;

describe("spatial navigation", () => {
  let nav: SpatialNav;
  let hooks: {
    openGame: ReturnType<typeof vi.fn<(gameId: string) => void>>;
    launchGame: ReturnType<typeof vi.fn<(gameId: string) => void>>;
    back: ReturnType<typeof vi.fn<() => void>>;
  };
  let page: HTMLElement;

  beforeEach(() => {
    document.body.innerHTML = "";
    page = document.createElement("div");
    page.className = "app-page";
    place(page, 0, 0, 1000, 800);
    document.body.append(page);

    hooks = {
      openGame: vi.fn<(gameId: string) => void>(),
      launchGame: vi.fn<(gameId: string) => void>(),
      back: vi.fn<() => void>(),
    };
    nav = createSpatialNav(hooks);
  });

  afterEach(() => {
    nav.destroy();
    document.body.innerHTML = "";
  });

  it("walks a horizontal rail with the arrow keys", () => {
    const first = button("first", 0, 400);
    const second = button("second", 120, 400);
    const third = button("third", 240, 400);
    page.append(first, second, third);

    first.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("second");
    press("ArrowRight");
    expect(focusedLabel()).toBe("third");
    press("ArrowLeft");
    expect(focusedLabel()).toBe("second");
  });

  it("prefers the aligned candidate over the merely closest one", () => {
    const origin = button("origin", 0, 400);
    // Nearer in raw pixels, but on another row.
    const offRow = button("off-row", 130, 300);
    const sameRow = button("same-row", 300, 400);
    page.append(origin, offRow, sameRow);

    origin.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("same-row");
  });

  it("moves between rows without leaving the column", () => {
    const topLeft = button("top-left", 0, 100);
    const topRight = button("top-right", 400, 100);
    const bottomLeft = button("bottom-left", 0, 300);
    const bottomRight = button("bottom-right", 400, 300);
    page.append(topLeft, topRight, bottomLeft, bottomRight);

    topRight.focus();
    press("ArrowDown");
    expect(focusedLabel()).toBe("bottom-right");
    press("ArrowUp");
    expect(focusedLabel()).toBe("top-right");
  });

  it("stays put at the edge of the page", () => {
    const only = button("only", 0, 400);
    page.append(only);

    only.focus();
    press("ArrowLeft");
    expect(focusedLabel()).toBe("only");
  });

  /**
   * The rail is the page's own verb: left and right step through the entries
   * and wrap, rather than scoring whatever control sits nearest.
   */
  const buildRail = (): { rail: HTMLDivElement; cards: HTMLButtonElement[] } => {
    const rail = document.createElement("div");
    rail.setAttribute("data-nav-rail", "");
    place(rail, 0, 400, 400, 120);
    const cards = ["first", "second", "third"].map((label, index) => {
      const card = button(label, index * 120, 400);
      card.dataset.navItem = "";
      return card;
    });
    cards[1]!.setAttribute("data-nav-selected", "");
    rail.append(...cards);
    page.append(rail);
    return { rail, cards };
  };

  /** A band of controls: the topbar, the browse bar, a strip of chips. */
  const row = (left: number, top: number, width: number, height: number): HTMLDivElement => {
    const band = document.createElement("div");
    band.setAttribute("data-nav-row", "");
    place(band, left, top, width, height);
    page.append(band);
    return band;
  };

  /** Moves the selection the way the library does when the keys ask it to. */
  const answerSteps = (rail: HTMLElement): void => {
    rail.addEventListener(NAV_STEP_EVENT, (event) => {
      const items = Array.from(rail.querySelectorAll<HTMLElement>("[data-nav-item]"));
      const index = items.findIndex((item) => item.hasAttribute("data-nav-selected"));
      const { delta } = (event as CustomEvent<NavStepDetail>).detail;
      items[index]?.removeAttribute("data-nav-selected");
      items[(index + delta + items.length) % items.length]?.setAttribute("data-nav-selected", "");
      event.preventDefault();
    });
  };

  it("steps along the rail on left and right, wrapping at the ends", () => {
    const { cards } = buildRail();

    cards[0]!.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("second");
    press("ArrowRight");
    expect(focusedLabel()).toBe("third");
    press("ArrowRight");
    expect(focusedLabel()).toBe("first");
    press("ArrowLeft");
    expect(focusedLabel()).toBe("third");
  });

  it("lets the page decide which entry comes next, then follows its selection", () => {
    const { rail, cards } = buildRail();
    answerSteps(rail);

    cards[1]!.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("third");
    expect(cards[2]!.hasAttribute("data-nav-selected")).toBe(true);
  });

  it("steps out of a control sitting inside an entry", () => {
    const { rail } = buildRail();
    const heart = button("heart", 60, 410);
    rail.append(heart);

    heart.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("first");
    press("ArrowRight");
    expect(focusedLabel()).toBe("second");
  });

  it("walks a row sideways and stops at its ends", () => {
    const topbar = row(0, 0, 1000, 60);
    const link = button("link", 0, 10);
    const search = button("search", 300, 10);
    topbar.append(link, search);

    link.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("search");
    press("ArrowRight");
    expect(focusedLabel()).toBe("search");
  });

  it("carries a row on into the band beside it on the same line, and no further", () => {
    const categories = row(0, 600, 300, 40);
    const last = button("last-category", 150, 600);
    categories.append(button("first-category", 0, 600), last);
    const platforms = row(400, 600, 300, 40);
    platforms.append(button("first-platform", 400, 600));
    // Below the strip, and nearer in raw pixels than the platforms.
    page.append(button("banner", 300, 700));

    last.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("first-platform");
    press("ArrowRight");
    expect(focusedLabel()).toBe("first-platform");
  });

  /**
   * The library's scene, stacked the way it is on screen: a topbar whose
   * current entry is not the one lined up with anything, Play on the left, a
   * rail whose selected card is not the one under the key's column.
   */
  const buildScene = (): {
    current: HTMLButtonElement;
    search: HTMLButtonElement;
    play: HTMLButtonElement;
    rail: HTMLDivElement;
    cards: HTMLButtonElement[];
  } => {
    const topbar = row(0, 0, 1000, 60);
    const current = button("current", 0, 10);
    current.setAttribute("aria-current", "page");
    const other = button("other", 120, 10);
    const search = button("search", 700, 10);
    topbar.append(current, other, search);

    const actions = document.createElement("div");
    actions.dataset.navSteps = "";
    place(actions, 40, 300, 100, 40);
    const play = button("play", 40, 300);
    actions.append(play);
    page.append(actions);

    const { rail, cards } = buildRail();
    // A card right under the search field, so geometry alone would skip Play.
    const under = button("under", 700, 400);
    under.dataset.navItem = "";
    rail.append(under);
    place(rail, 0, 400, 1000, 120);
    return { current, search, play, rail, cards: [...cards, under] };
  };

  it("climbs from any card to the scene's action, then to the topbar's current entry", () => {
    const { cards } = buildScene();

    cards[3]!.focus();
    press("ArrowUp");
    expect(focusedLabel()).toBe("play");
    press("ArrowUp");
    expect(focusedLabel()).toBe("current");
    press("ArrowUp");
    expect(focusedLabel()).toBe("current");
  });

  it("comes down from any topbar entry through the action to the selected card", () => {
    const { search } = buildScene();

    search.focus();
    press("ArrowDown");
    expect(focusedLabel()).toBe("play");
    press("ArrowDown");
    expect(focusedLabel()).toBe("second");
  });

  it("comes back to the control a band had last", () => {
    const { play } = buildScene();
    const other = page.querySelector<HTMLElement>("[data-test-label='other']")!;

    other.focus();
    press("ArrowDown");
    expect(focusedLabel()).toBe("play");
    press("ArrowUp");
    expect(focusedLabel()).toBe("other");
    expect(document.activeElement).not.toBe(play);
  });

  it("changes the game from Play and leaves focus on Play", () => {
    const { play, rail, cards } = buildScene();
    answerSteps(rail);

    play.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("play");
    expect(cards[2]!.hasAttribute("data-nav-selected")).toBe(true);
    press("ArrowLeft");
    press("ArrowLeft");
    expect(focusedLabel()).toBe("play");
    expect(cards[0]!.hasAttribute("data-nav-selected")).toBe(true);
  });

  it("moves focus onto the next card from Play when the page does not answer", () => {
    const { play } = buildScene();

    play.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("third");
  });

  it("starts from the selected card when nothing holds focus", () => {
    const { rail } = buildScene();
    answerSteps(rail);

    (document.activeElement as HTMLElement | null)?.blur();
    press("ArrowUp");
    expect(focusedLabel()).toBe("play");

    (document.activeElement as HTMLElement | null)?.blur();
    press("ArrowRight");
    expect(focusedLabel()).toBe("third");
  });

  it("lands on the page's anchor when nothing holds focus and no rail says where it is", () => {
    const back = button("back", 0, 100);
    const primary = button("primary", 0, 300);
    primary.dataset.navAnchor = "";
    page.append(back, primary);

    (document.activeElement as HTMLElement | null)?.blur();
    press("ArrowDown");
    expect(focusedLabel()).toBe("primary");
  });

  /** The settings column: tabs stacked beside the section they open. */
  const buildColumn = (): { tabs: HTMLButtonElement[]; control: HTMLButtonElement } => {
    const topbar = row(0, 0, 1000, 60);
    const settings = button("settings", 300, 10);
    settings.setAttribute("aria-current", "page");
    topbar.append(settings);

    const list = document.createElement("div");
    list.setAttribute("role", "tablist");
    place(list, 0, 100, 200, 300);
    const tabs = ["general", "libraries", "plugins"].map((label, index) => {
      const tab = button(label, 0, 100 + index * 60);
      tab.setAttribute("role", "tab");
      tab.setAttribute("aria-selected", String(index === 1));
      return tab;
    });
    list.append(...tabs);
    page.append(list);

    const control = button("control", 600, 230);
    page.append(control);
    return { tabs, control };
  };

  it("walks a column of tabs with up and down and climbs out of its top", () => {
    const { tabs } = buildColumn();

    tabs[0]!.focus();
    press("ArrowDown");
    expect(focusedLabel()).toBe("libraries");
    press("ArrowUp");
    press("ArrowUp");
    expect(focusedLabel()).toBe("settings");
    press("ArrowDown");
    expect(focusedLabel()).toBe("libraries");
  });

  it("leaves a column sideways for the section, and comes back to the selected tab", () => {
    const { tabs } = buildColumn();

    tabs[0]!.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("control");
    press("ArrowLeft");
    expect(focusedLabel()).toBe("libraries");
  });

  it("never skips a row lying between a control and what lines up above it", () => {
    // The settings tabs laid across a phone: the section's first control lines
    // up with the topbar's bell, not with any tab.
    const topbar = row(0, 0, 1000, 50);
    const bell = button("bell", 800, 5);
    topbar.append(bell);
    const list = document.createElement("div");
    list.setAttribute("role", "tablist");
    place(list, 0, 60, 700, 40);
    const general = button("general", 0, 60);
    general.setAttribute("role", "tab");
    general.setAttribute("aria-selected", "true");
    const about = button("about", 600, 60);
    about.setAttribute("role", "tab");
    list.append(general, about);
    const select = button("select", 780, 200);
    page.append(list, select);

    select.focus();
    press("ArrowUp");
    expect(focusedLabel()).toBe("general");
    press("ArrowUp");
    expect(focusedLabel()).toBe("bell");
  });

  it("offers B to whatever is open before it goes back", () => {
    const only = button("only", 0, 400);
    page.append(only);
    let open = true;
    const closePanel = (event: KeyboardEvent): void => {
      if (event.key !== "Escape" || !open) return;
      open = false;
      event.preventDefault();
    };
    window.addEventListener("keydown", closePanel);

    only.focus();
    nav.back();
    expect(open).toBe(false);
    expect(hooks.back).not.toHaveBeenCalled();

    nav.back();
    expect(hooks.back).toHaveBeenCalledTimes(1);
    window.removeEventListener("keydown", closePanel);
  });

  it("keeps left and right on the line a control sits on", () => {
    const origin = button("origin", 0, 400);
    page.append(origin, button("above-right", 300, 200));

    origin.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("origin");
  });

  it("walks past pointer-only affordances", () => {
    const first = button("first", 0, 400);
    const arrow = button("arrow", 120, 400);
    arrow.dataset.navSkip = "";
    const last = button("last", 240, 400);
    page.append(first, arrow, last);

    first.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("last");
  });

  it("never parks focus on a focusable container of other controls", () => {
    const tab = button("tab", 0, 100);
    const panel = document.createElement("section");
    panel.tabIndex = 0;
    place(panel, 200, 100, 600, 400);
    const inner = button("inner", 400, 120);
    panel.append(inner);
    page.append(tab, panel);

    tab.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("inner");
  });

  it("opens the game page on 'a' and launches it on Enter", () => {
    const card = button("card", 0, 400);
    card.dataset.navOpen = "steam:42";
    card.dataset.navLaunch = "steam:42";
    page.append(card);

    card.focus();
    press("a");
    expect(hooks.openGame).toHaveBeenCalledWith("steam:42");
    expect(hooks.launchGame).not.toHaveBeenCalled();

    press("Enter");
    expect(hooks.launchGame).toHaveBeenCalledWith("steam:42");
  });

  it("leaves Enter to the browser on a control that is not a game", () => {
    const plain = button("plain", 0, 400);
    const clicked = vi.fn();
    plain.addEventListener("click", clicked);
    page.append(plain);

    plain.focus();
    const event = new KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true });
    window.dispatchEvent(event);

    expect(event.defaultPrevented).toBe(false);
    expect(hooks.launchGame).not.toHaveBeenCalled();
  });

  it("presses a plain control with 'a'", () => {
    const plain = button("plain", 0, 400);
    const clicked = vi.fn();
    plain.addEventListener("click", clicked);
    page.append(plain);

    plain.focus();
    press("a");
    expect(clicked).toHaveBeenCalledTimes(1);
    expect(hooks.openGame).not.toHaveBeenCalled();
  });

  it("never walks into a page that is hidden or inert", () => {
    const here = button("here", 0, 400);
    page.append(here);

    const other = document.createElement("div");
    other.className = "app-page";
    other.hidden = true;
    other.inert = true;
    place(other, 0, 0, 1000, 800);
    const offscreen = button("offscreen", 300, 400);
    other.append(offscreen);
    document.body.append(other);

    here.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("here");
  });

  it("keeps focus inside an open modal", () => {
    const behind = button("behind", 0, 400);
    page.append(behind);

    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    dialog.setAttribute("aria-modal", "true");
    place(dialog, 200, 200, 400, 400);
    const inside = button("inside", 220, 220);
    const alsoInside = button("also-inside", 220, 300);
    dialog.append(inside, alsoInside);
    page.append(dialog);

    inside.focus();
    press("ArrowDown");
    expect(focusedLabel()).toBe("also-inside");
    press("ArrowLeft");
    expect(focusedLabel()).toBe("also-inside");
  });

  it("skips disabled controls", () => {
    const first = button("first", 0, 400);
    const blocked = button("blocked", 120, 400);
    blocked.disabled = true;
    const last = button("last", 240, 400);
    page.append(first, blocked, last);

    first.focus();
    press("ArrowRight");
    expect(focusedLabel()).toBe("last");
  });

  it("yields to a handler that already claimed the key", () => {
    const first = button("first", 0, 400);
    const second = button("second", 120, 400);
    page.append(first, second);

    first.focus();
    const event = new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true });
    event.preventDefault();
    window.dispatchEvent(event);

    expect(focusedLabel()).toBe("first");
  });

  it("ignores the arrow keys while the user is typing", () => {
    const field = document.createElement("textarea");
    place(field, 0, 100);
    const target = button("target", 0, 400);
    page.append(field, target);

    field.focus();
    field.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true, cancelable: true }));
    expect(document.activeElement).toBe(field);
  });

  it("leaves a one-line field with up and down, and sideways only from the caret's edge", () => {
    const topbar = row(0, 0, 1000, 60);
    const link = button("link", 0, 10);
    const field = document.createElement("input");
    field.type = "search";
    place(field, 300, 10);
    topbar.append(link, field);
    const below = button("below", 300, 300);
    page.append(below);

    const typeKey = (key: string): KeyboardEvent => {
      const event = new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true });
      field.dispatchEvent(event);
      return event;
    };

    field.value = "elden";
    field.focus();
    field.setSelectionRange(2, 2);
    expect(typeKey("ArrowLeft").defaultPrevented).toBe(false);
    expect(document.activeElement).toBe(field);

    field.setSelectionRange(0, 0);
    typeKey("ArrowLeft");
    expect(focusedLabel()).toBe("link");

    field.focus();
    typeKey("ArrowDown");
    expect(focusedLabel()).toBe("below");
  });

  it("marks the focused element so a controller gets a visible ring", () => {
    const first = button("first", 0, 400);
    const second = button("second", 120, 400);
    page.append(first, second);

    first.focus();
    press("ArrowRight");
    expect(second.dataset.navFocus).toBe("");
    expect(first.dataset.navFocus).toBeUndefined();

    nav.setInputMode("pointer");
    expect(second.dataset.navFocus).toBeUndefined();
  });

  it("falls back to the first control when nothing holds focus", () => {
    const first = button("first", 0, 400);
    const second = button("second", 120, 400);
    page.append(first, second);

    (document.activeElement as HTMLElement | null)?.blur();
    press("ArrowRight");
    expect(focusedLabel()).toBe("first");
  });

  it("goes back when there is nothing left to close", () => {
    const only = button("only", 0, 400);
    page.append(only);

    only.focus();
    press("Backspace");
    expect(hooks.back).toHaveBeenCalledTimes(1);
  });
});
