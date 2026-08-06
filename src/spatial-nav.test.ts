import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SpatialNav } from "./spatial-nav";
import { createSpatialNav } from "./spatial-nav";

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
    const field = document.createElement("input");
    place(field, 0, 100);
    const target = button("target", 0, 400);
    page.append(field, target);

    field.focus();
    press("ArrowDown");
    expect(document.activeElement).toBe(field);
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
