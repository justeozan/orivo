import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * The frontend reaches the Rust host by name: `invoke("get_library")` is a
 * string, and nothing at build time checks that a command of that name is
 * registered in `tauri::generate_handler![…]`. A command dropped from that list
 * — by a rename, a module split, a bad merge — still type-checks and compiles
 * on both sides, and fails only at run time, as a rejected promise on the one
 * screen that calls it.
 *
 * These read both sides as text and hold them together:
 *   - every command the frontend calls is registered;
 *   - every registered command is either called by the frontend or listed in
 *     `NOT_CALLED_BY_THE_FRONTEND` below, so removing one, or registering one
 *     nobody calls, is a deliberate edit to this file and not a side effect.
 *
 * The handler list is looked for in every `.rs` file under `src-tauri/src`,
 * not only `lib.rs`, so moving it into a `commands` module keeps this working.
 */

// Not `new URL(`../${path}`, import.meta.url)`: Vite rewrites that pattern into
// an asset lookup, which resolves a path it was not built with to `undefined`.
const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/**
 * Registered with Tauri, but called from nowhere in `src/`. Each is either
 * dead or reached some other way; either way the list is the record, and the
 * refactor that removes or wires one updates it in the same change.
 */
const NOT_CALLED_BY_THE_FRONTEND = [
  "associate_direct_game_with_wine_profile",
  "begin_wine_profile_setup",
  "cancel_plugin_update",
  "cancel_wine_detection",
  "cancel_wine_scan",
  "choose_wine_game_directory",
  "confirm_detected_wine_staging",
  "create_wine_profile",
  "get_quiky_diagnostics",
  "get_quiky_progress",
  "get_runner_plugins",
  "get_wine_profile_setup",
  "get_wine_runner_status",
  "get_wine_scan_page",
  "get_wine_scan_status",
  "import_wine_games",
  "open_auth_in_browser",
  "purge_plugin_compile_cache",
  "remove_wine_setup_directory",
  "select_wine_staging",
  "start_wine_profile_scan",
  "submit_epic_mobile_auth",
  "submit_gog_mobile_auth",
  "submit_microsoft_mobile_auth",
  "submit_steam_mobile_auth",
];

const filesUnder = (dir: string, extension: string): string[] =>
  readdirSync(join(ROOT, dir), { recursive: true, encoding: "utf8" })
    .filter((path) => path.endsWith(extension))
    .map((path) => join(ROOT, dir, path));

/** Every command name inside `generate_handler![…]`, `cfg`-gated ones included. */
function registeredCommands(): Set<string> {
  const lists = filesUnder("src-tauri/src", ".rs").flatMap((file) => {
    const source = readFileSync(file, "utf8");
    const marker = source.indexOf("generate_handler![");
    if (marker < 0) return [];
    // The list holds `#[cfg(…)]` attributes, so its end is the `]` that
    // balances the opening one, not the first `]`.
    const open = source.indexOf("[", marker);
    let depth = 0;
    for (let at = open; at < source.length; at++) {
      if (source[at] === "[") depth++;
      else if (source[at] === "]" && --depth === 0) return [source.slice(open + 1, at)];
    }
    return [];
  });
  expect(lists, "exactly one tauri::generate_handler! list").toHaveLength(1);
  const body = lists[0].replace(/\/\/[^\n]*/g, "").replace(/#\[[^\]]*\]/g, "");
  return new Set(
    body
      .split(",")
      .map((entry) => entry.trim())
      .filter(Boolean)
      // `module::command` registers as `command`.
      .map((entry) => entry.split("::").pop() as string),
  );
}

/**
 * Every string literal passed as the first argument of `invoke(…)` or of a
 * wrapper whose name contains `invoke` (`invokeWhileActive(…)`), with or
 * without a type argument, mapped to the files that call it.
 */
function invokedCommands(): Map<string, string[]> {
  const invoked = new Map<string, string[]>();
  const sources = filesUnder("src", ".ts").filter((file) => !file.endsWith(".test.ts"));
  for (const file of sources) {
    const source = readFileSync(file, "utf8");
    for (const call of source.matchAll(/\b\w*[iI]nvoke\w*\b/g)) {
      let at = (call.index ?? 0) + call[0].length;
      const skipSpace = () => {
        while (/\s/.test(source[at] ?? "")) at++;
      };
      skipSpace();
      if (source[at] === "<") {
        // A type argument can nest (`invoke<Array<{ id: string }>>`) and hold
        // an arrow (`=>`), whose `>` closes nothing.
        let depth = 0;
        for (; at < source.length; at++) {
          if (source[at] === "<") depth++;
          else if (source[at] === ">" && source[at - 1] !== "=" && --depth === 0) {
            at++;
            break;
          }
        }
        skipSpace();
      }
      if (source[at] !== "(") continue;
      const name = /^\(\s*(["'`])([a-z][a-z0-9_]*)\1/.exec(source.slice(at, at + 200))?.[2];
      if (!name) continue;
      const callers = invoked.get(name) ?? [];
      callers.push(relative(ROOT, file));
      invoked.set(name, callers);
    }
  }
  return invoked;
}

describe("the IPC contract between the frontend and the Rust host", () => {
  const registered = registeredCommands();
  const invoked = invokedCommands();

  it("finds both sides", () => {
    // A parser that silently matched nothing would pass every test below.
    expect(registered.size).toBeGreaterThan(100);
    expect(invoked.size).toBeGreaterThan(80);
  });

  it("registers every command the frontend calls", () => {
    const missing = [...invoked]
      .filter(([name]) => !registered.has(name))
      .map(([name, callers]) => `${name} (called from ${[...new Set(callers)].join(", ")})`);
    expect(missing).toEqual([]);
  });

  it("calls every registered command, apart from the ones listed as not called", () => {
    const notCalled = [...registered].filter((name) => !invoked.has(name)).sort();
    expect(notCalled).toEqual([...NOT_CALLED_BY_THE_FRONTEND].sort());
  });
});
