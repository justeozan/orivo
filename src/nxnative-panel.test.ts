import { describe, expect, it } from "vitest";
import {
  applyBuildMessage,
  applyFixMessage,
  canFix,
  drawnSettings,
  MAX_LOG_LINES,
  shouldAutoClone,
  visibleChecks,
  type FixLog,
  checkDetail,
  checkPill,
  currentValues,
  IDLE_BUILD,
  missingChecks,
  outcomeLine,
  readyToBuild,
  settingVisible,
  type BuildView,
  type LauncherSetting,
  type NxnativeGame,
  type NxnativeStatus,
} from "./nxnative-panel";

const setting = (fields: Partial<LauncherSetting> & Pick<LauncherSetting, "env" | "kind">): LauncherSetting => ({
  section: null,
  label: null,
  note: null,
  default: null,
  options: [],
  min: null,
  max: null,
  step: null,
  on: null,
  off: null,
  level: null,
  needs: null,
  ...fields,
});

describe("nxnative build view", () => {
  it("follows a build from start to a finished app", () => {
    let view: BuildView = { ...IDLE_BUILD };
    view = applyBuildMessage(view, { type: "started", game: "Wonder" });
    expect(view.running).toBe(true);
    view = applyBuildMessage(view, {
      type: "event",
      event: { kind: "stage", index: 2, count: 7, name: "Shaders" },
    });
    expect(view.stage).toContain("2");
    expect(view.stage).toContain("Shaders");
    view = applyBuildMessage(view, {
      type: "event",
      event: { kind: "progress", name: "Translating", fraction: 0.25, etaSeconds: 120, note: null },
    });
    expect(view.fraction).toBe(0.25);
    expect(view.step).toBe("Translating");
    view = applyBuildMessage(view, {
      type: "event",
      event: { kind: "game", titleId: "010015100B514000", name: "Super Mario Bros. Wonder", version: "2.0" },
    });
    expect(view.game).toBe("Super Mario Bros. Wonder");
    view = applyBuildMessage(view, { type: "finished", outcome: "finished", message: null, gameId: "local:1" });
    expect(view.running).toBe(false);
    expect(view.fraction).toBe(1);
    expect(outcomeLine(view)).toBeTruthy();
  });

  it("keeps the last error a failed build said", () => {
    let view = applyBuildMessage({ ...IDLE_BUILD }, { type: "started", game: "Wonder" });
    view = applyBuildMessage(view, {
      type: "event",
      event: { kind: "say", message: "prod.keys cannot open this game", error: true },
    });
    view = applyBuildMessage(view, { type: "finished", outcome: "failed", message: view.error, gameId: null });
    expect(outcomeLine(view)).toContain("prod.keys cannot open this game");
  });
});

describe("nxnative checklist", () => {
  const status = (states: Array<"ok" | "missing">): NxnativeStatus => ({
    supported: true,
    checkout: "nxnative",
    checks: states.map((state, index) => ({ id: `c${index}`, state, found: null, fix: null })),
    building: false,
    fixing: null,
    keysChosen: false,
  });

  it("offers a build only when every check passes", () => {
    expect(readyToBuild(status(["ok", "ok"]))).toBe(true);
    expect(readyToBuild(status(["ok", "missing"]))).toBe(false);
    expect(readyToBuild(status([]))).toBe(false);
    expect(readyToBuild(null)).toBe(false);
  });

  it("lists only what is still to fix", () => {
    const list = status(["ok", "missing", "ok", "missing"]);
    expect(missingChecks(list).map((check) => check.id)).toEqual(["c1", "c3"]);
    expect(missingChecks(status(["ok", "ok"]))).toEqual([]);
  });

  it("keeps the pill to a word or two and the detail in the tooltip", () => {
    const tools = { id: "build-tools", state: "missing" as const, found: "lz4,capstone,fontTools,nsz,zbic", fix: null };
    expect(checkPill(tools)).toBe("Missing");
    expect(checkDetail(tools)).toContain("capstone");
    // An outdated tool's pill is the version that was found.
    expect(checkPill({ id: "xcode", state: "outdated", found: "Xcode 26.4", fix: null })).toBe("Xcode 26.4");
    expect(checkPill({ id: "checkout", state: "ok", found: "a1b2c3d", fix: null })).toBe("Ready");
  });
});

describe("nxnative installs", () => {
  it("follows an install from its first line to its outcome", () => {
    let logs: Record<string, FixLog> = {};
    logs = applyFixMessage(logs, { type: "started", check: "build-tools" });
    expect(logs["build-tools"].state).toBe("running");
    logs = applyFixMessage(logs, { type: "line", check: "build-tools", line: "$ brew install glslang" });
    logs = applyFixMessage(logs, { type: "line", check: "build-tools", line: "==> Pouring glslang" });
    expect(logs["build-tools"].lines).toEqual(["$ brew install glslang", "==> Pouring glslang"]);
    logs = applyFixMessage(logs, { type: "finished", check: "build-tools", ok: false, message: "exit 1" });
    expect(logs["build-tools"]).toMatchObject({ state: "failed", message: "exit 1" });
    expect(logs["build-tools"].lines).toHaveLength(2);
    // A second run starts a clean log.
    logs = applyFixMessage(logs, { type: "started", check: "build-tools" });
    expect(logs["build-tools"].lines).toEqual([]);
  });

  it("keeps only the last lines of a long log", () => {
    let logs: Record<string, FixLog> = {};
    for (let index = 0; index < MAX_LOG_LINES + 50; index += 1) {
      logs = applyFixMessage(logs, { type: "line", check: "python", line: `line ${index}` });
    }
    expect(logs.python.lines).toHaveLength(MAX_LOG_LINES);
    expect(logs.python.lines.at(-1)).toBe(`line ${MAX_LOG_LINES + 49}`);
  });

  it("offers an install only where Orivo can do it, and keeps an installed row in view", () => {
    const check = (id: string, state: "ok" | "missing") => ({ id, state, found: null, fix: null });
    expect(canFix(check("build-tools", "missing"))).toBe(true);
    expect(canFix(check("checkout", "missing"))).toBe(true);
    expect(canFix(check("homebrew", "missing"))).toBe(false);
    expect(canFix(check("build-tools", "ok"))).toBe(false);
    const status: NxnativeStatus = {
      supported: true,
      checkout: null,
      checks: [check("macos", "ok"), check("build-tools", "ok"), check("python-modules", "missing")],
      building: false,
      fixing: null,
      keysChosen: false,
    };
    const ok: FixLog = { state: "ok", lines: [], message: null };
    expect(visibleChecks(status, {}).map((item) => item.id)).toEqual(["python-modules"]);
    expect(visibleChecks(status, { "build-tools": ok }).map((item) => item.id)).toEqual(["build-tools", "python-modules"]);
  });

  it("always draws the nxnative row, where Update and the folder's Change live", () => {
    const check = (id: string, state: "ok" | "missing") => ({ id, state, found: null, fix: null });
    const status: NxnativeStatus = {
      supported: true,
      checkout: "nxnative",
      checks: [check("macos", "ok"), check("checkout", "ok")],
      building: false,
      fixing: null,
      keysChosen: false,
    };
    expect(visibleChecks(status, {}).map((item) => item.id)).toEqual(["checkout"]);
  });

  it("makes the folder by itself, once, when there is none", () => {
    const check = (id: string, state: "ok" | "missing") => ({ id, state, found: null, fix: null });
    const missing: NxnativeStatus = {
      supported: true,
      checkout: null,
      checks: [check("macos", "ok"), check("checkout", "missing")],
      building: false,
      fixing: null,
      keysChosen: false,
    };
    expect(shouldAutoClone(missing, {}, false)).toBe(true);
    // Tried already (it may have failed): the Download button is the retry.
    expect(shouldAutoClone(missing, {}, true)).toBe(false);
    expect(shouldAutoClone({ ...missing, fixing: "checkout" }, {}, false)).toBe(false);
    expect(shouldAutoClone({ ...missing, supported: false }, {}, false)).toBe(false);
    expect(shouldAutoClone(null, {}, false)).toBe(false);
    const ready = { ...missing, checks: [check("checkout", "ok")] };
    expect(shouldAutoClone(ready, {}, false)).toBe(false);
  });
});

describe("nxnative game settings", () => {
  const game: NxnativeGame = {
    gameId: "local:1",
    title: "Wonder",
    titleId: "010015100B514000",
    settings: [
      setting({ env: "DM_RUMBLE", kind: "toggle", default: "on", on: "on", off: "off" }),
      setting({ env: "DM_RUMBLE_STRENGTH", kind: "range", default: "1.0", min: 0, max: 1, needs: "DM_RUMBLE=on" }),
      setting({ env: "DM_FPS_TARGET", kind: "choice", default: "60", options: [["30", "30 fps"], ["60", "60 fps"]] }),
    ],
    saved: { DM_FPS_TARGET: "30" },
    keepsSpace: false,
  };

  it("shows the saved choice over the tested default", () => {
    expect(currentValues(game)).toEqual({ DM_RUMBLE: "on", DM_RUMBLE_STRENGTH: "1.0", DM_FPS_TARGET: "30" });
  });

  it("draws a sub-setting only while its parent has the value it needs", () => {
    const strength = game.settings[1];
    expect(settingVisible(strength, { DM_RUMBLE: "on" })).toBe(true);
    expect(settingVisible(strength, { DM_RUMBLE: "off" })).toBe(false);
    // A row naming a variable the list does not have stays drawn.
    expect(settingVisible(strength, {})).toBe(true);
  });

  it("leaves Display mode out when Orivo starts the game on the player's desktop", () => {
    const display = setting({ env: "DM_FULLSCREEN", kind: "choice", default: "0", options: [["0", "Window"], ["1", "Fullscreen"]] });
    const withDisplay = { ...game, settings: [...game.settings, display] };
    expect(drawnSettings(withDisplay)).toContain(display);
    expect(drawnSettings({ ...withDisplay, keepsSpace: true })).not.toContain(display);
  });
});
