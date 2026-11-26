import { describe, expect, it } from "vitest";
import {
  defaultPluginView,
  managedPluginStatus,
  nxnativeCardStatus,
  pluginMetaLine,
  profileCount,
  wineCardStatus,
} from "./plugins-model";

describe("defaultPluginView", () => {
  it("opens the first plugin when none was picked", () => {
    expect(defaultPluginView("list", ["wine", "wallpaper-searcher"])).toBe("wine");
  });

  it("keeps the person's own pick", () => {
    expect(defaultPluginView("wallpaper-searcher", ["wine", "wallpaper-searcher"])).toBe("wallpaper-searcher");
  });

  it("skips a card that leads to another page", () => {
    expect(defaultPluginView("list", ["winlator", "wallpaper-searcher"], (view) => view === "winlator")).toBe(
      "wallpaper-searcher",
    );
    expect(defaultPluginView("list", ["winlator"], (view) => view === "winlator")).toBe("list");
    expect(defaultPluginView("list", [])).toBe("list");
  });
});

describe("managedPluginStatus", () => {
  it("is ready unless something is wrong or an update runs", () => {
    expect(managedPluginStatus({ trouble: "", updating: false })).toEqual({ text: "Ready", tone: "ok" });
    expect(managedPluginStatus({ trouble: "", updating: true })).toEqual({ text: "Updating…", tone: "muted" });
    expect(managedPluginStatus({ trouble: "Degraded · 3 failures", updating: true })).toEqual({
      text: "Degraded · 3 failures",
      tone: "warn",
    });
  });
});

describe("wineCardStatus", () => {
  it("follows the runner check", () => {
    expect(wineCardStatus(null, false)).toBeNull();
    expect(wineCardStatus(null, true)).toEqual({ text: "Checking…", tone: "muted" });
    expect(wineCardStatus({ state: "ready", message: "" }, false)).toEqual({ text: "Ready", tone: "ok" });
    expect(wineCardStatus({ state: "unavailable", message: "" }, false)).toEqual({ text: "Missing", tone: "warn" });
    expect(wineCardStatus({ state: "invalid", message: "Broken" }, false)).toEqual({
      text: "Needs attention",
      tone: "warn",
    });
  });

  it("counts profiles in a few words", () => {
    expect(profileCount(1)).toBe("1 profile");
    expect(profileCount(3)).toBe("3 profiles");
  });
});

describe("nxnativeCardStatus", () => {
  const ok = { state: "ok" };
  it("puts what is left to install first, then the keys", () => {
    expect(nxnativeCardStatus(null)).toBeNull();
    expect(nxnativeCardStatus({ supported: false, checks: [], keysChosen: false })).toEqual({
      text: "Not supported",
      tone: "warn",
    });
    expect(
      nxnativeCardStatus({ supported: true, checks: [ok, { state: "missing" }, { state: "outdated" }], keysChosen: false }),
    ).toEqual({ text: "2 to install", tone: "warn" });
    expect(nxnativeCardStatus({ supported: true, checks: [ok], keysChosen: false })).toEqual({
      text: "Keys missing",
      tone: "warn",
    });
    expect(nxnativeCardStatus({ supported: true, checks: [ok], keysChosen: true })).toEqual({
      text: "Ready",
      tone: "ok",
    });
  });
});

describe("pluginMetaLine", () => {
  it("reads version · source · updates", () => {
    expect(pluginMetaLine({ version: "0.1.0", trusted: true, automatic: true })).toBe(
      "v0.1.0 · Official · Auto-updates",
    );
    expect(pluginMetaLine({ version: "", trusted: false, automatic: false })).toBe("Community · Manual updates");
  });
});
