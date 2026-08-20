/**
 * Spatial navigation: one engine that makes every page walkable with the arrow
 * keys and, through `gamepad.ts`, with a controller.
 *
 * The app is a stack of independent page modules, so the engine deliberately
 * knows nothing about them. It reads the live DOM, keeps only what is really
 * focusable and visible on the page that is currently on screen, and picks the
 * next target geometrically. Pages opt into the two game-specific verbs by
 * tagging an element with `data-nav-open` / `data-nav-launch`.
 */

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

const FOCUSABLE = [
  "a[href]",
  "button:not(:disabled)",
  "input:not(:disabled):not([type='hidden'])",
  "select:not(:disabled)",
  "textarea:not(:disabled)",
  "[tabindex]:not([tabindex='-1'])",
  "[data-nav-focusable]",
].join(",");

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

const collect = (scope: HTMLElement): HTMLElement[] =>
  Array.from(scope.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(isVisible);

const centre = (rect: DOMRect): { x: number; y: number } => ({
  x: rect.left + rect.width / 2,
  y: rect.top + rect.height / 2,
});

/**
 * Lower is better; `null` means "not in that direction at all".
 *
 * Distance along the travel axis dominates, drift across it is penalised, and
 * candidates that share a row (or a column) get a bonus. That combination is
 * what keeps a horizontal rail feeling like a rail instead of jumping to
 * whatever happens to be closest in raw pixels.
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
    (aligned ? Math.min(overlap, 240) * 0.5 : 0) +
    (aligned ? 0 : 600)
  );
};

export function createSpatialNav(hooks: SpatialNavHooks): SpatialNav {
  let inputMode: NavInputMode = "pointer";
  let marked: HTMLElement | null = null;

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
   * see happened — so the ring is driven by an explicit attribute instead.
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

  const reducedMotion = (): boolean =>
    window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;

  const focusElement = (element: HTMLElement): void => {
    element.focus({ preventScroll: true });
    mark(element);
    element.scrollIntoView?.({
      behavior: reducedMotion() ? "auto" : "smooth",
      block: "nearest",
      inline: "nearest",
    });
  };

  const focusFirst = (): boolean => {
    const page = visiblePage();
    const candidates = page ? collect(page) : collect(document.body);
    const first = candidates[0];
    if (!first) return false;
    focusElement(first);
    return true;
  };

  const move = (direction: NavDirection): boolean => {
    const from = current();
    if (!from) return focusFirst();

    const scope = activeScope(from);
    const origin = from.getBoundingClientRect();
    let best: HTMLElement | null = null;
    let bestScore = Number.POSITIVE_INFINITY;

    for (const candidate of collect(scope)) {
      if (candidate === from || candidate.contains(from) || from.contains(candidate)) continue;
      const value = score(origin, candidate.getBoundingClientRect(), direction);
      if (value === null || value >= bestScore) continue;
      best = candidate;
      bestScore = value;
    }

    if (!best) return false;
    focusElement(best);
    return true;
  };

  const gameIdFor = (element: HTMLElement | null, attribute: "navOpen" | "navLaunch"): string | null => {
    if (!element) return null;
    const holder = element.closest<HTMLElement>(
      attribute === "navOpen" ? "[data-nav-open]" : "[data-nav-launch]",
    );
    return holder?.dataset[attribute] ?? null;
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
    focused.click();
    return true;
  };

  /** `Enter` / `X`: skip the detail page and start the game. */
  const launchFocused = (): boolean => {
    const gameId = gameIdFor(current(), "navLaunch");
    if (!gameId) return false;
    hooks.launchGame(gameId);
    return true;
  };

  /** Close whatever is on top before falling back to the router. */
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
    const menu = focused?.closest<HTMLElement>("[role='menu']:not([hidden])");
    if (menu) {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      return;
    }
    hooks.back();
  };

  const scrollBy = (delta: number): void => {
    const target = visiblePage() ?? document.scrollingElement ?? document.body;
    target.scrollBy?.({ top: delta, behavior: "auto" });
  };

  /** After a route change nothing holds focus, so hand the new page a landing spot. */
  const enterPage = (): void => {
    window.requestAnimationFrame(() => {
      const active = current();
      const page = visiblePage();
      if (active && page?.contains(active)) {
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
    // The shell and the pages get first refusal on every key.
    if (event.defaultPrevented) return;
    if (event.metaKey || event.ctrlKey || event.altKey) return;
    if (isTypingEvent(event)) return;

    setInputMode("keyboard");

    const direction = DIRECTIONS[event.key];
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
    if (event.target instanceof HTMLElement) mark(event.target);
  };
  const onFocusOut = (): void => unmark();
  const onPointerDown = (): void => setInputMode("pointer");

  window.addEventListener("keydown", onKeyDown);
  document.addEventListener("focusin", onFocusIn);
  document.addEventListener("focusout", onFocusOut);
  document.addEventListener("pointerdown", onPointerDown, true);
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
    },
  };
}
