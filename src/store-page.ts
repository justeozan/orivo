import "./store-page.css";
import { invoke } from "@tauri-apps/api/core";
import type { AppRoute, GameSummary, PageRestoreState, StoreOffer, StoreTabId } from "./contracts";
import { t } from "./i18n";
import { icon, type IconName } from "./icons";
import { prefersReducedMotion } from "./motion";
import type { AppPage, PageActivation } from "./page-lifecycle";
import {
  createDefaultQuikyClient,
  createQuikyController,
  formatQuikyLabel,
  isQuikyBusy,
  matchTitle,
  quikyErrorMessage,
  quikyPercent,
  type QuikyClient,
  type QuikyController,
  type QuikyTitle,
} from "./quiky-install";
import { exclusiveLabel, STORE_TAB_LOOKS, storeTabLook } from "./store-brands";
import {
  adjacentStore,
  createInitialStoreState,
  editorialStoreHome,
  formatPrice,
  genreLabel,
  hasExclusives,
  isOfferStale,
  matchesQuery,
  offerVerifiedOn,
  ownershipKey,
  reduceStorePageState,
  selectBestOffer,
  selectStoreGames,
  selectStoreSections,
  criticScoreOf,
  type StoreBrowsePage,
  type StoreBrowseRequest,
  type StoreHomeView,
  type StorePageAction,
} from "./store-model";
import { openPluginStoreGamePage, syncPluginStoreTabs } from "./plugin-store-tabs";
import {
  listPluginStoreTabs,
  onStoreTabsChange,
  pluginStoreTab,
  type PluginStoreGame,
  type PluginStoreTab,
} from "./store-tabs";

export interface StorePageClient {
  getHome(signal: AbortSignal): Promise<StoreHomeView>;
  browse(request: StoreBrowseRequest, signal: AbortSignal): Promise<StoreBrowsePage>;
  refreshSources(signal: AbortSignal): Promise<void>;
  setWishlist(gameId: string, wishlisted: boolean, signal: AbortSignal): Promise<void>;
  openOffer(offerId: string, signal: AbortSignal): Promise<void>;
  /** Ids already in the library. The Store never sells a game you own. */
  listOwnedGameIds(signal: AbortSignal): Promise<string[]>;
}

export interface StorePageOptions {
  /**
   * Route changes stay owned by the application shell. The detail's "game
   * page" passes a game route with `from: "store"`; a tab passes a Store route.
   */
  navigate(route: AppRoute): void;
  client?: StorePageClient;
  /**
   * The installer is a plugin the Store may or may not find. Injecting the
   * client keeps that decision out of the page and testable.
   */
  quiky?: QuikyClient;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("La requête Store a été annulée.", "AbortError");
}

async function invokeWhileActive<T>(
  command: string,
  args: Record<string, unknown> | undefined,
  signal: AbortSignal,
): Promise<T> {
  assertActive(signal);
  const result = await invoke<T>(command, args);
  assertActive(signal);
  return result;
}

function cloneGame(game: GameSummary): GameSummary {
  return {
    ...game,
    genres: [...game.genres],
    tags: [...game.tags],
    supportedPlatforms: [...game.supportedPlatforms],
    recommendationReasons: [...game.recommendationReasons],
    offers: game.offers.map((offer) => ({ ...offer })),
    gallery: game.gallery ? [...game.gallery] : undefined,
    curation: game.curation
      ? {
          ...game.curation,
          genres: [...game.curation.genres],
          stats: game.curation.stats.map((stat) => ({ ...stat })),
          highlights: game.curation.highlights.map((highlight) => ({ ...highlight })),
          categories: [...game.curation.categories],
          platforms: [...game.curation.platforms],
        }
      : undefined,
  };
}

function cloneEditorialHome(): StoreHomeView {
  const home = editorialStoreHome();
  return { ...home, games: home.games.map(cloneGame) };
}

export function createDefaultStorePageClient(): StorePageClient {
  return {
    async getHome(signal) {
      if (!isTauriRuntime()) return cloneEditorialHome();
      return invokeWhileActive<StoreHomeView>("get_store_home", undefined, signal);
    },
    async browse(request, signal) {
      if (!isTauriRuntime()) {
        // Outside the desktop shell the bundled catalog is the catalog, so the
        // search pages through it rather than pretending to fail. It has to
        // match the query itself: a browse page is read as already searched,
        // and here there is no host to have done it.
        const home = cloneEditorialHome();
        const catalog = home.games.filter((game) => matchesQuery(game, request.query));
        const offset = request.cursor?.startsWith("store_")
          ? Number.parseInt(request.cursor.slice("store_".length), 10)
          : 0;
        const safeOffset = Number.isFinite(offset) && offset >= 0 ? offset : 0;
        const games = catalog.slice(safeOffset, safeOffset + request.limit);
        const nextOffset = safeOffset + games.length;
        return {
          games,
          nextCursor: nextOffset < catalog.length ? `store_${nextOffset}` : null,
          providerStatuses: home.providerStatuses,
        };
      }
      return invokeWhileActive<StoreBrowsePage>(
        "browse_store_games",
        { request: { ...request, providers: [] } },
        signal,
      );
    },
    async refreshSources(signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("refresh_store_sources", undefined, signal);
    },
    async setWishlist(gameId, wishlisted, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("set_game_wishlist", { gameId, wishlisted }, signal);
    },
    async openOffer(offerId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("open_store_offer", { offerId }, signal);
    },
    async listOwnedGameIds(signal) {
      if (!isTauriRuntime()) return [];
      try {
        const library = await invokeWhileActive<{ games?: Array<{ id: string; title: string }> }>(
          "get_library",
          undefined,
          signal,
        );
        // Ids and titles both go back: the same game can sit in the library
        // under a different source id than the Store row carries.
        return (library.games ?? []).flatMap((game) => [game.id, ownershipKey(game.title)]);
      } catch {
        // A library that cannot be read is not a reason to show no store.
        return [];
      }
    },
  };
}

interface FocusSnapshot {
  focusKey: string;
  selectionStart: number | null;
  selectionEnd: number | null;
}

/**
 * The nodes an install owns on a card. They are held rather than queried
 * because a download ticks several times a second and a shelf that rebuilds a
 * card per tick would drop the pointer, the focus and the artwork's fade.
 */
interface InstallNodes {
  title: QuikyTitle;
  button: HTMLButtonElement;
  label: HTMLElement;
  glyph: HTMLElement;
  glyphName: IconName;
  meter: HTMLElement;
  fill: HTMLElement;
}

/** The detail that is open, and where it came from so closing can go back there. */
interface OpenDetail {
  game: GameSummary;
  slides: string[];
  slide: number;
  originKey: string | null;
}

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function iconElement(name: IconName, className = ""): HTMLElement {
  const wrapper = element("span", `store-icon ${className}`.trim());
  wrapper.innerHTML = icon(name);
  wrapper.setAttribute("aria-hidden", "true");
  return wrapper;
}

function requestErrorMessage(error: unknown): string {
  if (error instanceof Error && error.name === "AbortError") return "";
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return t("store.error.unreachable");
}

/**
 * Under the price: the shop, and how fresh the reading is. A price older than
 * a day is still shown — with the day it was read, so the shopper knows how
 * fresh it is instead of being told a number Orivo cannot vouch for today.
 */
function offerCaption(offer: StoreOffer): string {
  const provider = offer.providerLabel;
  if (offer.priceMinor === null) return t("store.offer.unverified", { provider });
  if (!isOfferStale(offer)) return `${provider} · ${t("store.offer.verifiedToday")}`;
  const day = offerVerifiedOn(offer);
  return day ? `${provider} · ${t("store.offer.readOn", { day })}` : `${provider} · ${t("store.offer.recheck")}`;
}

/**
 * What a card prints as its price. A plugin's game carries its own quote
 * rather than an offer — a plugin is not one of the Store's shops — and it is
 * printed by the same rules.
 */
function priceLabel(game: GameSummary): string {
  const quote = (game as PluginStoreGame).pluginPrice;
  if (quote) return formatPrice(pluginOffer(game.id, quote.amountMinor, quote.currency));
  return formatPrice(selectBestOffer(game));
}

function pluginOffer(gameId: string, priceMinor: number, currency: string): StoreOffer {
  return {
    id: `plugin_${gameId}`,
    gameId,
    provider: "steam",
    providerLabel: "",
    priceMinor,
    currency,
    region: "",
    verifiedAt: null,
    availability: "available",
    stale: false,
  };
}

/**
 * The pictures the detail cycles through: the wide art first, then the
 * screenshots, then the card's own art. A portrait cover is only used when
 * nothing else exists, because a landscape stage would crop it to a strip.
 */
export function detailSlides(game: GameSummary): string[] {
  const wide = [game.heroUrl, ...(game.gallery ?? []), game.landscapeUrl].filter(Boolean);
  const slides = wide.length ? wide : [game.coverUrl].filter(Boolean);
  return [...new Set(slides)];
}

/** How long one picture stays up in the detail before the next one fades in. */
export const DETAIL_SLIDE_MS = 4_200;

const STORE_QUERY_LIMIT = 48;

/**
 * Creates the Store page. The shell injects navigation and imports
 * `store-page.css`; the page owns no topbar and no global styles.
 */
export function createStorePage(options: StorePageOptions): AppPage {
  const client = options.client ?? createDefaultStorePageClient();
  const quikyClient = options.quiky ?? createDefaultQuikyClient();
  let state = createInitialStoreState();
  let container: HTMLElement | null = null;
  let pageRoot: HTMLElement | null = null;
  let activation: PageActivation | null = null;
  let requestSequence = 0;
  let statusTimer: ReturnType<typeof setTimeout> | null = null;
  let slideTimer: ReturnType<typeof setInterval> | null = null;
  let transientStatus = "";
  /** When the host last answered for the home shelf, so a tab switch need not ask again. */
  let homeLoadedAt = 0;
  let detail: OpenDetail | null = null;
  let renderedMast = "";
  let renderedShelf = "";
  let tabsOff: (() => void) | null = null;
  // One controller per activation: it owns the event subscription and dies with
  // the page's teardown path, so a re-entry starts from a fresh status.
  let quiky: QuikyController | null = null;
  let quikyOff: (() => void) | null = null;
  const installNodes = new Map<string, InstallNodes>();
  let quikySearchPhase: "idle" | "loading" | "ready" | "empty" | "error" = "idle";
  let quikySearchMessage = "";
  let quikySearchResults: QuikyTitle[] = [];
  let quikySearchSequence = 0;
  let renderedQuikySearch = "";

  // Long-lived nodes. The page paints once and then updates in place: a full
  // rebuild on every change would drop focus, scroll and every image's fade.
  const nodes = {
    body: null as HTMLElement | null,
    previousStore: null as HTMLButtonElement | null,
    nextStore: null as HTMLButtonElement | null,
    mastLogo: null as HTMLElement | null,
    mastName: null as HTMLElement | null,
    mastShop: null as HTMLElement | null,
    mastLockup: null as HTMLImageElement | null,
    consoles: null as HTMLElement | null,
    shelf: null as HTMLElement | null,
    emptyState: null as HTMLElement | null,
    status: null as HTMLElement | null,
    quikyPanel: null as HTMLElement | null,
    quikyHeading: null as HTMLElement | null,
    quikyList: null as HTMLElement | null,
    detail: null as HTMLElement | null,
    detailPanel: null as HTMLElement | null,
    detailStage: null as HTMLElement | null,
    detailDots: null as HTMLElement | null,
    detailInfo: null as HTMLElement | null,
  };

  const dispatch = (action: StorePageAction, shouldRender = true): void => {
    state = reduceStorePageState(state, action);
    if (shouldRender) render();
  };

  const isActive = (context = activation): context is PageActivation =>
    Boolean(context && context.isCurrent() && !context.signal.aborted);

  const currentPluginTab = (): PluginStoreTab | null =>
    state.store.startsWith("plugin:") ? pluginStoreTab(state.store) : null;

  const showTransientStatus = (message: string): void => {
    transientStatus = message;
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
      transientStatus = "";
      renderStatus();
    }, 4_500);
    renderStatus();
  };

  // ---------------------------------------------------------------------------
  // Data
  // ---------------------------------------------------------------------------

  const loadOwned = async (context: PageActivation): Promise<void> => {
    try {
      const ids = await client.listOwnedGameIds(context.signal);
      if (!isActive(context) || ids.length === 0) return;
      dispatch({ type: "owned-games-loaded", gameIds: ids });
    } catch {
      // Silent: the library is an optional filter, not a dependency.
    }
  };

  const failRequest = (requestId: number, error: unknown): void => {
    const message = requestErrorMessage(error);
    if (!message) return;
    dispatch({
      type: "request-failed",
      requestId,
      message,
      offline: typeof navigator !== "undefined" && !navigator.onLine,
    });
  };

  const loadHome = async (context: PageActivation): Promise<void> => {
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId, refresh: state.home.games.length > 0 });
    try {
      const home = await client.getHome(context.signal);
      if (!isActive(context)) return;
      dispatch({ type: "home-loaded", requestId, home });
      homeLoadedAt = Date.now();
      if (typeof navigator === "undefined" || navigator.onLine) void refreshSources(context);
    } catch (error) {
      if (!isActive(context)) return;
      failRequest(requestId, error);
    }
  };

  const refreshSources = async (context: PageActivation): Promise<void> => {
    if (!isActive(context)) return;
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId, refresh: true });
    try {
      await client.refreshSources(context.signal);
      const home = await client.getHome(context.signal);
      if (!isActive(context)) return;
      dispatch({ type: "home-loaded", requestId, home });
      homeLoadedAt = Date.now();
    } catch (error) {
      if (!isActive(context)) return;
      failRequest(requestId, error);
    }
  };

  /** A search on "Tout" asks the host, which can reach games the shelf does not carry. */
  const search = async (context: PageActivation, cursor: string | null, append: boolean): Promise<void> => {
    if (!isActive(context) || !state.query.trim() || state.store !== "all") return;
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId, refresh: true });
    try {
      const page = await client.browse(
        { category: "all-games", platforms: [], query: state.query.trim(), cursor, limit: STORE_QUERY_LIMIT },
        context.signal,
      );
      if (!isActive(context)) return;
      dispatch({ type: "browse-loaded", requestId, page, append });
    } catch (error) {
      if (!isActive(context)) return;
      failRequest(requestId, error);
    }
  };

  const handleWishlist = async (game: GameSummary): Promise<void> => {
    const context = activation;
    if (!isActive(context)) return;
    const wishlisted = !game.wishlisted;
    dispatch({ type: "wishlist-changed", gameId: game.id, wishlisted });
    try {
      await client.setWishlist(game.id, wishlisted, context.signal);
      if (isActive(context)) {
        showTransientStatus(
          wishlisted
            ? t("store.wishlist.added", { title: game.title })
            : t("store.wishlist.removed", { title: game.title }),
        );
      }
    } catch (error) {
      if (!isActive(context)) return;
      dispatch({ type: "wishlist-changed", gameId: game.id, wishlisted: !wishlisted });
      showTransientStatus(requestErrorMessage(error) || t("store.wishlist.cancelled"));
    }
  };

  const handleBuy = async (offer: StoreOffer): Promise<void> => {
    const context = activation;
    if (!isActive(context)) return;
    try {
      await client.openOffer(offer.id, context.signal);
    } catch (error) {
      if (!isActive(context)) return;
      const message = requestErrorMessage(error);
      if (message) showTransientStatus(message);
    }
  };

  const selectStore = (store: StoreTabId): void => {
    if (state.store === store) return;
    closeDetail(false);
    dispatch({ type: "store-changed", store });
    options.navigate({ page: "store", store, query: state.query.trim() });
    container?.scrollTo?.({ top: 0 });
  };

  // ---------------------------------------------------------------------------
  // Install (Quiky plugin)
  // ---------------------------------------------------------------------------

  /**
   * The gate for the whole feature. No plugin, no match, no affordance — and a
   * card that gets no affordance is exactly the card the Store painted before
   * the installer existed.
   */
  const quikyTitleFor = (game: GameSummary): QuikyTitle | null => {
    const status = quiky?.status();
    if (!status?.available) return null;
    return matchTitle(status.titles, game.title);
  };

  const quikyTitleBySlug = (slug: string): QuikyTitle | null =>
    quiky?.status().titles.find((title) => title.slug === slug) ?? null;

  const handleInstall = async (slug: string, gameId: string | null): Promise<void> => {
    const controller = quiky;
    if (!controller) return;
    // One control, two jobs: while the host is working it is the way out.
    if (isQuikyBusy(controller.progressFor(slug))) {
      await controller.cancel(slug);
      return;
    }
    if (quikyTitleBySlug(slug)?.installed) return;
    await controller.install(slug, gameId);
    const progress = controller.progressFor(slug);
    if (progress?.phase === "failed" && progress.message) showTransientStatus(progress.message);
  };

  const quikyResultMeta = (title: QuikyTitle): string => {
    if (title.installed) return t("store.quiky.installed");
    return title.source === "fitgirl" ? t("store.quiky.fitgirl") : t("store.quiky.windowsInstaller");
  };

  const buildQuikyResult = (title: QuikyTitle): HTMLElement => {
    const row = element("article", "quiky-result");
    row.dataset.slug = title.slug;
    const copy = element("div", "quiky-result__copy");
    copy.append(
      element("h3", "quiky-result__title", title.title),
      element("p", "quiky-result__meta", quikyResultMeta(title)),
    );
    const install = buildInstall(`quiky:${title.slug}`, title, null);
    install.button.classList.add("quiky-result__install");
    row.append(copy, install.button);
    return row;
  };

  /**
   * The panel is driven by the same query as the Store search. That is the
   * whole point: typing a game name should find the repack even when no shop
   * carries it, without a second search box that would look like a different
   * product.
   */
  const renderQuikySearch = (): void => {
    const panel = nodes.quikyPanel;
    const list = nodes.quikyList;
    if (!panel || !list) return;
    const query = state.query.trim();
    const status = quiky?.status();
    const visible = Boolean(status?.available && status.searchable && query);
    panel.hidden = !visible;
    if (!visible) {
      renderedQuikySearch = "";
      quikySearchPhase = "idle";
      quikySearchMessage = "";
      quikySearchResults = [];
      list.replaceChildren();
      return;
    }

    const signature = `${quikySearchPhase}|${query}|${quikySearchResults.map((title) => title.slug).join(",")}`;
    if (signature !== renderedQuikySearch) {
      renderedQuikySearch = signature;
      for (const key of [...installNodes.keys()]) {
        if (key.startsWith("quiky:")) installNodes.delete(key);
      }
      const fragment = document.createDocumentFragment();
      if (quikySearchPhase === "ready") {
        for (const title of quikySearchResults) fragment.append(buildQuikyResult(title));
      } else {
        const message =
          quikySearchPhase === "loading"
            ? t("store.quiky.searching")
            : quikySearchPhase === "error"
              ? quikySearchMessage || t("store.quiky.failed")
              : quikySearchPhase === "empty"
                ? t("store.quiky.empty")
                : "";
        if (message) fragment.append(element("p", "quiky-results__empty", message));
      }
      list.replaceChildren(fragment);
    }

    if (nodes.quikyHeading) {
      const short = query.length > 48 ? `${query.slice(0, 48)}…` : query;
      nodes.quikyHeading.textContent =
        quikySearchPhase === "loading"
          ? t("store.quiky.headingLoading", { query: short })
          : quikySearchPhase === "error"
            ? t("store.quiky.headingError", { query: short })
            : t("store.quiky.headingResults", { query: short });
    }
    renderInstallState();
  };

  const searchQuiky = async (context: PageActivation, query: string): Promise<void> => {
    const controller = quiky;
    const trimmed = query.trim();
    const sequence = ++quikySearchSequence;
    const status = controller?.status();
    if (!controller || !status?.available || !status.searchable || trimmed.length < 2) {
      quikySearchPhase = "idle";
      quikySearchMessage = "";
      quikySearchResults = [];
      renderQuikySearch();
      return;
    }

    quikySearchPhase = "loading";
    quikySearchMessage = "";
    quikySearchResults = [];
    renderQuikySearch();
    try {
      const results = await controller.search(trimmed, context.signal);
      if (sequence !== quikySearchSequence || !isActive(context)) return;
      quikySearchResults = results;
      quikySearchPhase = results.length > 0 ? "ready" : "empty";
    } catch (error) {
      if (sequence !== quikySearchSequence || !isActive(context)) return;
      if (error instanceof Error && error.name === "AbortError") return;
      quikySearchResults = [];
      quikySearchPhase = "error";
      quikySearchMessage = quikyErrorMessage(error);
    }
    renderQuikySearch();
  };

  /**
   * The install control and its bar. The button joins the card's floating
   * controls, under the wishlist so neither has to move when both are there;
   * the bar is three pixels on the foot of the artwork and nothing else.
   */
  const buildInstall = (
    key: string,
    title: QuikyTitle,
    gameId: string | null,
  ): { button: HTMLButtonElement; meter: HTMLElement } => {
    const button = element("button", "store-card__install");
    button.type = "button";
    button.dataset.focusKey = `install-${key}`;
    button.dataset.phase = "idle";
    button.setAttribute("aria-label", t("store.install.aria", { title: title.title }));
    const glyph = iconElement("download", "store-card__install-glyph");
    const label = element("span", "store-card__install-label", t("store.install.label"));
    button.append(glyph, label);
    button.addEventListener("click", (event) => {
      // The card opens the detail; installing is not opening.
      event.stopPropagation();
      void handleInstall(title.slug, gameId);
    });

    // Spans, not divs: the artwork lives inside the card's open button, and a
    // button may only carry phrasing content.
    const meter = element("span", "store-card__meter");
    meter.setAttribute("role", "progressbar");
    meter.setAttribute("aria-valuemin", "0");
    meter.setAttribute("aria-valuemax", "100");
    meter.setAttribute("aria-valuenow", "0");
    meter.setAttribute("aria-label", t("store.install.meterIdle", { title: title.title }));
    meter.hidden = true;
    const fill = element("span", "store-card__fill");
    fill.style.width = "0%";
    meter.append(fill);

    installNodes.set(key, { title, button, label, glyph, glyphName: "download", meter, fill });
    return { button, meter };
  };

  const installGlyph = (busy: boolean, installed: boolean, failed: boolean): IconName => {
    if (busy) return "close";
    if (installed) return "check";
    return failed ? "refresh" : "download";
  };

  /**
   * The whole per-tick repaint: two text nodes, one width and a handful of
   * attributes, on nodes that already exist. Nothing here builds an element.
   */
  const renderInstallState = (): void => {
    const controller = quiky;
    if (!controller || installNodes.size === 0) return;
    for (const [, entry] of installNodes) {
      const slug = entry.title.slug;
      const title = quikyTitleBySlug(slug) ?? entry.title;
      entry.title = title;
      const progress = controller.progressFor(slug);
      const busy = isQuikyBusy(progress);
      const percent = quikyPercent(progress);
      const phaseLabel = formatQuikyLabel(progress, title);
      const installed = title.installed || progress?.phase === "installed";
      const failed = progress?.phase === "failed";

      const label = busy ? t("store.install.cancel") : phaseLabel;
      if (entry.label.textContent !== label) entry.label.textContent = label;

      const glyphName = installGlyph(busy, installed, failed);
      if (glyphName !== entry.glyphName) {
        entry.glyph.innerHTML = icon(glyphName);
        entry.glyphName = glyphName;
      }

      // `aria-disabled` rather than `disabled`: the page restores focus by key
      // after every render, and a disabled button cannot be focused.
      entry.button.setAttribute("aria-disabled", String(installed && !busy));
      entry.button.dataset.phase = progress?.phase ?? (installed ? "installed" : "idle");
      entry.button.title = progress?.message ?? "";
      entry.button.setAttribute(
        "aria-label",
        busy
          ? t("store.install.cancelAria", { title: title.title, phase: phaseLabel })
          : installed
            ? t("store.install.alreadyAria", { title: title.title })
            : t("store.install.aria", { title: title.title }),
      );

      entry.meter.hidden = !busy;
      entry.meter.setAttribute("aria-valuenow", String(percent));
      entry.meter.setAttribute("aria-label", t("store.install.meterPhase", { title: title.title, phase: phaseLabel }));
      entry.fill.style.width = `${percent}%`;
    }
  };

  // ---------------------------------------------------------------------------
  // Skeleton
  // ---------------------------------------------------------------------------

  /**
   * The brand's light: a wash, two sweeps and a horizon. All of it is drawn
   * from the page's accent token, so changing tab is one variable changing.
   */
  const buildAmbient = (): HTMLElement => {
    const ambient = element("div", "store-ambient");
    ambient.setAttribute("aria-hidden", "true");
    ambient.append(
      element("span", "store-ambient__wash"),
      element("span", "store-ambient__sweep store-ambient__sweep--high"),
      element("span", "store-ambient__sweep store-ambient__sweep--low"),
    );
    return ambient;
  };

  const buildMasthead = (): HTMLElement => {
    const mast = element("header", "store-mast");
    const brand = element("div", "store-mast__brand");
    nodes.mastLogo = element("span", "store-mast__logo");
    nodes.mastLogo.setAttribute("aria-hidden", "true");
    const heading = element("h1", "store-mast__title");
    nodes.mastName = element("span", "store-mast__name");
    nodes.mastShop = element("span", "store-mast__shop");
    nodes.mastLockup = element("img", "store-mast__lockup");
    nodes.mastLockup.hidden = true;
    nodes.mastLockup.addEventListener("error", () => {
      if (!nodes.mastLockup || !nodes.mastName || !nodes.mastShop) return;
      nodes.mastLockup.hidden = true;
      nodes.mastName.hidden = false;
      nodes.mastShop.hidden = false;
    });
    heading.append(nodes.mastLockup, nodes.mastName, nodes.mastShop);
    brand.append(nodes.mastLogo, heading);
    nodes.consoles = element("div", "store-mast__consoles");
    nodes.consoles.setAttribute("aria-hidden", "true");
    nodes.previousStore = switchButton(-1);
    nodes.nextStore = switchButton(1);
    mast.append(nodes.previousStore, brand, nodes.consoles, element("span", "store-mast__horizon"), nodes.nextStore);
    return mast;
  };

  /**
   * The way between stores: one chevron on each side of the page, walking the
   * stores in a loop. The store on screen is the masthead; there is no bar of
   * names to read.
   */
  const switchButton = (step: 1 | -1): HTMLButtonElement => {
    const button = element("button", `store-switch store-switch--${step < 0 ? "previous" : "next"}`);
    button.type = "button";
    button.dataset.focusKey = step < 0 ? "store-previous" : "store-next";
    button.append(iconElement(step < 0 ? "chevron-left" : "chevron-right"));
    button.addEventListener("click", () => selectStore(adjacentStore(storeOrder(), state.store, step)));
    return button;
  };

  /** Every store, in the order the chevrons walk them: Tout, the brands, then plugins. */
  const storeOrder = (): StoreTabId[] => [
    ...STORE_TAB_LOOKS.map((look) => look.id),
    ...listPluginStoreTabs().map((tab) => tab.id),
  ];

  const storeLabel = (id: StoreTabId): string =>
    storeTabLook(id)?.label() ?? pluginStoreTab(id)?.label ?? id;

  /**
   * Search results live beside the shelf, not inside it: a FitGirl repack has
   * no Store artwork, price or offer, and pretending it is a Store card would
   * make the page promise things it cannot show. The panel appears only while
   * the Store search has a query and the plugin declares a source.
   */
  const buildQuikySearch = (): HTMLElement => {
    const panel = element("section", "quiky-results");
    panel.hidden = true;
    panel.setAttribute("aria-label", t("store.quiky.resultsLabel"));
    const head = element("div", "quiky-results__head");
    nodes.quikyHeading = element("p", "quiky-results__title");
    head.append(nodes.quikyHeading, element("span", "quiky-results__badge", "FitGirl"));
    nodes.quikyList = element("div", "quiky-results__list");
    panel.append(head, nodes.quikyList);
    nodes.quikyPanel = panel;
    return panel;
  };

  /**
   * The detail is a sibling of the page body, never inside it: opening it
   * makes the body `inert`, and an inert ancestor would take the dialog with
   * it. Nothing between it and the viewport is transformed or filtered, so its
   * `position: fixed` really is the window.
   */
  const buildDetail = (): HTMLElement => {
    const root = element("div", "store-detail");
    root.hidden = true;
    root.setAttribute("role", "dialog");
    root.setAttribute("aria-modal", "true");
    root.setAttribute("aria-labelledby", "store-detail-title");
    const scrim = element("div", "store-detail__scrim");
    scrim.addEventListener("click", () => closeDetail());
    const panel = element("article", "store-detail__panel");
    const stage = element("div", "store-detail__stage");
    const dots = element("div", "store-detail__dots");
    const info = element("div", "store-detail__info");
    const close = element("button", "store-detail__close");
    close.type = "button";
    close.dataset.focusKey = "detail-close";
    close.setAttribute("aria-label", t("store.detail.close"));
    close.append(iconElement("close"));
    close.addEventListener("click", () => closeDetail());
    panel.append(stage, dots, info, close);
    root.append(scrim, panel);
    root.addEventListener("keydown", trapTab);
    nodes.detail = root;
    nodes.detailPanel = panel;
    nodes.detailStage = stage;
    nodes.detailDots = dots;
    nodes.detailInfo = info;
    return root;
  };

  const buildSkeleton = (): void => {
    if (!pageRoot) return;
    const body = element("div", "store-page__body");
    nodes.body = body;
    nodes.shelf = element("div", "store-shelf");
    nodes.emptyState = element("div", "store-empty");
    nodes.emptyState.hidden = true;
    body.append(buildMasthead(), nodes.shelf, nodes.emptyState, buildQuikySearch());
    nodes.status = element("div", "store-status");
    nodes.status.setAttribute("aria-live", "polite");
    nodes.status.hidden = true;
    const topfade = element("div", "store-topfade");
    topfade.setAttribute("aria-hidden", "true");
    pageRoot.replaceChildren(buildAmbient(), topfade, body, nodes.status, buildDetail());
  };

  // ---------------------------------------------------------------------------
  // Rendering
  // ---------------------------------------------------------------------------

  const visibleGames = (): GameSummary[] => selectStoreGames(state, currentPluginTab()?.games ?? null);

  /**
   * The page's colour. A brand's is a token in the stylesheet; a plugin's is
   * the one it declared, already checked to be a plain `#rrggbb`.
   */
  const renderTheme = (): void => {
    if (!pageRoot) return;
    const plugin = currentPluginTab();
    pageRoot.dataset.store = plugin ? "plugin" : state.store;
    if (plugin) pageRoot.style.setProperty("--store-accent", plugin.accent);
    else pageRoot.style.removeProperty("--store-accent");
  };

  /** The chevrons name the store each one leads to. */
  const renderSwitches = (): void => {
    const order = storeOrder();
    const entries: Array<[HTMLButtonElement | null, 1 | -1]> = [
      [nodes.previousStore, -1],
      [nodes.nextStore, 1],
    ];
    for (const [button, step] of entries) {
      if (!button) continue;
      const target = storeLabel(adjacentStore(order, state.store, step));
      const label = t(step < 0 ? "store.switch.previous" : "store.switch.next", { store: target });
      button.setAttribute("aria-label", label);
      button.title = target;
    }
  };

  const renderMasthead = (): void => {
    if (!nodes.mastLogo || !nodes.mastName || !nodes.mastShop || !nodes.mastLockup || !nodes.consoles) return;
    const look = storeTabLook(state.store);
    const plugin = currentPluginTab();
    const signature = `${state.store}|${plugin?.label ?? ""}`;
    if (signature === renderedMast) return;
    renderedMast = signature;
    const lockup = nodes.mastLockup;
    lockup.hidden = !look?.lockupUrl;
    nodes.mastLogo.hidden = Boolean(look?.lockupUrl);
    nodes.mastName.hidden = Boolean(look?.lockupUrl);
    nodes.mastShop.hidden = Boolean(look?.lockupUrl);
    if (look?.lockupUrl) {
      // The store's own logo already says its name; the text stays as the
      // picture's alternative, and comes back if the picture does not.
      lockup.src = look.lockupUrl;
      lockup.alt = `${look.name} ${look.shop()}`;
      nodes.mastName.textContent = look.name;
      nodes.mastShop.textContent = look.shop();
    } else if (look) {
      const mark = element("img", "store-mast__image");
      mark.src = look.markUrl ?? "";
      mark.alt = "";
      mark.addEventListener("error", () => { if (nodes.mastLogo) nodes.mastLogo.innerHTML = icon(look.icon); }, { once: true });
      nodes.mastLogo.replaceChildren(mark);
      nodes.mastName.textContent = look.name;
      nodes.mastShop.textContent = look.shop();
    } else {
      // A plugin's store, or one whose plugin has not answered yet: its name
      // in the masthead, and no machines it never declared.
      nodes.mastLogo.replaceChildren(element("span", "store-mast__dot"));
      nodes.mastName.textContent = plugin?.label ?? "";
      nodes.mastShop.textContent = "";
    }
    nodes.consoles.replaceChildren(
      ...(look?.machines ?? []).map((machine) => {
        const figure = element("span", `store-console store-console--${machine.place}`);
        const picture = element("img", "store-console__art");
        picture.src = machine.src;
        picture.alt = "";
        picture.decoding = "async";
        picture.addEventListener("error", () => figure.remove(), { once: true });
        figure.append(picture);
        return figure;
      }),
    );
    // The masthead rises in again on every change of store, from a class the
    // stylesheet animates only when motion is allowed.
    const mast = nodes.consoles.parentElement;
    mast?.classList.remove("store-mast--enter");
    void mast?.offsetWidth;
    mast?.classList.add("store-mast--enter");
  };

  const restSectionTitle = (): string => {
    if (state.store.startsWith("plugin:")) return t("store.section.allGames");
    if (state.store === "all") {
      return hasExclusives(state.home.games) ? t("store.section.allExclusives") : t("store.section.allGames");
    }
    return t("store.section.moreExclusives");
  };

  const buildSection = (
    key: string,
    title: string,
    cards: HTMLElement[],
    variant: "row" | "grid",
  ): HTMLElement => {
    const section = element("section", `store-section store-section--${key}`);
    const heading = element("h2", "store-section__title", title);
    heading.id = `store-section-${key}`;
    section.setAttribute("aria-labelledby", heading.id);
    const grid = element("div", `store-grid store-grid--${variant}`);
    grid.setAttribute("role", "list");
    // A row of three is one band for the keys; the long grid is walked by
    // position, so down goes to the card below rather than out of the grid.
    if (variant === "row") grid.setAttribute("data-nav-row", "");
    grid.append(...cards);
    section.append(heading, grid);
    return section;
  };

  /**
   * The card set, not the state inside a card. A download stepping from 41% to
   * 42% must never land here. The Quiky slug is part of it because a plugin
   * that finishes loading after the first paint adds a control to cards that
   * already exist.
   */
  const shelfSignature = (games: GameSummary[]): string =>
    `${state.store}|${state.query.trim()}|${games
      .map((game) => `${game.id}:${game.wishlisted ? 1 : 0}:${quikyTitleFor(game)?.slug ?? ""}`)
      .join("|")}`;

  const renderShelf = (games: GameSummary[]): void => {
    const shelf = nodes.shelf;
    const empty = nodes.emptyState;
    if (!shelf || !empty) return;
    const signature = shelfSignature(games);
    if (signature !== renderedShelf) {
      renderedShelf = signature;
      for (const key of [...installNodes.keys()]) {
        if (!key.startsWith("quiky:") && !key.startsWith("detail:")) installNodes.delete(key);
      }
      let index = 0;
      const card = (game: GameSummary, rank?: number): HTMLElement => buildCard(game, index++, rank);
      const sections: HTMLElement[] = [];
      if (state.query.trim()) {
        if (games.length) sections.push(buildSection("results", t("store.section.results"), games.map((game) => card(game)), "grid"));
      } else {
        const { featured, topRated, rest } = selectStoreSections(games);
        if (featured.length) sections.push(buildSection("featured", t("store.recommended"), featured.map((game) => card(game)), "row"));
        if (topRated.length) {
          sections.push(
            buildSection("top-rated", t("store.section.topRated"), topRated.map((entry) => card(entry.game, entry.rank)), "row"),
          );
        }
        if (rest.length) sections.push(buildSection("rest", restSectionTitle(), rest.map((game) => card(game)), "grid"));
      }
      shelf.replaceChildren(...sections);
    }
    shelf.hidden = games.length === 0;
    empty.hidden = games.length > 0;
    if (games.length === 0) {
      const brandEmpty = !state.query.trim() && state.store !== "all";
      empty.replaceChildren(
        iconElement(brandEmpty ? "sparkle" : "search"),
        element("h2", "store-empty__title", brandEmpty ? t("store.empty.brand") : t("store.empty.title")),
      );
    }
    renderInstallState();
  };

  const buildCard = (game: GameSummary, index: number, rank?: number): HTMLElement => {
    const card = element("article", `store-card${rank ? " store-card--ranked" : ""}`);
    card.dataset.gameId = game.id;
    card.setAttribute("role", "listitem");

    const open = element("button", "store-card__open");
    open.type = "button";
    open.dataset.focusKey = `game-${game.id}`;
    open.dataset.navOpen = game.id;
    open.setAttribute(
      "aria-label",
      rank ? t("store.card.ranked", { rank, title: game.title }) : t("store.card.open", { title: game.title }),
    );
    open.addEventListener("click", () => openDetail(game, `game-${game.id}`));

    const media = element("span", "store-card__media");
    const art = element("img", "store-card__art");
    // The card is the key art the game is sold with — its capsule — which
    // carries the wordmark. The wide banner is the detail's job. Each fallback
    // is tried once.
    const artSources = [game.landscapeUrl, game.heroUrl, game.coverUrl].filter(Boolean);
    let artIndex = 0;
    art.src = artSources[0] ?? "";
    art.alt = "";
    art.loading = index < 6 ? "eager" : "lazy";
    art.decoding = "async";
    art.addEventListener("error", () => {
      artIndex += 1;
      if (artIndex < artSources.length) art.src = artSources[artIndex];
      else media.classList.add("store-card__media--missing");
    });
    media.append(art, element("span", "store-card__veil"));
    // Store art carries the game's name — that is what it is for. The shelf's
    // own files are `capsule.jpg`; a game that arrives live from Steam brings
    // `capsule_616x353.jpg`, or `header.jpg`. Printing a title over any of them
    // shows the name twice.
    const artHasWordmark = /\/(capsule[^/]*|header)\.jpg(\?|$)/.test(artSources[0] ?? "");
    if (!artHasWordmark) media.append(element("span", "store-card__title", game.title));
    if (rank) {
      const numeral = element("span", "store-card__rank", String(rank));
      numeral.setAttribute("aria-hidden", "true");
      media.append(numeral);
    }
    // A shop that quoted nothing leaves the slot empty rather than printing a
    // blank price the shopper would read as "free".
    const price = priceLabel(game);
    if (price) media.append(element("span", "store-card__price", price));
    open.append(media);

    const wishlist = element("button", "store-card__wishlist");
    wishlist.type = "button";
    wishlist.dataset.focusKey = `wishlist-${game.id}`;
    wishlist.dataset.navSkip = "";
    wishlist.setAttribute("aria-pressed", String(game.wishlisted));
    wishlist.setAttribute(
      "aria-label",
      game.wishlisted
        ? t("store.wishlist.removeLabel", { title: game.title })
        : t("store.wishlist.addLabel", { title: game.title }),
    );
    wishlist.append(iconElement(game.wishlisted ? "heart-filled" : "heart"));
    wishlist.addEventListener("click", (event) => {
      event.stopPropagation();
      void handleWishlist(game);
    });
    // A plugin's game is not in any shop the library can wishlist from.
    card.append(open);
    if (!isPluginGame(game)) card.append(wishlist);

    const quikyTitle = quikyTitleFor(game);
    if (quikyTitle) {
      const install = buildInstall(game.id, quikyTitle, game.id);
      media.append(install.meter);
      card.append(install.button);
    }
    return card;
  };

  /**
   * Every change the installer reports. A phase tick repaints the nodes it
   * owns; only a change to the card set itself — a plugin that just became
   * available, a title that just started matching — rebuilds the shelf.
   */
  const onQuikyChange = (): void => {
    if (!nodes.shelf) return;
    const games = visibleGames();
    if (shelfSignature(games) === renderedShelf) renderInstallState();
    else renderShelf(games);
    renderQuikySearch();
  };

  const teardownQuiky = (): void => {
    quikyOff?.();
    quikyOff = null;
    quiky?.dispose();
    quiky = null;
    installNodes.clear();
  };

  const renderStatus = (): void => {
    if (!nodes.status) return;
    const message = transientStatus || state.errorMessage;
    if (!message) {
      nodes.status.hidden = true;
      nodes.status.replaceChildren();
      return;
    }
    nodes.status.hidden = false;
    nodes.status.className = `store-status store-status--${state.phase}`;
    nodes.status.setAttribute("role", state.phase === "error" ? "alert" : "status");
    nodes.status.replaceChildren(
      iconElement(state.phase === "refreshing" || state.phase === "loading" ? "refresh" : "alert"),
      element("span", "store-status__copy", message),
    );
  };

  const render = (): void => {
    if (!pageRoot) return;
    const focusSnapshot = captureFocus();
    const games = visibleGames();
    renderTheme();
    renderSwitches();
    renderMasthead();
    renderShelf(games);
    renderDetailFacts();
    renderStatus();
    renderQuikySearch();
    restoreFocus(focusSnapshot);
  };

  // ---------------------------------------------------------------------------
  // Detail
  // ---------------------------------------------------------------------------

  const stopSlides = (): void => {
    if (slideTimer) clearInterval(slideTimer);
    slideTimer = null;
  };

  const showSlide = (index: number): void => {
    if (!detail || !nodes.detailStage || !nodes.detailDots) return;
    const count = detail.slides.length;
    if (count === 0) return;
    detail.slide = ((index % count) + count) % count;
    nodes.detailStage.querySelectorAll<HTMLElement>(".store-detail__slide").forEach((slide, position) => {
      slide.classList.toggle("is-active", position === detail?.slide);
    });
    nodes.detailDots.querySelectorAll<HTMLElement>(".store-detail__dot").forEach((dot, position) => {
      dot.setAttribute("aria-current", String(position === detail?.slide));
    });
  };

  /** The pictures cycle on their own, and never under reduced motion. */
  const startSlides = (): void => {
    stopSlides();
    if (!detail || detail.slides.length < 2 || prefersReducedMotion()) return;
    slideTimer = setInterval(() => showSlide((detail?.slide ?? 0) + 1), DETAIL_SLIDE_MS);
  };

  const buildDetailStage = (current: OpenDetail): void => {
    const stage = nodes.detailStage;
    const dots = nodes.detailDots;
    if (!stage || !dots) return;
    stage.replaceChildren(
      ...current.slides.map((url, index) => {
        const slide = element("img", `store-detail__slide${index === 0 ? " is-active" : ""}`);
        slide.src = url;
        slide.alt = "";
        slide.decoding = "async";
        slide.loading = index < 2 ? "eager" : "lazy";
        slide.addEventListener("error", () => slide.remove(), { once: true });
        return slide;
      }),
    );
    dots.hidden = current.slides.length < 2;
    dots.replaceChildren(
      ...current.slides.map((_, index) => {
        const dot = element("button", "store-detail__dot");
        dot.type = "button";
        dot.dataset.navSkip = "";
        dot.setAttribute("aria-label", t("store.detail.slide", { index: index + 1, count: current.slides.length }));
        dot.setAttribute("aria-current", String(index === 0));
        dot.addEventListener("click", () => {
          showSlide(index);
          startSlides();
        });
        return dot;
      }),
    );
  };

  /** Title, pills, price and verbs. Rebuilt on every render: the wishlist can change under it. */
  const renderDetailFacts = (): void => {
    const info = nodes.detailInfo;
    if (!detail || !info) return;
    const game = visibleGames().find((entry) => entry.id === detail?.game.id)
      ?? state.home.games.find((entry) => entry.id === detail?.game.id)
      ?? detail.game;
    detail.game = game;

    const pills = element("div", "store-detail__pills");
    if (game.exclusiveTo) {
      const exclusive = element("span", "store-pill store-pill--brand", exclusiveLabel(game.exclusiveTo));
      exclusive.dataset.brand = game.exclusiveTo;
      pills.append(exclusive);
    }
    const genres = genreLabel(game);
    if (game.genres.length || game.curation?.genres.length) pills.append(element("span", "store-pill", genres));
    if (game.curation?.duration) pills.append(element("span", "store-pill", game.curation.duration));
    const score = criticScoreOf(game);
    if (score !== null) pills.append(element("span", "store-pill", t("store.detail.score", { score })));

    const title = element("h2", "store-detail__title", game.title);
    title.id = "store-detail-title";
    const lead = element("p", "store-detail__lead", game.curation?.tagline || game.shortDescription);

    const buy = element("div", "store-detail__buy");
    if (isPluginGame(game)) {
      info.replaceChildren(pills, title, lead, buy);
      renderPluginBuy(game as PluginStoreGame, buy);
      return;
    }
    const offer = selectBestOffer(game);
    const price = formatPrice(offer);
    if (offer) {
      const priceBlock = element("div", "store-detail__price");
      if (price) priceBlock.append(element("span", "store-detail__amount", price));
      priceBlock.append(element("span", "store-detail__offer", offerCaption(offer)));
      buy.append(priceBlock);
    }

    const actions = element("div", "store-detail__actions");
    if (offer && offer.availability !== "unavailable") {
      const purchase = element("button", "store-detail__action store-detail__action--primary");
      purchase.type = "button";
      purchase.dataset.focusKey = "detail-buy";
      purchase.append(iconElement("link"), element("span", "", t("store.detail.buy", { provider: offer.providerLabel })));
      purchase.addEventListener("click", () => void handleBuy(offer));
      actions.append(purchase);
    }
    if (!game.id.startsWith("plugin:") && !currentPluginTab()) {
      const page = element("button", "store-detail__action");
      page.type = "button";
      page.dataset.focusKey = "detail-page";
      page.append(element("span", "", t("store.detail.open")));
      page.addEventListener("click", () => {
        const gameId = game.id;
        // Focus goes back to the card first, so coming back to the Store
        // lands on the game that was left.
        closeDetail();
        options.navigate({ page: "game", gameId, from: "store" });
      });
      actions.append(page);
    }
    const wishlist = element("button", "store-detail__action store-detail__action--icon");
    wishlist.type = "button";
    wishlist.dataset.focusKey = "detail-wishlist";
    wishlist.setAttribute("aria-pressed", String(game.wishlisted));
    wishlist.setAttribute(
      "aria-label",
      game.wishlisted
        ? t("store.wishlist.removeLabel", { title: game.title })
        : t("store.wishlist.addLabel", { title: game.title }),
    );
    wishlist.append(iconElement(game.wishlisted ? "heart-filled" : "heart"));
    wishlist.addEventListener("click", () => void handleWishlist(game));
    actions.append(wishlist);
    for (const key of [...installNodes.keys()]) if (key.startsWith("detail:")) installNodes.delete(key);
    const quikyTitle = quikyTitleFor(game);
    if (quikyTitle) {
      const install = buildInstall(`detail:${game.id}`, quikyTitle, game.id);
      install.button.classList.add("store-detail__install");
      actions.append(install.button);
    }
    buy.append(actions);

    info.replaceChildren(pills, title, lead, buy);
    renderInstallState();
  };

  const isPluginGame = (game: GameSummary): boolean => currentPluginTab()?.games.some((entry) => entry.id === game.id) ?? false;

  /**
   * A plugin's game: its own quote, and its page in the player's browser. The
   * host resolves the address from the id; the page never holds it.
   */
  const renderPluginBuy = (game: PluginStoreGame, buy: HTMLElement): void => {
    const tab = currentPluginTab();
    const price = game.pluginPrice ? priceLabel(game) : "";
    if (price) {
      const priceBlock = element("div", "store-detail__price");
      priceBlock.append(element("span", "store-detail__amount", price));
      if (tab) priceBlock.append(element("span", "store-detail__offer", tab.label));
      buy.append(priceBlock);
    }
    const actions = element("div", "store-detail__actions");
    if (game.hasPluginPage) {
      const visit = element("button", "store-detail__action store-detail__action--primary");
      visit.type = "button";
      visit.dataset.focusKey = "detail-buy";
      visit.append(iconElement("link"), element("span", "", t("store.detail.visit")));
      visit.addEventListener("click", () => {
        void openPluginStoreGamePage(game.id).catch((error: unknown) => {
          const message = requestErrorMessage(error);
          if (message) showTransientStatus(message);
        });
      });
      actions.append(visit);
    }
    if (actions.childElementCount) buy.append(actions);
  };

  const detailFocusTarget = (): HTMLElement | null =>
    nodes.detail?.querySelector<HTMLElement>("[data-focus-key='detail-buy'], [data-focus-key='detail-page'], [data-focus-key='detail-close']") ?? null;

  /**
   * The detail grows out of the card it was opened from: the stage starts on
   * the card's own rectangle and settles into place, the facts rise in after
   * it. Under reduced motion it simply appears.
   */
  const animateOpen = (originKey: string | null): void => {
    const stage = nodes.detailStage;
    const panel = nodes.detailPanel;
    if (!stage || !panel || prefersReducedMotion() || typeof panel.animate !== "function") return;
    const origin = originKey ? focusTargetFor(originKey)?.querySelector<HTMLElement>(".store-card__media") : null;
    const from = origin?.getBoundingClientRect();
    const to = stage.getBoundingClientRect();
    const box = panel.getBoundingClientRect();
    if (from && from.width > 0 && to.width > 0) {
      // The whole panel moves, scaled about the stage's centre, so the picture
      // keeps its proportions all the way from the card and nothing clips it.
      const originX = to.left + to.width / 2 - box.left;
      const originY = to.top + to.height / 2 - box.top;
      const dx = from.left + from.width / 2 - (to.left + to.width / 2);
      const dy = from.top + from.height / 2 - (to.top + to.height / 2);
      const scale = from.width / to.width;
      panel.style.transformOrigin = `${originX}px ${originY}px`;
      panel.animate(
        [
          { transform: `translate(${dx}px, ${dy}px) scale(${scale})`, opacity: 0.4 },
          { transform: "none", opacity: 1 },
        ],
        { duration: 480, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
      );
    }
    nodes.detailInfo?.animate(
      [{ opacity: 0, transform: "translateY(18px)" }, { opacity: 1, transform: "none" }],
      { duration: 380, delay: 200, easing: "cubic-bezier(0.2, 0.8, 0.2, 1)", fill: "backwards" },
    );
    nodes.detail?.querySelector<HTMLElement>(".store-detail__scrim")?.animate(
      [{ opacity: 0 }, { opacity: 1 }],
      { duration: 320, easing: "ease-out" },
    );
  };

  const openDetail = (game: GameSummary, originKey: string | null): void => {
    const root = nodes.detail;
    if (!root || !nodes.body) return;
    detail = { game, slides: detailSlides(game), slide: 0, originKey };
    root.dataset.gameId = game.id;
    buildDetailStage(detail);
    renderDetailFacts();
    root.hidden = false;
    // Everything behind the dialog leaves the keyboard and the screen reader,
    // so Tab and the controller stay inside it.
    nodes.body.inert = true;
    pageRoot?.classList.add("store-page--detail");
    animateOpen(originKey);
    detailFocusTarget()?.focus({ preventScroll: true });
    startSlides();
  };

  /** Closes the detail; `returnFocus` puts focus back on the card it came from. */
  const closeDetail = (returnFocus = true): void => {
    const root = nodes.detail;
    if (!detail || !root) return;
    const originKey = detail.originKey;
    stopSlides();
    for (const key of [...installNodes.keys()]) if (key.startsWith("detail:")) installNodes.delete(key);
    detail = null;
    root.hidden = true;
    delete root.dataset.gameId;
    nodes.detailStage?.replaceChildren();
    nodes.detailInfo?.replaceChildren();
    if (nodes.body) nodes.body.inert = false;
    pageRoot?.classList.remove("store-page--detail");
    if (returnFocus && originKey) focusTargetFor(originKey)?.focus({ preventScroll: true });
  };

  /** Tab and Shift+Tab go round the dialog's own controls and never out of it. */
  function trapTab(event: KeyboardEvent): void {
    if (event.key !== "Tab" || !nodes.detail) return;
    const stops = [...nodes.detail.querySelectorAll<HTMLElement>("button, [href], [tabindex]:not([tabindex='-1'])")].filter(
      (node) => !node.hidden && !node.closest("[hidden]") && !(node as HTMLButtonElement).disabled,
    );
    if (stops.length === 0) return;
    const first = stops[0];
    const last = stops[stops.length - 1];
    const active = document.activeElement;
    if (event.shiftKey && (active === first || !nodes.detail.contains(active))) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && (active === last || !nodes.detail.contains(active))) {
      event.preventDefault();
      first.focus();
    }
  }

  // ---------------------------------------------------------------------------
  // Focus and lifecycle
  // ---------------------------------------------------------------------------

  const focusTargetFor = (focusKey: string): HTMLElement | null =>
    pageRoot?.querySelector<HTMLElement>(`[data-focus-key="${CSS.escape(focusKey)}"]`) ?? null;

  const captureFocus = (): FocusSnapshot | null => {
    const active = document.activeElement;
    if (!(active instanceof HTMLElement) || !pageRoot?.contains(active)) return null;
    const focusKey = active.dataset.focusKey;
    if (!focusKey) return null;
    const input = active instanceof HTMLInputElement ? active : null;
    return {
      focusKey,
      selectionStart: input?.selectionStart ?? null,
      selectionEnd: input?.selectionEnd ?? null,
    };
  };

  const restoreFocus = (snapshot: FocusSnapshot | null): void => {
    if (!snapshot) return;
    const target = focusTargetFor(snapshot.focusKey);
    if (!target || target === document.activeElement) return;
    target.focus({ preventScroll: true });
    if (!(target instanceof HTMLInputElement) || snapshot.selectionStart === null) return;
    try {
      target.setSelectionRange(snapshot.selectionStart, snapshot.selectionEnd ?? snapshot.selectionStart);
    } catch {
      // Some input types reject selection ranges; focus alone is enough there.
    }
  };

  const readScrollTop = (): number => Math.max(pageRoot?.scrollTop ?? 0, container?.scrollTop ?? 0);

  const writeScrollTop = (value: number): void => {
    if (pageRoot) pageRoot.scrollTop = value;
    if (container) container.scrollTop = value;
  };

  const restorePageState = (restoreState: PageRestoreState | null): void => {
    if (!pageRoot || !restoreState) return;
    writeScrollTop(Math.max(0, restoreState.scrollTop));
    if (!restoreState.focusKey) return;
    focusTargetFor(restoreState.focusKey)?.focus({ preventScroll: true });
  };

  const onKeyDown = (event: KeyboardEvent): void => {
    // The shell keeps every page mounted and hides the inactive ones, so a
    // window-level listener would otherwise fire while the Store is off-screen.
    if (!pageRoot?.isConnected || pageRoot.closest("[hidden]")) return;
    if (event.key !== "Escape" || !detail) return;
    // The detail used the key: a controller's B stops here instead of also
    // leaving the Store.
    event.preventDefault();
    closeDetail();
  };

  const onOnline = (): void => {
    dispatch({ type: "connectivity-changed", online: true });
    if (activation) void refreshSources(activation);
  };
  const onOffline = (): void => dispatch({ type: "connectivity-changed", online: false });

  /** A plugin tab added, changed or removed while the Store is open. */
  const onTabsChange = (): void => {
    render();
  };

  return {
    mount(host) {
      container = host;
      // Exactly one `main` per screen. The shell wrapper is a plain `div`, so
      // this page root is that landmark — never a nested one.
      pageRoot = element("main", "store-page");
      pageRoot.tabIndex = -1;
      pageRoot.setAttribute("aria-label", t("store.page"));
      const root = pageRoot;
      // The host scrolls, as every page host does; the band under the topbar
      // follows it.
      host.addEventListener(
        "scroll",
        () => root.classList.toggle("store-page--scrolled", host.scrollTop > 8),
        { passive: true },
      );
      container.replaceChildren(pageRoot);
      buildSkeleton();
      window.addEventListener("online", onOnline);
      window.addEventListener("offline", onOffline);
      window.addEventListener("keydown", onKeyDown);
      render();
    },
    activate(context) {
      activation = context;
      // `context.route` is a union; narrowing it into a local const keeps the
      // Store fields readable inside the async continuation below.
      const route = context.route;
      if (route.page !== "store") return;
      closeDetail(false);
      dispatch({
        type: "activate",
        store: route.store,
        query: route.query,
        online: typeof navigator === "undefined" || navigator.onLine,
      });
      tabsOff?.();
      tabsOff = onStoreTabsChange(onTabsChange);
      // A plugin installed since the last visit brings its store with it.
      void syncPluginStoreTabs();
      requestAnimationFrame(() => {
        if (isActive(context)) restorePageState(context.restoreState);
      });
      // An activation that replaces another one takes the installer with it:
      // the old controller's event subscription belongs to the old signal.
      teardownQuiky();
      const controller = createQuikyController(quikyClient);
      quiky = controller;
      quikyOff = controller.onChange(onQuikyChange);
      void controller.load(context.signal).then(() => {
        if (!isActive(context)) return;
        void searchQuiky(context, route.query);
      });
      void loadOwned(context);
      // A change of tab is a re-activation too. The shelf it filters is the
      // one already loaded, so the host is only asked again once it is stale.
      const fresh = homeLoadedAt > 0 && Date.now() - homeLoadedAt < 5 * 60_000;
      const home = fresh ? Promise.resolve() : loadHome(context);
      void home.then(() => {
        if (isActive(context) && route.query.trim()) void search(context, null, false);
      });
    },
    deactivate() {
      if (statusTimer) clearTimeout(statusTimer);
      statusTimer = null;
      transientStatus = "";
      closeDetail(false);
      tabsOff?.();
      tabsOff = null;
      quikySearchSequence += 1;
      quikySearchPhase = "idle";
      quikySearchMessage = "";
      quikySearchResults = [];
      renderedQuikySearch = "";
      teardownQuiky();
      const focusKey = captureFocus()?.focusKey ?? null;
      const restoreState: PageRestoreState = {
        scrollTop: readScrollTop(),
        focusKey,
        query: state.query,
        filters: [state.store],
      };
      activation = null;
      return restoreState;
    },
  };
}
