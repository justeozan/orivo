import { invoke } from "@tauri-apps/api/core";
import type {
  AppRoute,
  GameDetailView,
  GameMediaKind,
  GameMediaView,
  GameSummary,
  WallpaperSearchView,
  WallpaperSource,
} from "./contracts";
import { icon } from "./icons";
import type { AppPage, PageActivation } from "./page-lifecycle";
import {
  buildMetaFacts,
  buildStatFacts,
  createFallbackGameDetail,
  createFallbackImportedWallpaper,
  createFallbackWallpaperSearch,
  createInitialGameDetailState,
  featureIcon,
  formatAchievementProgress,
  formatRelativeTime,
  formatReleaseDate,
  heroImageUrl,
  mediaForKind,
  normaliseGameDetail,
  normaliseGameMedia,
  normaliseWallpaperSearch,
  platformLabels,
  previewedMedia,
  reduceGameDetailState,
  resolvePrimaryAction,
  shouldOfferAboutToggle,
  shouldRenderSection,
  toGameDetailRestoreState,
  type GameDetailPageAction,
  type GameDetailViewModel,
} from "./game-detail-model";

export interface GameDetailPageClient {
  getDetail(gameId: string, signal: AbortSignal): Promise<GameDetailView>;
  setWishlist(gameId: string, wishlisted: boolean, signal: AbortSignal): Promise<void>;
  selectMedia(gameId: string, mediaId: string, signal: AbortSignal): Promise<GameMediaView[]>;
  importMedia(gameId: string, kind: GameMediaKind, signal: AbortSignal): Promise<GameMediaView[]>;
  exportMedia(gameId: string, mediaId: string, signal: AbortSignal): Promise<void>;
  cancelMediaDownload(gameId: string, signal: AbortSignal): Promise<void>;
  searchWallpapers(
    source: WallpaperSource,
    query: string,
    offset: number,
    signal: AbortSignal,
  ): Promise<WallpaperSearchView>;
  importWallpaper(gameId: string, candidateId: string, signal: AbortSignal): Promise<GameMediaView[]>;
  openOffer(offerId: string, signal: AbortSignal): Promise<void>;
  /** Fetch cover/hero art for a game the same way an import does. */
  searchArtwork(gameId: string, signal: AbortSignal): Promise<void>;
  /** Remove a game from the library (does not touch the game's own files). */
  removeGame(gameId: string, signal: AbortSignal): Promise<void>;
  /** Promote a chosen media to a game card role: background, cover or landscape. */
  setHomeImage(
    gameId: string,
    mediaId: string,
    role: WallpaperRole,
    signal: AbortSignal,
  ): Promise<void>;
}

/** Which library card slot a chosen wallpaper fills. */
export type WallpaperRole = "background" | "cover" | "landscape";

/** The role picker's segments, in display order. */
const WALLPAPER_ROLES: ReadonlyArray<{ id: WallpaperRole; label: string }> = [
  { id: "background", label: "Background" },
  { id: "cover", label: "Portrait cover" },
  { id: "landscape", label: "Landscape cover" },
];

/** Apply-button labels per role, shown for a single pick. */
const WALLPAPER_ROLE_ACTION: Record<WallpaperRole, string> = {
  background: "Set as background",
  cover: "Set as cover",
  landscape: "Set as landscape",
};

/** Confirmation toasts per role. */
const WALLPAPER_ROLE_APPLIED: Record<WallpaperRole, string> = {
  background: "Wallpaper set as your home background.",
  cover: "Portrait cover updated.",
  landscape: "Landscape cover updated.",
};

export interface GameDetailPageOptions {
  /** Routing stays owned by the application shell. */
  navigate(route: AppRoute): void;
  /** History stays owned by the application shell. */
  back(): void;
  /** Launching stays owned by the application shell. */
  play(gameId: string): void;
  /**
   * Notifies the shell that a catalog change (new home art, refetched cover,
   * or a removed game) means the Library needs to reload so its cards and hero
   * reflect the change.
   */
  onLibraryChanged?(): void;
  client?: GameDetailPageClient;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("The game request was cancelled.", "AbortError");
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

export function createDefaultGameDetailPageClient(): GameDetailPageClient {
  return {
    async getDetail(gameId, signal) {
      if (!isTauriRuntime()) return createFallbackGameDetail(gameId);
      return invokeWhileActive<GameDetailView>("get_game_detail", { gameId }, signal);
    },
    async setWishlist(gameId, wishlisted, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("set_game_wishlist", { gameId, wishlisted }, signal);
    },
    async selectMedia(gameId, mediaId, signal) {
      if (!isTauriRuntime()) return [];
      return invokeWhileActive<GameMediaView[]>("select_game_media", { gameId, mediaId }, signal);
    },
    async importMedia(gameId, kind, signal) {
      if (!isTauriRuntime()) return [];
      return invokeWhileActive<GameMediaView[]>("import_game_media", { gameId, kind }, signal);
    },
    async exportMedia(gameId, mediaId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("export_game_media", { gameId, mediaId }, signal);
    },
    async cancelMediaDownload(gameId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("cancel_game_media_download", { gameId }, signal);
    },
    async searchWallpapers(source, query, offset, signal) {
      if (!isTauriRuntime()) return createFallbackWallpaperSearch(source, query, offset);
      return invokeWhileActive<WallpaperSearchView>("search_wallpapers", { source, query, offset }, signal);
    },
    async importWallpaper(gameId, candidateId, signal) {
      if (!isTauriRuntime()) return [createFallbackImportedWallpaper()];
      return invokeWhileActive<GameMediaView[]>(
        "import_wallpaper_candidate",
        { gameId, candidateId },
        signal,
      );
    },
    async openOffer(offerId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("open_store_offer", { offerId }, signal);
    },
    async searchArtwork(gameId, signal) {
      if (!isTauriRuntime()) return;
      // The explicit action re-runs the search even if art already exists.
      await invokeWhileActive("fetch_game_artwork", { gameId, force: true }, signal);
    },
    async removeGame(gameId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("remove_game", { gameId }, signal);
    },
    async setHomeImage(gameId, mediaId, role, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("set_home_image", { gameId, mediaId, role }, signal);
    },
  };
}

type FocusableElement = HTMLElement & { dataset: DOMStringMap };

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
  const wrapper = element("span", `gd-icon ${className}`.trim());
  wrapper.innerHTML = icon(name);
  return wrapper;
}

function isAbort(error: unknown): boolean {
  return error instanceof Error && error.name === "AbortError";
}

/** A native file dialog dismissal is a normal outcome, not a failure. */
function isUserCancellation(error: unknown): boolean {
  if (isAbort(error)) return true;
  const message = typeof error === "string" ? error : error instanceof Error ? error.message : "";
  return /cancell?ed|dismissed|no file selected/i.test(message);
}

function requestErrorMessage(error: unknown, fallback: string): string {
  if (isAbort(error)) return "";
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return fallback;
}

function attachImage(
  frame: HTMLElement,
  image: HTMLImageElement,
  src: string,
  missingClass: string,
): void {
  if (!src) {
    frame.classList.add(missingClass);
    return;
  }
  image.src = src;
  image.alt = "";
  image.addEventListener("error", () => frame.classList.add(missingClass));
}

/**
 * Creates the Game detail page. The shell injects navigation, history and
 * launching, and imports `game-detail-page.css`; the page owns no topbar,
 * no routing and no global styles.
 */
export function createGameDetailPage(options: GameDetailPageOptions): AppPage {
  const client = options.client ?? createDefaultGameDetailPageClient();
  let state = createInitialGameDetailState();
  let container: HTMLElement | null = null;
  let pageRoot: HTMLElement | null = null;
  let activation: PageActivation | null = null;
  let requestSequence = 0;
  let statusTimer: ReturnType<typeof setTimeout> | null = null;
  /** Tears down the open "…" menu's listeners; null when no menu is open. */
  let moreMenuCleanup: (() => void) | null = null;
  /** Slide ids ticked in the wallpaper grid; downloaded/applied together. */
  const wallpaperSelection = new Set<string>();
  /** Which card role a single ticked wallpaper is applied to. */
  let wallpaperRole: WallpaperRole = "background";
  /** The hero media rail's current page (three tiles per page). */
  let galleryPage = 0;

  const isActive = (context = activation): context is PageActivation =>
    Boolean(context && context.isCurrent() && !context.signal.aborted);

  /** A response is only painted when it still belongs to the visible game. */
  const isFresh = (context: PageActivation | null, gameId: string): boolean =>
    isActive(context) && state.gameId === gameId;

  const dispatch = (action: GameDetailPageAction, shouldRender = true): void => {
    state = reduceGameDetailState(state, action);
    if (shouldRender) render();
  };

  const showTransientStatus = (message: string): void => {
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = setTimeout(() => {
      statusTimer = null;
      dispatch({ type: "status-changed", message: "" });
    }, 4_500);
    dispatch({ type: "status-changed", message });
  };

  /** Drop a pending progress message the moment its result is on screen. */
  const clearTransientStatus = (): void => {
    if (statusTimer) clearTimeout(statusTimer);
    statusTimer = null;
    dispatch({ type: "status-changed", message: "" });
  };

  const loadDetail = async (context: PageActivation, gameId: string): Promise<void> => {
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId });
    try {
      const payload = await client.getDetail(gameId, context.signal);
      if (!isFresh(context, gameId)) return;
      dispatch({ type: "detail-loaded", requestId, detail: normaliseGameDetail(payload) });
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      const message = requestErrorMessage(error, "This game could not be loaded right now.");
      if (!message) return;
      dispatch({
        type: "request-failed",
        requestId,
        message,
        offline: typeof navigator !== "undefined" && !navigator.onLine,
      });
    }
  };

  const handlePrimaryAction = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (!isActive(context) || !gameId) return;
    const action = resolvePrimaryAction(state.detail, gameId);
    if (action.disabled) return;
    if (action.intent === "play") {
      options.play(gameId);
      return;
    }
    if (action.intent === "navigate" && action.route) {
      options.navigate(action.route);
      return;
    }
    if (action.intent !== "open-offer" || !action.offerId) return;
    try {
      await client.openOffer(action.offerId, context.signal);
      if (isFresh(context, gameId)) showTransientStatus("Offer opened in your browser.");
    } catch (error) {
      if (!isFresh(context, gameId) || isUserCancellation(error)) return;
      showTransientStatus(requestErrorMessage(error, "The offer could not be opened."));
    }
  };

  /** Refetch cover/hero art (the same search a fresh import runs), then repaint. */
  const handleSearchArtwork = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (!isActive(context) || !gameId) return;
    showTransientStatus("Searching for cover & images…");
    try {
      await client.searchArtwork(gameId, context.signal);
      if (!isFresh(context, gameId)) return;
      options.onLibraryChanged?.();
      await loadDetail(context, gameId);
      // The refreshed cover and screenshots are the confirmation; a message
      // saying so only repeats what is already on screen.
      if (isFresh(context, gameId)) clearTransientStatus();
    } catch (error) {
      if (!isFresh(context, gameId) || isUserCancellation(error)) return;
      showTransientStatus(requestErrorMessage(error, "No cover or images were found for this game."));
    }
  };

  /** Remove the game from the library (its own files are left untouched). */
  const handleRemoveGame = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    const title = state.detail?.title ?? "this game";
    if (!isActive(context) || !gameId) return;
    if (typeof window !== "undefined" && !window.confirm(`Remove ${title} from your library?`)) return;
    try {
      await client.removeGame(gameId, context.signal);
      if (!isActive(context)) return;
      options.onLibraryChanged?.();
      showTransientStatus(`${title} removed from your library.`);
      options.back();
    } catch (error) {
      if (!isActive(context) || isUserCancellation(error)) return;
      showTransientStatus(requestErrorMessage(error, "This game could not be removed."));
    }
  };

  const openWallpaperSearch = (): void => {
    wallpaperSelection.clear();
    wallpaperRole = "background";
    dispatch({ type: "wallpaper-search-opened" });
    // Populate the grid straight away — the reference shows a full board on open,
    // not an empty search form.
    if (state.wallpaperSearch.candidates.length === 0) void handleWallpaperSearch();
    pageRoot?.querySelector<HTMLElement>("[data-focus-key='wallpaper-modal-close']")?.focus();
  };

  const closeWallpaperSearch = (): void => {
    wallpaperSelection.clear();
    dispatch({ type: "wallpaper-search-closed" });
    pageRoot?.querySelector<HTMLElement>("[data-focus-key='wallpaper-search-toggle']")?.focus();
  };

  /**
   * A click on a rail wallpaper chooses it as the game's home (Library)
   * background — it previews in the hero at once, persists the selection, and
   * refreshes the library. This is the reference's "the wallpaper you pick here
   * is the one the home screen paints" behaviour.
   */
  const handleChooseWallpaper = async (mediaId: string): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (!isActive(context) || !gameId || !mediaId || state.mediaBusy) return;
    // Immediate feedback in the detail hero.
    dispatch({ type: "media-previewed", mediaId });
    dispatch({ type: "media-busy-changed", busy: true });
    try {
      const media = await client.selectMedia(gameId, mediaId, context.signal);
      if (!isFresh(context, gameId)) return;
      const committed = normaliseGameMedia(media);
      // The browser fallback returns nothing; keep the preview rather than
      // wiping the rail.
      if (committed.length > 0) dispatch({ type: "media-committed", media: committed });
      else dispatch({ type: "media-busy-changed", busy: false });
      await client.setHomeImage(gameId, mediaId, "background", context.signal);
      if (!isFresh(context, gameId)) return;
      options.onLibraryChanged?.();
      showTransientStatus("Wallpaper set as your home background.");
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      dispatch({
        type: "media-failed",
        message: requestErrorMessage(error, "That wallpaper could not be set as your background."),
      });
    }
  };

  const handleWallpaperSearch = async (more = false): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (!isActive(context) || !gameId || state.wallpaperSearch.busy) return;
    const source = state.wallpaperSearch.source;
    const query = state.wallpaperSearch.query.trim();
    if (!query) return;
    dispatch({ type: "wallpaper-search-started", more });
    const offset = state.wallpaperSearch.offset;
    try {
      const results = await client.searchWallpapers(source, query, offset, context.signal);
      if (!isFresh(context, gameId)) return;
      dispatch({ type: "wallpaper-search-results", results: normaliseWallpaperSearch(results) });
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      dispatch({
        type: "wallpaper-search-failed",
        message: requestErrorMessage(error, "That search did not finish. Try again."),
      });
    }
  };

  /**
   * Applies the wallpaper grid's ticked tiles: every chosen search result is
   * downloaded (so several can be saved in one go), and the first pick becomes
   * the game's home (Library) background.
   */
  const handleApplySelection = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (!isActive(context) || !gameId || state.mediaBusy) return;
    const chosen = wallpaperSlides().filter((slide) => wallpaperSelection.has(slide.id));
    if (chosen.length === 0) return;
    dispatch({ type: "media-busy-changed", busy: true });
    try {
      // Download every chosen search result so several are saved in one go.
      const savedIds: string[] = [];
      for (const slide of chosen) {
        if (slide.candidate) {
          const media = await client.importWallpaper(gameId, slide.id, context.signal);
          if (!isFresh(context, gameId)) return;
          const normalised = normaliseGameMedia(media);
          if (normalised.length > 0) dispatch({ type: "media-imported", media: normalised });
          const saved = normalised.find((item) => item.selected) ?? normalised[normalised.length - 1];
          if (saved) savedIds.push(saved.id);
        } else {
          savedIds.push(slide.id);
        }
      }
      // A single pick also becomes the home (Library) background; a multi-pick
      // just adds them all to the game.
      const homeMediaId = chosen.length === 1 ? savedIds[0] ?? null : null;
      if (homeMediaId) {
        const media = await client.selectMedia(gameId, homeMediaId, context.signal);
        if (!isFresh(context, gameId)) return;
        const committed = normaliseGameMedia(media);
        if (committed.length > 0) dispatch({ type: "media-committed", media: committed });
        await client.setHomeImage(gameId, homeMediaId, wallpaperRole, context.signal);
      } else {
        dispatch({ type: "media-busy-changed", busy: false });
      }
      if (!isFresh(context, gameId)) return;
      options.onLibraryChanged?.();
      wallpaperSelection.clear();
      dispatch({ type: "wallpaper-search-closed" });
      showTransientStatus(
        chosen.length > 1
          ? `${chosen.length} wallpapers added to this game.`
          : WALLPAPER_ROLE_APPLIED[wallpaperRole],
      );
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      dispatch({
        type: "media-failed",
        message: requestErrorMessage(error, "Those wallpapers could not be saved."),
      });
    }
  };

  /* ---------------------------------------------------------------------- */
  /* Rendering                                                               */
  /* ---------------------------------------------------------------------- */

  const renderBackButton = (): HTMLElement => {
    const back = element("button", "gd-back");
    back.type = "button";
    back.dataset.focusKey = "back";
    back.append(
      element("span", "gd-back__glyph"),
      element("span", "gd-back__label", state.from === "store" ? "Back to Store" : "Back to Library"),
    );
    (back.firstElementChild as HTMLElement).innerHTML = icon("chevron-left");
    back.addEventListener("click", () => options.back());
    return back;
  };

  const renderBackBar = (): HTMLElement => {
    const bar = element("div", "gd-topline");
    bar.append(renderBackButton());
    return bar;
  };

  const renderStatus = (): HTMLElement | null => {
    const message = state.statusMessage || state.errorMessage;
    if (!message && state.phase !== "loading") return null;
    const status = element("div", `gd-status gd-status--${state.phase}`);
    status.setAttribute("role", state.phase === "error" ? "alert" : "status");
    status.setAttribute("aria-live", "polite");
    status.append(
      iconElement(state.phase === "loading" ? "refresh" : state.errorMessage ? "alert" : "navigate"),
      element("span", "gd-status__copy", message || "Loading game details…"),
    );
    return status;
  };

  const renderNotice = (title: string, copy: string, retry: boolean): HTMLElement => {
    const notice = element("section", "gd-notice");
    notice.setAttribute("role", "status");
    notice.append(iconElement("alert", "gd-notice__icon"));
    notice.append(element("h1", "gd-notice__title", title), element("p", "gd-notice__copy", copy));
    const actions = element("div", "gd-notice__actions");
    const back = element("button", "gd-button gd-button--ghost", "Go back");
    back.type = "button";
    back.dataset.focusKey = "notice-back";
    back.addEventListener("click", () => options.back());
    actions.append(back);
    if (retry) {
      const again = element("button", "gd-button gd-button--primary", "Try again");
      again.type = "button";
      again.dataset.focusKey = "notice-retry";
      again.addEventListener("click", () => {
        if (activation && state.gameId) void loadDetail(activation, state.gameId);
      });
      actions.append(again);
    }
    notice.append(actions);
    return notice;
  };

  const renderSkeleton = (): HTMLElement => {
    const skeleton = element("section", "gd-skeleton");
    skeleton.setAttribute("aria-hidden", "true");
    skeleton.append(
      element("span", "gd-skeleton__hero"),
      element("span", "gd-skeleton__line gd-skeleton__line--title"),
      element("span", "gd-skeleton__line"),
      element("span", "gd-skeleton__line gd-skeleton__line--short"),
    );
    return skeleton;
  };

  const renderFactList = (
    className: string,
    facts: ReturnType<typeof buildMetaFacts>,
    separator: boolean,
  ): HTMLElement => {
    const list = element("ul", className);
    for (const fact of facts) {
      const item = element("li", `${className}__item`);
      item.dataset.factId = fact.id;
      if (fact.tone === "accent") item.dataset.tone = "accent";
      // The reference's meta row carries icons too (clock, star, thumbs-up),
      // so every fact with a glyph renders it — not just the stats row.
      if (fact.icon) item.append(iconElement(fact.icon));
      item.append(element("span", `${className}__text`, fact.text));
      if (separator) item.dataset.separator = "dot";
      list.append(item);
    }
    return list;
  };

  const renderPrimaryAction = (detail: GameDetailViewModel): HTMLElement => {
    const descriptor = resolvePrimaryAction(detail, state.gameId);
    const button = element("button", "gd-button gd-button--primary gd-primary-action");
    button.type = "button";
    button.dataset.focusKey = "primary-action";
    button.dataset.action = descriptor.kind;
    button.disabled = descriptor.disabled;
    button.title = descriptor.hint;
    button.setAttribute("aria-label", `${descriptor.label}: ${descriptor.hint}`);
    button.append(iconElement(descriptor.icon), element("span", "gd-button__label", descriptor.label));
    button.addEventListener("click", () => void handlePrimaryAction());
    return button;
  };

  /**
   * Wraps the primary action so a launchable game gets the reference's split
   * "Play ▾" control: the label commits, the chevron opens run options. The
   * chevron is a visual affordance for now and never blocks the primary action.
   */
  const renderPrimaryActionGroup = (detail: GameDetailViewModel): HTMLElement => {
    const action = renderPrimaryAction(detail);
    const descriptor = resolvePrimaryAction(detail, state.gameId);
    if (descriptor.kind !== "play") return action;
    const group = element("div", "gd-play-split");
    action.classList.add("gd-play-split__main");
    const more = element("button", "gd-button gd-button--primary gd-play-split__more");
    more.type = "button";
    more.dataset.focusKey = "play-options";
    more.setAttribute("aria-label", `Run options for ${detail.title}`);
    more.append(iconElement("chevron-down"));
    group.append(action, more);
    return group;
  };

  const closeMoreMenu = (button: HTMLElement, menu: HTMLElement): void => {
    menu.hidden = true;
    button.setAttribute("aria-expanded", "false");
    moreMenuCleanup?.();
    moreMenuCleanup = null;
  };

  const openMoreMenu = (button: HTMLElement, menu: HTMLElement): void => {
    moreMenuCleanup?.();
    menu.hidden = false;
    button.setAttribute("aria-expanded", "true");
    const onDocClick = (event: MouseEvent): void => {
      if (!menu.contains(event.target as Node) && event.target !== button) {
        closeMoreMenu(button, menu);
      }
    };
    const onKey = (event: KeyboardEvent): void => {
      if (event.key === "Escape") {
        closeMoreMenu(button, menu);
        button.focus();
      }
    };
    // Deferred so the opening click itself does not immediately close it.
    const timer = setTimeout(() => document.addEventListener("click", onDocClick), 0);
    document.addEventListener("keydown", onKey);
    moreMenuCleanup = () => {
      clearTimeout(timer);
      document.removeEventListener("click", onDocClick);
      document.removeEventListener("keydown", onKey);
    };
    menu.querySelector<HTMLElement>("[role='menuitem']")?.focus();
  };

  /**
   * The "…" control opens a small actions menu — refetch cover/hero art (the
   * same search an import runs) and remove the game from the library. The menu
   * is toggled directly on the DOM so opening it never repaints the page.
   */
  const renderMoreButton = (detail: GameDetailViewModel): HTMLElement => {
    const wrap = element("div", "gd-more-wrap");
    const button = element("button", "gd-button gd-button--ghost gd-more");
    button.type = "button";
    button.dataset.focusKey = "more-actions";
    button.setAttribute("aria-haspopup", "menu");
    button.setAttribute("aria-expanded", "false");
    button.setAttribute("aria-label", `More actions for ${detail.title}`);
    button.append(iconElement("more"));

    const menu = element("div", "gd-menu");
    menu.setAttribute("role", "menu");
    menu.hidden = true;

    const item = (
      focusKey: string,
      iconName: Parameters<typeof icon>[0],
      label: string,
      run: () => void,
      danger = false,
    ): HTMLElement => {
      const entry = element("button", `gd-menu__item${danger ? " gd-menu__item--danger" : ""}`);
      entry.type = "button";
      entry.dataset.focusKey = focusKey;
      entry.setAttribute("role", "menuitem");
      entry.append(iconElement(iconName), element("span", "gd-menu__label", label));
      entry.addEventListener("click", () => {
        closeMoreMenu(button, menu);
        run();
      });
      return entry;
    };

    menu.append(
      item("menu-artwork", "search", "Search cover & images", () => void handleSearchArtwork()),
      item("menu-remove", "close", "Remove from library", () => void handleRemoveGame(), true),
    );

    button.addEventListener("click", (event) => {
      event.stopPropagation();
      if (menu.hidden) openMoreMenu(button, menu);
      else closeMoreMenu(button, menu);
    });

    wrap.append(button, menu);
    return wrap;
  };

  const renderHeroMedia = (detail: GameDetailViewModel): HTMLElement => {
    const frame = element("div", "gd-hero__media");
    const preview = previewedMedia(state);
    if (preview && preview.kind === "video") {
      const video = document.createElement("video");
      video.className = "gd-hero__video";
      video.controls = true;
      video.preload = "metadata";
      video.playsInline = true;
      video.autoplay = false;
      video.src = preview.previewUrl;
      if (preview.posterUrl) video.poster = preview.posterUrl;
      video.setAttribute("aria-label", `${preview.title} trailer for ${detail.title}`);
      const play = element("button", "gd-hero__play");
      play.type = "button";
      play.dataset.focusKey = "hero-play";
      play.setAttribute("aria-label", `Play ${preview.title}`);
      play.append(iconElement("play"));
      play.addEventListener("click", () => {
        play.hidden = true;
        // Explicit user intent only: nothing here ever autoplays.
        void video.play().catch(() => {
          play.hidden = false;
        });
      });
      video.addEventListener("pause", () => {
        play.hidden = false;
      });
      frame.append(video, play);
      return frame;
    }
    const image = element("img", "gd-hero__image");
    image.decoding = "async";
    image.setAttribute("aria-hidden", "true");
    attachImage(frame, image, heroImageUrl(state), "gd-hero__media--missing");
    frame.append(image);
    return frame;
  };

  /** Media eligible for the hero rail: showcase art and trailers, in order. */
  const galleryMedia = (): GameMediaView[] =>
    state.media.filter((item) => item.kind === "video" || item.kind === "wallpaper");

  /** The "Add wallpaper" tile that opens the wallpaper dialog. */
  const renderGalleryAddTile = (detail: GameDetailViewModel): HTMLElement => {
    const add = element("button", "gd-gallery__add");
    add.type = "button";
    add.dataset.focusKey = "wallpaper-search-toggle";
    add.setAttribute("aria-haspopup", "dialog");
    add.setAttribute("aria-label", `Add a wallpaper for ${detail.title}`);
    add.append(iconElement("search"), element("span", "gd-gallery__add-label", "Add wallpaper"));
    add.addEventListener("click", openWallpaperSearch);
    return add;
  };

  /**
   * The reference's vertical rail overlapping the hero's top-right: a
   * "‹ Wallpaper ›" header with dot pagination, three media tiles per page (the
   * selected one framed), then a dashed "Add wallpaper" tile. Tiles preview /
   * choose the home background; the dialog owns full selection and search.
   */
  const renderGalleryRail = (detail: GameDetailViewModel): HTMLElement => {
    const items = galleryMedia();
    const rail = element("aside", "gd-gallery");
    rail.setAttribute("aria-label", "Wallpapers");

    // No media yet: just the add-wallpaper affordance (a freshly imported local
    // or Wine title can still gain the wallpaper the Library paints behind it).
    if (items.length === 0) {
      rail.classList.add("gd-gallery--empty");
      rail.append(renderGalleryAddTile(detail));
      return rail;
    }

    const PAGE = 3;
    const pageCount = Math.max(1, Math.ceil(items.length / PAGE));
    galleryPage = Math.min(Math.max(0, galleryPage), pageCount - 1);

    const head = element("div", "gd-gallery__head");
    const prev = element("button", "gd-gallery__nav");
    prev.type = "button";
    prev.dataset.focusKey = "gallery-prev";
    prev.setAttribute("aria-label", "Previous wallpapers");
    prev.disabled = galleryPage <= 0;
    prev.append(iconElement("chevron-left"));
    prev.addEventListener("click", () => {
      galleryPage -= 1;
      render();
    });
    const next = element("button", "gd-gallery__nav");
    next.type = "button";
    next.dataset.focusKey = "gallery-next";
    next.setAttribute("aria-label", "More wallpapers");
    next.disabled = galleryPage >= pageCount - 1;
    next.append(iconElement("chevron-right"));
    next.addEventListener("click", () => {
      galleryPage += 1;
      render();
    });
    head.append(prev, element("span", "gd-gallery__title", "Wallpaper"), next);
    rail.append(head);

    if (pageCount > 1) {
      const dots = element("div", "gd-gallery__dots");
      dots.setAttribute("aria-hidden", "true");
      for (let page = 0; page < pageCount; page += 1) {
        const dot = element("span", "gd-gallery__dot");
        dot.classList.toggle("gd-gallery__dot--active", page === galleryPage);
        dots.append(dot);
      }
      rail.append(dots);
    }

    // Three fixed slots so the rail keeps a steady shape, like the reference;
    // short pages pad with empty plates.
    const pageItems = items.slice(galleryPage * PAGE, galleryPage * PAGE + PAGE);
    for (let slot = 0; slot < PAGE; slot += 1) {
      const item = pageItems[slot];
      if (item) {
        rail.append(renderGalleryTile(item));
      } else {
        const empty = element("div", "gd-gallery__tile gd-gallery__tile--empty");
        empty.setAttribute("aria-hidden", "true");
        rail.append(empty);
      }
    }

    rail.append(renderGalleryAddTile(detail));
    return rail;
  };

  /** One tile in the hero rail: previews on click; a trailer keeps its play cue. */
  const renderGalleryTile = (item: GameMediaView): HTMLElement => {
    const selected = state.previewMediaId === item.id;
    const tile = element("button", "gd-gallery__tile");
    tile.type = "button";
    tile.dataset.focusKey = `media-${item.id}`;
    tile.dataset.mediaKind = item.kind;
    tile.classList.toggle("gd-gallery__tile--selected", selected);
    tile.setAttribute("aria-label", `${item.title}${item.availableOffline ? "" : " — needs a download"}`);
    const image = element("img", "gd-gallery__tile-image");
    image.loading = "lazy";
    image.decoding = "async";
    attachImage(tile, image, item.posterUrl ?? item.previewUrl, "gd-gallery__tile--missing");
    tile.append(image);
    if (item.kind === "video") {
      const play = element("span", "gd-gallery__play");
      play.setAttribute("aria-hidden", "true");
      play.append(iconElement("play"));
      tile.append(play);
    }
    // A wallpaper click chooses the home background; a trailer just previews.
    tile.addEventListener("click", () => {
      if (item.kind === "wallpaper") void handleChooseWallpaper(item.id);
      else dispatch({ type: "media-previewed", mediaId: item.id });
    });
    return tile;
  };

  interface WallpaperSlide {
    id: string;
    title: string;
    url: string;
    candidate: boolean;
  }

  const wallpaperSlides = (): WallpaperSlide[] => [
    ...state.wallpaperSearch.candidates.map((candidate) => ({
      id: candidate.id,
      title: candidate.title,
      url: candidate.thumbnailUrl,
      candidate: true,
    })),
    ...mediaForKind(state.media, "wallpaper").map((item) => ({
      id: item.id,
      title: item.title,
      url: item.posterUrl ?? item.previewUrl,
      candidate: false,
    })),
  ];

  const renderWallpaperModal = (): HTMLElement | null => {
    const search = state.wallpaperSearch;
    if (!search.open) return null;
    const modal = element("section", "gd-modal");
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-labelledby", "gd-wallpaper-title");

    const backdrop = element("div", "gd-modal__backdrop");
    backdrop.addEventListener("click", closeWallpaperSearch);

    const dialog = element("div", "gd-modal__dialog gd-modal__dialog--grid");
    const header = element("header", "gd-modal__header");
    const heading = element("div", "gd-modal__heading");
    const title = element("h2", "gd-modal__title", "Wallpapers");
    title.id = "gd-wallpaper-title";
    heading.append(title);
    const close = element("button", "gd-modal__close");
    close.type = "button";
    close.dataset.focusKey = "wallpaper-modal-close";
    close.setAttribute("aria-label", "Close wallpapers");
    close.append(iconElement("close"));
    close.addEventListener("click", closeWallpaperSearch);
    header.append(heading, close);
    dialog.append(header);

    dialog.append(element("h3", "gd-modal__subtitle", "Choose your vibe"));
    dialog.append(
      element(
        "p",
        "gd-modal__hint",
        "Pick one to set your home background, or tick several to save them all at once.",
      ),
    );

    const form = element("form", "gd-search__form");
    form.setAttribute("role", "search");

    const source = element("select", "gd-search__source");
    source.dataset.focusKey = "wallpaper-search-source";
    source.setAttribute("aria-label", "Wallpaper source");
    // Steam Store is the built-in keyless source that returns real game art;
    // Wikimedia and Openverse follow as keyless fallbacks. IGDB and Google
    // Images need keys saved in Settings.
    for (const option of [
      ["steam-store", "Steam Store"],
      ["wikimedia", "Wikimedia Commons"],
      ["openverse", "Openverse"],
      ["igdb", "IGDB"],
      ["google-images", "Google Images"],
    ] as const) {
      const entry = document.createElement("option");
      entry.value = option[0];
      entry.textContent = option[1];
      source.append(entry);
    }
    source.value = search.source;
    source.addEventListener("change", () =>
      dispatch({ type: "wallpaper-search-source-changed", source: source.value as WallpaperSource }),
    );

    const input = element("input", "gd-search__input") as HTMLInputElement;
    input.type = "text";
    input.placeholder = "e.g. Elden Ring wallpaper";
    input.dataset.focusKey = "wallpaper-search-input";
    input.value = search.query;
    input.setAttribute("aria-label", "Wallpaper search query");
    input.addEventListener("input", () =>
      dispatch({ type: "wallpaper-search-query-changed", query: input.value }, false),
    );

    const submit = element("button", "gd-button gd-button--primary gd-search__submit", "Search");
    submit.type = "submit";
    submit.dataset.focusKey = "wallpaper-search-button";
    submit.disabled = search.busy || !search.query.trim();
    submit.prepend(iconElement("search"));

    form.append(source, input, submit);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      void handleWallpaperSearch();
    });
    dialog.append(form);

    // A tick-selectable grid: several picks download together, and the first
    // one becomes the home background.
    const slides = wallpaperSlides();
    const grid = element("div", "gd-wallgrid");
    grid.setAttribute("role", "group");
    grid.setAttribute("aria-label", "Wallpapers");
    for (const slide of slides) {
      const tile = element("button", "gd-wallgrid__tile");
      tile.type = "button";
      tile.dataset.focusKey = `wall-${slide.id}`;
      const picked = wallpaperSelection.has(slide.id);
      tile.classList.toggle("gd-wallgrid__tile--selected", picked);
      tile.classList.toggle("gd-wallgrid__tile--current", slide.id === state.appliedMediaId);
      tile.setAttribute("aria-pressed", String(picked));
      tile.setAttribute("aria-label", slide.title);
      const image = element("img", "gd-wallgrid__image");
      image.loading = "lazy";
      image.decoding = "async";
      attachImage(tile, image, slide.url, "gd-wallgrid__tile--missing");
      const check = element("span", "gd-wallgrid__check");
      check.setAttribute("aria-hidden", "true");
      check.append(iconElement("check"));
      tile.append(image, check);
      tile.addEventListener("click", () => {
        if (wallpaperSelection.has(slide.id)) wallpaperSelection.delete(slide.id);
        else wallpaperSelection.add(slide.id);
        render();
      });
      grid.append(tile);
    }
    dialog.append(grid);

    if (search.busy) {
      const busy = element("p", "gd-search__status", "Searching…");
      busy.setAttribute("role", "status");
      dialog.append(busy);
    } else if (search.phase === "not-configured") {
      dialog.append(
        element(
          "p",
          "gd-search__notice",
          search.message || "This source is not configured yet. Add the required keys and try again.",
        ),
      );
    } else if (search.phase === "error") {
      const error = element("p", "gd-search__notice gd-search__notice--error", search.message);
      error.setAttribute("role", "alert");
      dialog.append(error);
    } else if (search.phase === "ready" && slides.length === 0) {
      dialog.append(
        element("p", "gd-search__notice", "No wallpapers matched that search. Try another query."),
      );
    }

    if (state.mediaBusy) {
      const busy = element("p", "gd-search__status", "Saving wallpapers…");
      busy.setAttribute("role", "status");
      dialog.append(busy);
    }

    if (state.mediaError) {
      const error = element("p", "gd-modal__error", state.mediaError);
      error.setAttribute("role", "alert");
      dialog.append(error);
    }

    const picks = wallpaperSelection.size;

    // A single pick can target a specific card role; multi-pick just downloads.
    if (picks === 1) {
      const roles = element("div", "gd-roles");
      roles.setAttribute("role", "group");
      roles.setAttribute("aria-label", "Apply the wallpaper as");
      roles.append(element("span", "gd-roles__label", "Apply as"));
      for (const role of WALLPAPER_ROLES) {
        const button = element("button", "gd-roles__button");
        button.type = "button";
        button.dataset.focusKey = `wallpaper-role-${role.id}`;
        const active = wallpaperRole === role.id;
        button.classList.toggle("gd-roles__button--active", active);
        button.setAttribute("aria-pressed", String(active));
        button.textContent = role.label;
        button.addEventListener("click", () => {
          wallpaperRole = role.id;
          render();
        });
        roles.append(button);
      }
      dialog.append(roles);
    }

    const actions = element("div", "gd-modal__actions");
    if (search.phase === "ready" && search.hasMore && !search.busy) {
      const more = element("button", "gd-button gd-button--ghost gd-search__more", "Search more");
      more.type = "button";
      more.dataset.focusKey = "wallpaper-search-more";
      more.addEventListener("click", () => void handleWallpaperSearch(true));
      actions.append(more);
    }
    const apply = element(
      "button",
      "gd-button gd-button--primary gd-modal__use",
      picks > 1 ? `Add ${picks} wallpapers` : picks === 1 ? WALLPAPER_ROLE_ACTION[wallpaperRole] : "Apply wallpaper",
    );
    apply.type = "button";
    apply.dataset.focusKey = "wallpaper-apply";
    apply.disabled = picks === 0 || state.mediaBusy;
    apply.addEventListener("click", () => void handleApplySelection());
    actions.append(apply);
    dialog.append(actions);

    // Escape closes the dialog; Tab stays inside it.
    dialog.addEventListener("keydown", (event) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeWallpaperSearch();
        return;
      }
      if (event.key === "Tab") {
        const focusable = dialog.querySelectorAll<HTMLElement>(
          "button, select, input, [href], [tabindex]:not([tabindex='-1'])",
        );
        if (focusable.length === 0) return;
        const first = focusable[0];
        const last = focusable[focusable.length - 1];
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }
    });

    modal.append(backdrop, dialog);
    return modal;
  };

  const renderHero = (detail: GameDetailViewModel): HTMLElement => {
    const hero = element("section", "gd-hero");
    hero.append(renderHeroMedia(detail), element("div", "gd-hero__veil"));
    // The back control and media rail float over the art, matching the
    // reference; the copy column is inset to the shared left margin.
    hero.append(renderBackButton());
    hero.append(renderGalleryRail(detail));
    const copy = element("div", "gd-hero__copy");
    const title = element("h1", "gd-hero__title", detail.title);
    title.id = "gd-hero-title";
    copy.append(title);
    const metaFacts = buildMetaFacts(detail);
    if (metaFacts.length > 0) copy.append(renderFactList("gd-meta", metaFacts, true));
    if (detail.shortDescription) {
      copy.append(element("p", "gd-hero__summary", detail.shortDescription));
    }
    const actions = element("div", "gd-hero__actions");
    actions.append(renderPrimaryActionGroup(detail), renderMoreButton(detail));
    copy.append(actions);
    const stats = buildStatFacts(detail);
    if (stats.length > 0) copy.append(renderFactList("gd-stats", stats, false));
    hero.append(copy);
    return hero;
  };

  const renderAbout = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-about");
    section.append(element("h2", "gd-panel__title", "About this game"));
    const body = element("div", "gd-about__body");
    body.classList.toggle("gd-about__body--clamped", !state.aboutExpanded);
    for (const paragraph of detail.about.split(/\n+/).filter((line) => line.trim())) {
      body.append(element("p", "gd-about__paragraph", paragraph));
    }
    section.append(body);
    if (shouldOfferAboutToggle(detail.about)) {
      const toggle = element("button", "gd-about__toggle", state.aboutExpanded ? "Read less" : "Read more");
      toggle.type = "button";
      toggle.dataset.focusKey = "about-toggle";
      toggle.setAttribute("aria-expanded", String(state.aboutExpanded));
      toggle.append(iconElement("chevron-down"));
      toggle.addEventListener("click", () => dispatch({ type: "about-toggled" }));
      section.append(toggle);
    }
    return section;
  };

  /** Maps a supported platform to its brand glyph for the Game info row. */
  const platformIcon = (platform: GameSummary["supportedPlatforms"][number]): Parameters<typeof icon>[0] =>
    platform === "windows" ? "windows" : platform === "macos" || platform === "ios" ? "monitor" : "monitor";

  const renderGameInfo = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-info");
    section.append(element("h2", "gd-panel__title", "Game info"));
    const list = element("dl", "gd-info__list");
    const rows: Array<[string, string | null]> = [
      ["Developer", detail.developer],
      ["Publisher", detail.publisher],
      ["Release date", detail.releaseDate ? formatReleaseDate(detail.releaseDate) : null],
      ["Genre", detail.genres.length > 0 ? detail.genres.join(", ") : null],
    ];
    for (const [label, value] of rows) {
      if (!value) continue;
      list.append(element("dt", "gd-info__term", label), element("dd", "gd-info__value", value));
    }
    // Platform renders as brand glyphs, matching the reference's Windows mark.
    if (detail.supportedPlatforms.length > 0) {
      list.append(element("dt", "gd-info__term", "Platform"));
      const value = element("dd", "gd-info__value gd-info__value--icons");
      for (const platform of detail.supportedPlatforms) {
        const glyph = iconElement(platformIcon(platform), "gd-info__platform");
        glyph.setAttribute("title", platformLabels(detail).join(", "));
        value.append(glyph);
      }
      list.append(value);
    }
    section.append(list);
    return section;
  };

  const renderFeatures = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-features");
    section.append(element("h2", "gd-panel__title", "Features"));
    const list = element("ul", "gd-features__list");
    for (const feature of detail.features) {
      const item = element("li", "gd-features__item");
      item.append(iconElement(featureIcon(feature)), element("span", "gd-features__label", feature));
      list.append(item);
    }
    section.append(list);
    return section;
  };

  const renderAchievements = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-achievements");
    section.append(element("h2", "gd-panel__title", "Achievements"));
    const progress = formatAchievementProgress(detail.achievements);
    if (progress) {
      section.append(element("p", "gd-achievements__count", progress.label));
      const meter = element("div", "gd-achievements__meter");
      meter.setAttribute("role", "progressbar");
      meter.setAttribute("aria-valuemin", "0");
      meter.setAttribute("aria-valuemax", "100");
      meter.setAttribute("aria-valuenow", String(progress.percent));
      meter.setAttribute("aria-label", `${progress.label} (${progress.percent}%)`);
      const fill = element("span", "gd-achievements__fill");
      fill.style.width = `${progress.percent}%`;
      meter.append(fill);
      const row = element("div", "gd-achievements__progress");
      row.append(meter, element("span", "gd-achievements__percent", `${progress.percent}%`));
      section.append(row);
    }
    const items = detail.achievements?.items ?? [];
    if (items.length > 0) {
      const rail = element("ul", "gd-achievements__rail");
      const shown = items.slice(0, 5);
      // The reference features one badge mid-rail; mirror that on the middle
      // tile so the row reads as a highlighted showcase.
      const featured = Math.floor((shown.length - 1) / 2);
      for (const [index, item] of shown.entries()) {
        const cell = element("li", "gd-achievements__cell");
        cell.classList.toggle("gd-achievements__cell--featured", index === featured);
        cell.title = item.title;
        const image = element("img", "gd-achievements__icon");
        image.loading = "lazy";
        image.alt = item.title;
        if (item.iconUrl) {
          image.src = item.iconUrl;
          image.addEventListener("error", () => cell.classList.add("gd-achievements__cell--missing"));
        } else {
          cell.classList.add("gd-achievements__cell--missing");
        }
        cell.append(image);
        rail.append(cell);
      }
      section.append(rail);
    }
    return section;
  };

  const renderFriends = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-friends");
    section.append(element("h2", "gd-panel__title", "Friends who play"));
    const list = element("ul", "gd-friends__list");
    const friends = detail.friends ?? [];
    for (const friend of friends.slice(0, 4)) {
      const item = element("li", "gd-friends__item");
      item.title = friend.status ? `${friend.name} — ${friend.status}` : friend.name;
      const image = element("img", "gd-friends__avatar");
      image.loading = "lazy";
      image.alt = friend.name;
      if (friend.avatarUrl) {
        image.src = friend.avatarUrl;
        image.addEventListener("error", () => item.classList.add("gd-friends__item--missing"));
      } else {
        item.classList.add("gd-friends__item--missing");
      }
      item.append(image);
      list.append(item);
    }
    if (friends.length > 4) {
      const more = element("li", "gd-friends__count", `+${friends.length - 4}`);
      more.setAttribute("aria-label", `${friends.length - 4} more friends`);
      list.append(more);
    }
    section.append(list);
    return section;
  };

  const renderActivity = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-activity");
    section.append(element("h2", "gd-panel__title", "Activity feed"));
    const list = element("ul", "gd-activity__list");
    for (const entry of (detail.activity ?? []).slice(0, 3)) {
      const item = element("li", "gd-activity__item");
      const head = element("div", "gd-activity__head");
      const image = element("img", "gd-activity__avatar");
      image.loading = "lazy";
      image.alt = "";
      if (entry.avatarUrl) {
        image.src = entry.avatarUrl;
        image.addEventListener("error", () => head.classList.add("gd-activity__head--missing"));
      } else {
        head.classList.add("gd-activity__head--missing");
      }
      const copy = element("div", "gd-activity__copy");
      copy.append(element("p", "gd-activity__actor", entry.actorName));
      copy.append(element("p", "gd-activity__summary", entry.summary));
      if (entry.detail) copy.append(element("p", "gd-activity__detail", entry.detail));
      head.append(image, copy);
      item.append(head);
      const relative = formatRelativeTime(entry.occurredAt);
      if (relative) item.append(element("p", "gd-activity__time", relative));
      list.append(item);
    }
    section.append(list);
    return section;
  };

  const renderRelatedCard = (game: GameSummary): HTMLElement => {
    const card = element("li", "gd-related__cell");
    const button = element("button", "gd-related__card");
    button.type = "button";
    button.dataset.focusKey = `related-${game.id}`;
    button.setAttribute("aria-label", `Open ${game.title}`);
    const frame = element("span", "gd-related__media");
    const image = element("img", "gd-related__cover");
    image.loading = "lazy";
    // The reference's related tiles are landscape art with the title baked in,
    // so the card is the framed image alone — the name lives in the aria-label.
    attachImage(frame, image, game.landscapeUrl || game.coverUrl, "gd-related__media--missing");
    frame.append(image);
    button.append(frame);
    button.addEventListener("click", () =>
      options.navigate({ page: "game", gameId: game.id, from: state.from }),
    );
    card.append(button);
    return card;
  };

  const renderRelated = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-related");
    section.append(element("h2", "gd-panel__title", "Related games"));
    const list = element("ul", "gd-related__list");
    for (const game of detail.relatedGames.slice(0, 8)) list.append(renderRelatedCard(game));
    section.append(list);
    return section;
  };

  const renderDetail = (detail: GameDetailViewModel): DocumentFragment => {
    const fragment = document.createDocumentFragment();
    fragment.append(renderHero(detail));
    const panels = element("div", "gd-panels");
    if (shouldRenderSection(detail, "about")) panels.append(renderAbout(detail));
    // Game info / Features / Achievements share one raised card, as in the
    // reference; About stays flat beside it.
    const infocard = element("div", "gd-infocard");
    if (shouldRenderSection(detail, "info")) infocard.append(renderGameInfo(detail));
    if (shouldRenderSection(detail, "features")) infocard.append(renderFeatures(detail));
    if (shouldRenderSection(detail, "achievements")) infocard.append(renderAchievements(detail));
    if (infocard.childElementCount > 0) panels.append(infocard);
    if (panels.childElementCount > 0) fragment.append(panels);
    const social = element("div", "gd-social");
    if (shouldRenderSection(detail, "friends")) social.append(renderFriends(detail));
    if (shouldRenderSection(detail, "activity")) social.append(renderActivity(detail));
    if (shouldRenderSection(detail, "related")) social.append(renderRelated(detail));
    if (social.childElementCount > 0) fragment.append(social);
    const modal = renderWallpaperModal();
    if (modal) fragment.append(modal);
    return fragment;
  };

  const currentFocusKey = (): string | null => {
    const active = document.activeElement;
    return active instanceof HTMLElement && pageRoot?.contains(active)
      ? active.dataset.focusKey ?? null
      : null;
  };

  const focusByKey = (focusKey: string | null): void => {
    if (!pageRoot || !focusKey) return;
    const target = [...pageRoot.querySelectorAll<FocusableElement>("[data-focus-key]")].find(
      (candidate) => candidate.dataset.focusKey === focusKey,
    );
    target?.focus();
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

  const render = (): void => {
    if (!pageRoot) return;
    // A repaint discards the menu DOM, so drop its document listeners first.
    moreMenuCleanup?.();
    moreMenuCleanup = null;
    const restoreFocusKey = currentFocusKey();
    const scrollTop = readScrollTop();
    const fragment = document.createDocumentFragment();
    // With a loaded game the back control floats inside the hero; the standalone
    // top bar is only for the loading, error and empty states below.
    if (!state.detail) fragment.append(renderBackBar());
    const status = renderStatus();
    if (status) fragment.append(status);
    if (state.phase === "not-found") {
      fragment.append(
        renderNotice(
          "Game not found",
          "This game is no longer in your library or the catalog. It may have been removed or renamed.",
          false,
        ),
      );
    } else if (!state.detail && state.phase === "error") {
      fragment.append(
        renderNotice("Game details unavailable", state.errorMessage || "Something went wrong.", true),
      );
    } else if (!state.detail && state.phase === "offline") {
      fragment.append(
        renderNotice(
          "You are offline",
          "Details for this game are not cached yet. Reconnect and try again.",
          true,
        ),
      );
    } else if (!state.detail) {
      fragment.append(renderSkeleton());
    } else {
      fragment.append(renderDetail(state.detail));
    }
    pageRoot.replaceChildren(fragment);
    writeScrollTop(scrollTop);
    focusByKey(restoreFocusKey);
  };

  const restorePageState = (): void => {
    if (!pageRoot || !activation?.restoreState) return;
    writeScrollTop(Math.max(0, activation.restoreState.scrollTop));
    focusByKey(activation.restoreState.focusKey);
  };

  const onOnline = (): void => {
    dispatch({ type: "connectivity-changed", online: true });
    if (activation && state.gameId && !state.detail) void loadDetail(activation, state.gameId);
  };
  const onOffline = (): void => dispatch({ type: "connectivity-changed", online: false });

  return {
    mount(host) {
      container = host;
      // Exactly one `main` per screen. The shell wrapper is a plain `div`
      // (see the shell comment in app.ts: "each page owns the only <main> on
      // screen"), so this page root is that landmark — never a nested one.
      pageRoot = element("main", "gd-page");
      pageRoot.tabIndex = -1;
      pageRoot.setAttribute("aria-label", "Game details");
      container.replaceChildren(pageRoot);
      window.addEventListener("online", onOnline);
      window.addEventListener("offline", onOffline);
      render();
    },
    activate(context) {
      activation = context;
      if (context.route.page !== "game") return;
      galleryPage = 0;
      const gameId = context.route.gameId;
      dispatch({
        type: "activate",
        gameId,
        from: context.route.from,
        online: typeof navigator === "undefined" || navigator.onLine,
        restore: context.restoreState,
      });
      requestAnimationFrame(() => {
        if (isActive(context)) restorePageState();
      });
      void loadDetail(context, gameId).then(() => {
        if (isActive(context)) restorePageState();
      });
    },
    deactivate() {
      if (statusTimer) clearTimeout(statusTimer);
      statusTimer = null;
      moreMenuCleanup?.();
      moreMenuCleanup = null;
      const restoreState = toGameDetailRestoreState(state, readScrollTop(), currentFocusKey());
      activation = null;
      return restoreState;
    },
  };
}
