import { describe, expect, it } from "vitest";
import { pluginCard, pluginCategory, pluginIcon } from "./plugin-cards";

describe("pluginCategory", () => {
  it("reads the shelf from what the plugin is", () => {
    expect(pluginCategory("com.orivo.gamestream", "Moonlight")).toBe("streaming");
    expect(pluginCategory("org.ryujinx.runner", "Ryujinx")).toBe("emulation");
    expect(pluginCategory("wine", "Wine")).toBe("compatibility");
    expect(pluginCategory("winlator", "Winlator")).toBe("compatibility");
    expect(pluginCategory("wallpaper-searcher", "Wallpaper Searcher")).toBe("visuals");
    expect(pluginCategory("", "Spotify")).toBe("music");
    expect(pluginCategory("", "Ludusavi")).toBe("saves");
    expect(pluginCategory("", "Playnite")).toBe("library");
    expect(pluginCategory("dev.quiky.installer", "Quiky")).toBe("tools");
  });

  it("falls back on the kind of extension, then on tools", () => {
    expect(pluginCategory("dev.x.thing", "Thing", ["runner"])).toBe("emulation");
    expect(pluginCategory("dev.x.thing", "Thing", ["source"])).toBe("library");
    expect(pluginCategory("dev.x.thing", "Thing")).toBe("tools");
  });
});

describe("pluginIcon", () => {
  it("uses the real logo when the plugin has one, a glyph otherwise", () => {
    expect(pluginIcon("wine", "Wine", "compatibility").querySelector("img")?.getAttribute("src")).toBe(
      "/media/plugins/wine.png",
    );
    expect(
      pluginIcon("com.orivo.gamestream", "Game streaming", "streaming").querySelector("img")?.getAttribute("src"),
    ).toBe("/media/plugins/moonlight.png");
    const glyph = pluginIcon("org.ryujinx.runner", "Ryujinx", "emulation");
    expect(glyph.querySelector("img")).toBeNull();
    expect(glyph.querySelector("svg")).not.toBeNull();
    expect(glyph.dataset.category).toBe("emulation");
  });
});

describe("pluginCard", () => {
  it("opens from anywhere on the card while its own control keeps its click", () => {
    const install = document.createElement("button");
    install.dataset.pluginInstall = "org.ryujinx.runner";
    const card = pluginCard({
      id: "org.ryujinx.runner",
      name: "Ryujinx",
      category: "emulation",
      lines: ["Switch games", "Official"],
      open: { view: "plugin:org.ryujinx.runner", label: "Open Ryujinx" },
      action: install,
      selected: true,
    });
    const open = card.querySelector<HTMLButtonElement>(".plugin-card__open")!;
    expect(open.dataset.pluginOpen).toBe("plugin:org.ryujinx.runner");
    expect(open.getAttribute("aria-current")).toBe("true");
    expect(card.classList.contains("is-selected")).toBe(true);
    expect(install.classList.contains("plugin-card__action")).toBe(true);
    // A card with its own control draws no chevron beside it.
    expect(card.querySelector(".plugin-card__chevron")).toBeNull();
    expect(card.querySelector(".plugin-pill")?.getAttribute("data-category")).toBe("emulation");
  });

  it("marks trouble in the corner and draws progress while something downloads", () => {
    const card = pluginCard({
      id: "x",
      name: "X",
      category: "tools",
      lines: ["Installed"],
      flag: { tone: "warn", label: "Incompatible" },
      progress: 42,
    });
    const flag = card.querySelector<HTMLElement>(".plugin-card__flag--warn")!;
    expect(flag.getAttribute("aria-label")).toBe("Incompatible");
    expect(card.querySelector<HTMLElement>(".plugin-progress__bar")!.style.width).toBe("42%");
    // Not openable: no open target and no chevron.
    expect(card.querySelector(".plugin-card__open")).toBeNull();
    expect(card.querySelector(".plugin-card__chevron")).toBeNull();
  });
});
