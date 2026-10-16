import { afterEach, describe, expect, it, vi } from "vitest";
import { stepAsideCopy } from "./host-device";

const pretendToBe = (userAgent: string): void => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(userAgent);
};

afterEach(() => {
  vi.restoreAllMocks();
});

describe("how Orivo says it gets out of a game's way", () => {
  it("hides on a Mac, which is what brings it back with ⌘-Tab", () => {
    pretendToBe("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15");
    expect(stepAsideCopy().action).toBe("Hide Orivo");
    expect(stepAsideCopy().hint).toContain("⌘-Tab");
  });

  it("minimises on Windows and Linux, to the taskbar", () => {
    pretendToBe("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Edg/131.0");
    expect(stepAsideCopy().action).toBe("Minimise Orivo");
    pretendToBe("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15");
    expect(stepAsideCopy().hint).toContain("taskbar");
  });
});
