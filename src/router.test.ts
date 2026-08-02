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

describe("hash routing", () => {
  it("uses Library as the stable default", () => {
    expect(parseAppRoute("")).toEqual({ page: "library" });
    expect(parseAppRoute("#/library")).toEqual({ page: "library" });
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
        "#/store?category=short-sessions&provider=steam&provider=steam&provider=unknown&q=co-op",
      ),
    ).toEqual({
      page: "store",
      category: "short-sessions",
      providers: ["steam"],
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
