import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Orivo's window is declared twice: once for every platform, and once more for
 * macOS, where it must not be created natively full screen — that would give
 * it a desktop of its own, and the host switches it to simple full screen
 * instead (`set_simple_fullscreen` in lib.rs). Tauri merges the macOS file with
 * JSON Merge Patch, which replaces an array outright, so the macOS file has to
 * restate the whole window. These keep the two declarations from drifting.
 */

// Not `new URL(`../${path}`, import.meta.url)`: Vite rewrites that pattern into
// an asset lookup, which resolves a path it was not built with to `undefined`.
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
type WindowConfig = Record<string, unknown>;
const windowsIn = (path: string): WindowConfig[] =>
  JSON.parse(readFileSync(join(ROOT, path), "utf8")).app.windows;

describe("the main window", () => {
  const shared = windowsIn("src-tauri/tauri.conf.json");
  const macos = windowsIn("src-tauri/tauri.macos.conf.json");

  it("is created full screen everywhere but macOS", () => {
    expect(shared).toHaveLength(1);
    expect(shared[0].fullscreen).toBe(true);
    expect(macos).toHaveLength(1);
    expect(macos[0].fullscreen).toBe(false);
  });

  // Centring is a position, and Tauri hands a position to tao after the window
  // exists, which tao applies on the next turn of the main queue — after setup
  // has already made the window cover the screen. A 1536-point window centred
  // on a 1512-point display is at x = -12, so the screen-sized window landed
  // 12 points to the left, with a strip of the desktop showing down its right
  // edge. Without a position, tao centres synchronously while creating it.
  it("is not centred on macOS, where a deferred position would undo the full screen", () => {
    expect(macos[0].center).toBe(false);
  });

  it("is otherwise the same window on macOS", () => {
    const { shadow, ...rest } = macos[0] as typeof macos[0] & { shadow?: boolean };
    expect({ ...rest, fullscreen: shared[0].fullscreen, center: shared[0].center }).toEqual(shared[0]);
    // The macOS shadow draws a light rim, a white line along a covered screen.
    expect(shadow).toBe(false);
  });

  it("is the void before the WebView has painted anything", () => {
    // Without it, the frames before the first paint are white — and on macOS
    // no full-screen transition hides them any more.
    expect(shared[0].backgroundColor).toBe("#0b0b0d");
  });
});
