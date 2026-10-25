import { describe, expect, it } from "vitest";
import { displayTitle } from "./game-title";

describe("displayTitle", () => {
  it("turns a slug into words, keeping small words small", () => {
    expect(displayTitle("mio-memories-in-orbit")).toBe("Mio Memories in Orbit");
    expect(displayTitle("the-perfect-pencil")).toBe("The Perfect Pencil");
    expect(displayTitle("hollow_knight")).toBe("Hollow Knight");
  });

  it("reads a spaced dash as the subtitle it stands for", () => {
    expect(displayTitle("Assassin's Creed - Mirage")).toBe("Assassin's Creed: Mirage");
    expect(displayTitle("Kena – Bridge of Spirits")).toBe("Kena: Bridge of Spirits");
  });

  it("leaves real hyphenated names alone", () => {
    expect(displayTitle("Spider-Man")).toBe("Spider-Man");
    expect(displayTitle("Half-Life 2")).toBe("Half-Life 2");
    expect(displayTitle("half-life")).toBe("half-life");
    expect(displayTitle("X-COM 2")).toBe("X-COM 2");
  });

  it("drops trademark marks and stray spacing", () => {
    expect(displayTitle("  Halo™   Infinite ")).toBe("Halo Infinite");
  });
});
