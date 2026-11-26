import { describe, expect, it, vi } from "vitest";
import { appRouteToHash, HashRouter, parseAppRoute } from "./router";

describe("HashRouter.back", () => {
  it("falls back to Library instead of leaving the app on a deep link", () => {
    window.location.hash = "#/games/steam%3A1245620";
    const router = new HashRouter();
    const historyBack = vi.spyOn(window.history, "back").mockImplementation(() => {});

    // A fresh tab opened straight on a game still reports history.length > 1,
    // so the router must rely on its own push count instead.
    router.back();

    expect(historyBack).not.toHaveBeenCalled();
    expect(window.location.hash).toBe("#/library");
  });

  it("returns through its own entries once it has pushed some", () => {
    window.location.hash = "#/library";
    const router = new HashRouter();
    const historyBack = vi.spyOn(window.history, "back").mockImplementation(() => {});

    router.navigate({ page: "game", gameId: "steam:1", from: "library" });
    router.back();

    expect(historyBack).toHaveBeenCalledTimes(1);

    // Depth is spent, so the next Back falls back rather than escaping.
    router.back();
    expect(historyBack).toHaveBeenCalledTimes(1);
    expect(window.location.hash).toBe("#/library");
  });
});

describe("HashRouter.navigate", () => {
  it("tells its listeners at once, and the hashchange that follows is only an echo", async () => {
    window.location.hash = "#/settings/general";
    const router = new HashRouter();
    const heard: string[] = [];
    const stop = router.start((route) => heard.push(appRouteToHash(route)));

    router.navigate({ page: "settings", section: "libraries", attachGameId: null });
    router.navigate({ page: "settings", section: "general", attachGameId: null });
    // Before any queued hashchange has run: the page already shows the route
    // the URL names, so the next key lands on it and not on the one before.
    expect(heard).toEqual(["#/settings/general", "#/settings/libraries", "#/settings/general"]);

    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(heard).toHaveLength(3);
    stop();
  });

  it("still follows a hash it did not set itself", async () => {
    window.location.hash = "#/library";
    const router = new HashRouter();
    const heard: string[] = [];
    const stop = router.start((route) => heard.push(appRouteToHash(route)));

    // Back, forward or a typed URL reach the router only through hashchange.
    window.location.hash = "#/store";
    await new Promise((resolve) => window.addEventListener("hashchange", resolve, { once: true }));
    expect(heard).toEqual(["#/library", "#/store"]);
    stop();
  });
});

describe("hash routing", () => {
  it("uses Library as the stable default", () => {
    expect(parseAppRoute("")).toEqual({ page: "library" });
    expect(parseAppRoute("#/library")).toEqual({ page: "library" });
  });

  it("round-trips the Me page", () => {
    expect(parseAppRoute("#/me")).toEqual({ page: "me" });
    expect(appRouteToHash({ page: "me" })).toBe("#/me");
  });

  it("round-trips opaque unicode game IDs", () => {
    const hash = appRouteToHash({ page: "game", gameId: "igdb:竜の旅", from: "store" });
    expect(parseAppRoute(hash)).toEqual({
      page: "game",
      gameId: "igdb:竜の旅",
      from: "store",
    });
  });

  it("rejects malformed or path-like IDs", () => {
    expect(parseAppRoute("#/games/%E0%A4%A").page).toBe("not-found");
    expect(parseAppRoute("#/games/local:%2FUsers%2Fsecret").page).toBe("not-found");
  });

  it("round-trips every Store tab and drops the old filters", () => {
    for (const store of ["playstation", "xbox", "nintendo", "steam", "epic"] as const) {
      const route = parseAppRoute(`#/store?store=${store}`);
      expect(route).toEqual({ page: "store", store, query: "" });
      expect(appRouteToHash(route)).toBe(`#/store?store=${store}`);
    }
    expect(parseAppRoute("#/store?category=short-sessions&platform=pc&q=co-op")).toEqual({
      page: "store",
      store: "all",
      query: "co-op",
    });
    // "Tout" is the default, so the canonical Store URL stays bare.
    expect(appRouteToHash({ page: "store", store: "all", query: "" })).toBe("#/store");
  });

  it("keeps a plugin tab and refuses a store it cannot name", () => {
    const route = parseAppRoute("#/store?store=plugin%3Aio.orivo.demo%3Adeals");
    expect(route).toEqual({ page: "store", store: "plugin:io.orivo.demo:deals", query: "" });
    expect(parseAppRoute(appRouteToHash(route))).toEqual(route);
    for (const bad of ["gog", "plugin:", "plugin:Not.A.Plugin", "plugin:io.orivo.demo:<b>"]) {
      expect(parseAppRoute(`#/store?store=${encodeURIComponent(bad)}`)).toMatchObject({ store: "all" });
    }
  });

  it("keeps Settings attachment IDs opaque", () => {
    const route = parseAppRoute("#/settings/plugins?attachGame=local%3Aabc123");
    expect(route).toEqual({
      page: "settings",
      section: "plugins",
      attachGameId: "local:abc123",
    });
  });

  it("returns a branded not-found route for invalid paths", () => {
    expect(parseAppRoute("#/collections")).toEqual({
      page: "not-found",
      path: "/collections",
    });
    expect(parseAppRoute("#/settings/not-real").page).toBe("not-found");
  });
});
