/**
 * Spatial navigation: one engine that makes every page walkable with the arrow
 * keys and, through `gamepad.ts`, with a controller.
 *
 * The app is a stack of independent page modules, so the engine deliberately
 * knows nothing about them. It reads the live DOM, keeps only what is really
 * focusable and visible on the page that is currently on screen, and picks the
 * next target geometrically. Pages opt into the two game-specific verbs by
 * tagging an element with `data-nav-open` / `data-nav-launch`.
 *
 * Pages also describe their layout, and the keys read it the way a console
 * does: up and down go from band to band, left and right move along the band
 * focus is on.
 *
 * - `data-nav-row` is a band of controls read left to right: the topbar, the
 *   browse bar, a strip of filters. Left and right walk it and stop at its
 *   ends. Up and down leave it for the next band, and coming back lands on the
 *   control it had last, else on its current one (`aria-current`,
 *   `aria-pressed`).
 * - `data-nav-rail` holds the page's games as `data-nav-item`s. Left and right
 *   step one entry and wrap; coming back lands on `data-nav-selected`, the game
 *   the page is showing. Each step is first offered to the page as a `navstep`
 *   event on the rail, because a page that renders a window of a long list is
 *   the only one that knows which game comes after the last card on screen.
 *   While the keys drive a rail it carries `data-nav-driven`, so the page can
 *   keep the next card in sight and set its pointer-only snapping aside until
 *   a hand scrolls the rail again.
 * - `data-nav-steps` marks a control that acts on the rail's selection, the
 *   library's Play: left and right on it change the game and leave focus where
 *   it is.
 * - `data-nav-skip` is a pointer affordance the keys already cover, like the
 *   scene's previous and next arrows. It stays clickable and tabbable.
 * - `role="tablist"` is a band too, walked with up and down when its tabs are
 *   stacked, and entered on the selected tab.
 *
 * With nothing focused, the keys start from the rail's selected entry, so the
 * first press already means what every later one does: left and right change
 * the game, up reaches Play.
 */

import { prefersReducedMotion } from "./motion";

export type NavDirection = "up" | "down" | "left" | "right";

export type NavInputMode = "pointer" | "keyboard" | "gamepad";

export interface SpatialNavHooks {
  /** `A` on a controller, `a` on a keyboard: enter the game's own page. */
  openGame(gameId: string): void;
  /** `Enter` on a keyboard, `X` on a controller: start the game right away. */
  launchGame(gameId: string): void;
  /** `B` / `Escape` with nothing left to close. */
  back(): void;
}

export interface SpatialNav {
  move(direction: NavDirection): boolean;
  activate(): boolean;
  launchFocused(): boolean;
  back(): void;
  focusFirst(): boolean;
  /** Put focus back on the page after a route change. */
  enterPage(): void;
  setInputMode(mode: NavInputMode): void;
  scrollBy(delta: number): void;
  destroy(): void;
}

/**
 * Dispatched, cancelable, on a rail before the keys step along it. A page that
 * moves its own selection by `delta` calls `preventDefault()`, and the engine
 * then follows whichever entry the page marks `data-nav-selected`.
 */
export const NAV_STEP_EVENT = "navstep";

export interface NavStepDetail {
  delta: 1 | -1;
}

const FOCUSABLE = [
  "a[href]",
  "button:not(:disabled)",
  "input:not(:disabled):not([type='hidden'])",
  "select:not(:disabled)",
  "textarea:not(:disabled)",
  "[tabindex]:not([tabindex='-1'])",
  "[data-nav-focusable]",
].join(",");

const BAND = "[data-nav-row], [data-nav-rail], [role='tablist']";

/** What a band shows as chosen: entering the band always lands there. */
const SELECTED = "[data-nav-selected], [role='tab'][aria-selected='true']";

/** A band's current control, where it has no memory yet: strongest claim first. */
const CURRENT = ["[aria-current]:not([aria-current='false'])", "[aria-pressed='true']"];

const isTypingTarget = (node: EventTarget | null): boolean => {
  if (!(node instanceof HTMLElement)) return false;
  if (node.isContentEditable) return true;
  const tag = node.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
};

/**
 * The element a key event really came from.
 *
 * A listener on `window` sees `event.target` retargeted to the shadow host, so
 * a keystroke typed inside a shadow root arrives looking like it landed on a
 * plain wrapper element. Sentry's feedback form is exactly that case: every
 * single-key shortcut fired while someone was writing a bug report, which ate
 * the letters bound to shortcuts. `composedPath()[0]` is the way back to the
 * field being typed in.
 */
export const composedTarget = (event: Event): HTMLElement | null => {
  const [first] = event.composedPath();
  const node = first ?? event.target;
  return node instanceof HTMLElement ? node : null;
};

/** Focus followed through any shadow roots, for the same reason. */
const deepActiveElement = (): Element | null => {
  let active: Element | null = document.activeElement;
  while (active?.shadowRoot?.activeElement) active = active.shadowRoot.activeElement;
  return active;
};

/**
 * True when a key belongs to a text field rather than to a shortcut. Both
 * checks are needed: the event path catches the keystroke, and focus catches a
 * field that is being typed into after an event dispatched straight at window.
 */
export const isTypingEvent = (event: Event): boolean =>
  isTypingTarget(composedTarget(event)) || isTypingTarget(deepActiveElement());

const SINGLE_LINE = new Set(["text", "search", "url", "tel", "password", "email"]);

/**
 * A one-line field has no use for up and down, and none for left or right once
 * the caret sits against the edge the key points at — so those presses move on
 * instead of dying in the field, and typing a search then pressing down reaches
 * the page. A field that does use the keys keeps them: a number, one with
 * suggestions, a radio group, a text area, anything inside a shadow root.
 */
const leavesField = (event: KeyboardEvent, direction: NavDirection): boolean => {
  const field = composedTarget(event);
  if (field !== document.activeElement) return false;
  // A closed select answers the arrows differently on every platform — a new
  // value on Windows, a popup on a Mac — so they walk on from it, and Space
  // (A on a pad) opens it instead.
  if (field instanceof HTMLSelectElement) return !field.multiple;
  if (!(field instanceof HTMLInputElement)) return false;
  // A switch has no use for any arrow at all.
  if (field.type === "checkbox") return true;
  if (!SINGLE_LINE.has(field.type) || field.list !== null) return false;
  if (field.getAttribute("role") === "combobox") return false;
  if (direction === "up" || direction === "down") return true;
  // `email` reports no caret at all, so it keeps left and right.
  const { selectionStart: start, selectionEnd: end } = field;
  if (start === null || end === null || start !== end) return false;
  return direction === "left" ? start === 0 : end === field.value.length;
};

/**
 * `inert` and `hidden` are how the page host parks the routes that are not on
 * screen, so honouring them is what keeps the engine scoped to one page.
 */
const isVisible = (element: HTMLElement): boolean => {
  if (element.hidden || element.closest("[hidden]") !== null) return false;
  if (element.closest("[inert]") !== null) return false;
  if (element.closest("[aria-hidden='true']") !== null) return false;
  const rect = element.getBoundingClientRect();
  if (rect.width < 2 || rect.height < 2) return false;
  const style = window.getComputedStyle(element);
  return style.visibility !== "hidden" && style.display !== "none" && style.opacity !== "0";
};

/**
 * A modal, a menu or an open popover owns the arrow keys while it is up —
 * otherwise focus would wander behind the overlay.
 */
const activeScope = (focused: HTMLElement | null): HTMLElement => {
  const dialog = focused?.closest<HTMLElement>("[role='dialog'][aria-modal='true']");
  if (dialog) return dialog;
  const openDialog = document.querySelector<HTMLElement>("[role='dialog'][aria-modal='true']");
  if (openDialog && isVisible(openDialog)) return openDialog;
  const menu = focused?.closest<HTMLElement>("[role='menu']:not([hidden])");
  if (menu) return menu;
  return document.body;
};

/**
 * Everything the keys can land on. A focusable container of other stops is
 * left out — a tab panel with `tabindex="0"`, say — because the walk never
 * steps from an element into its own children, so landing on the container
 * would strand focus above everything inside it.
 */
const collect = (scope: HTMLElement): HTMLElement[] => {
  const found = Array.from(scope.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (element) => element.closest("[data-nav-skip]") === null && isVisible(element),
  );
  const stops = new Set(found);
  const containers = new Set<HTMLElement>();
  for (const element of found) {
    for (let parent = element.parentElement; parent && parent !== scope; parent = parent.parentElement) {
      if (stops.has(parent) && !parent.hasAttribute("data-nav-item")) containers.add(parent);
    }
  }
  return containers.size === 0 ? found : found.filter((element) => !containers.has(element));
};

const centre = (rect: DOMRect): { x: number; y: number } => ({
  x: rect.left + rect.width / 2,
  y: rect.top + rect.height / 2,
});

/** Share a line: some of one's height overlaps some of the other's. */
const overlapsVertically = (a: DOMRect, b: DOMRect): boolean =>
  Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) > 0;

/** On another line: less than half of the shorter one overlaps the other. */
const onAnotherLine = (a: DOMRect, b: DOMRect): boolean =>
  Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) < Math.min(a.height, b.height) / 2;

/**
 * Lower is better; `null` means "not in that direction at all".
 *
 * Distance along the travel axis dominates, drift across it is penalised, and
 * candidates that share a row (or a column) get a bonus. That combination is
 * what keeps a horizontal rail feeling like a rail instead of jumping to
 * whatever happens to be closest in raw pixels. The bonus grows with the
 * overlap only a little: a wide card lined up under a button must not beat the
 * narrower chip sitting between them.
 */
const score = (from: DOMRect, to: DOMRect, direction: NavDirection): number | null => {
  const a = centre(from);
  const b = centre(to);
  let travel: number;
  let drift: number;
  let overlap: number;

  if (direction === "left" || direction === "right") {
    if (direction === "right" && b.x <= a.x + 1) return null;
    if (direction === "left" && b.x >= a.x - 1) return null;
    travel = direction === "right" ? to.left - from.right : from.left - to.right;
    if (travel < -Math.min(from.width, to.width) * 0.6) return null;
    drift = Math.abs(b.y - a.y);
    overlap = Math.max(0, Math.min(from.bottom, to.bottom) - Math.max(from.top, to.top));
  } else {
    if (direction === "down" && b.y <= a.y + 1) return null;
    if (direction === "up" && b.y >= a.y - 1) return null;
    travel = direction === "down" ? to.top - from.bottom : from.top - to.bottom;
    if (travel < -Math.min(from.height, to.height) * 0.6) return null;
    drift = Math.abs(b.x - a.x);
    overlap = Math.max(0, Math.min(from.right, to.right) - Math.max(from.left, to.left));
  }

  const aligned = overlap > 0;
  return (
    Math.max(0, travel) +
    drift * 2 -
    (aligned ? Math.min(overlap, 240) * 0.25 : 0) +
    (aligned ? 0 : 600)
  );
};

/** The best-scoring candidate in a direction, if any lies that way at all. */
const bestOf = (
  candidates: HTMLElement[],
  from: HTMLElement,
  direction: NavDirection,
): HTMLElement | null => {
  const origin = from.getBoundingClientRect();
  let best: HTMLElement | null = null;
  let bestScore = Number.POSITIVE_INFINITY;
  for (const candidate of candidates) {
    if (candidate === from || candidate.contains(from) || from.contains(candidate)) continue;
    const value = score(origin, candidate.getBoundingClientRect(), direction);
    if (value === null || value >= bestScore) continue;
    best = candidate;
    bestScore = value;
  }
  return best;
};

/** Two bands whose gaps differ by less than this sit at the same height. */
const SAME_HEIGHT = 16;

export function createSpatialNav(hooks: SpatialNavHooks): SpatialNav {
  let inputMode: NavInputMode = "pointer";
  let marked: HTMLElement | null = null;
  /** The control each band held last, so leaving a band and coming back returns to it. */
  const remembered = new WeakMap<HTMLElement, HTMLElement>();
  let offeringEscape = false;

  const setInputMode = (mode: NavInputMode): void => {
    if (inputMode === mode) return;
    inputMode = mode;
    document.body.dataset.inputMode = mode;
    if (mode === "pointer") unmark();
    else mark(current());
  };

  const unmark = (): void => {
    if (!marked) return;
    delete marked.dataset.navFocus;
    marked = null;
  };

  /**
   * `:focus-visible` never fires for controller input — nothing the browser can
   * see happened — so the focus style is driven by an explicit attribute. It is
   * only ever set while the keys or a pad are in use, never for the pointer.
   */
  const mark = (element: HTMLElement | null): void => {
    if (marked === element) return;
    unmark();
    if (!element || inputMode === "pointer") return;
    if (element.tagName === "MAIN" || element.classList.contains("app-page")) return;
    element.dataset.navFocus = "";
    marked = element;
  };

  const current = (): HTMLElement | null => {
    const active = document.activeElement;
    if (!(active instanceof HTMLElement) || active === document.body) return null;
    return active;
  };

  /** The page that is actually on screen, so a cold start lands in the right place. */
  const visiblePage = (): HTMLElement | null =>
    Array.from(document.querySelectorAll<HTMLElement>(".app-page")).find(
      (page) => !page.hidden && !page.inert,
    ) ?? null;

  const reducedMotion = (): boolean => prefersReducedMotion();

  /**
   * The keys are driving this rail now. Set before anything scrolls it, so the
   * page's `data-nav-driven` styles are in force for the scroll itself.
   */
  const drive = (rail: HTMLElement | null): void => {
    if (rail) rail.dataset.navDriven = "";
  };

  /** A hand on the rail — a wheel, a trackpad, a finger — takes it back. */
  const onHandScroll = (event: Event): void => {
    const target = event.target instanceof Element ? event.target : null;
    const rail = target?.closest<HTMLElement>("[data-nav-driven]");
    if (rail) delete rail.dataset.navDriven;
    // The hand wins: a box it is scrolling stops being followed.
    if (target instanceof HTMLElement) for (const box of scrollBoxes(target)) unfollow(box);
  };

  /** The scroll containers between an element and the page it lives on. */
  const scrollBoxes = (element: HTMLElement): HTMLElement[] => {
    const boxes: HTMLElement[] = [];
    for (let box = element.parentElement; box && box !== document.body; box = box.parentElement) {
      const style = window.getComputedStyle(box);
      const scrolls =
        /auto|scroll|overlay/.test(style.overflowX) || /auto|scroll|overlay/.test(style.overflowY);
      if (!scrolls) continue;
      if (box.scrollWidth > box.clientWidth + 1 || box.scrollHeight > box.clientHeight + 1) {
        boxes.push(box);
      }
    }
    return boxes;
  };

  /** The room a control asks to be given around it, straight from `scroll-margin`. */
  const scrollMargin = (element: HTMLElement): { x: number; y: number } => {
    const style = window.getComputedStyle(element);
    return {
      x: Math.max(Number.parseFloat(style.scrollMarginLeft) || 0, Number.parseFloat(style.scrollMarginRight) || 0),
      y: Math.max(Number.parseFloat(style.scrollMarginTop) || 0, Number.parseFloat(style.scrollMarginBottom) || 0),
    };
  };

  /** How far a box is from showing the element, nearest-edge first. Zero when it already does. */
  const showingGap = (box: HTMLElement, element: HTMLElement): { x: number; y: number } => {
    const view = box.getBoundingClientRect();
    const rect = element.getBoundingClientRect();
    const room = scrollMargin(element);
    const left = rect.left - room.x - view.left;
    const right = rect.right + room.x - view.right;
    const top = rect.top - room.y - view.top;
    const bottom = rect.bottom + room.y - view.bottom;
    return {
      x: left < 0 ? left : right > 0 ? right : 0,
      y: top < 0 ? top : bottom > 0 ? bottom : 0,
    };
  };

  /**
   * Bringing a control into view, as a camera that follows rather than as a
   * scroll that is launched.
   *
   * `scrollIntoView({ behavior: "smooth" })` begins a fresh eased animation on
   * every call. A held key calls it faster than the ease can run, so the rail
   * only ever played the first, slowest sliver of it: the shelf crawled a few
   * pixels a frame while the selection ran thousands of pixels ahead of it, and
   * the walk looked frozen until the key was let go. Worse, the rendered window
   * follows the selection, so the shelf being that far behind put the card the
   * window was pinned to outside the new window — and the next move of the
   * window teleported the whole rail.
   *
   * One loop per box closes a share of the remaining distance each frame and
   * simply re-aims at whatever is focused now, so a burst of presses converges
   * instead of restarting, and a rail that has just been re-rendered is followed
   * exactly as one that has not.
   */
  const followed = new WeakMap<HTMLElement, { element: HTMLElement; frame: number }>();

  const unfollow = (box: HTMLElement): void => {
    const following = followed.get(box);
    if (!following) return;
    window.cancelAnimationFrame(following.frame);
    followed.delete(box);
  };

  /**
   * `behavior: "instant"` every time, because the animation is the loop above.
   * Not `"auto"`, which means "whatever the stylesheet says": the store's shelf
   * asks for `scroll-behavior: smooth`, which turned each of these into an
   * eased scroll of its own, and reading a position still easing towards the
   * last one left the loop chasing its own tail.
   */
  const scrollBoxTo = (box: HTMLElement, x: number, y: number): void => {
    box.scrollTo({ left: box.scrollLeft + x, top: box.scrollTop + y, behavior: "instant" });
  };

  const follow = (box: HTMLElement, element: HTMLElement): void => {
    if (reducedMotion()) {
      const gap = showingGap(box, element);
      scrollBoxTo(box, gap.x, gap.y);
      unfollow(box);
      return;
    }

    const following = followed.get(box);
    if (following) {
      following.element = element;
      return;
    }

    const aim = { element, frame: 0 };
    followed.set(box, aim);
    const chase = (): void => {
      if (!aim.element.isConnected || !box.isConnected) {
        followed.delete(box);
        return;
      }
      const gap = showingGap(box, aim.element);
      if (Math.abs(gap.x) < 1 && Math.abs(gap.y) < 1) {
        scrollBoxTo(box, gap.x, gap.y);
        followed.delete(box);
        return;
      }
      // A share of what is left each frame, larger the further behind it is:
      // a nudge is eased, a key held down is kept up with, and the target can
      // move mid-flight without anything restarting.
      const close = (distance: number): number => {
        if (Math.abs(distance) < 2) return distance;
        return distance * Math.min(0.5, 0.18 + Math.abs(distance) / 2600);
      };
      scrollBoxTo(box, close(gap.x), close(gap.y));
      aim.frame = window.requestAnimationFrame(chase);
    };
    aim.frame = window.requestAnimationFrame(chase);
  };

  const reveal = (element: HTMLElement): void => {
    for (const box of scrollBoxes(element)) follow(box, element);
  };

  const focusElement = (element: HTMLElement): void => {
    element.focus({ preventScroll: true });
    mark(element);
    drive(element.closest<HTMLElement>("[data-nav-rail]"));
    reveal(element);
  };

  /** The band an element belongs to, as long as it lies inside the scope in play. */
  const bandOf = (element: HTMLElement, scope: HTMLElement): HTMLElement | null => {
    const band = element.closest<HTMLElement>(BAND);
    return band && band !== scope && scope.contains(band) ? band : null;
  };

  const isRail = (band: HTMLElement): boolean => band.hasAttribute("data-nav-rail");

  /**
   * A tablist whose tabs are stacked is walked with up and down. Measured
   * rather than read from `aria-orientation`: the Settings column lies on its
   * side on a phone, and its markup cannot know that.
   */
  const isColumn = (band: HTMLElement): boolean => {
    if (!band.matches("[role='tablist']")) return false;
    const [first, second] = collect(band);
    if (!first || !second) return band.getAttribute("aria-orientation") === "vertical";
    const a = first.getBoundingClientRect();
    const b = second.getBoundingClientRect();
    return Math.abs(b.top - a.top) > Math.abs(b.left - a.left);
  };

  const railItems = (rail: HTMLElement): HTMLElement[] =>
    Array.from(rail.querySelectorAll<HTMLElement>("[data-nav-item]")).filter(isVisible);

  /** The page's rail, when it has one on screen. */
  const pageRail = (): HTMLElement | null => {
    const rail = visiblePage()?.querySelector<HTMLElement>("[data-nav-rail]");
    return rail && isVisible(rail) ? rail : null;
  };

  const selectedItem = (rail: HTMLElement | null): HTMLElement | null =>
    rail ? (railItems(rail).find((item) => item.hasAttribute("data-nav-selected")) ?? null) : null;

  const remember = (element: HTMLElement): void => {
    const band = element.closest<HTMLElement>(BAND);
    if (!band) return;
    if (!isRail(band)) {
      remembered.set(band, element);
      return;
    }
    const item = railItems(band).find((entry) => entry === element || entry.contains(element));
    if (item) remembered.set(band, item);
  };

  /**
   * Where a key coming from outside a band lands in it: what the page shows as
   * selected (the game in the hero, the open tab), else the control the band
   * held last, else its current one, else the one nearest the column the key
   * came from.
   */
  const entryOf = (band: HTMLElement, origin: DOMRect): HTMLElement | null => {
    const members = isRail(band) ? railItems(band) : collect(band);
    if (members.length === 0) return null;
    const selected = members.find((member) => member.matches(SELECTED));
    if (selected) return selected;
    const memory = remembered.get(band);
    if (memory && members.includes(memory)) return memory;
    for (const marker of CURRENT) {
      const match = members.find((member) => member.matches(marker));
      if (match) return match;
    }
    const x = centre(origin).x;
    const distance = (member: HTMLElement): number =>
      Math.abs(centre(member.getBoundingClientRect()).x - x);
    return members.reduce((closest, member) => (distance(member) < distance(closest) ? member : closest));
  };

  /** A candidate chosen by geometry, redirected to its band's entry when that band says where to land. */
  const land = (target: HTMLElement, from: HTMLElement, scope: HTMLElement, always: boolean): HTMLElement => {
    const band = bandOf(target, scope);
    if (!band || (!always && !isRail(band) && !isColumn(band))) return target;
    return entryOf(band, from.getBoundingClientRect()) ?? target;
  };

  /**
   * Up or down out of a band goes to the next band that way — the nearest one,
   * edge to edge, wherever it sits across the page — so the library reads
   * topbar, Play, games, browse bar from any column rather than jumping to
   * whatever lines up with the key. Between two bands at the same height the
   * one nearer the key's column wins. A column of tabs only hands over to what
   * sits right above or below it.
   */
  const nextBand = (
    band: HTMLElement,
    from: HTMLElement,
    direction: "up" | "down",
    scope: HTMLElement,
  ): HTMLElement | null => {
    const edge = band.getBoundingClientRect();
    const middle = centre(edge).y;
    const x = centre(from.getBoundingClientRect()).x;
    const column = isColumn(band);
    const stops: Array<{ stop: HTMLElement; gap: number; drift: number }> = [];
    const seen = new Set<HTMLElement>();

    for (const candidate of collect(scope)) {
      if (band.contains(candidate)) continue;
      const stop = bandOf(candidate, scope) ?? candidate;
      if (seen.has(stop)) continue;
      seen.add(stop);
      const rect = stop.getBoundingClientRect();
      const beyond = direction === "down" ? rect.top >= middle : rect.bottom <= middle;
      if (!beyond) continue;
      const drift = x < rect.left ? rect.left - x : x > rect.right ? x - rect.right : 0;
      if (column && drift > 0) continue;
      const gap = Math.max(0, direction === "down" ? rect.top - edge.bottom : edge.top - rect.bottom);
      stops.push({ stop, gap, drift });
    }
    if (stops.length === 0) return null;

    const nearest = Math.min(...stops.map((entry) => entry.gap));
    const [chosen] = stops
      .filter((entry) => entry.gap <= nearest + SAME_HEIGHT)
      .sort((a, b) => a.drift - b.drift || a.gap - b.gap);
    const stop = chosen!.stop;
    return stop.matches(BAND) ? entryOf(stop, from.getBoundingClientRect()) : stop;
  };

  const moveVertically = (
    from: HTMLElement,
    direction: "up" | "down",
    scope: HTMLElement,
  ): HTMLElement | null => {
    const band = bandOf(from, scope);
    if (band) {
      // A column's tabs, or a row that wrapped onto a second line, are walked
      // inside the band before anything leaves it.
      if (!isRail(band)) {
        const origin = from.getBoundingClientRect();
        const lines = collect(band).filter((member) =>
          onAnotherLine(origin, member.getBoundingClientRect()),
        );
        const inside = bestOf(lines, from, direction);
        if (inside) return inside;
      }
      return nextBand(band, from, direction, scope);
    }
    const best = bestOf(collect(scope), from, direction);
    if (!best) return null;
    return bandBetween(from, best, direction, scope) ?? land(best, from, scope, true);
  };

  /**
   * A band lying wholly between a control and where geometry would send it is
   * a row the key would skip — the settings tabs laid across a phone, between
   * a section's first control and the topbar lined up above it. The key stops
   * there first, on the band's entry.
   */
  const bandBetween = (
    from: HTMLElement,
    target: HTMLElement,
    direction: "up" | "down",
    scope: HTMLElement,
  ): HTMLElement | null => {
    const origin = from.getBoundingClientRect();
    const far = target.getBoundingClientRect();
    let nearest: { band: HTMLElement; distance: number } | null = null;
    for (const band of Array.from(scope.querySelectorAll<HTMLElement>(BAND))) {
      if (band.contains(target) || !isVisible(band)) continue;
      const rect = band.getBoundingClientRect();
      const between =
        direction === "up"
          ? rect.top >= far.bottom - 1 && rect.bottom <= origin.top + 1
          : rect.bottom <= far.top + 1 && rect.top >= origin.bottom - 1;
      if (!between) continue;
      const distance = direction === "up" ? origin.top - rect.bottom : rect.top - origin.bottom;
      if (!nearest || distance < nearest.distance) nearest = { band, distance };
    }
    return nearest ? entryOf(nearest.band, origin) : null;
  };

  /**
   * One entry along the rail from `origin` (the entry focus is on, or the
   * selected one), wrapping at both ends. The page is asked first; when it
   * moves its selection, the entry it now marks is the answer.
   */
  const stepRail = (rail: HTMLElement, origin: HTMLElement | null, delta: 1 | -1): HTMLElement | null => {
    drive(rail);
    const request = new CustomEvent<NavStepDetail>(NAV_STEP_EVENT, {
      bubbles: true,
      cancelable: true,
      detail: { delta },
    });
    if (!rail.dispatchEvent(request)) return selectedItem(rail);
    const items = railItems(rail);
    if (items.length === 0) return null;
    const index = origin ? items.indexOf(origin) : -1;
    if (index < 0) return items[delta > 0 ? 0 : items.length - 1] ?? null;
    return items[(index + delta + items.length) % items.length] ?? null;
  };

  /**
   * The entry a control is sitting on: a heart hanging off a card, anything
   * drawn over an item rather than inside it. The biggest intersection wins, so
   * a control straddling two cards lands on the one it covers most.
   */
  const overlappingItem = (from: HTMLElement, items: HTMLElement[]): HTMLElement | null => {
    const origin = from.getBoundingClientRect();
    let best: HTMLElement | null = null;
    let bestArea = 0;
    for (const item of items) {
      const rect = item.getBoundingClientRect();
      const width = Math.min(rect.right, origin.right) - Math.max(rect.left, origin.left);
      const height = Math.min(rect.bottom, origin.bottom) - Math.max(rect.top, origin.top);
      if (width <= 0 || height <= 0) continue;
      if (width * height > bestArea) {
        bestArea = width * height;
        best = item;
      }
    }
    return best;
  };

  const moveSideways = (
    from: HTMLElement,
    direction: "left" | "right",
    scope: HTMLElement,
  ): HTMLElement | null => {
    const band = bandOf(from, scope);
    const delta = direction === "right" ? 1 : -1;

    // Along the rail: one entry over, and round again at the end.
    if (band && isRail(band)) {
      const items = railItems(band);
      const item = items.find((entry) => entry === from || entry.contains(from));
      if (!item) return overlappingItem(from, items) ?? entryOf(band, from.getBoundingClientRect());
      return stepRail(band, item, delta);
    }

    // Along a row: its own controls, then whatever sits beside the band at its
    // height — the store's platform chips after its categories, a settings
    // section's controls beside the column of tabs — and never a band above or
    // below it.
    if (band) {
      const origin = from.getBoundingClientRect();
      if (!isColumn(band)) {
        const line = collect(band).filter((member) =>
          overlapsVertically(origin, member.getBoundingClientRect()),
        );
        const inside = bestOf(line, from, direction);
        if (inside) return inside;
      }
      // What shares the focused control's own line comes first, however far
      // across the page it sits; only then what merely shares the band's.
      const edge = band.getBoundingClientRect();
      const outside = collect(scope).filter((candidate) => !band.contains(candidate));
      const onLine = outside.filter((candidate) =>
        overlapsVertically(origin, candidate.getBoundingClientRect()),
      );
      const beside = outside.filter((candidate) =>
        overlapsVertically(edge, candidate.getBoundingClientRect()),
      );
      const next = bestOf(onLine, from, direction) ?? bestOf(beside, from, direction);
      return next ? land(next, from, scope, false) : null;
    }

    // Play changes the game it would start, and stays Play.
    if (from.closest("[data-nav-steps]") !== null && scope === document.body) {
      const rail = pageRail();
      if (rail) {
        const next = stepRail(rail, selectedItem(rail), delta);
        if (next) remembered.set(rail, next);
        return next && next.hasAttribute("data-nav-selected") ? from : next;
      }
    }

    // Anywhere else, left and right keep to the line the control sits on.
    const origin = from.getBoundingClientRect();
    const line = collect(scope).filter((candidate) =>
      overlapsVertically(origin, candidate.getBoundingClientRect()),
    );
    const best = bestOf(line, from, direction);
    return best ? land(best, from, scope, false) : null;
  };

  /**
   * Nothing holds focus and no rail says where the page is: land on the page's
   * own starting point — a control marked `data-nav-anchor`, the selected tab,
   * or failing both the first control on screen.
   */
  const focusFirst = (): boolean => {
    const page = visiblePage();
    const selected = selectedItem(pageRail());
    if (selected) {
      focusElement(selected);
      return true;
    }
    const candidates = collect(page ?? document.body);
    const anchor =
      candidates.find((candidate) => candidate.hasAttribute("data-nav-anchor")) ??
      candidates.find((candidate) => candidate.matches("[role='tab'][aria-selected='true']")) ??
      candidates[0];
    if (!anchor) return false;
    focusElement(anchor);
    return true;
  };

  const move = (direction: NavDirection): boolean => {
    let from = current();
    if (!from) {
      // With nothing focused the page is still somewhere: on the game it is
      // showing. The keys start from there, so the first press already changes
      // the game or climbs to Play instead of hunting for a first tab stop.
      from = selectedItem(pageRail());
      if (!from) return focusFirst();
    }

    const scope = activeScope(from);
    const target =
      direction === "up" || direction === "down"
        ? moveVertically(from, direction, scope)
        : moveSideways(from, direction, scope);

    if (!target) {
      if (from === current()) return false;
      // A cold start that has nowhere to go still settles on the game.
      focusElement(from);
      return true;
    }
    focusElement(target);
    return true;
  };

  const gameIdFor = (element: HTMLElement | null, attribute: "navOpen" | "navLaunch"): string | null => {
    if (!element) return null;
    const holder = element.closest<HTMLElement>(
      attribute === "navOpen" ? "[data-nav-open]" : "[data-nav-launch]",
    );
    return holder?.dataset[attribute] ?? null;
  };

  /**
   * A select only opens for a real click. `showPicker()` is the one way a key
   * or a pad can open it; where the engine has no such method there is nothing
   * better to do than leave it focused.
   */
  const openPicker = (select: HTMLSelectElement): void => {
    try {
      select.showPicker?.();
    } catch {
      // Refused without a user gesture, or not supported: focus stays put.
    }
  };

  /** `A`: enter the game's page when there is one, otherwise just press the control. */
  const activate = (): boolean => {
    const focused = current();
    if (!focused) return focusFirst();
    const gameId = gameIdFor(focused, "navOpen");
    if (gameId) {
      hooks.openGame(gameId);
      return true;
    }
    if (focused instanceof HTMLSelectElement) openPicker(focused);
    else focused.click();
    return true;
  };

  /** `Enter` / `X`: skip the detail page and start the game — or open a focused select. */
  const launchFocused = (): boolean => {
    const focused = current();
    if (focused instanceof HTMLSelectElement) {
      openPicker(focused);
      return true;
    }
    const gameId = gameIdFor(focused, "navLaunch");
    if (!gameId) return false;
    hooks.launchGame(gameId);
    return true;
  };

  /**
   * Close whatever is on top before falling back to the router. A modal closes
   * through its own close button. Anything else that closes on Escape — a menu,
   * the notifications, a store panel — is offered an Escape first, and only an
   * Escape nobody used goes back a page, so B never leaves a panel hanging open
   * over the page it went back to.
   */
  const back = (): void => {
    const focused = current();
    const dialog = focused?.closest<HTMLElement>("[role='dialog'][aria-modal='true']");
    if (dialog) {
      const close = dialog.querySelector<HTMLElement>(
        "[data-focus-key$='close'], [aria-label*='Close' i], [aria-label*='Fermer' i]",
      );
      if (close) {
        close.click();
        return;
      }
    }
    const escape = new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true });
    offeringEscape = true;
    try {
      (focused ?? document.body).dispatchEvent(escape);
    } finally {
      offeringEscape = false;
    }
    if (escape.defaultPrevented) return;
    hooks.back();
  };

  const scrollBy = (delta: number): void => {
    const target = visiblePage() ?? document.scrollingElement ?? document.body;
    target.scrollBy?.({ top: delta, behavior: "auto" });
  };

  /**
   * After a route change. A control still on screen keeps focus — the topbar
   * entry that was just pressed, or one the page restored — so walking the
   * topbar is not undone by every page it opens. Only focus the old page took
   * with it is handed a landing spot on the new one.
   */
  const enterPage = (): void => {
    window.requestAnimationFrame(() => {
      const active = current();
      if (active && isVisible(active)) {
        mark(active);
        return;
      }
      if (inputMode !== "pointer") focusFirst();
    });
  };

  const DIRECTIONS: Record<string, NavDirection> = {
    ArrowUp: "up",
    ArrowDown: "down",
    ArrowLeft: "left",
    ArrowRight: "right",
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    // The Escape `back()` offers is for the pages, not a keystroke of ours.
    if (offeringEscape) return;
    // The shell and the pages get first refusal on every key.
    if (event.defaultPrevented) return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;

    const direction = DIRECTIONS[event.key];
    if (isTypingEvent(event) && !(direction && leavesField(event, direction))) return;

    setInputMode("keyboard");

    if (direction) {
      if (move(direction)) event.preventDefault();
      return;
    }

    if (event.key === "Enter") {
      if (launchFocused()) event.preventDefault();
      return;
    }

    if (event.key === "a" || event.key === "A") {
      if (activate()) event.preventDefault();
      return;
    }

    if (event.key === "Backspace") {
      event.preventDefault();
      back();
    }
  };

  const onFocusIn = (event: FocusEvent): void => {
    if (!(event.target instanceof HTMLElement)) return;
    mark(event.target);
    remember(event.target);
  };
  const onFocusOut = (): void => unmark();
  const onPointerDown = (): void => setInputMode("pointer");

  window.addEventListener("keydown", onKeyDown);
  document.addEventListener("focusin", onFocusIn);
  document.addEventListener("focusout", onFocusOut);
  document.addEventListener("pointerdown", onPointerDown, true);
  document.addEventListener("wheel", onHandScroll, { capture: true, passive: true });
  document.addEventListener("touchstart", onHandScroll, { capture: true, passive: true });
  document.body.dataset.inputMode = inputMode;

  return {
    move,
    activate,
    launchFocused,
    back,
    focusFirst,
    enterPage,
    setInputMode,
    scrollBy,
    destroy() {
      unmark();
      window.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
      document.removeEventListener("pointerdown", onPointerDown, true);
      document.removeEventListener("wheel", onHandScroll, true);
      document.removeEventListener("touchstart", onHandScroll, true);
    },
  };
}
