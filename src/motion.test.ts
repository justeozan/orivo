import { afterEach, describe, expect, it } from "vitest";
import { applyRememberedMotionPreference, prefersReducedMotion, rememberMotionPreference } from "./motion";

function memory() {
  const values = new Map<string, string>();
  return {
    values,
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
  };
}

afterEach(() => {
  delete document.documentElement.dataset.motion;
});

describe("the motion preference before the host has answered", () => {
  it("is remembered as applied, and set on the next start before anything paints", () => {
    const store = memory();
    rememberMotionPreference("reduced", store);

    applyRememberedMotionPreference(store);

    expect(document.documentElement.dataset.motion).toBe("reduced");
    // Which is what every animation guard reads, the startup one included.
    expect(prefersReducedMotion()).toBe(true);
  });

  it("follows the system on a first start, with nothing remembered", () => {
    applyRememberedMotionPreference(memory());

    expect(document.documentElement.dataset.motion).toBeUndefined();
  });

  it("never remembers or applies a value that is not a preference", () => {
    const store = memory();
    rememberMotionPreference("dramatic", store);
    expect(store.values.size).toBe(0);

    store.values.set("orivo.motion", "dramatic");
    applyRememberedMotionPreference(store);
    expect(document.documentElement.dataset.motion).toBeUndefined();
  });

  it("carries on without storage it is refused", () => {
    const refusing = {
      getItem: (): string | null => {
        throw new Error("SecurityError");
      },
      setItem: (): void => {
        throw new Error("QuotaExceededError");
      },
    };

    expect(() => rememberMotionPreference("full", refusing)).not.toThrow();
    expect(() => applyRememberedMotionPreference(refusing)).not.toThrow();
    expect(() => applyRememberedMotionPreference(null)).not.toThrow();
  });
});
