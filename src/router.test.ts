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

  it("normalises Store filters and ignores unknown providers", () => {
    expect(
      parseAppRoute(
        "#/store?category=short-sessions&platform=steam&platform=steam&platform=unknown&q=co-op",
      ),
    ).toEqual({
      page: "store",
      // A shelf the bar no longer offers lands on the default one.
      category: "for-you",
      platforms: [],
      query: "co-op",
    });
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

describe("store route", () => {
  it("round-trips the store and drops an invalid or default one", () => {
    const route = parseAppRoute("#/store?store=playstation");
    expect(route).toMatchObject({ page: "store", store: "playstation" });
    expect(appRouteToHash(route)).toBe("#/store?store=playstation");
    expect(parseAppRoute("#/store?store=All%20Bad")).not.toHaveProperty("store");
    expect(parseAppRoute("#/store?store=all")).not.toHaveProperty("store");
    expect(parseAppRoute("#/store?category=legacy-mood")).toMatchObject({ category: "for-you" });
  });
});
