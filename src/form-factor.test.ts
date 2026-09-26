import { afterEach, describe, expect, it, vi } from "vitest";
import { COMPACT_QUERY, applyFormFactor, isCompactHeight } from "./form-factor";

type ChangeListener = (event: { matches: boolean }) => void;

function stubMatchMedia(matches: boolean) {
  const listeners: ChangeListener[] = [];
  const matchMedia = vi.fn((query: string) => ({
    media: query,
    matches,
    addEventListener: (_type: string, listener: ChangeListener) => listeners.push(listener),
  }));
  Object.defineProperty(window, "matchMedia", { value: matchMedia, configurable: true });
  return { matchMedia, emit: (next: boolean) => listeners.forEach((listener) => listener({ matches: next })) };
}

afterEach(() => {
  delete document.documentElement.dataset.formFactor;
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

  it("marks the document and follows the viewport when the device rotates", () => {
    const { matchMedia, emit } = stubMatchMedia(true);

    applyFormFactor();
    expect(matchMedia).toHaveBeenCalledWith(COMPACT_QUERY);
    expect(document.documentElement.dataset.formFactor).toBe("compact");

    emit(false);
    expect(document.documentElement.dataset.formFactor).toBeUndefined();
    emit(true);
    expect(document.documentElement.dataset.formFactor).toBe("compact");
  });

  it("leaves the document alone where matchMedia does not exist", () => {
    expect(() => applyFormFactor()).not.toThrow();
    expect(document.documentElement.dataset.formFactor).toBeUndefined();
  });
});
