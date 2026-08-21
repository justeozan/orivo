import { invoke } from "@tauri-apps/api/core";
import { WALLPAPER_CATEGORIES } from "./contracts";
import type {
  AppRoute,
  EpicInstallStatus,
  GameDetailView,
  GameMediaKind,
  GameMediaView,
  GameSummary,
  WallpaperCategory,
  WallpaperSearchView,
} from "./contracts";
import { icon } from "./icons";
import { sourceBadge } from "./source-model";
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
  statusChips,
  previewedMedia,
  reduceGameDetailState,
  resolvePrimaryAction,
  shouldOfferAboutToggle,
  shouldRenderSection,
  toGameDetailRestoreState,
  withSampleSocialData,
  type GameDetailPageAction,
  type GameDetailViewModel,
} from "./game-detail-model";

export interface GameDetailPageClient {
  getDetail(gameId: string, signal: AbortSignal): Promise<GameDetailView>;
  setWishlist(
    gameId: string,
    wishlisted: boolean,
    signal: AbortSignal,
  ): Promise<void>;
  selectMedia(
    gameId: string,
    mediaId: string,
    signal: AbortSignal,
  ): Promise<GameMediaView[]>;
  importMedia(
    gameId: string,
    kind: GameMediaKind,
    signal: AbortSignal,
  ): Promise<GameMediaView[]>;
  exportMedia(
    gameId: string,
    mediaId: string,
    signal: AbortSignal,
  ): Promise<void>;
  cancelMediaDownload(gameId: string, signal: AbortSignal): Promise<void>;
  /**
   * One request per row, and no source: the backend asks every provider at
   * once and merges them. The category the caller names is the only shape that
   * comes back, which is what keeps a 16:9 screenshot out of the portrait Cover
   * row.
   */
  searchWallpapers(
    category: WallpaperCategory,
    query: string,
    offset: number,
    signal: AbortSignal,
  ): Promise<WallpaperSearchView>;
  importWallpaper(
    gameId: string,
    candidateId: string,
    signal: AbortSignal,
  ): Promise<GameMediaView[]>;
  openOffer(offerId: string, signal: AbortSignal): Promise<void>;
  /** Ask the local Epic Games Launcher to start downloading an owned game. */
  installEpicGame(gameId: string, signal: AbortSignal): Promise<void>;
  /** Read what that launcher reports for the game right now. */
  epicInstallStatus(
    gameId: string,
    signal: AbortSignal,
  ): Promise<EpicInstallStatus>;
  /** Ask the launcher to remove an installed game; it owns the deletion. */
  uninstallEpicGame(gameId: string, signal: AbortSignal): Promise<void>;
  /** Fetch cover/hero art for a game the same way an import does. */
  searchArtwork(gameId: string, signal: AbortSignal): Promise<void>;
  /** Refill the portrait cover, landscape cover and background from a reliable source. */
  resetArtwork(
    gameId: string,
    signal: AbortSignal,
  ): Promise<ArtworkResetResult>;
  /** Remove a game from the library (does not touch the game's own files). */
  removeGame(gameId: string, signal: AbortSignal): Promise<void>;
  setGameHidden(
    gameId: string,
    hidden: boolean,
    signal: AbortSignal,
  ): Promise<void>;
  /** Promote a chosen media to a game card role: background, cover or landscape. */
  setHomeImage(
    gameId: string,
    mediaId: string,
    role: WallpaperRole,
    signal: AbortSignal,
  ): Promise<void>;
}

/** Which library card slot a chosen wallpaper fills. */
export type WallpaperRole = "background" | "cover" | "landscape" | "logo";

/** What a cover reset actually replaced. A role with no art found is absent. */
export interface ArtworkResetResult {
  title: string;
  replaced: WallpaperRole[];
}

/** Role names as they read in a sentence, for the reset's partial result. */
const ROLE_LABELS: ReadonlyArray<{ role: WallpaperRole; label: string }> = [
  { role: "cover", label: "portrait cover" },
  { role: "landscape", label: "landscape cover" },
  { role: "background", label: "background" },
];

/**
 * The row a tile was ticked in decides the slot it fills. The two vocabularies
 * line up one-to-one, which is what let the "Apply as" picker go: asking again
 * which slot the user meant, right after they picked from a row named after it,
 * was a question with only one sensible answer.
 */
const WALLPAPER_ROLE_FOR_CATEGORY: Record<WallpaperCategory, WallpaperRole> = {
  cover: "cover",
  landscape: "landscape",
  background: "background",
  logo: "logo",
};

/** How long each background holds before the hero cross-fades to the next. */
const HERO_SLIDE_MS = 7000;
/**
 * How often the Epic download percentage is re-measured. A download is minutes
 * long and the measurement walks a directory, so a slow tick keeps the bar
 * honest without turning progress into disk churn.
 */
const EPIC_PROGRESS_INTERVAL_MS = 2000;

/** Confirmation toasts per role. */
const WALLPAPER_ROLE_APPLIED: Record<WallpaperRole, string> = {
  background: "Wallpaper set as your home background.",
  cover: "Portrait cover updated.",
  landscape: "Landscape cover updated.",
  logo: "Logo updated.",
};

/** The label and glyph each wallpaper row and its filter chip carry. */
const WALLPAPER_CATEGORY_META: Record<
  WallpaperCategory,
  { label: string; icon: Parameters<typeof icon>[0] }
> = {
  cover: { label: "Cover", icon: "cover" },
  landscape: { label: "Landscape cover", icon: "landscape" },
  background: { label: "Background", icon: "background" },
  logo: { label: "Logo", icon: "landscape" },
};

/** Tiles a row shows before "Voir tout" expands it to everything it holds. */
const WALLPAPER_ROW_TILES = 5;
/**
 * How many achievements the panel shows as chips. The header already states the
 * full count, so these are a sample sized to stay in view — a game with eighty
 * of them must not turn the panel back into a scroll.
 */
const MAX_ACHIEVEMENT_CHIPS = 8;

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
  /**
   * Debug-only: when this returns true, a loaded game is overlaid with sample
   * achievements, friends and activity for any section it ships empty. Wired to
   * the Settings "Sample social data" toggle.
   */
  sampleSocialEnabled?(): boolean;
  client?: GameDetailPageClient;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted)
    throw new DOMException("The game request was cancelled.", "AbortError");
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
      return invokeWhileActive<GameDetailView>(
        "get_game_detail",
        { gameId },
        signal,
      );
    },
    async setWishlist(gameId, wishlisted, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive(
        "set_game_wishlist",
        { gameId, wishlisted },
        signal,
      );
    },
    async selectMedia(gameId, mediaId, signal) {
      if (!isTauriRuntime()) return [];
      return invokeWhileActive<GameMediaView[]>(
        "select_game_media",
        { gameId, mediaId },
        signal,
      );
    },
    async importMedia(gameId, kind, signal) {
      if (!isTauriRuntime()) return [];
      return invokeWhileActive<GameMediaView[]>(
        "import_game_media",
        { gameId, kind },
        signal,
      );
    },
    async exportMedia(gameId, mediaId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("export_game_media", { gameId, mediaId }, signal);
    },
    async cancelMediaDownload(gameId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("cancel_game_media_download", { gameId }, signal);
    },
    async searchWallpapers(category, query, offset, signal) {
      if (!isTauriRuntime())
        return createFallbackWallpaperSearch(category, query, offset);
      return invokeWhileActive<WallpaperSearchView>(
        "search_wallpapers",
        { category, query, offset },
        signal,
      );
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
    async installEpicGame(gameId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("install_epic_game", { gameId }, signal);
    },
    async uninstallEpicGame(gameId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("uninstall_epic_game", { gameId }, signal);
    },
    async epicInstallStatus(gameId, signal) {
      if (!isTauriRuntime()) {
        return {
          appName: gameId,
          state: "not-installed",
          percent: 0,
          installedBytes: 0,
          totalBytes: 0,
          installPath: null,
        };
      }
      return invokeWhileActive<EpicInstallStatus>(
        "get_epic_install_status",
        { gameId },
        signal,
      );
    },
    async searchArtwork(gameId, signal) {
      if (!isTauriRuntime()) return;
      // The explicit action re-runs the search even if art already exists.
      await invokeWhileActive(
        "fetch_game_artwork",
        { gameId, force: true },
        signal,
      );
    },
    async resetArtwork(gameId, signal) {
      if (!isTauriRuntime()) return { title: "", replaced: [] };
      return invokeWhileActive<ArtworkResetResult>(
        "reset_game_artwork",
        { gameId },
        signal,
      );
    },
    async removeGame(gameId, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("remove_game", { gameId }, signal);
    },
    async setGameHidden(gameId, hidden, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive("set_game_hidden", { gameId, hidden }, signal);
    },
    async setHomeImage(gameId, mediaId, role, signal) {
      if (!isTauriRuntime()) return;
      await invokeWhileActive(
        "set_home_image",
        { gameId, mediaId, role },
        signal,
      );
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

function iconElement(
  name: Parameters<typeof icon>[0],
  className = "",
): HTMLElement {
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
  const message =
    typeof error === "string"
      ? error
      : error instanceof Error
        ? error.message
        : "";
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
  /** The hero media rail's current page (three tiles per page). */
  /** Index into the hero's auto-cycling backgrounds, and its timer. */
  let heroSlide = 0;
  let heroTimer: number | null = null;
  /** Ticks the Epic download percentage while one is running; null otherwise. */
  let epicProgressTimer: ReturnType<typeof setTimeout> | null = null;
  /** The percentage the poll last measured, so the hero chip moves with it. */
  let epicProgressPercent: number | null = null;

  const isActive = (context = activation): context is PageActivation =>
    Boolean(context && context.isCurrent() && !context.signal.aborted);

  /** A response is only painted when it still belongs to the visible game. */
  const isFresh = (context: PageActivation | null, gameId: string): boolean =>
    isActive(context) && state.gameId === gameId;

  const dispatch = (
    action: GameDetailPageAction,
    shouldRender = true,
  ): void => {
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

  const stopEpicProgress = (): void => {
    if (epicProgressTimer) clearTimeout(epicProgressTimer);
    epicProgressTimer = null;
    epicProgressPercent = null;
  };

  /**
   * Poll the launcher while a download runs. Epic never calls back, so the only
   * honest progress is a re-read; once it finishes, the detail is reloaded so
   * the button becomes Play instead of Install.
   */
  const trackEpicInstall = (context: PageActivation, gameId: string): void => {
    stopEpicProgress();
    const tick = async (): Promise<void> => {
      if (!isFresh(context, gameId)) return stopEpicProgress();
      let status: EpicInstallStatus;
      try {
        status = await client.epicInstallStatus(gameId, context.signal);
      } catch {
        // A launcher that will not answer is not a failed install. Stop
        // reporting rather than replacing a running download with an error.
        return stopEpicProgress();
      }
      if (!isFresh(context, gameId)) return stopEpicProgress();
      // Any settled state ends the watch: this same loop follows an uninstall,
      // where "not installed" is the successful outcome rather than a failure.
      if (status.state !== "installing") {
        stopEpicProgress();
        void loadDetail(context, gameId);
        return;
      }
      if (status.state === "installing") {
        dispatch({
          type: "status-changed",
          message: `Downloading… ${status.percent}%`,
        });
      }
      epicProgressTimer = setTimeout(
        () => void tick(),
        EPIC_PROGRESS_INTERVAL_MS,
      );
    };
    epicProgressTimer = setTimeout(
      () => void tick(),
      EPIC_PROGRESS_INTERVAL_MS,
    );
  };

  const loadDetail = async (
    context: PageActivation,
    gameId: string,
  ): Promise<void> => {
    const requestId = ++requestSequence;
    dispatch({ type: "request-started", requestId });
    try {
      const payload = await client.getDetail(gameId, context.signal);
      if (!isFresh(context, gameId)) return;
      let detail = normaliseGameDetail(payload);
      // Debug overlay only: fills empty social sections when the Settings toggle
      // is on, leaving anything the backend actually shipped untouched.
      if (detail && options.sampleSocialEnabled?.())
        detail = withSampleSocialData(detail);
      dispatch({ type: "detail-loaded", requestId, detail });
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      const message = requestErrorMessage(
        error,
        "This game could not be loaded right now.",
      );
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
    const action = resolvePrimaryAction(state.detail, gameId, epicProgressPercent);
    if (action.disabled) return;
    if (action.intent === "play") {
      options.play(gameId);
      return;
    }
    if (action.intent === "navigate" && action.route) {
      options.navigate(action.route);
      return;
    }
    if (action.intent === "install-epic") {
      try {
        await client.installEpicGame(gameId, context.signal);
        if (!isFresh(context, gameId)) return;
        showTransientStatus("Epic Games is starting the download…");
        trackEpicInstall(context, gameId);
      } catch (error) {
        if (!isFresh(context, gameId) || isUserCancellation(error)) return;
        showTransientStatus(
          requestErrorMessage(
            error,
            "Epic Games could not start this download.",
          ),
        );
      }
      return;
    }
    if (action.intent !== "open-offer" || !action.offerId) return;
    try {
      await client.openOffer(action.offerId, context.signal);
      if (isFresh(context, gameId))
        showTransientStatus("Offer opened in your browser.");
    } catch (error) {
      if (!isFresh(context, gameId) || isUserCancellation(error)) return;
      showTransientStatus(
        requestErrorMessage(error, "The offer could not be opened."),
      );
    }
  };

  /**
   * Refill all three artwork roles — portrait cover, landscape cover and
   * background — from a reliable high-resolution source, then repaint.
   *
   * This replaces the old single-image search, which downloaded one picture and
   * used it for every role. That is why a game synced from Xbox or the
   * Microsoft Store ended up with a small square thumbnail everywhere.
   */
  const handleResetArtwork = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (!isActive(context) || !gameId) return;
    showTransientStatus("Fetching fresh covers…");
    try {
      const result = await client.resetArtwork(gameId, context.signal);
      if (!isFresh(context, gameId)) return;
      options.onLibraryChanged?.();
      await loadDetail(context, gameId);
      if (!isFresh(context, gameId)) return;
      // A partial result is stated rather than hidden: a game whose publisher
      // never uploaded a wide capsule keeps the landscape image it had.
      const missing = ROLE_LABELS.filter(
        ({ role }) => !result.replaced.includes(role),
      );
      if (missing.length === 0) {
        clearTransientStatus();
      } else {
        showTransientStatus(
          `Updated, but no ${missing.map(({ label }) => label).join(" or ")} was found.`,
        );
      }
    } catch (error) {
      if (!isFresh(context, gameId) || isUserCancellation(error)) return;
      showTransientStatus(
        requestErrorMessage(error, "No covers were found for this game."),
      );
    }
  };

  /** Remove the game from the library (its own files are left untouched). */
  const handleRemoveGame = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    const title = state.detail?.title ?? "this game";
    if (!isActive(context) || !gameId) return;
    try {
      await client.removeGame(gameId, context.signal);
      if (!isActive(context)) return;
      options.onLibraryChanged?.();
      showTransientStatus(`${title} removed from your library.`);
      options.back();
    } catch (error) {
      if (!isActive(context) || isUserCancellation(error)) return;
      showTransientStatus(
        requestErrorMessage(error, "This game could not be removed."),
      );
    }
  };

  const handleUninstall = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    const title = state.detail?.title ?? "this game";
    if (!isActive(context) || !gameId) return;
    try {
      await client.uninstallEpicGame(gameId, context.signal);
      if (!isFresh(context, gameId)) return;
      showTransientStatus(`${title} has been uninstalled.`);
      // The files are gone by the time this returns, so the page can repaint
      // straight away rather than waiting on a launcher that was never asked.
      options.onLibraryChanged?.();
      void loadDetail(context, gameId);
    } catch (error) {
      if (!isFresh(context, gameId) || isUserCancellation(error)) return;
      showTransientStatus(
        requestErrorMessage(error, "This game could not be uninstalled."),
      );
    }
  };

  const handleHideGame = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    const title = state.detail?.title ?? "this game";
    if (!isActive(context) || !gameId) return;
    try {
      await client.setGameHidden(gameId, true, context.signal);
      if (!isActive(context)) return;
      options.onLibraryChanged?.();
      showTransientStatus(`${title} is hidden from your library.`);
      options.back();
    } catch (error) {
      if (!isActive(context) || isUserCancellation(error)) return;
      showTransientStatus(
        requestErrorMessage(error, "This game could not be hidden."),
      );
    }
  };

  const openWallpaperSearch = (): void => {
    wallpaperSelection.clear();
    dispatch({ type: "wallpaper-search-opened" });
    // Every row is filled straight away, and each from its own scoped request:
    // the three searches are fired together rather than chained, so a slow
    // Cover fetch never holds up the Background row.
    for (const category of WALLPAPER_CATEGORIES) {
      const row = state.wallpaperSearch.categories[category];
      if (!row.busy && row.candidates.length === 0)
        void handleWallpaperSearch(category);
    }
    pageRoot
      ?.querySelector<HTMLElement>("[data-focus-key='wallpaper-modal-close']")
      ?.focus();
  };

  const closeWallpaperSearch = (): void => {
    wallpaperSelection.clear();
    dispatch({ type: "wallpaper-search-closed" });
    // The dialog is opened from the "…" menu now, so focus returns there.
    pageRoot
      ?.querySelector<HTMLElement>("[data-focus-key='more-actions']")
      ?.focus();
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
      if (committed.length > 0)
        dispatch({ type: "media-committed", media: committed });
      else dispatch({ type: "media-busy-changed", busy: false });
      await client.setHomeImage(gameId, mediaId, "background", context.signal);
      if (!isFresh(context, gameId)) return;
      options.onLibraryChanged?.();
      showTransientStatus("Wallpaper set as your home background.");
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      dispatch({
        type: "media-failed",
        message: requestErrorMessage(
          error,
          "That wallpaper could not be set as your background.",
        ),
      });
    }
  };

  /**
   * Fills one row. The category travels all the way to the backend, so the
   * response is already scoped to that shape; nothing is filtered client-side.
   */
  const handleWallpaperSearch = async (
    category: WallpaperCategory,
    more = false,
  ): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (
      !isActive(context) ||
      !gameId ||
      state.wallpaperSearch.categories[category].busy
    )
      return;
    const query = state.wallpaperSearch.query.trim();
    if (!query) return;
    dispatch({ type: "wallpaper-search-started", category, more });
    const offset = state.wallpaperSearch.categories[category].offset;
    try {
      const results = await client.searchWallpapers(
        category,
        query,
        offset,
        context.signal,
      );
      if (!isFresh(context, gameId)) return;
      dispatch({
        type: "wallpaper-search-results",
        category,
        results: normaliseWallpaperSearch(results, category),
      });
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      dispatch({
        type: "wallpaper-search-failed",
        category,
        message: requestErrorMessage(
          error,
          "That search did not finish. Try again.",
        ),
      });
    }
  };

  /**
   * Submitting the form refills every visible row: all three normally, or just
   * the narrowed one while a chip (or "Voir tout") holds the focus.
   */
  /**
   * Refills every row, focus or not. A new source or a new query invalidates
   * all three, and `focus` only decides what is *shown* — a narrowed row is a
   * view, not a fetch scope. Refilling just the focused one would leave the
   * other two holding results for the previous query, or, after a source
   * change wiped them, holding nothing at all with no request in flight.
   */
  const runWallpaperSearches = (): void => {
    for (const category of WALLPAPER_CATEGORIES) {
      void handleWallpaperSearch(category);
    }
  };

  /** Chips and "Voir tout" share one state: narrowing to a row, or clearing it. */
  const setWallpaperFocus = (focus: WallpaperCategory | null): void => {
    dispatch({ type: "wallpaper-search-focus-changed", focus });
    if (!focus) return;
    // A row can be narrowed to before it ever ran (its own request failed to
    // start, or the query only changed afterwards).
    const row = state.wallpaperSearch.categories[focus];
    if (!row.busy && row.phase === "idle" && row.candidates.length === 0) {
      void handleWallpaperSearch(focus);
    }
  };

  /**
   * Applies the ticked tiles. Each one fills the card slot its row stands for,
   * so ticking one tile per row sets cover, landscape and background in a
   * single pass — which is why the dialog has no "Apply as" picker.
   */
  const handleApplySelection = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    if (!isActive(context) || !gameId || state.mediaBusy) return;
    const chosen = allWallpaperSlides().filter((slide) =>
      wallpaperSelection.has(slide.id),
    );
    if (chosen.length === 0) return;
    dispatch({ type: "media-busy-changed", busy: true });
    try {
      // Download every chosen search result so several are saved in one go,
      // keeping each saved id paired with the slot its row asked for.
      const saved: Array<{ mediaId: string; role: WallpaperRole }> = [];
      for (const slide of chosen) {
        const role = WALLPAPER_ROLE_FOR_CATEGORY[slide.category];
        if (slide.candidate) {
          const media = await client.importWallpaper(
            gameId,
            slide.id,
            context.signal,
          );
          if (!isFresh(context, gameId)) return;
          const normalised = normaliseGameMedia(media);
          if (normalised.length > 0)
            dispatch({ type: "media-imported", media: normalised });
          const stored =
            normalised.find((item) => item.selected) ??
            normalised[normalised.length - 1];
          if (stored) saved.push({ mediaId: stored.id, role });
        } else {
          saved.push({ mediaId: slide.id, role });
        }
      }
      // The background pick (or a lone pick of any shape) is what the hero and
      // the Library card paint, so that one is also the committed selection.
      const home =
        saved.find((entry) => entry.role === "background") ?? saved[0];
      if (home) {
        const media = await client.selectMedia(
          gameId,
          home.mediaId,
          context.signal,
        );
        if (!isFresh(context, gameId)) return;
        const committed = normaliseGameMedia(media);
        if (committed.length > 0)
          dispatch({ type: "media-committed", media: committed });
        for (const entry of saved) {
          await client.setHomeImage(
            gameId,
            entry.mediaId,
            entry.role,
            context.signal,
          );
          if (!isFresh(context, gameId)) return;
        }
      } else {
        dispatch({ type: "media-busy-changed", busy: false });
      }
      if (!isFresh(context, gameId)) return;
      options.onLibraryChanged?.();
      wallpaperSelection.clear();
      dispatch({ type: "wallpaper-search-closed" });
      showTransientStatus(
        chosen.length > 1
          ? `${chosen.length} wallpapers applied to this game.`
          : WALLPAPER_ROLE_APPLIED[
              WALLPAPER_ROLE_FOR_CATEGORY[chosen[0].category]
            ],
      );
    } catch (error) {
      if (!isFresh(context, gameId)) return;
      dispatch({
        type: "media-failed",
        message: requestErrorMessage(
          error,
          "Those wallpapers could not be saved.",
        ),
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
      element(
        "span",
        "gd-back__label",
        state.from === "store" ? "Back to Store" : "Back to Library",
      ),
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
      iconElement(
        state.phase === "loading"
          ? "refresh"
          : state.errorMessage
            ? "alert"
            : "navigate",
      ),
      element("span", "gd-status__copy", message || "Loading game details…"),
    );
    return status;
  };

  const renderNotice = (
    title: string,
    copy: string,
    retry: boolean,
  ): HTMLElement => {
    const notice = element("section", "gd-notice");
    notice.setAttribute("role", "status");
    notice.append(iconElement("alert", "gd-notice__icon"));
    notice.append(
      element("h1", "gd-notice__title", title),
      element("p", "gd-notice__copy", copy),
    );
    const actions = element("div", "gd-notice__actions");
    const back = element("button", "gd-button gd-button--ghost", "Go back");
    back.type = "button";
    back.dataset.focusKey = "notice-back";
    back.addEventListener("click", () => options.back());
    actions.append(back);
    if (retry) {
      const again = element(
        "button",
        "gd-button gd-button--primary",
        "Try again",
      );
      again.type = "button";
      again.dataset.focusKey = "notice-retry";
      again.addEventListener("click", () => {
        if (activation && state.gameId)
          void loadDetail(activation, state.gameId);
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
    const descriptor = resolvePrimaryAction(
      detail,
      state.gameId,
      epicProgressPercent,
    );
    const button = element(
      "button",
      "gd-button gd-button--primary gd-primary-action",
    );
    button.type = "button";
    button.dataset.focusKey = "primary-action";
    button.dataset.action = descriptor.kind;
    button.disabled = descriptor.disabled;
    button.title = descriptor.hint;
    button.setAttribute(
      "aria-label",
      `${descriptor.label}: ${descriptor.hint}`,
    );
    // The fill sits under the label rather than replacing it, so the button
    // stays a button — same size, same position — while it doubles as the bar.
    if (typeof descriptor.progress === "number") {
      const fill = element("span", "gd-primary-action__fill");
      fill.style.width = `${Math.min(100, Math.max(0, descriptor.progress))}%`;
      button.append(fill);
      button.dataset.progress = String(descriptor.progress);
      button.setAttribute("role", "progressbar");
      button.setAttribute("aria-valuenow", String(descriptor.progress));
      button.setAttribute("aria-valuemin", "0");
      button.setAttribute("aria-valuemax", "100");
    }
    button.append(
      iconElement(descriptor.icon),
      element("span", "gd-button__label", descriptor.label),
    );
    button.addEventListener("click", () => void handlePrimaryAction());
    return button;
  };

  /** The primary action is a single pill — the old split "Play ▾" chevron is gone. */
  const renderPrimaryActionGroup = (detail: GameDetailViewModel): HTMLElement =>
    renderPrimaryAction(detail);

  /** Optimistic favourite toggle; a failed save flips the button back. */
  const handleWishlist = async (): Promise<void> => {
    const context = activation;
    const gameId = state.gameId;
    const detail = state.detail;
    if (!isActive(context) || !gameId || !detail) return;
    const next = !detail.wishlisted;
    dispatch({ type: "wishlist-changed", wishlisted: next });
    try {
      await client.setWishlist(gameId, next, context.signal);
    } catch (error) {
      if (!isFresh(context, gameId) || isUserCancellation(error)) return;
      dispatch({ type: "wishlist-changed", wishlisted: !next });
      showTransientStatus(
        requestErrorMessage(error, "Your favourites could not be updated."),
      );
    }
  };

  /**
   * Favourites, not a wishlist. The library is what you already own, so
   * "wishlist" never described what this button did. The stored flag keeps its
   * original name for now — renaming a persisted field is a migration, not a
   * label change — but nothing user-facing says "wishlist" any more.
   */
  const renderFavouriteButton = (detail: GameDetailViewModel): HTMLElement => {
    const button = element("button", "gd-button gd-wishlist");
    button.type = "button";
    button.dataset.focusKey = "wishlist";
    button.classList.toggle("gd-wishlist--active", detail.wishlisted);
    button.setAttribute("aria-pressed", String(detail.wishlisted));
    button.setAttribute(
      "aria-label",
      detail.wishlisted
        ? `Remove ${detail.title} from your favourites`
        : `Add ${detail.title} to your favourites`,
    );
    button.append(
      iconElement(detail.wishlisted ? "heart-filled" : "heart"),
      element(
        "span",
        "gd-button__label",
        detail.wishlisted ? "Favourite" : "Favourite",
      ),
    );
    button.addEventListener("click", () => void handleWishlist());
    return button;
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
    const timer = setTimeout(
      () => document.addEventListener("click", onDocClick),
      0,
    );
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
      const entry = element(
        "button",
        `gd-menu__item${danger ? " gd-menu__item--danger" : ""}`,
      );
      entry.type = "button";
      entry.dataset.focusKey = focusKey;
      entry.setAttribute("role", "menuitem");
      entry.append(
        iconElement(iconName),
        element("span", "gd-menu__label", label),
      );
      entry.addEventListener("click", () => {
        closeMoreMenu(button, menu);
        run();
      });
      return entry;
    };

    const remove = element("button", "gd-menu__item gd-menu__item--danger");
    remove.type = "button";
    remove.dataset.focusKey = "menu-remove";
    remove.setAttribute("role", "menuitem");
    const removeLabel = element(
      "span",
      "gd-menu__label",
      "Remove from library",
    );
    remove.append(iconElement("close"), removeLabel);
    let removeArmed = false;
    let uninstallArmed = false;
    let resetUninstall = (): void => {};
    remove.addEventListener("click", () => {
      if (!removeArmed) {
        removeArmed = true;
        remove.classList.add("gd-menu__item--armed");
        removeLabel.textContent = "Confirm removal";
        return;
      }
      closeMoreMenu(button, menu);
      void handleRemoveGame();
    });

    menu.append(
      item(
        "menu-wallpaper",
        "background",
        "Change wallpaper",
        openWallpaperSearch,
      ),
      // "Search cover & images" is deliberately gone: it downloaded one picture
      // and used it as cover, landscape and background at once, which is how
      // games ended up wearing each other's art. "Reset the covers" refills the
      // three roles from their own sources instead.
      item(
        "menu-artwork",
        "refresh",
        "Reset the covers",
        () => void handleResetArtwork(),
      ),
      // Hidden, not removed: the game stays in the catalog with its artwork and
      // play time, it simply stops appearing in the library.
      item("menu-hide", "close", "Hide game", () => void handleHideGame()),
      remove,
    );
    // Only a store that can actually be asked to remove the files gets the
    // entry, and only when there is something installed to remove.
    if (detail.source === "epic" && detail.installState === "installed") {
      // This really does delete the game's files, so it arms first — the same
      // two-step the library removal uses, for the same reason.
      const uninstall = element("button", "gd-menu__item gd-menu__item--danger");
      uninstall.type = "button";
      uninstall.dataset.focusKey = "menu-uninstall";
      uninstall.setAttribute("role", "menuitem");
      const uninstallLabel = element("span", "gd-menu__label", "Uninstall");
      uninstall.append(iconElement("close"), uninstallLabel);
      uninstall.addEventListener("click", () => {
        if (!uninstallArmed) {
          uninstallArmed = true;
          uninstall.classList.add("gd-menu__item--armed");
          uninstallLabel.textContent = "Delete the game files";
          return;
        }
        closeMoreMenu(button, menu);
        void handleUninstall();
      });
      menu.insertBefore(uninstall, remove);
      resetUninstall = () => {
        uninstallArmed = false;
        uninstall.classList.remove("gd-menu__item--armed");
        uninstallLabel.textContent = "Uninstall";
      };
    }

    // Re-opening the menu starts the removal over: an armed danger action must
    // not survive out of sight.
    button.addEventListener("click", () => {
      removeArmed = false;
      remove.classList.remove("gd-menu__item--armed");
      removeLabel.textContent = "Remove from library";
      resetUninstall();
    });

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
      video.setAttribute(
        "aria-label",
        `${preview.title} trailer for ${detail.title}`,
      );
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
    // A background the user is previewing or has applied wins outright; with
    // none chosen, the hero cycles the game's backgrounds on its own.
    attachImage(
      frame,
      image,
      heroSlideshowUrl() ?? heroImageUrl(state),
      "gd-hero__media--missing",
    );
    frame.append(image);
    return frame;
  };

  /**
   * The auto-cycling hero background. It only runs when the user has not
   * singled one out — previewing or applying a wallpaper is an explicit choice,
   * and rotating away from it a second later would undo their intent.
   */
  const heroSlideshowUrl = (): string | null => {
    if (state.previewMediaId || state.appliedMediaId) return null;
    const backgrounds = galleryMedia();
    if (backgrounds.length < 2) return null;
    const item = backgrounds[heroSlide % backgrounds.length];
    return item?.previewUrl ?? item?.posterUrl ?? null;
  };

  const startHeroSlideshow = (): void => {
    stopHeroSlideshow();
    heroTimer = window.setInterval(() => {
      // Re-render only while the slideshow is actually what paints the hero,
      // so a chosen wallpaper never gets repainted underneath the user.
      if (heroSlideshowUrl() === null) return;
      heroSlide += 1;
      render();
    }, HERO_SLIDE_MS);
  };

  const stopHeroSlideshow = (): void => {
    if (heroTimer !== null) {
      window.clearInterval(heroTimer);
      heroTimer = null;
    }
  };

  /**
   * The hero rail shows backgrounds only. It is the picker for the art painted
   * behind the hero, so covers and trailers — which can never fill that slot —
   * would only be noise to scroll past.
   */
  const galleryMedia = (): GameMediaView[] =>
    state.media.filter((item) => item.kind === "wallpaper");

  /**
   * The vertical rail overlapping the hero's top-right: every saved background,
   * scrolled rather than paged. Adding a wallpaper now lives in the "…" menu, so
   * a game with no backgrounds yet has no rail at all.
   */
  const renderGalleryRail = (): HTMLElement | null => {
    const items = galleryMedia();
    if (items.length === 0) return null;

    const rail = element("aside", "gd-gallery");
    rail.setAttribute("aria-label", "Wallpapers");
    const track = element("div", "gd-gallery__track");
    track.setAttribute("role", "group");
    track.setAttribute("aria-label", "Backgrounds");
    for (const item of items) track.append(renderGalleryTile(item));
    rail.append(track);
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
    tile.setAttribute(
      "aria-label",
      `${item.title}${item.availableOffline ? "" : " — needs a download"}`,
    );
    const image = element("img", "gd-gallery__tile-image");
    image.loading = "lazy";
    image.decoding = "async";
    attachImage(
      tile,
      image,
      item.posterUrl ?? item.previewUrl,
      "gd-gallery__tile--missing",
    );
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
    /** The row it was ticked in, which is also the card slot it fills. */
    category: WallpaperCategory;
  }

  /** The candidates one row shows, in the order they were fetched. */
  const categorySlides = (category: WallpaperCategory): WallpaperSlide[] => {
    const slides: WallpaperSlide[] = state.wallpaperSearch.categories[
      category
    ].candidates.map((candidate) => ({
      id: candidate.id,
      title: candidate.title,
      url: candidate.thumbnailUrl,
      candidate: true,
      category,
    }));
    // Wallpapers already saved on this game are backgrounds — that is what the
    // kind means — so they join the Background row and no other.
    if (category === "background") {
      for (const item of mediaForKind(state.media, "wallpaper")) {
        slides.push({
          id: item.id,
          title: item.title,
          url: item.posterUrl ?? item.previewUrl,
          candidate: false,
          category: "background",
        });
      }
    }
    return slides;
  };

  /** Every tile the dialog can show, used to resolve a ticked id back to it. */
  const allWallpaperSlides = (): WallpaperSlide[] =>
    WALLPAPER_CATEGORIES.flatMap((category) => categorySlides(category));

  /** One tick-selectable wallpaper tile; its shape comes from its row. */
  const renderWallpaperTile = (slide: WallpaperSlide): HTMLElement => {
    const tile = element("button", "gd-wallgrid__tile");
    tile.type = "button";
    tile.dataset.focusKey = `wall-${slide.id}`;
    const picked = wallpaperSelection.has(slide.id);
    tile.classList.toggle("gd-wallgrid__tile--selected", picked);
    tile.classList.toggle(
      "gd-wallgrid__tile--current",
      slide.id === state.appliedMediaId,
    );
    tile.setAttribute("aria-pressed", String(picked));
    tile.setAttribute("aria-label", slide.title);
    const image = element("img", "gd-wallgrid__image");
    image.loading = "lazy";
    image.decoding = "async";
    // A candidate that will not load is not a candidate. Several of the Steam
    // rows lead with assets a publisher may never have uploaded
    // (`capsule_616x353`, `page_bg_raw`), so the first tiles of the landscape
    // and background rows were often dead links offered as choices.
    image.addEventListener("error", () => tile.remove());
    attachImage(tile, image, slide.url, "gd-wallgrid__tile--missing");
    const check = element("span", "gd-wallgrid__check");
    check.setAttribute("aria-hidden", "true");
    check.append(iconElement("check"));
    tile.append(image, check);
    tile.addEventListener("click", () => {
      // One pick per row, because a row *is* a slot. Ticking a second image in
      // the same row queued both and the page applied them in turn, so the card
      // ended up wearing whichever download finished last. Picking across rows
      // is still how the three slots are filled in one pass.
      const wasPicked = wallpaperSelection.has(slide.id);
      for (const sibling of categorySlides(slide.category)) {
        wallpaperSelection.delete(sibling.id);
      }
      if (!wasPicked) wallpaperSelection.add(slide.id);
      render();
    });
    return tile;
  };

  /**
   * A row's own busy / empty / error line. It lives inside the row so one
   * failing category never blanks the ones beside it — which matters more now
   * that a row is fed by six sources at once and any of them may be down.
   */
  const renderWallpaperRowStatus = (
    category: WallpaperCategory,
    tiles: number,
  ): HTMLElement | null => {
    const row = state.wallpaperSearch.categories[category];
    if (row.busy) {
      const busy = element(
        "p",
        "gd-wallrow__status gd-search__status",
        "Searching…",
      );
      busy.setAttribute("role", "status");
      return busy;
    }
    if (row.phase === "error") {
      const error = element(
        "p",
        "gd-wallrow__status gd-search__notice gd-search__notice--error",
        row.message || "That search did not finish. Try again.",
      );
      error.setAttribute("role", "alert");
      return error;
    }
    if (row.phase === "ready" && tiles === 0) {
      // The backend's own message is the useful one when a row comes back
      // empty: for a 4K-only row it is usually "add a SteamGridDB key", which
      // "try another query" would talk straight over.
      return element(
        "p",
        "gd-wallrow__status gd-search__notice",
        row.message ||
          `Nothing matched for ${WALLPAPER_CATEGORY_META[category].label.toLowerCase()}.`,
      );
    }
    return null;
  };

  /** The shape filters. The lit one narrows the dialog to its row alone. */
  const renderWallpaperChips = (
    focus: WallpaperCategory | null,
  ): HTMLElement => {
    const chips = element("div", "gd-chips");
    chips.setAttribute("role", "group");
    chips.setAttribute("aria-label", "Filter wallpapers by shape");
    for (const category of WALLPAPER_CATEGORIES) {
      const meta = WALLPAPER_CATEGORY_META[category];
      const active = focus === category;
      const chip = element("button", "gd-chip");
      chip.type = "button";
      chip.dataset.focusKey = `wallpaper-chip-${category}`;
      chip.dataset.category = category;
      chip.classList.toggle("gd-chip--active", active);
      chip.setAttribute("aria-pressed", String(active));
      const check = element("span", "gd-chip__check");
      check.setAttribute("aria-hidden", "true");
      check.append(iconElement("check"));
      chip.append(
        iconElement(meta.icon, "gd-chip__icon"),
        element("span", "gd-chip__label", meta.label),
        check,
      );
      // Clicking the lit chip clears the filter and brings every row back.
      chip.addEventListener("click", () =>
        setWallpaperFocus(active ? null : category),
      );
      chips.append(chip);
    }
    return chips;
  };

  /**
   * One category section: a titled header, then every tile that row holds in a
   * scroller shaped for it.
   *
   * The card shapes scroll sideways and the background scrolls down, which is
   * what puts them in different columns: a 2:3 cover and a 3:1 hero cannot
   * share a rail without one of them being tiny, and a full-bleed background is
   * worth showing large enough to actually judge. There is no "see all" and no
   * five-tile cap any more — a scroller already holds everything, so a control
   * that only revealed the sixth tile had nothing left to do.
   */
  const renderWallpaperRow = (
    category: WallpaperCategory,
    focus: WallpaperCategory | null,
  ): HTMLElement => {
    const meta = WALLPAPER_CATEGORY_META[category];
    const row = state.wallpaperSearch.categories[category];
    const slides = categorySlides(category);

    const section = element("section", "gd-wallrow");
    section.dataset.category = category;
    section.classList.toggle("gd-wallrow--expanded", focus === category);

    const header = element("div", "gd-wallrow__header");
    const title = element("h3", "gd-wallrow__title", meta.label);
    title.id = `gd-wallrow-${category}`;
    header.append(iconElement(meta.icon, "gd-wallrow__icon"), title);
    // Paging is the one control a scroller cannot replace: the tiles it would
    // reveal have not been fetched yet.
    if (row.phase === "ready" && row.hasMore && !row.busy) {
      const more = element("button", "gd-wallrow__more", "Search more");
      more.type = "button";
      more.dataset.focusKey = `wallpaper-more-${category}`;
      more.addEventListener(
        "click",
        () => void handleWallpaperSearch(category, true),
      );
      header.append(more);
    }
    section.append(header);

    const grid = element("div", "gd-wallgrid");
    grid.dataset.category = category;
    grid.setAttribute("role", "group");
    grid.setAttribute("aria-labelledby", title.id);
    for (const slide of slides) grid.append(renderWallpaperTile(slide));
    // Placeholders only while a row is still waiting on its first answer, so
    // the column holds its size instead of snapping open as requests land.
    if (row.busy && slides.length === 0) {
      for (let slot = 0; slot < WALLPAPER_ROW_TILES; slot += 1) {
        const ghost = element("div", "gd-wallgrid__ghost");
        ghost.setAttribute("aria-hidden", "true");
        grid.append(ghost);
      }
    }
    section.append(grid);

    const status = renderWallpaperRowStatus(category, slides.length);
    if (status) section.append(status);
    return section;
  };

  /**
   * Which column a row lives in. The two card shapes and the wordmark are drawn
   * small and read as a strip; the background is the one image that fills a
   * screen, so it gets a column of its own where a tile can be big enough to
   * choose between.
   */
  const WALLPAPER_COLUMN: Record<WallpaperCategory, "cards" | "feed"> = {
    cover: "cards",
    landscape: "cards",
    logo: "cards",
    background: "feed",
  };

  const renderWallpaperModal = (): HTMLElement | null => {
    const search = state.wallpaperSearch;
    if (!search.open) return null;
    const modal = element("section", "gd-modal");
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-label", "Wallpapers");

    const backdrop = element("div", "gd-modal__backdrop");
    backdrop.addEventListener("click", closeWallpaperSearch);

    const dialog = element(
      "div",
      "gd-modal__dialog gd-modal__dialog--wallpapers",
    );

    // The header is the whole set of controls: which shape to look at, and what
    // to look for. There is no source picker — every provider is asked at once,
    // so choosing one was a question with no good answer.
    const header = element("header", "gd-wallhead");
    header.append(renderWallpaperChips(search.focus));

    const form = element("form", "gd-wallhead__search");
    form.setAttribute("role", "search");
    const input = element("input", "gd-search__input") as HTMLInputElement;
    input.type = "text";
    input.placeholder = "Search another title…";
    input.dataset.focusKey = "wallpaper-search-input";
    input.value = search.query;
    input.setAttribute("aria-label", "Wallpaper search query");
    input.addEventListener("input", () =>
      dispatch(
        { type: "wallpaper-search-query-changed", query: input.value },
        false,
      ),
    );
    const anyRowBusy = WALLPAPER_CATEGORIES.some(
      (category) => search.categories[category].busy,
    );
    const submit = element("button", "gd-wallhead__submit");
    submit.type = "submit";
    submit.dataset.focusKey = "wallpaper-search-button";
    submit.setAttribute("aria-label", "Search wallpapers");
    submit.disabled = anyRowBusy || !search.query.trim();
    submit.append(iconElement("search"));
    form.append(input, submit);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      runWallpaperSearches();
    });
    header.append(form);

    const close = element("button", "gd-modal__close");
    close.type = "button";
    close.dataset.focusKey = "wallpaper-modal-close";
    close.setAttribute("aria-label", "Close wallpapers");
    close.append(iconElement("close"));
    close.addEventListener("click", closeWallpaperSearch);
    header.append(close);
    dialog.append(header);

    // Two columns: the card shapes stack on the left as sideways rails, the
    // background runs down the right. A chip collapses this to the one row it
    // names, which then takes the full width.
    const panes = element("div", "gd-wallpanes");
    panes.classList.toggle("gd-wallpanes--single", search.focus !== null);
    const cards = element("div", "gd-wallpanes__cards");
    const feed = element("div", "gd-wallpanes__feed");
    for (const category of WALLPAPER_CATEGORIES) {
      if (search.focus && search.focus !== category) continue;
      const row = renderWallpaperRow(category, search.focus);
      (WALLPAPER_COLUMN[category] === "feed" ? feed : cards).append(row);
    }
    if (cards.childElementCount) panes.append(cards);
    if (feed.childElementCount) panes.append(feed);
    dialog.append(panes);

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

    // No "Apply as" picker: the row a tile was ticked in already says which
    // slot it fills, so one pick per row applies all of them in a single go.
    const actions = element("div", "gd-modal__actions");
    const apply = element(
      "button",
      "gd-button gd-button--primary gd-modal__use",
      picks > 1 ? `Apply ${picks} wallpapers` : "Apply wallpaper",
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

  /**
   * The larger source logo under the title: each store's own mark for a game
   * that came from a connected account, a local-machine glyph for everything
   * living on this device. A logo, not a text badge — the label only exists for
   * assistive tech and the tooltip.
   */
  const renderSourceBadge = (detail: GameDetailViewModel): HTMLElement => {
    const source = sourceBadge(detail.source) ?? {
      label: "Local",
      icon: "local" as const,
    };
    const badge = element("span", "gd-source");
    badge.dataset.source = detail.source;
    badge.setAttribute("role", "img");
    badge.setAttribute("aria-label", `Source: ${source.label}`);
    badge.title = source.label;
    badge.innerHTML = icon(source.icon);
    return badge;
  };

  /**
   * "Not installed" and "Windows only" are the two facts the Play button cannot
   * express, so they get their own row rather than being folded into the meta
   * line — which deliberately strips runtime state.
   */
  const renderStatusChips = (
    detail: GameDetailViewModel,
  ): HTMLElement | null => {
    const chips = statusChips(detail);
    if (chips.length === 0) return null;
    const row = element("div", "gd-chiprow");
    for (const chip of chips) {
      const item = element("span", "gd-chiprow__chip");
      item.dataset.tone = chip.tone;
      item.dataset.chip = chip.id;
      item.append(iconElement(chip.icon, "gd-chiprow__icon"));
      item.append(element("span", "gd-chiprow__label", chip.label));
      row.append(item);
    }
    return row;
  };

  const renderHero = (detail: GameDetailViewModel): HTMLElement => {
    const hero = element("section", "gd-hero");
    hero.append(renderHeroMedia(detail), element("div", "gd-hero__veil"));
    // The back control and media rail float over the art, matching the
    // reference; the copy column is inset to the shared left margin.
    hero.append(renderBackButton());
    const rail = renderGalleryRail();
    if (rail) hero.append(rail);
    const copy = element("div", "gd-hero__copy");
    const title = element("h1", "gd-hero__title", detail.title);
    title.id = "gd-hero-title";
    copy.append(title);
    // The approved design's meta row starts straight at the developer name —
    // no source logo in front of it.
    const subline = element("div", "gd-hero__subline");
    subline.append(renderSourceBadge(detail));
    const metaFacts = buildMetaFacts(detail);
    if (metaFacts.length > 0)
      subline.append(renderFactList("gd-meta", metaFacts, true));
    copy.append(subline);
    const chips = renderStatusChips(detail);
    if (chips) copy.append(chips);
    // No summary here: "About this game" carries it a few hundred pixels below,
    // and printing it twice cost the hero the height its actions needed.
    const actions = element("div", "gd-hero__actions");
    actions.append(
      renderPrimaryActionGroup(detail),
      renderFavouriteButton(detail),
      renderMoreButton(detail),
    );
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
    for (const paragraph of detail.about
      .split(/\n+/)
      .filter((line) => line.trim())) {
      body.append(element("p", "gd-about__paragraph", paragraph));
    }
    section.append(body);
    if (shouldOfferAboutToggle(detail.about)) {
      const toggle = element(
        "button",
        "gd-about__toggle",
        state.aboutExpanded ? "Read less" : "Read more",
      );
      toggle.type = "button";
      toggle.dataset.focusKey = "about-toggle";
      toggle.setAttribute("aria-expanded", String(state.aboutExpanded));
      toggle.append(iconElement("chevron-down"));
      toggle.addEventListener("click", () =>
        dispatch({ type: "about-toggled" }),
      );
      section.append(toggle);
    }
    return section;
  };

  /** Maps a supported platform to its brand glyph for the Game info row. */
  const platformIcon = (
    platform: GameSummary["supportedPlatforms"][number],
  ): Parameters<typeof icon>[0] =>
    platform === "windows"
      ? "windows"
      : platform === "macos" || platform === "ios"
        ? "monitor"
        : "monitor";

  const renderGameInfo = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-info");
    section.append(element("h2", "gd-panel__title", "Game info"));
    const list = element("dl", "gd-info__list");
    // Store comes first and is always known: a game Orivo can show came from
    // somewhere. Without it this panel vanished entirely for any title whose
    // provider publishes no developer, publisher, date, genre or platform —
    // which is most of a Microsoft Store or local library.
    const rows: Array<[string, string | null]> = [
      ["Store", sourceBadge(detail.source)?.label ?? "This Mac"],
      ["Developer", detail.developer],
      ["Publisher", detail.publisher],
      [
        "Release date",
        detail.releaseDate ? formatReleaseDate(detail.releaseDate) : null,
      ],
      ["Genre", detail.genres.length > 0 ? detail.genres.join(", ") : null],
    ];
    for (const [label, value] of rows) {
      if (!value) continue;
      list.append(
        element("dt", "gd-info__term", label),
        element("dd", "gd-info__value", value),
      );
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
      item.append(
        iconElement(featureIcon(feature)),
        element("span", "gd-features__label", feature),
      );
      list.append(item);
    }
    section.append(list);
    return section;
  };

  const renderAchievements = (detail: GameDetailViewModel): HTMLElement => {
    const section = element("section", "gd-panel gd-achievements");

    // Header row: the title on the left, the meter in the top-right corner.
    // The panel used to stack title, count, bar and list down a column, which
    // needed more height than a third of the screen has — so most of it was
    // behind a scroll. Everything it says now says itself at once.
    const header = element("div", "gd-achievements__header");
    header.append(element("h2", "gd-panel__title", "Achievements"));
    const progress = formatAchievementProgress(detail.achievements);
    if (progress) {
      const gauge = element("div", "gd-achievements__gauge");
      const meter = element("div", "gd-achievements__meter");
      meter.setAttribute("role", "progressbar");
      meter.setAttribute("aria-valuemin", "0");
      meter.setAttribute("aria-valuemax", "100");
      meter.setAttribute("aria-valuenow", String(progress.percent));
      meter.setAttribute(
        "aria-label",
        `${progress.label} (${progress.percent}%)`,
      );
      const fill = element("span", "gd-achievements__fill");
      fill.style.width = `${progress.percent}%`;
      meter.append(fill);
      gauge.append(
        element("span", "gd-achievements__count", progress.label),
        meter,
        element("span", "gd-achievements__percent", `${progress.percent}%`),
      );
      header.append(gauge);
    }
    section.append(header);

    const items = detail.achievements?.items ?? [];
    if (items.length > 0) {
      // Across, not down: a horizontal row of named chips fits the panel's
      // shape, and the panel is wider than it is tall.
      // Capped at what the panel can hold in view. The header already states
      // the full count, so the chips are a sample and never a scroll.
      const list = element("ul", "gd-achievements__list");
      for (const item of items.slice(0, MAX_ACHIEVEMENT_CHIPS)) {
        const chip = element("li", "gd-achievements__chip");
        const glyph = element("span", "gd-icon gd-achievements__glyph");
        glyph.setAttribute("aria-hidden", "true");
        glyph.innerHTML = icon("trophy");
        chip.append(
          glyph,
          element("span", "gd-achievements__name", item.title),
        );
        chip.title = item.title;
        list.append(chip);
      }
      section.append(list);
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
      item.title = friend.status
        ? `${friend.name} — ${friend.status}`
        : friend.name;
      const image = element("img", "gd-friends__avatar");
      image.loading = "lazy";
      image.alt = friend.name;
      if (friend.avatarUrl) {
        image.src = friend.avatarUrl;
        image.addEventListener("error", () =>
          item.classList.add("gd-friends__item--missing"),
        );
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
    for (const entry of (detail.activity ?? []).slice(0, 6)) {
      const item = element("li", "gd-activity__item");
      const head = element("div", "gd-activity__head");
      const image = element("img", "gd-activity__avatar");
      image.loading = "lazy";
      image.alt = "";
      if (entry.avatarUrl) {
        image.src = entry.avatarUrl;
        image.addEventListener("error", () =>
          head.classList.add("gd-activity__head--missing"),
        );
      } else {
        head.classList.add("gd-activity__head--missing");
      }
      const copy = element("div", "gd-activity__copy");
      copy.append(element("p", "gd-activity__actor", entry.actorName));
      copy.append(element("p", "gd-activity__summary", entry.summary));
      if (entry.detail)
        copy.append(element("p", "gd-activity__detail", entry.detail));
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
    button.dataset.navOpen = game.id;
    button.setAttribute("aria-label", `Open ${game.title}`);
    const frame = element("span", "gd-related__media");
    const image = element("img", "gd-related__cover");
    image.loading = "lazy";
    // The reference's related tiles are landscape art with the title baked in,
    // so the card is the framed image alone — the name lives in the aria-label.
    attachImage(
      frame,
      image,
      game.landscapeUrl || game.coverUrl,
      "gd-related__media--missing",
    );
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
    for (const game of detail.relatedGames.slice(0, 8))
      list.append(renderRelatedCard(game));
    section.append(list);
    return section;
  };

  const renderDetail = (detail: GameDetailViewModel): DocumentFragment => {
    const fragment = document.createDocumentFragment();
    fragment.append(renderHero(detail));

    // One grid, not two stacked rows. The page is a fixed frame now: the hero
    // takes the top band and every panel shares what is left, so a game's
    // information is seen rather than scrolled to. A panel with more than fits
    // scrolls inside its own card.
    const body = element("div", "gd-body");
    if (shouldRenderSection(detail, "about")) body.append(renderAbout(detail));
    // The design's raised zone is two glass cards: Game info + Features share
    // one (split by a hairline), Achievements stands in its own.
    const facts = element("div", "gd-infocard gd-infocard--facts");
    if (shouldRenderSection(detail, "info"))
      facts.append(renderGameInfo(detail));
    if (shouldRenderSection(detail, "features"))
      facts.append(renderFeatures(detail));
    if (facts.childElementCount > 0) body.append(facts);
    const trophies = element("div", "gd-infocard gd-infocard--achievements");
    if (shouldRenderSection(detail, "achievements"))
      trophies.append(renderAchievements(detail));
    if (trophies.childElementCount > 0) body.append(trophies);
    if (shouldRenderSection(detail, "friends"))
      body.append(renderFriends(detail));
    if (shouldRenderSection(detail, "activity"))
      body.append(renderActivity(detail));
    if (shouldRenderSection(detail, "related"))
      body.append(renderRelated(detail));
    if (body.childElementCount > 0) fragment.append(body);

    const modal = renderWallpaperModal();
    if (modal) fragment.append(modal);
    return fragment;
  };

  const currentFocusKey = (): string | null => {
    const active = document.activeElement;
    return active instanceof HTMLElement && pageRoot?.contains(active)
      ? (active.dataset.focusKey ?? null)
      : null;
  };

  const focusByKey = (focusKey: string | null): void => {
    if (!pageRoot || !focusKey) return;
    const target = [
      ...pageRoot.querySelectorAll<FocusableElement>("[data-focus-key]"),
    ].find((candidate) => candidate.dataset.focusKey === focusKey);
    target?.focus();
  };

  /**
   * The shell host (`.app-page--scroll`) is the scroll container, so scroll
   * position lives there — the page root itself never scrolls. Both nodes are
   * read and written so the page keeps working when it is mounted into a host
   * that does not scroll.
   */
  const readScrollTop = (): number =>
    Math.max(pageRoot?.scrollTop ?? 0, container?.scrollTop ?? 0);

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
        renderNotice(
          "Game details unavailable",
          state.errorMessage || "Something went wrong.",
          true,
        ),
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
    // Only a loaded game gets the fixed frame. Loading, error and not-found are
    // short, centred states that would look stranded stretched over a viewport.
    pageRoot.classList.toggle("gd-page--fit", Boolean(state.detail));
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
    if (activation && state.gameId && !state.detail)
      void loadDetail(activation, state.gameId);
  };
  const onOffline = (): void =>
    dispatch({ type: "connectivity-changed", online: false });

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
      heroSlide = 0;
      startHeroSlideshow();
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
      stopEpicProgress();
      stopHeroSlideshow();
      moreMenuCleanup?.();
      moreMenuCleanup = null;
      const restoreState = toGameDetailRestoreState(
        state,
        readScrollTop(),
        currentFocusKey(),
      );
      activation = null;
      return restoreState;
    },
  };
}
