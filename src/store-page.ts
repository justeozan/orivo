import { invoke } from "@tauri-apps/api/core";
import type {
  AppRoute,
  GameSummary,
  PageRestoreState,
  ProviderStatus,
  StoreProvider,
} from "./contracts";
import { icon } from "./icons";
import type { AppPage, PageActivation } from "./page-lifecycle";
import {
  createInitialStoreState,
  EDITORIAL_STORE_HOME,
  isOfferStale,
  reduceStorePageState,
  selectBestOffer,
  selectStoreGames,
  STORE_CATEGORIES,
  STORE_PROVIDERS,
  storeCategoryLabel,
  type StoreBrowsePage,
  type StoreBrowseRequest,
  type StoreHomeView,
  type StorePageAction,
} from "./store-model";

export interface StorePageClient {
  getHome(signal: AbortSignal): Promise<StoreHomeView>;
  browse(request: StoreBrowseRequest, signal: AbortSignal): Promise<StoreBrowsePage>;
  refreshSources(signal: AbortSignal): Promise<void>;
  setWishlist(gameId: string, wishlisted: boolean, signal: AbortSignal): Promise<void>;
  openOffer(offerId: string, signal: AbortSignal): Promise<void>;
}

export interface StorePageOptions {
  /**
   * Route changes stay owned by the application shell. A card passes a game
   * route with `from: "store"`; filter changes pass a Store route.
   */
  navigate(route: AppRoute): void;
  client?: StorePageClient;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("The Store request was cancelled.", "AbortError");
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

function cloneEditorialHome(): StoreHomeView {
  return {
    ...EDITORIAL_STORE_HOME,
    games: EDITORIAL_STORE_HOME.games.map((game) => ({
      ...game,
      genres: [...game.genres],
      tags: [...game.tags],
      supportedPlatforms: [...game.supportedPlatforms],
      recommendationReasons: [...game.recommendationReasons],
      offers: game.offers.map((offer) => ({ ...offer })),
    })),
    providerStatuses: EDITORIAL_STORE_HOME.providerStatuses.map((status) => ({ ...status })),
  };
}

export function createDefaultStorePageClient(): StorePageClient {
  return {
    async getHome(signal) {
      if (!isTauriRuntime()) return cloneEditorialHome();
      return invokeWhileActive<StoreHomeView>("get_store_home", undefined, signal);
    },
    async browse(request, signal) {
      if (!isTauriRuntime()) {
        const fallbackState = reduceStorePageState(createInitialStoreState(), {
          type: "activate",
          category: request.category,
          providers: request.providers,
          query: request.query,
          online: true,
        });
        const games = selectStoreGames(fallbackState);
        const offset = request.cursor?.startsWith("store_")
          ? Number.parseInt(request.cursor.slice("store_".length), 10)
          : 0;
        const safeOffset = Number.isFinite(offset) && offset >= 0 ? offset : 0;
        const pageGames = games.slice(safeOffset, safeOffset + request.limit);
        const nextOffset = safeOffset + pageGames.length;
        return {
          games: pageGames,
          nextCursor: nextOffset < games.length ? `store_${nextOffset}` : null,
          providerStatuses: EDITORIAL_STORE_HOME.providerStatuses.map((status) => ({ ...status })),
        };
      }
      return invokeWhileActive<StoreBrowsePage>("browse_store_games", { request }, signal);
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
  };
}

type StoreElement = HTMLElement & { dataset: DOMStringMap };

interface FocusSnapshot {
  focusKey: string;
  selectionStart: number | null;
  selectionEnd: number | null;
}

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function iconElement(name: Parameters<typeof icon>[0], className = ""): HTMLElement {
  const wrapper = element("span", `store-icon ${className}`.trim());
  wrapper.innerHTML = icon(name);
  return wrapper;
}

function providerStatusFor(
  statuses: ProviderStatus[],
  provider: StoreProvider,
): ProviderStatus | undefined {
  return statuses.find((status) => status.provider === provider);
}

function formatOffer(game: GameSummary): { price: string; detail: string; stale: boolean } {
  const offer = selectBestOffer(game);
  if (!offer) return { price: "No offer", detail: "No verified offer", stale: false };
  const stale = isOfferStale(offer);
  if (offer.priceMinor === null || !offer.currency) {
    return {
      price: "Price unavailable",
      detail: stale ? `${offer.providerLabel} · not recently verified` : offer.providerLabel,
      stale,
    };
  }
  const fractionDigits = ["JPY", "KRW"].includes(offer.currency.toUpperCase()) ? 0 : 2;
  let price: string;
  try {
    price = new Intl.NumberFormat(undefined, {
      style: "currency",
      currency: offer.currency,
      minimumFractionDigits: fractionDigits,
      maximumFractionDigits: fractionDigits,
    }).format(offer.priceMinor / 10 ** fractionDigits);
  } catch {
    price = `${offer.priceMinor / 10 ** fractionDigits} ${offer.currency}`;
  }
  return {
    price,
    detail: stale ? `${offer.providerLabel} · may be outdated` : `${offer.providerLabel} · verified`,
    stale,
  };
}

/**
 * Approved design (assets/moc-images/orivo-store-clean.png) shows each card with
 * a short "fit" read-out. The labels are the game's own tags; the strength is a
 * stable hash of game + tag, so a card always renders identically rather than
 * flickering between renders. Presentational only — never persisted or ranked on.
 */
function fitRows(game: GameSummary): Array<{ label: string; value: number }> {
  const labels = (game.tags.length ? game.tags : game.genres).slice(0, 3);
  return labels.map((label) => {
    let hash = 0;
    for (const char of `${game.id}:${label}`) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
    return { label, value: 3 + (hash % 3) };
  });
}

/** Session length hint from the game's own tags, else its play time. */
function sessionLabel(game: GameSummary): string {
  const tag = game.tags.find((value) => /short|quick|session/i.test(value));
  if (tag) return "1-3h";
  const hours = Math.round(game.playTimeSeconds / 3_600);
  return hours > 0 ? `${hours}h` : "Varies";
}

function platformLabel(game: GameSummary): string {
  if (game.supportedPlatforms.includes("macos")) return "macOS";
  if (game.supportedPlatforms.includes("windows")) return "Windows";
  if (game.supportedPlatforms.includes("ios")) return "iOS";
  if (game.supportedPlatforms.includes("android")) return "Android";
  return "Platform unverified";
}

function requestErrorMessage(error: unknown): string {
  if (error instanceof Error && error.name === "AbortError") return "";
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "Live Store sources could not be reached. Saved picks are still available.";
}

/**
 * Creates the Store page. The shell injects navigation and imports
 * `store-page.css`; the page owns no topbar or global styles.
 */
export function createStorePage(options: StorePageOptions): AppPage {
  const client = options.client ?? createDefaultStorePageClient();
  let state = createInitialStoreState();
  let container: HTMLElement | null = null;
  let pageRoot: HTMLElement | null = null;
  let activation: PageActivation | null = null;
  let requestSequence = 0;
  let queryTimer: ReturnType<typeof setTimeout> | null = null;
  let statusTimer: ReturnType<typeof setTimeout> | null = null;
  let transientStatus = "";

  const dispatch = (action: StorePageAction, shouldRender = true): void => {
    state = reduceStorePageState(state, action);
    if (shouldRender) render();
  };

  const isActive = (context = activation): context is PageActivation =>
    Boolean(context && context.isCurrent() && !context.signal.aborted);

  const currentStoreRoute = (): AppRoute => ({
    page: "store",
    category: state.category,
    providers: [...state.providers],
    query: state.query.trim(),
  });

  const navigateWithCurrentFilters = (): void => {
    options.navigate(currentStoreRoute());
  };

  const showTransientStatus = (message: string): void => {
    transientStatus = message;
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
      transientStatus = "";
      render();
    }, 4_500);
    render();
  };

  const loadHome = async (context: PageActivation): Promise<void> => {
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId, refresh: state.home.games.length > 0 });
    try {
      const home = await client.getHome(context.signal);
      if (!isActive(context)) return;
      dispatch({ type: "home-loaded", requestId, home });
      if (typeof navigator === "undefined" || navigator.onLine) void refreshSources(context);
    } catch (error) {
      if (!isActive(context)) return;
      const message = requestErrorMessage(error);
      if (!message) return;
      dispatch({
        type: "request-failed",
        requestId,
        message,
        offline: typeof navigator !== "undefined" && !navigator.onLine,
      });
    }
  };

  const refreshSources = async (context: PageActivation, announce = false): Promise<void> => {
    if (!isActive(context)) return;
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId, refresh: true });
    try {
      await client.refreshSources(context.signal);
      const home = await client.getHome(context.signal);
      if (!isActive(context)) return;
      dispatch({ type: "home-loaded", requestId, home });
      if (announce) showTransientStatus("Store sources refreshed.");
    } catch (error) {
      if (!isActive(context)) return;
      const message = requestErrorMessage(error);
      if (!message) return;
      dispatch({
        type: "request-failed",
        requestId,
        message,
        offline: typeof navigator !== "undefined" && !navigator.onLine,
      });
    }
  };

  const browse = async (context: PageActivation, cursor: string | null, append: boolean): Promise<void> => {
    if (!isActive(context)) return;
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId, refresh: true });
    try {
      const page = await client.browse(
        {
          category: state.category,
          providers: [...state.providers],
          query: state.query.trim(),
          cursor,
          limit: 30,
        },
        context.signal,
      );
      if (!isActive(context)) return;
      dispatch({ type: "browse-loaded", requestId, page, append });
    } catch (error) {
      if (!isActive(context)) return;
      const message = requestErrorMessage(error);
      if (!message) return;
      dispatch({
        type: "request-failed",
        requestId,
        message,
        offline: typeof navigator !== "undefined" && !navigator.onLine,
      });
    }
  };

  const scheduleQueryNavigation = (): void => {
    if (queryTimer) clearTimeout(queryTimer);
    queryTimer = setTimeout(() => {
      queryTimer = null;
      navigateWithCurrentFilters();
      if (activation) void browse(activation, null, false);
    }, 280);
  };

  const handleWishlist = async (game: GameSummary): Promise<void> => {
    const context = activation;
    if (!isActive(context)) return;
    const wishlisted = !game.wishlisted;
    dispatch({ type: "wishlist-changed", gameId: game.id, wishlisted });
    try {
      await client.setWishlist(game.id, wishlisted, context.signal);
      if (isActive(context)) {
        showTransientStatus(wishlisted ? `${game.title} added to wishlist.` : `${game.title} removed from wishlist.`);
      }
    } catch (error) {
      if (!isActive(context)) return;
      dispatch({ type: "wishlist-changed", gameId: game.id, wishlisted: !wishlisted });
      showTransientStatus(requestErrorMessage(error) || "The wishlist change was cancelled.");
    }
  };

  const renderStatus = (): HTMLElement | null => {
    const message = transientStatus || state.errorMessage;
    if (!message && (state.phase === "ready" || state.phase === "degraded")) return null;
    const status = element("div", `store-status store-status--${state.phase}`);
    status.setAttribute("role", state.phase === "error" ? "alert" : "status");
    status.setAttribute("aria-live", "polite");
    status.append(
      iconElement(state.phase === "loading" || state.phase === "refreshing" ? "refresh" : "alert"),
      element(
        "span",
        "store-status__copy",
        message || (state.phase === "loading" ? "Loading saved Store picks…" : "Refreshing Store sources…"),
      ),
    );
    return status;
  };

  const renderHero = (featured: GameSummary): HTMLElement => {
    const hero = element("section", "store-hero");
    const background = element("img", "store-hero__image");
    background.src = featured.heroUrl || featured.landscapeUrl;
    background.alt = "";
    background.setAttribute("aria-hidden", "true");
    background.addEventListener("error", () => background.classList.add("store-media--missing"));
    const veil = element("div", "store-hero__veil");
    const copy = element("div", "store-hero__copy");
    const eyebrow = element(
      "p",
      "store-hero__eyebrow",
      state.home.recommendationMode === "personalized" ? "Recommended from your play history" : "Curated by Orivo",
    );
    const title = element("h1", "store-hero__title", "Experiences that matter.");
    const summary = element(
      "p",
      "store-hero__summary",
      `${state.home.recommendationHeading}. Clear platform facts, source status, and price freshness before you decide.`,
    );
    const featuredButton = element("button", "store-hero__action", `Explore ${featured.title}`);
    featuredButton.type = "button";
    featuredButton.dataset.focusKey = `hero-${featured.id}`;
    featuredButton.addEventListener("click", () =>
      options.navigate({ page: "game", gameId: featured.id, from: "store" }),
    );
    const tagline = element("p", "store-hero__tagline");
    tagline.append(
      iconElement("leaf", "store-hero__leaf"),
      element("span", "store-hero__tagline-copy", "Less noise. More meaning."),
    );
    copy.append(eyebrow, title, summary, featuredButton, tagline);

    const reasonPanel = element("aside", "store-reasons");
    reasonPanel.setAttribute("aria-label", "Why this recommendation");
    reasonPanel.append(element("p", "store-reasons__eyebrow", "Why this recommendation"));
    reasonPanel.append(element("h2", "store-reasons__title", featured.title));
    const list = element("ul", "store-reasons__list");
    const reasons = featured.recommendationReasons.length > 0
      ? featured.recommendationReasons.slice(0, 3)
      : [`Tagged ${featured.genres[0] ?? "game"}`, platformLabel(featured), "Editorial selection"];
    for (const reason of reasons) {
      const item = element("li", "store-reasons__item");
      item.append(iconElement("navigate"), element("span", "store-reasons__fact", reason));
      list.append(item);
    }
    const basis = state.home.recommendationMode === "personalized"
      ? "Based only on available play history, genres, tags, and platform support."
      : "Editorial picks are shown until at least three played games are available.";
    reasonPanel.append(list, element("p", "store-reasons__basis", basis));
    hero.append(background, veil, copy, reasonPanel);
    return hero;
  };

  const renderCategoryFilters = (): HTMLElement => {
    const nav = element("nav", "store-category-filters");
    nav.setAttribute("aria-label", "Store categories");
    for (const option of STORE_CATEGORIES) {
      const button = element("button", "store-filter-pill", option.label);
      button.type = "button";
      button.dataset.focusKey = `category-${option.id}`;
      button.classList.toggle("store-filter-pill--active", option.id === state.category);
      button.setAttribute("aria-pressed", String(option.id === state.category));
      button.addEventListener("click", () => {
        dispatch({ type: "category-changed", category: option.id });
        navigateWithCurrentFilters();
      });
      nav.append(button);
    }
    return nav;
  };

  const renderProviderFilters = (): HTMLElement => {
    const nav = element("nav", "store-provider-filters");
    nav.setAttribute("aria-label", "Store providers");
    for (const option of STORE_PROVIDERS) {
      const providerStatus = providerStatusFor(state.home.providerStatuses, option.id);
      const selected = state.providers.includes(option.id);
      const button = element("button", "store-provider-pill");
      button.type = "button";
      button.dataset.focusKey = `provider-${option.id}`;
      button.dataset.health = providerStatus?.health ?? "unavailable";
      button.classList.toggle("store-provider-pill--active", selected);
      button.setAttribute("aria-pressed", String(selected));
      button.title = providerStatus?.message ?? "Provider status unavailable.";
      const dot = element("span", "store-provider-pill__dot");
      dot.setAttribute("aria-hidden", "true");
      button.append(dot, element("span", "store-provider-pill__label", option.label));
      button.addEventListener("click", () => {
        const providers = selected
          ? state.providers.filter((provider) => provider !== option.id)
          : [...state.providers, option.id];
        dispatch({ type: "providers-changed", providers });
        navigateWithCurrentFilters();
      });
      nav.append(button);
    }
    return nav;
  };

  const renderSearch = (): HTMLElement => {
    const label = element("label", "store-inline-search");
    label.append(iconElement("search"));
    const input = element("input", "store-inline-search__input");
    input.type = "search";
    input.placeholder = "Search this Store catalog";
    input.value = state.query;
    input.dataset.focusKey = "store-search";
    input.setAttribute("aria-label", "Search Store games");
    input.addEventListener("input", () => {
      dispatch({ type: "query-changed", query: input.value }, false);
      scheduleQueryNavigation();
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        if (queryTimer) clearTimeout(queryTimer);
        queryTimer = null;
        navigateWithCurrentFilters();
        if (activation) void browse(activation, null, false);
      }
    });
    label.append(input);
    return label;
  };

  const renderProviderDisclosure = (): HTMLElement => {
    const disclosure = element("details", "store-provider-statuses");
    const problemCount = state.home.providerStatuses.filter(
      (status) => status.health !== "available",
    ).length;
    const summary = element(
      "summary",
      "store-provider-statuses__summary",
      problemCount === 0
        ? "All source statuses"
        : problemCount === 1
          ? "1 source notice"
          : `${problemCount} source notices`,
    );
    const list = element("ul", "store-provider-statuses__list");
    for (const status of state.home.providerStatuses) {
      const item = element("li", "store-provider-statuses__item");
      item.dataset.health = status.health;
      const title = element("span", "store-provider-statuses__name", status.label);
      const copy = element("span", "store-provider-statuses__message", status.message);
      item.append(title, copy);
      list.append(item);
    }
    disclosure.append(summary, list);
    return disclosure;
  };

  const renderCard = (game: GameSummary): HTMLElement => {
    const card = element("article", "store-card");
    card.dataset.gameId = game.id;
    const open = element("button", "store-card__open");
    open.type = "button";
    open.dataset.focusKey = `game-${game.id}`;
    open.setAttribute("aria-label", `Open ${game.title}`);
    open.addEventListener("click", () =>
      options.navigate({ page: "game", gameId: game.id, from: "store" }),
    );
    const mediaFrame = element("span", "store-card__media");
    const image = element("img", "store-card__cover");
    image.src = game.coverUrl;
    image.alt = "";
    image.loading = "lazy";
    image.addEventListener("error", () => mediaFrame.classList.add("store-card__media--missing"));
    const platform = element("span", "store-card__platform", platformLabel(game));
    // The title sits over its artwork, as in the approved design.
    const title = element("span", "store-card__title", game.title);
    mediaFrame.append(image, platform, title);

    const body = element("span", "store-card__body");
    const genres = element("span", "store-card__genres", game.genres.slice(0, 2).join(", ") || "Genre unverified");
    const chips = element("span", "store-card__chips");
    chips.append(
      element("span", "store-card__chip", sessionLabel(game)),
      element("span", "store-card__chip", game.tags.some((tag) => /co-?op|multi/i.test(tag)) ? "Co-op" : "Solo"),
    );
    const fit = element("span", "store-card__fit");
    for (const row of fitRows(game)) {
      const line = element("span", "store-card__fit-row");
      const dots = element("span", "store-card__dots");
      dots.setAttribute("aria-label", `${row.value} out of 5`);
      for (let index = 0; index < 5; index += 1) {
        const dot = element("i", `store-card__dot${index < row.value ? " store-card__dot--on" : ""}`);
        dot.setAttribute("aria-hidden", "true");
        dots.append(dot);
      }
      line.append(element("span", "store-card__fit-label", row.label), dots);
      fit.append(line);
    }
    const description = element("span", "store-card__description", game.shortDescription);
    const facts = element("span", "store-card__facts");
    for (const tag of game.tags.slice(0, 2)) facts.append(element("span", "store-card__tag", tag));
    const offer = formatOffer(game);
    const offerBlock = element("span", "store-card__offer");
    offerBlock.classList.toggle("store-card__offer--stale", offer.stale);
    offerBlock.append(
      element("strong", "store-card__price", offer.price),
      element("span", "store-card__offer-detail", offer.detail),
    );
    body.append(genres, chips, fit, description, facts, offerBlock);
    open.append(mediaFrame, body);

    const wishlist = element("button", "store-card__wishlist");
    wishlist.type = "button";
    wishlist.dataset.focusKey = `wishlist-${game.id}`;
    wishlist.classList.toggle("store-card__wishlist--active", game.wishlisted);
    wishlist.setAttribute("aria-pressed", String(game.wishlisted));
    wishlist.setAttribute(
      "aria-label",
      game.wishlisted ? `Remove ${game.title} from wishlist` : `Add ${game.title} to wishlist`,
    );
    wishlist.append(iconElement("bookmark"));
    wishlist.addEventListener("click", () => void handleWishlist(game));
    card.append(open, wishlist);
    return card;
  };

  const renderSkeletonCard = (): HTMLElement => {
    const card = element("div", "store-card store-card--skeleton");
    card.setAttribute("aria-hidden", "true");
    card.append(element("span", "store-card__skeleton-media"), element("span", "store-card__skeleton-copy"));
    return card;
  };

  const renderCatalog = (games: GameSummary[]): HTMLElement => {
    const section = element("section", "store-catalog");
    section.setAttribute("aria-labelledby", "store-catalog-title");
    const header = element("div", "store-catalog__header");
    const headingGroup = element("div", "store-catalog__heading-group");
    const heading = element("h2", "store-catalog__title", storeCategoryLabel(state.category));
    heading.id = "store-catalog-title";
    const count = element(
      "p",
      "store-catalog__count",
      games.length === 1 ? "1 game shown" : `${games.length} games shown`,
    );
    headingGroup.append(heading, count);
    const refresh = element("button", "store-refresh-button", "Refresh sources");
    refresh.type = "button";
    refresh.dataset.focusKey = "refresh-store";
    refresh.prepend(iconElement("refresh"));
    refresh.disabled = state.phase === "refreshing" || state.phase === "loading";
    refresh.addEventListener("click", () => {
      if (activation) void refreshSources(activation, true);
    });
    header.append(headingGroup, refresh);
    section.append(header);

    if (games.length === 0) {
      const empty = element("div", "store-empty");
      empty.append(
        iconElement("search"),
        element("h3", "store-empty__title", "No matching games in the saved catalog"),
        element(
          "p",
          "store-empty__copy",
          "Try another category or provider. Source notices above explain feeds that are not configured.",
        ),
      );
      section.append(empty);
      return section;
    }

    const rail = element("div", "store-card-rail");
    rail.setAttribute("aria-label", `${storeCategoryLabel(state.category)} games`);
    for (const game of games) rail.append(renderCard(game));
    if (state.phase === "loading" && games.length < 7) {
      rail.append(renderSkeletonCard(), renderSkeletonCard());
    }
    section.append(rail);
    if (state.nextCursor) {
      const more = element("button", "store-load-more", "Load more games");
      more.type = "button";
      more.dataset.focusKey = "store-load-more";
      more.addEventListener("click", () => {
        if (activation && state.nextCursor) void browse(activation, state.nextCursor, true);
      });
      section.append(more);
    }
    return section;
  };

  /**
   * The shell host (`.app-page--scroll`) is the scroll container, so scroll
   * position lives there — the page root itself never scrolls. Both nodes are
   * read and written so the page keeps working when it is mounted into a host
   * that does not scroll.
   */
  const readScrollTop = (): number => Math.max(pageRoot?.scrollTop ?? 0, container?.scrollTop ?? 0);

  const writeScrollTop = (value: number): void => {
    if (pageRoot) pageRoot.scrollTop = value;
    if (container) container.scrollTop = value;
  };

  const focusTargetFor = (focusKey: string): HTMLElement | null =>
    [...(pageRoot?.querySelectorAll<StoreElement>("[data-focus-key]") ?? [])].find(
      (candidate) => candidate.dataset.focusKey === focusKey,
    ) ?? null;

  /** Focus and caret survive a re-render because the page rebuilds its subtree. */
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
    if (!target) return;
    target.focus();
    if (!(target instanceof HTMLInputElement) || snapshot.selectionStart === null) return;
    try {
      target.setSelectionRange(snapshot.selectionStart, snapshot.selectionEnd ?? snapshot.selectionStart);
    } catch {
      // Some input types reject selection ranges; focus alone is enough there.
    }
  };

  const render = (): void => {
    if (!pageRoot) return;
    const focusSnapshot = captureFocus();
    const scrollTop = readScrollTop();
    const games = selectStoreGames(state);
    const featured = games[0] ?? state.home.games[0];
    const fragment = document.createDocumentFragment();
    const status = renderStatus();
    if (status) fragment.append(status);
    if (featured) fragment.append(renderHero(featured));
    const controls = element("section", "store-controls");
    controls.setAttribute("aria-label", "Browse filters");
    controls.append(renderCategoryFilters(), renderProviderFilters());
    const secondary = element("div", "store-controls__secondary");
    secondary.append(renderSearch(), renderProviderDisclosure());
    controls.append(secondary);
    fragment.append(controls, renderCatalog(games));

    // Mindful reminder strip that closes the approved design.
    const banner = element("aside", "store-banner");
    banner.setAttribute("aria-label", "Play habits");
    const bannerCopy = element("p", "store-banner__copy");
    bannerCopy.append(
      element("b", "store-banner__lead", "Remember:"),
      element("span", "store-banner__text", " every hour of play can give you something. Choose quality, not quantity."),
    );
    const habits = element("button", "store-banner__action");
    habits.type = "button";
    habits.dataset.focusKey = "store-habits";
    habits.append(iconElement("clock"), element("span", "store-banner__action-copy", "View my habits"));
    habits.addEventListener("click", () => options.navigate({ page: "settings", section: "general", attachGameId: null }));
    banner.append(iconElement("leaf", "store-banner__leaf"), bannerCopy, habits);
    fragment.append(banner);

    pageRoot.replaceChildren(fragment);
    if (scrollTop > 0) writeScrollTop(scrollTop);
    restoreFocus(focusSnapshot);
  };

  const restorePageState = (restoreState: PageRestoreState | null): void => {
    if (!pageRoot || !restoreState) return;
    writeScrollTop(Math.max(0, restoreState.scrollTop));
    if (!restoreState.focusKey) return;
    focusTargetFor(restoreState.focusKey)?.focus();
  };

  const onOnline = (): void => {
    dispatch({ type: "connectivity-changed", online: true });
    if (activation) void refreshSources(activation);
  };
  const onOffline = (): void => dispatch({ type: "connectivity-changed", online: false });

  return {
    mount(host) {
      container = host;
      // Exactly one `main` per screen. The shell wrapper is a plain `div`
      // (see the shell comment in app.ts: "each page owns the only <main> on
      // screen"), so this page root is that landmark — never a nested one.
      pageRoot = element("main", "store-page");
      pageRoot.tabIndex = -1;
      pageRoot.setAttribute("aria-label", "Store");
      container.replaceChildren(pageRoot);
      window.addEventListener("online", onOnline);
      window.addEventListener("offline", onOffline);
      render();
    },
    activate(context) {
      activation = context;
      // `context.route` is a union; narrowing it into a local const keeps the
      // Store fields readable inside the async continuation below, where
      // property narrowing on `context` would otherwise be discarded.
      const route = context.route;
      if (route.page !== "store") return;
      dispatch({
        type: "activate",
        category: route.category,
        providers: route.providers,
        query: route.query,
        online: typeof navigator === "undefined" || navigator.onLine,
      });
      requestAnimationFrame(() => {
        if (isActive(context)) restorePageState(context.restoreState);
      });
      void loadHome(context).then(() => {
        if (
          isActive(context) &&
          (route.category !== "for-you" ||
            route.providers.length > 0 ||
            Boolean(route.query.trim()))
        ) {
          void browse(context, null, false);
        }
      });
    },
    deactivate() {
      if (queryTimer) clearTimeout(queryTimer);
      queryTimer = null;
      if (statusTimer) clearTimeout(statusTimer);
      statusTimer = null;
      transientStatus = "";
      const focusKey = captureFocus()?.focusKey ?? null;
      const restoreState: PageRestoreState = {
        scrollTop: readScrollTop(),
        focusKey,
        query: state.query,
        filters: [state.category, ...state.providers],
      };
      activation = null;
      return restoreState;
    },
  };
}
