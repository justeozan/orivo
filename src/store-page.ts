import { invoke } from "@tauri-apps/api/core";
import type {
  AppRoute,
  GameSummary,
  PageRestoreState,
  StoreHighlight,
  StorePlatform,
} from "./contracts";
import { icon, type IconName } from "./icons";
import type { AppPage, PageActivation } from "./page-lifecycle";
import {
  createInitialStoreState,
  EDITORIAL_STORE_HOME,
  fitStats,
  formatPrice,
  genreLabel,
  isOfferStale,
  modeLabel,
  offerVerifiedOn,
  matchesStoreFilters,
  ownershipKey,
  providersForPlatforms,
  reduceStorePageState,
  selectBestOffer,
  selectHeroCopy,
  selectStoreGames,
  sessionLabel,
  STORE_CATEGORIES,
  STORE_PLATFORMS,
  storeCategoryLabel,
  taglineLabel,
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
  /** Ids already in the library. The Store never sells a game you own. */
  listOwnedGameIds(signal: AbortSignal): Promise<string[]>;
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
  return {
    ...EDITORIAL_STORE_HOME,
    games: EDITORIAL_STORE_HOME.games.map(cloneGame),
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
        // Outside the desktop shell the bundled catalog is the catalog, so the
        // browse endpoint pages through it rather than pretending to fail. It
        // has to apply the filters itself: a browse page is read as already
        // filtered, and here there is no host to have done it.
        const catalog = cloneEditorialHome().games.filter((game) =>
          matchesStoreFilters(game, request),
        );
        const offset = request.cursor?.startsWith("store_")
          ? Number.parseInt(request.cursor.slice("store_".length), 10)
          : 0;
        const safeOffset = Number.isFinite(offset) && offset >= 0 ? offset : 0;
        const games = catalog.slice(safeOffset, safeOffset + request.limit);
        const nextOffset = safeOffset + games.length;
        return {
          games,
          nextCursor: nextOffset < catalog.length ? `store_${nextOffset}` : null,
          providerStatuses: EDITORIAL_STORE_HOME.providerStatuses.map((status) => ({ ...status })),
        };
      }
      return invokeWhileActive<StoreBrowsePage>(
        "browse_store_games",
        {
          request: {
            ...request,
            // The host still filters on shops; the page filters on machines.
            providers: providersForPlatforms(request.platforms),
          },
        },
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

const HIGHLIGHT_ICONS: Readonly<Record<string, IconName>> = {
  brain: "brain",
  clock: "clock",
  book: "book",
  leaf: "leaf",
  compass: "compass",
  sparkle: "sparkle",
  heart: "heart",
  puzzle: "puzzle",
  moon: "moon",
  palette: "palette",
};

function highlightIcon(name: string): IconName {
  return HIGHLIGHT_ICONS[name] ?? "sparkle";
}

const PLATFORM_ICONS: Readonly<Record<StorePlatform, IconName>> = {
  pc: "windows",
  playstation: "playstation",
  xbox: "xbox",
  switch: "switch",
  emulators: "emulator",
};

function requestErrorMessage(error: unknown): string {
  if (error instanceof Error && error.name === "AbortError") return "";
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "Les sources du Store sont injoignables. Les sélections enregistrées restent disponibles.";
}

/**
 * The cheapest verified offer, shown while a card is previewed. A price older
 * than a day is still shown — with the day it was read, so the shopper knows
 * how fresh it is instead of being told a number Orivo cannot vouch for today.
 */
function offerLine(game: GameSummary): string {
  const offer = selectBestOffer(game);
  if (!offer) return "";
  const price = formatPrice(offer);
  if (!price) return `Disponible sur ${offer.providerLabel} · tarif non vérifié`;
  if (offer.priceMinor === 0) return `Gratuit sur ${offer.providerLabel}`;
  const line = `Dès ${price} sur ${offer.providerLabel}`;
  if (!isOfferStale(offer)) return `${line} · vérifié aujourd'hui`;
  const day = offerVerifiedOn(offer);
  return day ? `${line} · relevé le ${day}` : `${line} · tarif à revérifier`;
}

/**
 * Creates the Store page ("Découvrir"). The shell injects navigation and
 * imports `store-page.css`; the page owns no topbar and no global styles.
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
  let previewTimer: ReturnType<typeof setTimeout> | null = null;
  let transientStatus = "";
  let morePlatformsOpen = false;
  let whyOpen = false;
  /** Which of the two stacked backdrop layers is currently showing. */
  let backdropFront = 0;
  let backdropUrl = "";
  let renderedGameIds = "";

  // Long-lived nodes. The page paints once and then updates in place: a full
  // rebuild would drop the rail's scroll position and restart the backdrop
  // crossfade on every hover.
  const nodes = {
    backdropLayers: [] as HTMLImageElement[],
    heroEyebrow: null as HTMLElement | null,
    heroTitle: null as HTMLElement | null,
    heroLead: null as HTMLElement | null,
    heroAction: null as HTMLButtonElement | null,
    heroOffer: null as HTMLElement | null,
    highlightList: null as HTMLElement | null,
    shelfLabel: null as HTMLElement | null,
    railTrack: null as HTMLElement | null,
    arrowPrevious: null as HTMLButtonElement | null,
    arrowNext: null as HTMLButtonElement | null,
    categoryBar: null as HTMLElement | null,
    platformBar: null as HTMLElement | null,
    morePanel: null as HTMLElement | null,
    status: null as HTMLElement | null,
    whyPanel: null as HTMLElement | null,
    emptyState: null as HTMLElement | null,
  };

  const dispatch = (action: StorePageAction, shouldRender = true): void => {
    state = reduceStorePageState(state, action);
    if (shouldRender) render();
  };

  const isActive = (context = activation): context is PageActivation =>
    Boolean(context && context.isCurrent() && !context.signal.aborted);

  const currentStoreRoute = (): AppRoute => ({
    page: "store",
    category: state.category,
    platforms: [...state.platforms],
    query: state.query.trim(),
  });

  const navigateWithCurrentFilters = (): void => options.navigate(currentStoreRoute());

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
      if (announce) showTransientStatus("Sources du Store actualisées.");
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

  const browse = async (
    context: PageActivation,
    cursor: string | null,
    append: boolean,
  ): Promise<void> => {
    if (!isActive(context)) return;
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId, refresh: true });
    try {
      const page = await client.browse(
        {
          category: state.category,
          platforms: [...state.platforms],
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

  const handleWishlist = async (game: GameSummary): Promise<void> => {
    const context = activation;
    if (!isActive(context)) return;
    const wishlisted = !game.wishlisted;
    dispatch({ type: "wishlist-changed", gameId: game.id, wishlisted });
    try {
      await client.setWishlist(game.id, wishlisted, context.signal);
      if (isActive(context)) {
        showTransientStatus(
          wishlisted ? `${game.title} ajouté à tes envies.` : `${game.title} retiré de tes envies.`,
        );
      }
    } catch (error) {
      if (!isActive(context)) return;
      dispatch({ type: "wishlist-changed", gameId: game.id, wishlisted: !wishlisted });
      showTransientStatus(requestErrorMessage(error) || "Le changement d'envie a été annulé.");
    }
  };

  // ---------------------------------------------------------------------------
  // Skeleton
  // ---------------------------------------------------------------------------

  const buildBackdrop = (): HTMLElement => {
    const backdrop = element("div", "store-backdrop");
    backdrop.setAttribute("aria-hidden", "true");
    for (let index = 0; index < 2; index += 1) {
      const layer = element("img", "store-backdrop__layer");
      layer.alt = "";
      layer.decoding = "async";
      backdrop.append(layer);
      nodes.backdropLayers.push(layer);
    }
    backdrop.append(
      element("div", "store-backdrop__scrim store-backdrop__scrim--left"),
      element("div", "store-backdrop__scrim store-backdrop__scrim--bottom"),
      element("div", "store-backdrop__scrim store-backdrop__scrim--top"),
    );
    return backdrop;
  };

  const buildHero = (): HTMLElement => {
    const hero = element("section", "store-hero");
    const copy = element("div", "store-hero__copy");
    nodes.heroEyebrow = element("p", "store-hero__eyebrow");
    nodes.heroTitle = element("h1", "store-hero__title");
    nodes.heroLead = element("p", "store-hero__lead");
    nodes.heroAction = element("button", "store-hero__action");
    nodes.heroAction.type = "button";
    nodes.heroAction.dataset.focusKey = "hero-action";
    nodes.heroAction.addEventListener("click", () => {
      const heroCopy = selectHeroCopy(state, visibleGames());
      if (heroCopy.gameId) {
        options.navigate({ page: "game", gameId: heroCopy.gameId, from: "store" });
        return;
      }
      whyOpen = !whyOpen;
      renderWhyPanel();
    });
    nodes.heroOffer = element("p", "store-hero__offer");
    copy.append(nodes.heroEyebrow, nodes.heroTitle, nodes.heroLead, nodes.heroAction, nodes.heroOffer);

    const highlights = element("aside", "store-highlights");
    highlights.setAttribute("aria-label", "Pourquoi cette sélection");
    nodes.highlightList = element("ul", "store-highlights__list");
    highlights.append(nodes.highlightList);

    nodes.whyPanel = element("div", "store-why");
    nodes.whyPanel.hidden = true;

    hero.append(copy, highlights, nodes.whyPanel);
    return hero;
  };

  const buildShelf = (): HTMLElement => {
    const shelf = element("section", "store-shelf");
    nodes.shelfLabel = element("p", "store-shelf__label");
    const rail = element("div", "store-rail");
    nodes.railTrack = element("div", "store-rail__track");
    nodes.railTrack.setAttribute("role", "list");
    nodes.railTrack.addEventListener("scroll", () => syncArrows(), { passive: true });

    nodes.arrowPrevious = element("button", "store-rail__arrow store-rail__arrow--previous");
    nodes.arrowPrevious.type = "button";
    nodes.arrowPrevious.dataset.focusKey = "rail-previous";
    nodes.arrowPrevious.setAttribute("aria-label", "Voir les jeux précédents");
    nodes.arrowPrevious.append(iconElement("arrow-left"));
    nodes.arrowPrevious.addEventListener("click", () => scrollRail(-1));

    nodes.arrowNext = element("button", "store-rail__arrow store-rail__arrow--next");
    nodes.arrowNext.type = "button";
    nodes.arrowNext.dataset.focusKey = "rail-next";
    nodes.arrowNext.setAttribute("aria-label", "Voir plus de jeux");
    nodes.arrowNext.append(iconElement("arrow-right"));
    nodes.arrowNext.addEventListener("click", () => scrollRail(1));

    nodes.emptyState = element("div", "store-empty");
    nodes.emptyState.hidden = true;

    rail.append(nodes.railTrack, nodes.arrowPrevious, nodes.arrowNext);
    shelf.append(nodes.shelfLabel, rail, nodes.emptyState);
    return shelf;
  };

  const buildFilters = (): HTMLElement => {
    const filters = element("section", "store-filters");
    filters.setAttribute("aria-label", "Filtres du Store");

    nodes.categoryBar = element("nav", "store-chipbar store-chipbar--categories");
    nodes.categoryBar.setAttribute("aria-label", "Catégories");
    for (const option of STORE_CATEGORIES) {
      const chip = element("button", "store-chip", option.label);
      chip.type = "button";
      chip.dataset.focusKey = `category-${option.id}`;
      chip.dataset.category = option.id;
      chip.addEventListener("click", () => {
        dispatch({ type: "category-changed", category: option.id });
        navigateWithCurrentFilters();
        if (activation) void browse(activation, null, false);
      });
      nodes.categoryBar.append(chip);
    }

    const platformGroup = element("div", "store-platform-group");
    nodes.platformBar = element("nav", "store-chipbar store-chipbar--platforms");
    nodes.platformBar.setAttribute("aria-label", "Plateformes");
    for (const option of STORE_PLATFORMS) {
      const chip = element("button", "store-chip store-chip--platform");
      chip.type = "button";
      chip.dataset.focusKey = `platform-${option.id}`;
      chip.dataset.platform = option.id;
      chip.append(
        iconElement(PLATFORM_ICONS[option.id], "store-chip__icon"),
        element("span", "store-chip__label", option.label),
      );
      chip.addEventListener("click", () => {
        const selected = state.platforms.includes(option.id);
        const platforms = selected
          ? state.platforms.filter((platform) => platform !== option.id)
          : [...state.platforms, option.id];
        dispatch({ type: "platforms-changed", platforms });
        navigateWithCurrentFilters();
        if (activation) void browse(activation, null, false);
      });
      nodes.platformBar.append(chip);
    }

    const more = element("button", "store-chip store-chip--more");
    more.type = "button";
    more.dataset.focusKey = "platform-more";
    more.setAttribute("aria-haspopup", "true");
    more.append(element("span", "store-chip__label", "Plus"), iconElement("chevron-down", "store-chip__caret"));
    more.addEventListener("click", () => {
      morePlatformsOpen = !morePlatformsOpen;
      renderMorePanel();
    });
    nodes.platformBar.append(more);

    nodes.morePanel = element("div", "store-more-panel");
    nodes.morePanel.hidden = true;
    platformGroup.append(nodes.platformBar, nodes.morePanel);

    filters.append(nodes.categoryBar, platformGroup);
    return filters;
  };

  const buildBanner = (): HTMLElement => {
    const banner = element("aside", "store-banner");
    banner.setAttribute("aria-label", "Habitudes de jeu");
    const copy = element("p", "store-banner__copy");
    copy.append(
      element("b", "store-banner__lead", "Rappelle-toi :"),
      element(
        "span",
        "store-banner__text",
        " chaque heure de jeu peut t'apporter quelque chose.\nChoisis la qualité, pas la quantité.",
      ),
    );
    const habits = element("button", "store-banner__action");
    habits.type = "button";
    habits.dataset.focusKey = "store-habits";
    habits.append(iconElement("chart"), element("span", "", "Voir mes habitudes"));
    habits.addEventListener("click", () =>
      options.navigate({ page: "settings", section: "general", attachGameId: null }),
    );
    banner.append(iconElement("leaf", "store-banner__leaf"), copy, habits);
    return banner;
  };

  const buildSkeleton = (): void => {
    if (!pageRoot) return;
    const body = element("div", "store-page__body");
    nodes.status = element("div", "store-status");
    nodes.status.setAttribute("aria-live", "polite");
    nodes.status.hidden = true;
    body.append(buildHero(), buildShelf(), buildFilters(), buildBanner());
    pageRoot.replaceChildren(buildBackdrop(), body, nodes.status);
  };

  // ---------------------------------------------------------------------------
  // Rendering
  // ---------------------------------------------------------------------------

  const visibleGames = (): GameSummary[] => selectStoreGames(state);

  const setBackdrop = (url: string): void => {
    if (!url || url === backdropUrl || nodes.backdropLayers.length < 2) return;
    backdropUrl = url;
    const next = nodes.backdropLayers[1 - backdropFront];
    const current = nodes.backdropLayers[backdropFront];
    next.src = url;
    const reveal = (): void => {
      next.classList.add("is-active");
      current.classList.remove("is-active");
      backdropFront = 1 - backdropFront;
    };
    if (next.complete && next.naturalWidth > 0) reveal();
    else next.addEventListener("load", reveal, { once: true });
  };

  const renderHighlights = (highlights: StoreHighlight[]): void => {
    if (!nodes.highlightList) return;
    const list = document.createDocumentFragment();
    for (const highlight of highlights.slice(0, 3)) {
      const item = element("li", "store-highlight");
      const copy = element("div", "store-highlight__copy");
      copy.append(
        element("h2", "store-highlight__title", highlight.title),
        element("p", "store-highlight__text", highlight.text),
      );
      item.append(iconElement(highlightIcon(highlight.icon), "store-highlight__icon"), copy);
      list.append(item);
    }
    nodes.highlightList.replaceChildren(list);
  };

  const renderHero = (games: GameSummary[]): void => {
    const heroCopy = selectHeroCopy(state, games);
    if (nodes.heroEyebrow) nodes.heroEyebrow.textContent = heroCopy.eyebrow;
    if (nodes.heroTitle) nodes.heroTitle.textContent = heroCopy.title;
    if (nodes.heroLead) nodes.heroLead.textContent = heroCopy.lead;
    if (nodes.heroAction) nodes.heroAction.textContent = heroCopy.actionLabel;
    if (nodes.heroOffer) {
      const previewed = heroCopy.gameId
        ? games.find((game) => game.id === heroCopy.gameId) ?? null
        : null;
      const line = previewed ? offerLine(previewed) : "";
      nodes.heroOffer.textContent = line;
      nodes.heroOffer.hidden = !line;
    }
    renderHighlights(heroCopy.highlights);
    setBackdrop(heroCopy.backgroundUrl);
  };

  const renderShelfLabel = (): void => {
    if (!nodes.shelfLabel) return;
    nodes.shelfLabel.replaceChildren(
      iconElement("leaf", "store-shelf__leaf"),
      element("strong", "store-shelf__lead", "Moins de bruit."),
      element("span", "store-shelf__tail", "Plus de sens."),
    );
  };

  const previewGame = (gameId: string | null): void => {
    if (state.previewGameId === gameId) return;
    if (previewTimer) clearTimeout(previewTimer);
    previewTimer = setTimeout(() => {
      previewTimer = null;
      state = reduceStorePageState(state, { type: "preview-changed", gameId });
      renderHero(visibleGames());
      syncFeatured();
    }, gameId ? 60 : 220);
  };

  const buildCard = (game: GameSummary, index: number): HTMLElement => {
    const card = element("article", "store-card");
    card.dataset.gameId = game.id;
    card.setAttribute("role", "listitem");

    const open = element("button", "store-card__open");
    open.type = "button";
    open.dataset.focusKey = `game-${game.id}`;
    open.dataset.navOpen = game.id;
    open.setAttribute("aria-label", `Ouvrir ${game.title}`);
    open.addEventListener("click", () =>
      options.navigate({ page: "game", gameId: game.id, from: "store" }),
    );
    open.addEventListener("focus", () => previewGame(game.id));

    const media = element("span", "store-card__media");
    const art = element("img", "store-card__art");
    // The card wants key art composed for a small landscape frame, which is the
    // capsule; the wide `heroUrl` banner is the page backdrop's job and crops to
    // an unreadable sliver at this size. Each fallback is tried once.
    const artSources = [game.landscapeUrl, game.coverUrl, game.heroUrl].filter(Boolean);
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
    // A few games have no screenshot that survives the crop and fall back to
    // their capsule, which already carries the wordmark; printing the title
    // over it would show the name twice.
    const artHasWordmark = /\/capsule\.jpg$/.test(art.src || "");
    media.append(art, element("span", "store-card__veil"));
    // A shop that quoted nothing leaves the slot empty rather than printing a
    // blank price frame the shopper would read as "free".
    const price = formatPrice(selectBestOffer(game));
    if (price) media.append(element("span", "store-card__price", price));
    if (!artHasWordmark) {
      const cardTitle = element("span", "store-card__title", game.title);
      // A long name is set tighter rather than ellipsised: a shelf is for
      // discovering games, and "THE CASE OF THE GOLDEN…" discovers nothing.
      cardTitle.classList.toggle("store-card__title--long", game.title.length > 21);
      media.append(cardTitle);
    }
    else media.classList.add("store-card__media--wordmark");

    const body = element("span", "store-card__body");
    body.append(element("span", "store-card__genres", genreLabel(game)));
    const chips = element("span", "store-card__chips");
    chips.append(
      element("span", "store-card__chip", sessionLabel(game)),
      element("span", "store-card__chip", modeLabel(game)),
    );
    body.append(chips);

    const stats = element("span", "store-card__stats");
    for (const stat of fitStats(game)) {
      const row = element("span", "store-card__stat");
      const dots = element("span", "store-card__dots");
      dots.setAttribute("role", "img");
      dots.setAttribute("aria-label", `${stat.value} sur 5`);
      for (let dot = 0; dot < 5; dot += 1) {
        dots.append(element("i", `store-card__dot${dot < stat.value ? " store-card__dot--on" : ""}`));
      }
      row.append(element("span", "store-card__stat-label", stat.label), dots);
      stats.append(row);
    }
    const tagline = element("span", "store-card__tagline", taglineLabel(game));
    tagline.classList.toggle("store-card__tagline--long", taglineLabel(game).length > 38);
    body.append(stats, tagline);
    open.append(media, body);

    const wishlist = element("button", "store-card__wishlist");
    wishlist.type = "button";
    wishlist.dataset.focusKey = `wishlist-${game.id}`;
    wishlist.setAttribute("aria-pressed", String(game.wishlisted));
    wishlist.setAttribute(
      "aria-label",
      game.wishlisted ? `Retirer ${game.title} de tes envies` : `Ajouter ${game.title} à tes envies`,
    );
    wishlist.append(iconElement(game.wishlisted ? "heart-filled" : "heart"));
    wishlist.addEventListener("click", (event) => {
      event.stopPropagation();
      void handleWishlist(game);
    });

    card.append(open, wishlist);
    card.addEventListener("pointerenter", () => previewGame(game.id));
    card.addEventListener("pointerleave", () => previewGame(null));
    return card;
  };

  const syncFeatured = (): void => {
    if (!nodes.railTrack) return;
    const previewId = state.previewGameId;
    pageRoot?.classList.toggle("store-page--previewing", Boolean(previewId));
    const cards = [...nodes.railTrack.querySelectorAll<HTMLElement>(".store-card")];
    cards.forEach((card, index) => {
      const featured = previewId ? card.dataset.gameId === previewId : index === 0;
      card.classList.toggle("store-card--featured", featured);
      card.classList.toggle("store-card--previewing", Boolean(previewId) && featured);
    });
  };

  const renderCards = (games: GameSummary[]): void => {
    if (!nodes.railTrack || !nodes.emptyState) return;
    const signature = games.map((game) => `${game.id}:${game.wishlisted ? 1 : 0}`).join("|");
    if (signature !== renderedGameIds) {
      renderedGameIds = signature;
      const fragment = document.createDocumentFragment();
      games.forEach((game, index) => fragment.append(buildCard(game, index)));
      nodes.railTrack.replaceChildren(fragment);
      nodes.railTrack.scrollLeft = 0;
    }
    nodes.railTrack.hidden = games.length === 0;
    nodes.emptyState.hidden = games.length > 0;
    if (games.length === 0) {
      nodes.emptyState.replaceChildren(
        iconElement("search"),
        element("h2", "store-empty__title", "Aucun jeu ne correspond"),
        element(
          "p",
          "store-empty__copy",
          "Essaie une autre catégorie ou une autre plateforme — ta bibliothèque est déjà retirée de ces résultats.",
        ),
      );
    }
    syncFeatured();
    syncArrows();
  };

  const renderFilters = (): void => {
    for (const chip of nodes.categoryBar?.querySelectorAll<HTMLElement>(".store-chip") ?? []) {
      const active = chip.dataset.category === state.category;
      chip.classList.toggle("store-chip--active", active);
      chip.setAttribute("aria-pressed", String(active));
    }
    for (const chip of nodes.platformBar?.querySelectorAll<HTMLElement>("[data-platform]") ?? []) {
      const active = state.platforms.includes(chip.dataset.platform as StorePlatform);
      chip.classList.toggle("store-chip--active", active);
      chip.setAttribute("aria-pressed", String(active));
    }
  };

  const renderMorePanel = (): void => {
    if (!nodes.morePanel) return;
    nodes.morePanel.hidden = !morePlatformsOpen;
    if (!morePlatformsOpen) return;
    const list = element("ul", "store-more-panel__list");
    for (const status of state.home.providerStatuses) {
      const item = element("li", "store-more-panel__item");
      item.dataset.provider = status.provider;
      item.dataset.health = status.health;
      item.append(
        element("span", "store-more-panel__dot"),
        element("span", "store-more-panel__name", status.label),
        element("span", "store-more-panel__message", status.message),
      );
      list.append(item);
    }
    nodes.morePanel.replaceChildren(
      element("p", "store-more-panel__title", "Sources de prix"),
      list,
    );
  };

  const renderWhyPanel = (): void => {
    if (!nodes.whyPanel) return;
    nodes.whyPanel.hidden = !whyOpen;
    if (!whyOpen) return;
    const close = element("button", "store-why__close");
    close.type = "button";
    close.setAttribute("aria-label", "Fermer");
    close.append(iconElement("close"));
    close.addEventListener("click", () => {
      whyOpen = false;
      renderWhyPanel();
      nodes.heroAction?.focus();
    });
    nodes.whyPanel.replaceChildren(
      close,
      element("h2", "store-why__title", "Pourquoi cette sélection"),
      element(
        "p",
        "store-why__text",
        "Orivo ne classe pas les jeux par budget marketing. Chaque titre est retenu pour ce qu'il te laisse : une idée, une émotion, une heure bien passée.",
      ),
      element(
        "p",
        "store-why__text",
        "Les durées annoncées sont celles de l'histoire principale, les prix viennent des boutiques elles-mêmes, et les jeux déjà dans ta bibliothèque n'apparaissent jamais ici.",
      ),
      element(
        "p",
        "store-why__text",
        "Moins de bruit, plus de sens : une étagère courte que tu peux lire en entier.",
      ),
    );
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
    renderShelfLabel();
    renderCards(games);
    renderHero(games);
    renderFilters();
    renderMorePanel();
    renderWhyPanel();
    renderStatus();
    restoreFocus(focusSnapshot);
  };

  // ---------------------------------------------------------------------------
  // Rail
  // ---------------------------------------------------------------------------

  const railStep = (): number => {
    const track = nodes.railTrack;
    if (!track) return 0;
    const card = track.querySelector<HTMLElement>(".store-card");
    if (!card) return track.clientWidth;
    const gap = Number.parseFloat(getComputedStyle(track).columnGap || "18") || 18;
    const perView = Math.max(1, Math.floor((track.clientWidth + gap) / (card.offsetWidth + gap)));
    return (card.offsetWidth + gap) * perView;
  };

  /**
   * The arrows walk the whole category, not just what is already loaded: when
   * the track runs out of cards and the catalog has more, the next page is
   * fetched and appended before the scroll continues.
   */
  const scrollRail = (direction: 1 | -1): void => {
    const track = nodes.railTrack;
    if (!track) return;
    const step = railStep();
    const maximum = track.scrollWidth - track.clientWidth;
    const target = Math.max(0, Math.min(maximum, track.scrollLeft + direction * step));
    track.scrollTo({ left: target, behavior: "smooth" });
    if (direction === 1 && maximum - target < step * 1.5 && state.nextCursor && activation) {
      void browse(activation, state.nextCursor, true);
    }
    // `scrollTo` is async; the arrows are re-evaluated once it settles.
    window.setTimeout(syncArrows, 420);
  };

  const syncArrows = (): void => {
    const track = nodes.railTrack;
    if (!track || !nodes.arrowPrevious || !nodes.arrowNext) return;
    const maximum = track.scrollWidth - track.clientWidth;
    const atStart = track.scrollLeft <= 2;
    const atEnd = track.scrollLeft >= maximum - 2;
    nodes.arrowPrevious.hidden = atStart;
    nodes.arrowNext.hidden = (atEnd && !state.nextCursor) || maximum <= 2;
  };

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
    if (event.key !== "Escape") return;
    if (whyOpen) {
      whyOpen = false;
      renderWhyPanel();
    }
    if (morePlatformsOpen) {
      morePlatformsOpen = false;
      renderMorePanel();
    }
  };

  const onOnline = (): void => {
    dispatch({ type: "connectivity-changed", online: true });
    if (activation) void refreshSources(activation);
  };
  const onOffline = (): void => dispatch({ type: "connectivity-changed", online: false });

  return {
    mount(host) {
      container = host;
      // Exactly one `main` per screen. The shell wrapper is a plain `div`, so
      // this page root is that landmark — never a nested one.
      pageRoot = element("main", "store-page");
      pageRoot.tabIndex = -1;
      pageRoot.setAttribute("aria-label", "Store");
      container.replaceChildren(pageRoot);
      buildSkeleton();
      window.addEventListener("online", onOnline);
      window.addEventListener("offline", onOffline);
      window.addEventListener("keydown", onKeyDown);
      window.addEventListener("resize", syncArrows);
      render();
    },
    activate(context) {
      activation = context;
      // `context.route` is a union; narrowing it into a local const keeps the
      // Store fields readable inside the async continuation below.
      const route = context.route;
      if (route.page !== "store") return;
      dispatch({
        type: "activate",
        category: route.category,
        platforms: route.platforms,
        query: route.query,
        online: typeof navigator === "undefined" || navigator.onLine,
      });
      requestAnimationFrame(() => {
        if (isActive(context)) restorePageState(context.restoreState);
      });
      void loadOwned(context);
      void loadHome(context).then(() => {
        if (
          isActive(context) &&
          (route.category !== "for-you" || route.platforms.length > 0 || Boolean(route.query.trim()))
        ) {
          void browse(context, null, false);
        }
      });
    },
    deactivate() {
      for (const timer of [queryTimer, statusTimer, previewTimer]) {
        if (timer) clearTimeout(timer);
      }
      queryTimer = null;
      statusTimer = null;
      previewTimer = null;
      transientStatus = "";
      morePlatformsOpen = false;
      whyOpen = false;
      const focusKey = captureFocus()?.focusKey ?? null;
      const restoreState: PageRestoreState = {
        scrollTop: readScrollTop(),
        focusKey,
        query: state.query,
        filters: [state.category, ...state.platforms],
      };
      activation = null;
      return restoreState;
    },
  };
}

export { storeCategoryLabel };
