import { afterEach, describe, expect, it, vi } from "vitest";
import {
  COMPACT_QUERY,
  TOUCH_QUERY,
  applyFormFactor,
  isCompactHeight,
  isPhone,
} from "./form-factor";

type ChangeListener = () => void;

/**
 * One switch per media query, each with its own listeners, so a test can turn
 * the screen short or the pointer into a finger independently.
 */
function stubMatchMedia(initial: Record<string, boolean>) {
  const state = { ...initial };
  const listeners = new Map<string, ChangeListener[]>();
  const matchMedia = vi.fn((query: string) => ({
    media: query,
    get matches() {
      return state[query] ?? false;
    },
    addEventListener: (_type: string, listener: ChangeListener) => {
      listeners.set(query, [...(listeners.get(query) ?? []), listener]);
    },
  }));
  Object.defineProperty(window, "matchMedia", { value: matchMedia, configurable: true });
  return {
    matchMedia,
    emit: (query: string, next: boolean) => {
      state[query] = next;
      listeners.get(query)?.forEach((listener) => listener());
    },
  };
}

afterEach(() => {
  delete document.documentElement.dataset.formFactor;
  delete document.documentElement.dataset.device;
  Reflect.deleteProperty(window, "matchMedia");
});

describe("form factor", () => {
  it("calls a phone held sideways compact and a small desktop window not", () => {
    expect(isCompactHeight(411)).toBe(true);
    expect(isCompactHeight(560)).toBe(true);
    expect(isCompactHeight(561)).toBe(false);
    // The two Playwright viewports must keep the desktop scene.
    expect(isCompactHeight(700)).toBe(false);
    expect(isCompactHeight(1024)).toBe(false);
  });

  it("asks about height alone, so a wide phone is not mistaken for a laptop", () => {
    expect(COMPACT_QUERY).toBe("(max-height: 560px)");
    expect(COMPACT_QUERY).not.toContain("width");
  });

  it("calls a screen a phone only when it is compact and only a finger points at it", () => {
    expect(TOUCH_QUERY).toBe("(hover: none) and (pointer: coarse)");
    expect(isPhone(true, true)).toBe(true);
    // A desktop window dragged short, and a tablet standing tall.
    expect(isPhone(true, false)).toBe(false);
    expect(isPhone(false, true)).toBe(false);
  });

  it("marks the document and follows the viewport when the device rotates", () => {
    const { matchMedia, emit } = stubMatchMedia({ [COMPACT_QUERY]: true, [TOUCH_QUERY]: true });

    applyFormFactor();
    expect(matchMedia).toHaveBeenCalledWith(COMPACT_QUERY);
    expect(matchMedia).toHaveBeenCalledWith(TOUCH_QUERY);
    expect(document.documentElement.dataset.formFactor).toBe("compact");
    expect(document.documentElement.dataset.device).toBe("phone");

    emit(COMPACT_QUERY, false);
    expect(document.documentElement.dataset.formFactor).toBeUndefined();
    expect(document.documentElement.dataset.device).toBeUndefined();
    emit(COMPACT_QUERY, true);
    expect(document.documentElement.dataset.formFactor).toBe("compact");
    expect(document.documentElement.dataset.device).toBe("phone");
  });

  it("gives a short desktop window the compact layout but not the phone's", () => {
    const { emit } = stubMatchMedia({ [COMPACT_QUERY]: true, [TOUCH_QUERY]: false });

    applyFormFactor();
    expect(document.documentElement.dataset.formFactor).toBe("compact");
    expect(document.documentElement.dataset.device).toBeUndefined();

    // A phone that loses its mouse becomes a phone again.
    emit(TOUCH_QUERY, true);
    expect(document.documentElement.dataset.device).toBe("phone");
  });

  it("leaves the document alone where matchMedia does not exist", () => {
    expect(() => applyFormFactor()).not.toThrow();
    expect(document.documentElement.dataset.formFactor).toBeUndefined();
    expect(document.documentElement.dataset.device).toBeUndefined();
  });
});
