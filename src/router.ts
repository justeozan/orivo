import type { AppRoute, GameId, SettingsSection } from "./contracts";
import { isStoreTabId } from "./store-tabs";
const settingsSections = new Set<SettingsSection>([
  "general",
  "libraries",
  "plugins",
  "appearance",
  "data",
  "about",
]);

function decodeSegment(value: string): string | null {
  try {
    const decoded = decodeURIComponent(value);
    return decoded && !/[\0/\\]/.test(decoded) ? decoded : null;
  } catch {
    return null;
  }
}

function readPathAndQuery(hash: string): { path: string; query: URLSearchParams } {
  const raw = hash.startsWith("#") ? hash.slice(1) : hash;
  const [rawPath, rawQuery = ""] = raw.split("?", 2);
  const path = rawPath.startsWith("/") ? rawPath : `/${rawPath}`;
  return { path, query: new URLSearchParams(rawQuery) };
}

export function parseAppRoute(hash: string): AppRoute {
  const { path, query } = readPathAndQuery(hash);
  if (path === "/" || path === "" || path === "/library") {
    return { page: "library" };
  }

  if (path === "/store") {
    // The filters the Store had before its tabs (`category`, `platform`) are
    // dropped rather than translated: an old link opens on "Tout".
    const requested = query.get("store") ?? "";
    return {
      page: "store",
      store: isStoreTabId(requested) ? requested : "all",
      query: query.get("q")?.trim() ?? "",
    };
  }

  if (path === "/me") {
    return { page: "me" };
  }

  const gameMatch = /^\/games\/([^/]+)$/.exec(path);
  if (gameMatch) {
    const gameId = decodeSegment(gameMatch[1]);
    if (!gameId) {
      return { page: "not-found", path };
    }
    const from = query.get("from");
    return {
      page: "game",
      gameId: gameId as GameId,
      from: from === "library" || from === "store" ? from : null,
    };
  }

  const settingsMatch = /^\/settings(?:\/([^/]+))?$/.exec(path);
  if (settingsMatch) {
    const requested = settingsMatch[1] as SettingsSection | undefined;
    if (requested && !settingsSections.has(requested)) {
      return { page: "not-found", path };
    }
    const attachGameId = query.get("attachGame");
    return {
      page: "settings",
      section: requested ?? "general",
      attachGameId: attachGameId ? (attachGameId as GameId) : null,
    };
  }

  return { page: "not-found", path };
}

export function appRouteToHash(route: AppRoute): string {
  if (route.page === "library") {
    return "#/library";
  }
  if (route.page === "store") {
    const query = new URLSearchParams();
    if (route.store !== "all") query.set("store", route.store);
    if (route.query.trim()) query.set("q", route.query.trim());
    const suffix = query.size ? `?${query.toString()}` : "";
    return `#/store${suffix}`;
  }
  if (route.page === "game") {
    const query = route.from ? `?from=${route.from}` : "";
    return `#/games/${encodeURIComponent(route.gameId)}${query}`;
  }
  if (route.page === "me") {
    return "#/me";
  }
  if (route.page === "settings") {
    const query = route.attachGameId
      ? `?attachGame=${encodeURIComponent(route.attachGameId)}`
      : "";
    return `#/settings/${route.section}${query}`;
  }
  return `#${route.path.startsWith("/") ? route.path : `/${route.path}`}`;
}

export type RouteListener = (route: AppRoute) => void;

export class HashRouter {
  readonly #listeners = new Set<RouteListener>();
  /**
   * The hash the listeners were last told about. `navigate()` tells them at
   * once, because `hashchange` is a queued task: a busy page (a key held
   * down, a controller, a CI runner) can take further keys before it runs,
   * and they would land on a page still showing the route the URL has left.
   * The event that follows is then only the echo of what they already have.
   */
  #heard: string | null = null;
  readonly #onHashChange = (): void => {
    // Back, forward or a hand-typed URL: something the router did not do.
    if (window.location.hash !== this.#heard) this.#emit();
  };
  /**
   * Entries this router pushed itself. `window.history.length` also counts
   * entries from before the app was loaded, so trusting it sends `back()`
   * out of the app entirely on a deep link opened in a fresh tab.
   */
  #pushDepth = 0;

  get current(): AppRoute {
    return parseAppRoute(window.location.hash);
  }

  start(listener: RouteListener): () => void {
    this.#listeners.add(listener);
    if (this.#listeners.size === 1) window.addEventListener("hashchange", this.#onHashChange);
    this.#heard = window.location.hash;
    listener(this.current);
    return () => {
      this.#listeners.delete(listener);
      if (this.#listeners.size === 0) window.removeEventListener("hashchange", this.#onHashChange);
    };
  }

  navigate(route: AppRoute, options: { replace?: boolean } = {}): void {
    const hash = appRouteToHash(route);
    if (options.replace) {
      window.history.replaceState(window.history.state, "", hash);
      this.#emit();
    } else if (window.location.hash !== hash) {
      this.#pushDepth += 1;
      window.location.hash = hash;
      this.#emit();
    } else {
      this.#emit();
    }
  }

  back(fallback: AppRoute = { page: "library" }): void {
    if (this.#pushDepth > 0) {
      this.#pushDepth -= 1;
      window.history.back();
    } else {
      this.navigate(fallback, { replace: true });
    }
  }

  #emit(): void {
    this.#heard = window.location.hash;
    const route = this.current;
    for (const listener of this.#listeners) listener(route);
  }
}
