import { getTauriVersion, getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  AppRoute,
  PageRestoreState,
  ProviderStatus,
  SettingsSection,
  WallpaperCredentials,
  WallpaperCredentialsUpdate,
} from "./contracts";
import { createGameDetailPage } from "./game-detail-page";
import { brandIcon, icon, type IconName } from "./icons";
import {
  type BrowseMode,
  type BrowseSegment,
  browseGames,
  browseModeLabel,
  browseSegments,
  formatLastPlayed,
  nextBrowseMode,
  resolveSegment,
} from "./library-browse";
import { isTauriRuntime, primeMediaDirectory, resolveMediaUrl, resolveMediaUrlSync } from "./media";
import { fallbackLibrary, formatPlayTime, type LibraryGame } from "./mock-library";
import type {
  ConnectedSource,
  SourceAccountStatus,
  SourceSyncResult,
  StoreProvider,
} from "./contracts";
import {
  CONNECTED_SOURCES,
  connectedSourceDescriptor,
  defaultSourceAccounts,
  isConnectedSource,
  normaliseSourceAccounts,
  normaliseSourceSyncResult,
  sourceBadge,
  sourceStatusLine,
  sourceSyncSummary,
} from "./source-model";
import { type AppPage, PageLifecycleHost } from "./page-lifecycle";
import { HashRouter } from "./router";
import {
  DEFAULT_PREFERENCES,
  EMPTY_WALLPAPER_CREDENTIALS,
  SETTINGS_SECTIONS,
  type DataUsage,
  type MotionPreference,
  type Preferences,
  type PreferencesUpdate,
  type StartPage,
  type StoreRegion,
  defaultProviderStatuses,
  formatDataSize,
  formatFreshness,
  normaliseDataUsage,
  normalisePreferences,
  normaliseProviderStatuses,
  normaliseWallpaperCredentials,
} from "./settings-model";
import {
  INITIAL_UPDATE_STATE,
  type UpdateState,
  applyCheckResult,
  applyError,
  applyProgress,
  describeUpdateState,
  markReady,
  startCheck,
  startDownload,
  updateProgressPercent,
} from "./updater-model";
import { createMePage } from "./me-page";
import {
  createDefaultPluginManagerClient,
  createPluginManagerController,
  formatInstallLabel,
  formatPluginSize,
  formatPluginStatus,
  isPluginInstallBusy,
  pluginErrorMessage,
  pluginPercent,
  type AvailablePluginView,
  type InstalledPluginView,
} from "./plugin-manager";
import { createDefaultQuikyClient } from "./quiky-install";
import { createStorePage } from "./store-page";
import { composedTarget, createSpatialNav, isTypingEvent } from "./spatial-nav";
import { createGamepadBridge } from "./gamepad";
import { attachFeedbackTo, initErrorReporting } from "./sentry";
import "./game-detail-page.css";
import "./me-page.css";
import "./store-page.css";

type BackendRecord = Record<string, unknown>;

// The updater plugin is only ever loaded from inside the click handler, so the
// browser preview and the test runner never execute it. `typeof import(...)` is
// a type position: it is erased at build time and pulls nothing into the
// bundle, which is what lets these handles stay exactly typed anyway.
type UpdaterModule = typeof import("@tauri-apps/plugin-updater");
type UpdateHandle = NonNullable<Awaited<ReturnType<UpdaterModule["check"]>>>;

type SteamPreviewStatus = "available" | "unavailable" | "error";
type SteamPanelPhase = "idle" | "scanning" | SteamPreviewStatus | "importing";
type SteamNoticeTone = "success" | "error" | "info";
type SteamAccountPhase =
  | "idle"
  | "loading"
  | "disconnected"
  | "connecting"
  | "api-key"
  | "saving-api-key"
  | "connected"
  | "syncing"
  | "error";

interface SteamPreviewGame {
  appId: string;
  title: string;
  locationLabel: string;
  lastUpdated: string;
  selected: boolean;
  alreadyImported: boolean;
  coverUrl: string;
  heroUrl: string;
}

interface SteamPreview {
  status: SteamPreviewStatus;
  libraries: number;
  games: SteamPreviewGame[];
  message: string;
}

interface SteamImportResult {
  importedIds: string[];
  updatedIds: string[];
  skippedAppIds: string[];
}

interface SteamPreviewMedia {
  appId: string;
  coverUrl: string;
  heroUrl: string;
}

interface SteamAccountStatus {
  connected: boolean;
  steamId: string;
  method: "web" | "api_key" | "";
}

interface SteamAccountSyncResult {
  totalGames: number;
  importedGames: number;
  updatedGames: number;
  installedGames: number;
}

type WineRunnerState = "checking" | "ready" | "unavailable" | "invalid" | "error";
type LaunchFeedbackPhase = "launching" | "started" | "failed";

interface WineRunnerStatus {
  state: WineRunnerState;
  available: boolean;
  version: string;
  message: string;
}

interface WineDirectory {
  id: string;
  label: string;
}

interface WineProfile {
  id: string;
  displayName: string;
  enabled: boolean;
  wineLabel: string;
  directories: WineDirectory[];
  lastImport: string;
  lastImportSummary: string;
  graphicsBackend: "wine_d3d" | "dxvk_macos";
  graphicsSummary: string;
}

interface WineSettingsState {
  loading: boolean;
  runner: WineRunnerStatus | null;
  profiles: WineProfile[];
  notice: string;
  noticeTone: SteamNoticeTone;
  pendingDeleteProfileId: string;
}

/** The built-in plugins Orivo ships with; the chevron opens their detail view. */
type PluginId = "wine" | "wallpaper-searcher";
/** `list` shows the plugin browser; a PluginId shows one plugin's detail view. */
type PluginView = "list" | PluginId;

interface LaunchFeedback {
  gameId: string;
  phase: LaunchFeedbackPhase;
  message: string;
}

interface SteamAccountConnectedEvent {
  steamId: string;
}

interface WineLaunchStatusEvent {
  gameId: string;
  phase: "preparing" | "started" | "failed";
  message: string;
}

interface LibraryMediaTokens {
  heroUrl: string;
  coverUrl: string;
  landscapeUrl: string;
  logoUrl: string;
}

interface LibraryLoad {
  games: LibraryGame[];
  mediaTokens: Map<string, LibraryMediaTokens>;
}

interface NormalisedLibraryGame {
  game: LibraryGame;
  mediaTokens: LibraryMediaTokens;
}

interface SteamPanelState {
  open: boolean;
  phase: SteamPanelPhase;
  preview: SteamPreview | null;
  selectedAppIds: Set<string>;
  query: string;
  notice: string;
  noticeTone: SteamNoticeTone;
}

interface SteamAccountState {
  open: boolean;
  phase: SteamAccountPhase;
  status: SteamAccountStatus | null;
  notice: string;
  noticeTone: SteamNoticeTone;
  lastSync: SteamAccountSyncResult | null;
  apiKeySteamId: string;
}

/**
 * Per-store UI state for the connected libraries. `busy` is what the store's
 * own row awaits, so signing into Epic never greys out the GOG row next to it.
 */
interface SourceAccountsState {
  loading: boolean;
  statuses: SourceAccountStatus[];
  busy: Set<ConnectedSource>;
  lastSync: Map<ConnectedSource, SourceSyncResult>;
  notice: string;
  noticeTone: SteamNoticeTone;
  noticeProvider: ConnectedSource | null;
  pendingDisconnect: ConnectedSource | null;
}

interface BrowseState {
  mode: BrowseMode;
  /** The segment last chosen in each mode, so cycling back restores the view. */
  segments: Partial<Record<BrowseMode, string>>;
  /** Rage: the same library, wearing the spiral. */
  rage: boolean;
}

interface State {
  games: LibraryGame[];
  libraryMediaTokens: Map<string, LibraryMediaTokens>;
  selectedId: string;
  query: string;
  browse: BrowseState;
  libraryMenuOpen: boolean;
  steam: SteamPanelState;
  steamAccount: SteamAccountState;
  sourceAccounts: SourceAccountsState;
  wineSettings: WineSettingsState;
  launchFeedback: LaunchFeedback | null;
  preferences: Preferences;
  dataUsage: DataUsage;
  providerStatuses: ProviderStatus[];
  settingsSearch: string;
  pluginView: PluginView;
  pluginCatalogSearch: string;
  wallpaperCredentials: WallpaperCredentials;
  wallpaperCredentialsSaving: boolean;
  update: UpdateState;
}

export interface MountAppOptions {
  storePage?: AppPage;
  mePage?: AppPage;
  gameDetailPage?: AppPage;
}

const lastUsedFallback = fallbackLibrary[0];
const MAX_RENDERED_STEAM_GAMES = 120;
const MAX_STEAM_PREVIEW_MEDIA = 16;
const MAX_STEAM_IMPORT_SELECTION = 2_000;
const MAX_AUTOMATIC_STEAM_SELECTION = 50;
const MAX_RENDERED_LIBRARY_CARDS = 48;
// Hydration covers everything the library actually renders. Capping it below
// the rendered window left later cards showing a placeholder indefinitely,
// because nothing re-runs hydration for a card that is already on screen.
const MAX_LIBRARY_MEDIA_HYDRATION = 64;
/**
 * How often the library re-reads the Epic launcher's manifests while a download
 * runs. A download is minutes long and measuring it walks a directory, so a
 * slow tick keeps the percentage honest without turning progress into churn.
 */
const INSTALL_WATCH_MS = 2500;
/**
 * How long a watch started by an explicit Install click keeps looking before
 * giving up: the Epic launcher shows a folder-choice dialog first, so the user
 * may take a while to actually start the transfer.
 */
const INSTALL_WATCH_GRACE_TICKS = 48;
/** How long the automatic update check waits for the shell to go quiet. */
const AUTOMATIC_UPDATE_CHECK_DELAY_MS = 4_000;
const STEAM_ACCOUNT_CONNECTED_EVENT = "steam-account-authenticated";
const STEAM_ACCOUNT_LOGIN_CANCELLED_EVENT = "steam-account-login-cancelled";
const STEAM_ACCOUNT_LOGIN_FAILED_EVENT = "steam-account-login-failed";
const STEAM_ACCOUNT_LOGIN_PENDING_EVENT = "steam-account-login-pending";
const SOURCE_ACCOUNT_CONNECTED_EVENT = "source-account-authenticated";
const SOURCE_ACCOUNT_LOGIN_CANCELLED_EVENT = "source-account-login-cancelled";
const SOURCE_ACCOUNT_LOGIN_FAILED_EVENT = "source-account-login-failed";
const SOURCE_LIBRARY_SYNCED_EVENT = "source-library-synced";
const WINE_LAUNCH_STATUS_EVENT = "wine-launch-status";

export function mountApp(root: HTMLElement, options: MountAppOptions = {}): void {
  const state: State = {
    games: fallbackLibrary.map((game) => ({ ...game })),
    libraryMediaTokens: new Map(),
    selectedId: fallbackLibrary[0].id,
    query: "",
    browse: { mode: "activity", segments: {}, rage: false },
    libraryMenuOpen: false,
    steam: {
      open: false,
      phase: "idle",
      preview: null,
      selectedAppIds: new Set(),
      query: "",
      notice: "",
      noticeTone: "info",
    },
    steamAccount: {
      open: false,
      phase: "idle",
      status: null,
      notice: "",
      noticeTone: "info",
      lastSync: null,
      apiKeySteamId: "",
    },
    sourceAccounts: {
      loading: false,
      statuses: defaultSourceAccounts(),
      busy: new Set(),
      lastSync: new Map(),
      notice: "",
      noticeTone: "info",
      noticeProvider: null,
      pendingDisconnect: null,
    },
    wineSettings: {
      loading: false,
      runner: null,
      profiles: [],
      notice: "",
      noticeTone: "info",
      pendingDeleteProfileId: "",
    },
    launchFeedback: null,
    preferences: { ...DEFAULT_PREFERENCES },
    dataUsage: { derivedCacheBytes: 0, derivedCacheEntries: 0, refreshedAt: null },
    providerStatuses: defaultProviderStatuses(),
    settingsSearch: "",
    pluginView: "list",
    pluginCatalogSearch: "",
    wallpaperCredentials: { ...EMPTY_WALLPAPER_CREDENTIALS },
    wallpaperCredentialsSaving: false,
    update: { ...INITIAL_UPDATE_STATE },
  };

  // The `Update` handle returned by the last successful check. It owns the
  // download, so it has to survive between the two button presses.
  let pendingUpdate: UpdateHandle | null = null;

  root.innerHTML = shell();

  const get = <T extends Element>(selector: string): T => {
    const element = root.querySelector<T>(selector);
    if (!element) {
      throw new Error(`Missing selector element: ${selector}`);
    }
    return element;
  };

  const refs = {
    topbar: get<HTMLElement>(".topbar"),
    feedbackButton: get<HTMLButtonElement>("#feedback-button"),
    heroLayers: [get<HTMLImageElement>("#hero-a"), get<HTMLImageElement>("#hero-b")],
    genre: get<HTMLElement>("#hero-genre"),
    title: get<HTMLElement>("#hero-title"),
    logo: get<HTMLImageElement>("#hero-logo"),
    playTime: get<HTMLElement>("#hero-play-time"),
    lastPlayed: get<HTMLElement>("#hero-last-played"),
    source: get<HTMLElement>("#hero-source"),
    sourceIcon: get<HTMLElement>("#hero-source-icon"),
    sourceLabel: get<HTMLElement>("#hero-source-label"),
    sourceDivider: get<HTMLElement>("#hero-source-divider"),
    metadata: get<HTMLElement>("#hero-metadata"),
    platform: get<HTMLElement>("#hero-platform"),
    status: get<HTMLElement>("#hero-status"),
    platformLabel: get<HTMLElement>("#hero-platform-label"),
    cards: get<HTMLElement>("#game-cards"),
    railTitle: get<HTMLElement>("#recently-played-title"),
    browseSegments: get<HTMLElement>("#browse-segments"),
    browseMode: get<HTMLButtonElement>("#browse-mode"),
    browseModeLabel: get<HTMLElement>("#browse-mode-label"),
    rageToggle: get<HTMLButtonElement>("#rage-toggle"),
    rageLabel: get<HTMLElement>("#rage-label"),
    brandMarkRing: get<HTMLElement>("#brand-mark-ring"),
    brandMarkSpiral: get<HTMLElement>("#brand-mark-spiral"),
    search: get<HTMLInputElement>("#topbar-search"),
    libraryMenu: get<HTMLElement>("#library-source-menu"),
    libraryMenuButton: get<HTMLButtonElement>("#library-menu-button"),
    librarySourceList: get<HTMLElement>("#library-source-list"),
    toast: get<HTMLElement>("#toast"),
    steamPanel: get<HTMLElement>("#steam-import-panel"),
    steamBody: get<HTMLElement>("#steam-import-body"),
    steamFooter: get<HTMLElement>("#steam-import-footer"),
    steamSelectionSummary: get<HTMLElement>("#steam-selection-summary"),
    steamImportButton: get<HTMLButtonElement>("#steam-import-selected"),
    steamRefresh: get<HTMLButtonElement>("#steam-refresh"),
    steamAccountPanel: get<HTMLElement>("#steam-account-panel"),
    steamAccountBody: get<HTMLElement>("#steam-account-body"),
    steamSourceRow: get<HTMLElement>("#steam-source-row"),
    sourceAccountsPanel: get<HTMLElement>("#source-accounts-panel"),
    sourceAccountsBody: get<HTMLElement>("#source-accounts-body"),
    playButton: get<HTMLButtonElement>("#play-button"),
    launchFeedback: get<HTMLElement>("#launch-feedback"),
    wineSettingsPanel: get<HTMLElement>("#wine-settings-panel"),
    wineSettingsBody: get<HTMLElement>("#wine-settings-body"),
    pluginsCatalogPanel: get<HTMLElement>("#plugins-catalog-panel"),
    pluginsInstalledList: get<HTMLElement>("#plugins-installed-list"),
    pluginsCatalogList: get<HTMLElement>("#plugins-catalog-list"),
    pluginsCatalogSearch: get<HTMLInputElement>("#plugins-catalog-search"),
    pluginsCatalogEmpty: get<HTMLElement>("#plugins-catalog-empty"),
    wallpaperPluginPanel: get<HTMLElement>("#wallpaper-plugin-panel"),
    wallpaperCredentialsSave: get<HTMLButtonElement>("#wallpaper-credentials-save"),
    wallpaperIgdbClientId: get<HTMLInputElement>("#wallpaper-igdb-client-id"),
    wallpaperIgdbClientSecret: get<HTMLInputElement>("#wallpaper-igdb-client-secret"),
    wallpaperGoogleApiKey: get<HTMLInputElement>("#wallpaper-google-api-key"),
    wallpaperGoogleCseId: get<HTMLInputElement>("#wallpaper-google-cse-id"),
    wallpaperSteamGridDbApiKey: get<HTMLInputElement>("#wallpaper-steamgriddb-api-key"),
    wallpaperSearchTermCover: get<HTMLInputElement>("#wallpaper-search-term-cover"),
    wallpaperSearchTermLandscape: get<HTMLInputElement>("#wallpaper-search-term-landscape"),
    wallpaperSearchTermBackground: get<HTMLInputElement>("#wallpaper-search-term-background"),
    wallpaperSearchTermLogo: get<HTMLInputElement>("#wallpaper-search-term-logo"),
    libraryPage: get<HTMLElement>("#app-page-library"),
    storePage: get<HTMLElement>("#app-page-store"),
    mePage: get<HTMLElement>("#app-page-me"),
    gamePage: get<HTMLElement>("#app-page-game"),
    settingsPage: get<HTMLElement>("#app-page-settings"),
    notFoundPage: get<HTMLElement>("#app-page-not-found"),
    notFoundDetail: get<HTMLElement>("#not-found-detail"),
    settingsTitle: get<HTMLElement>("#settings-page-title"),
    settingsDescription: get<HTMLElement>("#settings-page-description"),
    settingsPanels: Array.from(root.querySelectorAll<HTMLElement>("[data-settings-panel]")),
    settingsSectionButtons: Array.from(
      root.querySelectorAll<HTMLButtonElement>("[data-settings-section]"),
    ),
    navLinks: Array.from(root.querySelectorAll<HTMLButtonElement>(".primary-nav [data-nav-page]")),
  };

  let activeHero = 0;
  let heroRequest = 0;
  // One decoded wordmark is remembered per URL, so walking back along the rail
  // costs a set lookup rather than another load.
  let heroLogoSource = "";
  const heroLogosReady = new Set<string>();
  const heroLogoWaiters = new Map<string, Array<() => void>>();
  let toastTimer: number | undefined;
  let steamRequest = 0;
  let libraryRequest = 0;
  /** Ticks the library while an Epic download runs; null when nothing is. */
  let installWatchTimer: number | null = null;
  const pendingLibraryMediaIds = new Map<string, number>();
  const pendingSteamPreviewMediaIds = new Map<string, number>();
  let steamPreviewMediaRefreshQueued = false;
  let settingsRequest = 0;
  let currentRoute: AppRoute = { page: "library" };

  // A single router owns every route change. Pages never write the hash
  // directly: they call `navigate`, the router emits, and the shell decides
  // which page host activates.
  const router = new HashRouter();
  const navigate = (route: AppRoute, options: { replace?: boolean } = {}): void => {
    router.navigate(route, options);
  };
  const openGameDetail = (gameId: string): void => {
    navigate({ page: "game", gameId, from: "library" });
  };

  /** The segments the browse bar can offer for the library as it stands now. */
  const currentSegments = (): BrowseSegment[] => browseSegments(state.games, state.browse.mode);

  /** The segment on screen: the remembered one while it still exists. */
  const currentSegmentId = (): string =>
    resolveSegment(currentSegments(), state.browse.segments[state.browse.mode]);

  const visibleGames = (): LibraryGame[] => {
    const term = state.query.trim().toLocaleLowerCase();
    const searched = term
      ? state.games.filter((game) =>
          [game.title, game.genre, game.description, game.metadata]
            .join(" ")
            .toLocaleLowerCase()
            .includes(term),
        )
      : state.games;

    // Searching is the stronger intent: a query looks through the whole
    // library rather than through the segment that happens to be selected.
    if (term) {
      return searched;
    }

    return browseGames(searched, state.browse.mode, currentSegmentId());
  };

  const railGames = (games: LibraryGame[]): LibraryGame[] => {
    if (games.length <= MAX_RENDERED_LIBRARY_CARDS) {
      return games;
    }

    const selectedIndex = Math.max(0, games.findIndex((game) => game.id === state.selectedId));
    const start = Math.min(
      Math.max(0, selectedIndex - Math.floor(MAX_RENDERED_LIBRARY_CARDS / 2)),
      games.length - MAX_RENDERED_LIBRARY_CARDS,
    );
    return games.slice(start, start + MAX_RENDERED_LIBRARY_CARDS);
  };

  const selectedGame = (): LibraryGame => {
    const visible = visibleGames();
    return (
      visible.find((game) => game.id === state.selectedId) ??
      visible[0] ??
      state.games.find((game) => game.id === state.selectedId) ??
      state.games[0] ??
      lastUsedFallback
    );
  };

  const showToast = (message: string): void => {
    refs.toast.textContent = message;
    refs.toast.classList.add("is-visible");
    window.clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => refs.toast.classList.remove("is-visible"), 3_800);
  };

  const renderLaunchFeedback = (): void => {
    const feedback = state.launchFeedback;
    const isForSelection = feedback?.gameId === selectedGame().id;
    refs.launchFeedback.hidden = !feedback || !isForSelection;
    refs.launchFeedback.replaceChildren();
    if (!feedback || !isForSelection) {
      return;
    }

    refs.launchFeedback.className = "launch-feedback launch-feedback--" + feedback.phase;
    refs.launchFeedback.setAttribute("role", feedback.phase === "failed" ? "alert" : "status");
    refs.launchFeedback.setAttribute("aria-live", feedback.phase === "failed" ? "assertive" : "polite");

    const copy = document.createElement("span");
    copy.textContent = feedback.message;
    refs.launchFeedback.append(copy);

    if (feedback.phase === "failed") {
      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "launch-feedback__retry";
      retry.dataset.launchAction = "retry";
      retry.dataset.gameId = feedback.gameId;
      retry.textContent = "Retry in compatibility mode";
      refs.launchFeedback.append(retry);
    }
  };

  const steamAssetUrl = (game: LibraryGame, asset: string): string => {
    const appId = /^steam:(\d+)$/.exec(game.id)?.[1];
    return appId ? `https://cdn.cloudflare.steamstatic.com/steam/apps/${appId}/${asset}` : "";
  };

  const assignCardImage = (
    image: HTMLImageElement | null,
    source: string,
    fallback: string,
    eager: boolean,
  ): void => {
    if (!image) {
      return;
    }
    const initialSource = source || fallback;
    if (!initialSource || image.getAttribute("src") === initialSource) {
      return;
    }

    let usedFallback = initialSource === fallback;
    image.onerror = () => {
      if (!usedFallback && fallback) {
        usedFallback = true;
        image.src = fallback;
        return;
      }
      image.removeAttribute("src");
    };
    image.loading = eager ? "eager" : "lazy";
    image.src = initialSource;
  };

  /**
   * The wordmark, when the store published one.
   *
   * The image is decoded off-screen before anything on screen changes. Setting
   * the visible <img>'s src directly meant the title came back for the length
   * of every load, so walking the rail was a strobe of titles with the
   * occasional logo — the artwork was fine, the swap was not.
   *
   * The title is not a placeholder to be swapped out and forgotten either: it
   * stays in the markup, holds the accessible name, and is what stands when a
   * game has no wordmark, when a CDN 404s, and when an image decodes to
   * nothing.
   */
  const updateHeroLogo = (game: LibraryGame): void => {
    const source = game.logoUrl ?? "";
    // The artwork is the identity, not a counter: one selection is rendered
    // several times over, and a slower logo for a game already left behind
    // must never land on the one now on screen.
    heroLogoSource = source;

    if (!source) {
      showHeroTitle();
      return;
    }
    if (refs.logo.getAttribute("src") === source && !refs.logo.hidden) {
      return;
    }
    // Until this one is ready the title stands: showing the previous game's
    // wordmark over the new game's artwork would be worse than showing none.
    showHeroTitle();

    if (heroLogosReady.has(source)) {
      showHeroLogo(source, game.title);
      return;
    }
    preloadHeroLogo(source, () => {
      if (heroLogoSource === source) showHeroLogo(source, game.title);
    });
  };

  /**
   * Decode one wordmark and remember that it is good. A second caller for an
   * image already in flight waits on the same decode, which is what lets the
   * neighbour pass and the selection itself ask for the same picture without
   * racing.
   */
  const preloadHeroLogo = (source: string, done?: () => void): void => {
    if (!source || heroLogosReady.has(source)) {
      done?.();
      return;
    }
    const waiting = heroLogoWaiters.get(source);
    if (waiting) {
      if (done) waiting.push(done);
      return;
    }
    heroLogoWaiters.set(source, done ? [done] : []);

    const probe = new Image();
    probe.decoding = "async";
    probe.onload = () => {
      const waiters = heroLogoWaiters.get(source) ?? [];
      heroLogoWaiters.delete(source);
      if (probe.naturalWidth === 0) return;
      heroLogosReady.add(source);
      for (const waiter of waiters) waiter();
    };
    probe.onerror = () => heroLogoWaiters.delete(source);
    probe.src = source;
  };

  /** The wordmarks either side of the selection, decoded while nothing else is. */
  const primeNeighbourLogos = (): void => {
    const games = visibleGames();
    const index = games.findIndex((game) => game.id === state.selectedId);
    if (index < 0) return;
    const idle = window.requestIdleCallback ?? window.setTimeout;
    idle(() => {
      for (const neighbour of [games[index - 1], games[index + 1], games[index + 2]]) {
        if (neighbour?.logoUrl) preloadHeroLogo(neighbour.logoUrl);
      }
    });
  };

  const showHeroLogo = (source: string, title: string): void => {
    refs.logo.src = source;
    refs.logo.alt = title;
    refs.logo.hidden = false;
    refs.title.hidden = true;
  };

  const showHeroTitle = (): void => {
    refs.logo.hidden = true;
    refs.title.hidden = false;
  };

  /** The artwork a game is shown by, and the artwork its colour comes from. */
  const heroSourceFor = (game: LibraryGame): string =>
    game.heroUrl || game.coverUrl || steamAssetUrl(game, "header.jpg");

  const updateHeroImage = (game: LibraryGame, immediate = false): void => {
    const fallback = steamAssetUrl(game, "header.jpg");
    const source = heroSourceFor(game);
    const current = refs.heroLayers[activeHero];

    if (current.getAttribute("src") === source) {
      return;
    }

    if (immediate) {
      let usedFallback = source === fallback;
      current.onerror = () => {
        if (!usedFallback && fallback) {
          usedFallback = true;
          current.src = fallback;
          return;
        }
        current.removeAttribute("src");
      };
      current.src = source;
      current.classList.add("is-active");
      refs.heroLayers[1 - activeHero].classList.remove("is-active");
      return;
    }

    const request = ++heroRequest;
    const nextIndex = 1 - activeHero;
    const next = refs.heroLayers[nextIndex];
    const reveal = () => {
      if (request !== heroRequest) {
        return;
      }
      next.classList.add("is-active");
      current.classList.remove("is-active");
      activeHero = nextIndex;
    };

    let usedFallback = source === fallback;
    next.onload = reveal;
    next.onerror = () => {
      if (!usedFallback && fallback) {
        usedFallback = true;
        next.src = fallback;
        return;
      }
      next.removeAttribute("src");
      reveal();
    };
    next.src = source;

    if (next.complete && next.naturalWidth > 0) {
      reveal();
    }
  };

  /**
   * The browse bar and the rail heading. Both are derived from the same two
   * pieces of state — the mode and its segment — so the row of segments, the
   * heading above the cards and the cards themselves can never drift apart.
   */
  const renderBrowseBar = (): void => {
    const segments = currentSegments();
    const activeId = currentSegmentId();
    const active = segments.find((segment) => segment.id === activeId) ?? segments[0];

    // The accessible name keeps the visible word intact, so "click Activity"
    // still hits this button under speech control.
    refs.browseModeLabel.textContent = browseModeLabel(state.browse.mode);
    refs.browseMode.setAttribute(
      "aria-label",
      `Browsing by ${browseModeLabel(state.browse.mode)}, press to change`,
    );

    const existing = Array.from(
      refs.browseSegments.querySelectorAll<HTMLButtonElement>(".browse-bar__segment"),
    );
    // Every selection redraws this bar, so the row is only rebuilt when the
    // segments themselves changed. Re-appending a button detaches it first,
    // which would drop the focus a controller or the keyboard is holding.
    const matchesCurrentOrder =
      existing.length === segments.length &&
      existing.every((button, index) => button.dataset.segmentId === segments[index].id);

    if (matchesCurrentOrder) {
      for (const [index, segment] of segments.entries()) {
        syncSegmentButton(existing[index], segment, segment.id === active?.id);
      }
    } else {
      const byId = new Map(existing.map((button) => [button.dataset.segmentId ?? "", button]));
      const fragment = document.createDocumentFragment();
      for (const segment of segments) {
        const button = byId.get(segment.id) ?? createSegmentButton();
        syncSegmentButton(button, segment, segment.id === active?.id);
        fragment.append(button);
      }
      refs.browseSegments.replaceChildren(fragment);
    }

    // The heading names the shelf on screen; a search overrides it, because
    // what is on the rail then is the search, not the segment.
    const heading = state.query.trim() ? "Search results" : (active?.label ?? "Library");
    refs.railTitle.textContent = heading;
    refs.cards.setAttribute("aria-label", heading);
  };

  const createSegmentButton = (): HTMLButtonElement => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "browse-bar__segment";

    const label = document.createElement("span");
    label.className = "browse-bar__segment-label";

    button.append(label);
    button.addEventListener("click", () => {
      const id = button.dataset.segmentId;
      if (id) selectSegment(id);
    });
    return button;
  };

  const syncSegmentButton = (
    button: HTMLButtonElement,
    segment: BrowseSegment,
    selected: boolean,
  ): void => {
    button.dataset.segmentId = segment.id;
    button.classList.toggle("is-active", selected);
    button.setAttribute("aria-pressed", String(selected));
    // A genre or a store name comes from the backend, so it is set as text and
    // never interpolated into markup.
    const label = button.querySelector<HTMLElement>(".browse-bar__segment-label");
    if (label) label.textContent = segment.label;
  };

  /**
   * Changing what the rail holds moves the hero with it: the selection is only
   * kept when the game is still on the new shelf, otherwise the shelf's first
   * game takes over rather than leaving the hero showing something the rail no
   * longer offers.
   */
  const applyBrowseChange = (): void => {
    const games = visibleGames();
    if (!games.some((game) => game.id === state.selectedId)) {
      state.selectedId = games[0]?.id ?? state.selectedId;
    }
    renderSelection();
    refs.cards.scrollTo?.({ left: 0, behavior: prefersReducedMotion() ? "auto" : "smooth" });
  };

  const selectSegment = (id: string): void => {
    if (currentSegmentId() === id) return;
    state.browse.segments[state.browse.mode] = id;
    applyBrowseChange();
  };

  const cycleBrowseMode = (): void => {
    state.browse.mode = nextBrowseMode(state.browse.mode);
    applyBrowseChange();
  };

  /**
   * Rage mode. It changes no data and filters nothing: the mark becomes the
   * spiral and the accent turns, and that is the whole feature for now.
   */
  const setRageMode = (rage: boolean): void => {
    state.browse.rage = rage;
    if (rage) document.body.dataset.mood = "rage";
    else delete document.body.dataset.mood;
    refs.rageToggle.setAttribute("aria-checked", String(rage));
    refs.rageToggle.classList.toggle("is-active", rage);
    refs.rageLabel.textContent = rage ? "Rage" : "Orivo";
    refs.brandMarkRing.hidden = rage;
    refs.brandMarkSpiral.hidden = !rage;

    // The spiral arrives spinning. Restarting the animation needs the class off,
    // a reflow, then the class on — the same three steps the hero title uses,
    // because re-adding a class the element already wears animates nothing.
    if (rage && !prefersReducedMotion()) {
      refs.brandMarkSpiral.classList.remove("brand-mark--arriving");
      void refs.brandMarkSpiral.offsetWidth;
      refs.brandMarkSpiral.classList.add("brand-mark--arriving");
    } else {
      refs.brandMarkSpiral.classList.remove("brand-mark--arriving");
    }
  };

  const renderCards = (): void => {
    const games = railGames(visibleGames());

    if (games.length === 0) {
      if (!refs.cards.querySelector(".rail-empty")) {
        const empty = document.createElement("p");
        empty.className = "rail-empty";
        empty.textContent = "No games match that search.";
        refs.cards.replaceChildren(empty);
      }
      return;
    }

    refs.cards.querySelector(".rail-empty")?.remove();

    const cardsById = new Map(
      Array.from(refs.cards.querySelectorAll<HTMLButtonElement>(".game-card")).map((card) => [
        card.dataset.gameId,
        card,
      ]),
    );
    const wanted = new Set(games.map((game) => game.id));

    // A card that leaves the rendered window goes, and nothing else is touched.
    // Rebuilding the whole rail used to detach every card, which cancels its
    // transitions: past the 48-card window the cover stopped growing from
    // portrait to landscape on selection, because the browser never saw the
    // class change happen — the node arrived already wearing the result.
    for (const [id, card] of cardsById) {
      if (!wanted.has(id ?? "")) {
        card.remove();
        cardsById.delete(id);
      }
    }

    // Whatever survived kept its relative order, so this walk only ever
    // inserts, and a card that stays put is never moved.
    let cursor = refs.cards.firstElementChild;
    for (const [index, game] of games.entries()) {
      const card = cardsById.get(game.id) ?? createGameCard();
      if (cursor === card) {
        cursor = cursor.nextElementSibling;
      } else {
        refs.cards.insertBefore(card, cursor);
      }
      syncGameCard(card, game, index, game.id === state.selectedId);
    }
  };

  /**
   * The two facts the Play button cannot carry: whether the game is on this
   * machine, and whether it runs natively here. The meta row deliberately
   * strips runtime state, so these get their own row.
   */
  const renderHeroStatus = (game: LibraryGame): void => {
    const chips: Array<{ label: string; icon: IconName; tone: string }> = [];
    // Only "installed" earns a chip. "Not installed" and "downloading" are
    // already what the Play button says — the first as its Install label, the
    // second as the bar filling behind it.
    if (game.installState === "installed") {
      chips.push({ label: "Installed", icon: "check", tone: "ready" });
    }
    if (game.macCompatibility === "native") {
      chips.push({ label: "Mac native", icon: "check", tone: "ready" });
    } else if (game.macCompatibility === "not-native") {
      chips.push({ label: "Windows only", icon: "windows", tone: "warn" });
    }

    refs.status.hidden = chips.length === 0;
    refs.status.replaceChildren();
    for (const chip of chips) {
      const item = document.createElement("span");
      item.className = "hero-status__chip";
      item.dataset.tone = chip.tone;
      item.innerHTML = icon(chip.icon);
      const label = document.createElement("span");
      label.textContent = chip.label;
      item.append(label);
      refs.status.append(item);
    }
  };

  const createGameCard = (): HTMLButtonElement => {
    const card = document.createElement("button");
    card.type = "button";
    card.className = "game-card";
    let pressedWhileDeployed = false;

    const media = document.createElement("span");
    media.className = "card-media";

    const portrait = document.createElement("img");
    portrait.className = "card-art card-art--portrait";
    portrait.alt = "";
    portrait.decoding = "async";

    const landscape = document.createElement("img");
    landscape.className = "card-art card-art--landscape";
    landscape.alt = "";
    landscape.decoding = "async";

    media.append(portrait, landscape);

    // The art stands alone: no name overlay, no darkening gradient and no
    // playtime stamp. Everything about the game is in the hero above the rail,
    // and the title lives in the aria-label.
    card.append(media);
    card.addEventListener("focus", () => {
      const id = card.dataset.gameId;
      if (id && currentRoute.page === "library") selectGame(id, false);
    });
    card.addEventListener("mousedown", () => {
      const id = card.dataset.gameId;
      pressedWhileDeployed = id !== undefined && id === state.selectedId;
    });
    card.addEventListener("click", () => {
      const id = card.dataset.gameId;
      if (!id) return;
      if (pressedWhileDeployed) {
        // A second click on the deployed card opens its detail page.
        // Launching a game stays exclusive to the Play button so a
        // mis-click never starts a download or a Wine prefix.
        openGameDetail(id);
      } else {
        // First click just deploys the card; the user confirms with a
        // second click before navigation happens.
        selectGame(id, false);
      }
    });
    return card;
  };

  const syncGameCard = (
    card: HTMLButtonElement,
    game: LibraryGame,
    index: number,
    selected: boolean,
  ): void => {
    const portrait = card.querySelector<HTMLImageElement>(".card-art--portrait");
    const landscape = card.querySelector<HTMLImageElement>(".card-art--landscape");
    const portraitSource = game.coverUrl || game.heroUrl;
    const landscapeSource = game.landscapeUrl || game.heroUrl || portraitSource;
    const fallback = steamAssetUrl(game, "header.jpg");

    card.dataset.gameId = game.id;
    // Spatial navigation verbs: A opens the game's page, Enter starts it.
    card.dataset.navOpen = game.id;
    card.dataset.navLaunch = game.id;
    card.classList.toggle("is-selected", selected);
    card.setAttribute("aria-pressed", String(selected));
    card.setAttribute("aria-label", `Open details for ${game.title}`);

    assignCardImage(portrait, portraitSource, fallback, index < 7);
    assignCardImage(landscape, landscapeSource, fallback, index < 7);
  };

  const hydrateLibraryMedia = (candidates: LibraryGame[]): void => {
    const request = libraryRequest;
    const seenIds = new Set<string>();
    const targets: Array<{ id: string; tokens: LibraryMediaTokens }> = [];

    for (const game of candidates) {
      if (targets.length >= MAX_LIBRARY_MEDIA_HYDRATION || seenIds.has(game.id)) {
        continue;
      }
      seenIds.add(game.id);
      const tokens = state.libraryMediaTokens.get(game.id);
      if (
        !tokens ||
        (!tokens.heroUrl && !tokens.coverUrl && !tokens.landscapeUrl && !tokens.logoUrl) ||
        pendingLibraryMediaIds.has(game.id)
      ) {
        continue;
      }
      pendingLibraryMediaIds.set(game.id, request);
      targets.push({ id: game.id, tokens });
    }

    if (targets.length === 0) {
      return;
    }

    void Promise.all(
      targets.map(async ({ id, tokens }) => {
        const [heroUrl, coverUrl, landscapeUrl, logoUrl] = await Promise.all([
          tokens.heroUrl ? resolveMediaUrl(tokens.heroUrl) : Promise.resolve(""),
          tokens.coverUrl ? resolveMediaUrl(tokens.coverUrl) : Promise.resolve(""),
          tokens.landscapeUrl ? resolveMediaUrl(tokens.landscapeUrl) : Promise.resolve(""),
          tokens.logoUrl ? resolveMediaUrl(tokens.logoUrl) : Promise.resolve(""),
        ]);
        return { id, heroUrl, coverUrl, landscapeUrl, logoUrl };
      }),
    )
      .then((resolved) => {
        if (request !== libraryRequest) {
          return;
        }
        const mediaById = new Map(resolved.map((media) => [media.id, media]));
        let changed = false;
        for (const game of state.games) {
          const media = mediaById.get(game.id);
          if (!media) {
            continue;
          }
          const heroUrl = media.heroUrl || game.heroUrl;
          const coverUrl = media.coverUrl || game.coverUrl;
          const landscapeUrl = media.landscapeUrl || game.landscapeUrl;
          const logoUrl = media.logoUrl || game.logoUrl || "";
          if (
            heroUrl !== game.heroUrl ||
            coverUrl !== game.coverUrl ||
            landscapeUrl !== game.landscapeUrl ||
            logoUrl !== game.logoUrl
          ) {
            game.heroUrl = heroUrl;
            game.coverUrl = coverUrl;
            game.landscapeUrl = landscapeUrl;
            game.logoUrl = logoUrl;
            changed = true;
          }
        }
        if (changed) {
          renderSelection();
        }
      })
      .catch(() => {
        // A cache token is optional presentation data. Keep the stable
        // fallback artwork if a single asset cannot be resolved.
      })
      .finally(() => {
        for (const { id } of targets) {
          if (pendingLibraryMediaIds.get(id) === request) {
            pendingLibraryMediaIds.delete(id);
          }
        }
      });
  };

  const renderSelection = (immediateHero = false): void => {
    const game = selectedGame();
    const titleChanged = refs.title.textContent !== game.title;
    if (!visibleGames().some((candidate) => candidate.id === state.selectedId)) {
      state.selectedId = game.id;
    }

    refs.genre.textContent = game.genre || "Library";
    refs.title.textContent = game.title;
    updateHeroLogo(game);
    if (titleChanged && !immediateHero && !prefersReducedMotion()) {
      refs.title.classList.remove("is-changing");
      void refs.title.offsetWidth;
      refs.title.classList.add("is-changing");
    }
    refs.playTime.textContent = formatPlayTime(game.playTimeSeconds);
    if (refs.playTime.parentElement) refs.playTime.parentElement.hidden = game.playTimeSeconds <= 0;
    // A game that was never launched shows no "last played" chip at all rather
    // than a "Not played yet" placeholder. Connectors hand back a raw instant
    // ("2024-02-26T19:22:09.4406448Z"), which the hero never prints as-is.
    const lastPlayed = formatLastPlayed(game.lastPlayedAt);
    refs.lastPlayed.textContent = lastPlayed ? `Last played ${lastPlayed}` : "";
    if (refs.lastPlayed.parentElement) refs.lastPlayed.parentElement.hidden = !game.lastPlayedAt;
    const isSteamInstallable = game.source === "steam" && !game.launchable;
    // Epic is the one connected store that will accept an install request from
    // Orivo, so it is the one that gets a live Install button here.
    const isEpicInstallable =
      game.source === "epic" && game.installState === "not-installed";
    const isEpicInstalling = game.installState === "installing";
    renderHeroStatus(game);
    // Every source the catalog can return has a badge, so a game synced from a
    // connected store is never mislabelled as a local one.
    const badge = sourceBadge(game.source);
    const sourceName = badge?.label ?? "";
    const hasSource = sourceName !== "";
    // The Play button already states runnability (Play / Install / Unavailable),
    // so the meta row never repeats runtime, install or compatibility mentions
    // ("Wine-Staging", "installed", "incompatible…").
    const cleanMetadata = (game.metadata || "")
      .split("·")
      .map((part) => part.trim())
      .filter(
        (part) =>
          part !== "" &&
          !/steam|wine|installed|ready to play|incompatible|compatible|macos|windows only/i.test(part),
      )
      .join(" · ");
    // Keep the badge only while it carries something; a lone source glyph with
    // no label (or an empty metadata run) never appears.
    refs.source.hidden = !hasSource && !cleanMetadata;
    refs.sourceIcon.hidden = !hasSource;
    // A real, legible source logo: Steam's mark, the Windows mark for Wine
    // titles, a folder for local games, each connected store's own mark.
    refs.sourceIcon.innerHTML = badge ? icon(badge.icon) : "";
    refs.sourceLabel.hidden = !hasSource;
    refs.sourceLabel.textContent = sourceName;
    refs.metadata.textContent = cleanMetadata;
    refs.metadata.hidden = !cleanMetadata;
    refs.sourceDivider.hidden = !(hasSource && cleanMetadata);
    // Compatibility is conveyed by the Play button, so the platform chip stays
    // hidden.
    refs.platform.hidden = true;
    refs.platformLabel.textContent = "";
    // A download already running is not a second install to start, so the
    // button reports it rather than offering to queue another.
    refs.playButton.disabled =
      !game.launchable &&
      !isSteamInstallable &&
      !isEpicInstallable &&
      !isEpicInstalling;
    // The button doubles as the progress bar, so it keeps its size and place
    // rather than being swapped for a separate control mid-download.
    const fill = refs.playButton.querySelector<HTMLElement>(".play-button__fill");
    if (fill) {
      const percent =
        isEpicInstalling && typeof game.installPercent === "number"
          ? Math.min(100, Math.max(0, game.installPercent))
          : null;
      fill.hidden = percent === null;
      fill.style.width = percent === null ? "0%" : `${percent}%`;
      refs.playButton.classList.toggle("is-downloading", percent !== null);
    }
    refs.playButton.setAttribute(
      "aria-label",
      game.launchable
        ? "Play " + game.title
        : isSteamInstallable
          ? "Install " + game.title + " in Steam"
          : isEpicInstallable
            ? "Install " + game.title + " from Epic Games"
            : isEpicInstalling
              ? "Downloading " + game.title
              : game.title + " is unavailable",
    );
    const playLabel = refs.playButton.querySelector<HTMLElement>("span");
    if (playLabel) {
      playLabel.textContent = game.launchable
        ? "Play"
        : isSteamInstallable || isEpicInstallable
          ? "Install"
          : isEpicInstalling
            ? typeof game.installPercent === "number"
              ? `Downloading ${game.installPercent}%`
              : "Downloading"
            : "Unavailable";
    }
    updateHeroImage(game, immediateHero);
    primeNeighbourLogos();
    renderBrowseBar();
    renderCards();
    renderLaunchFeedback();
    hydrateLibraryMedia([game, ...railGames(visibleGames())]);
  };

  const selectGame = (id: string, scroll = true): void => {
    if (!state.games.some((game) => game.id === id)) {
      return;
    }

    state.selectedId = id;
    renderSelection();

    if (scroll) {
      requestAnimationFrame(() => {
        Array.from(refs.cards.querySelectorAll<HTMLElement>(".game-card"))
          .find((card) => card.dataset.gameId === id)
          ?.scrollIntoView({
            behavior: prefersReducedMotion() ? "auto" : "smooth",
            block: "nearest",
            inline: "nearest",
          });
      });
    }
  };

  const moveSelection = (direction: 1 | -1): void => {
    const games = visibleGames();
    if (games.length === 0) {
      return;
    }

    const currentIndex = Math.max(0, games.findIndex((game) => game.id === state.selectedId));
    const nextIndex = (currentIndex + direction + games.length) % games.length;
    selectGame(games[nextIndex].id);
  };

  const closeLibraryMenu = (): void => {
    state.libraryMenuOpen = false;
    refs.libraryMenu.hidden = true;
    refs.libraryMenuButton.setAttribute("aria-expanded", "false");
    refs.topbar.classList.remove("is-library-menu-open");
  };

  const libraryMenuItems = (): HTMLButtonElement[] =>
    Array.from(refs.libraryMenu.querySelectorAll<HTMLButtonElement>("[role='menuitem']:not(:disabled)"));

  const focusLibraryMenuItem = (edge: "first" | "last"): void => {
    const items = libraryMenuItems();
    (edge === "first" ? items[0] : items.at(-1))?.focus();
  };

  // The Steam account status only loads once Settings has been visited, so the
  // library itself is the reliable witness that a Steam source is connected.
  const steamSourceConnected = (): boolean =>
    state.steamAccount.status?.connected === true ||
    state.games.some((game) => game.source === "steam");

  // Same rule for every other store: a synced game is proof the account was
  // connected, even before Settings has loaded its status list.
  const renderLibrarySources = (): void => {
    const list = refs.librarySourceList;
    list.replaceChildren();

    if (steamSourceConnected()) {
      const steam = document.createElement("button");
      steam.type = "button";
      steam.className = "library-source-action";
      steam.setAttribute("role", "menuitem");
      steam.dataset.libraryAction = "source-steam";
      steam.innerHTML =
        `<span class="library-source-action__icon library-source-action__icon--library" aria-hidden="true">${icon("steam")}</span>` +
        `<span class="library-source-action__copy"><strong>Steam</strong><small>Connected · import installed games</small></span>` +
        icon("chevron-right", "library-source-action__chevron");
      list.append(steam);
    }

    // Connected stores are deliberately not listed here. They live in
    // Settings › Libraries & Sources, which is where they can actually be
    // managed; repeating them in this menu made it long without adding an
    // action beyond "sync now".

  };

  const setLibraryMenuOpen = (open: boolean, focus?: "first" | "last", restoreFocus = false): void => {
    state.libraryMenuOpen = open;
    refs.libraryMenu.hidden = !open;
    refs.libraryMenuButton.setAttribute("aria-expanded", String(open));
    refs.topbar.classList.toggle("is-library-menu-open", open);
    if (open) {
      renderLibrarySources();
    }

    if (open && focus) {
      requestAnimationFrame(() => focusLibraryMenuItem(focus));
    } else if (!open && restoreFocus) {
      refs.libraryMenuButton.focus();
    }
  };

  /**
   * Epic never calls back while it downloads, so Orivo measures the launcher's
   * own manifests itself. Refreshing the library re-reads them, which is what
   * makes the percentage move — including for a download the user started in
   * the Epic launcher rather than here.
   */
  const startInstallWatch = (graceTicks = 0): void => {
    if (installWatchTimer !== null) return;
    // Epic takes a few seconds to write its pending manifest and create the
    // folder, so a watch started by an explicit Install click waits that out
    // rather than concluding after one empty tick that nothing is happening.
    let remainingGrace = graceTicks;
    const tick = async (): Promise<void> => {
      installWatchTimer = null;
      await refreshLibrary();
      const inFlight = state.games.some(
        (game) => game.installState === "installing",
      );
      if (inFlight) remainingGrace = 0;
      // Stop as soon as nothing is in flight: an idle library must not keep
      // re-reading the disk every few seconds forever.
      if (inFlight || remainingGrace-- > 0) {
        installWatchTimer = window.setTimeout(
          () => void tick(),
          INSTALL_WATCH_MS,
        );
      }
    };
    installWatchTimer = window.setTimeout(() => void tick(), INSTALL_WATCH_MS);
  };

  const stopInstallWatch = (): void => {
    if (installWatchTimer !== null) window.clearTimeout(installWatchTimer);
    installWatchTimer = null;
  };

  const refreshLibrary = async (importedId?: string): Promise<void> => {
    // A library refresh mutates state every page reads, not just the Library
    // DOM, so it is deliberately *not* gated on the Library page still being
    // active: an import that lands after the user clicked Store must still be
    // applied. The generation check stays, so an older load never clobbers a
    // newer one.
    const request = ++libraryRequest;
    const library = await loadLibrary();
    if (request !== libraryRequest || !library) {
      return;
    }

    state.games = library.games;
    state.libraryMediaTokens = library.mediaTokens;
    pendingLibraryMediaIds.clear();
    state.selectedId = library.games.some((game) => game.id === importedId)
      ? importedId!
      : library.games.some((game) => game.id === state.selectedId)
        ? state.selectedId
        : library.games[0]?.id ?? lastUsedFallback.id;
    renderSelection();
    // A download started in the Epic launcher is just as real as one started
    // here, so the watch follows the data rather than the click.
    if (state.games.some((game) => game.installState === "installing")) {
      startInstallWatch();
    } else {
      stopInstallWatch();
    }
    // A pending Wine attachment cannot resolve its game until the library has
    // arrived, so the Wine plugin detail re-renders as soon as it does.
    if (state.pluginView === "wine") {
      renderWineSettingsPanel();
    }
  };

  const importGame = async (): Promise<void> => {
    closeLibraryMenu();
    if (!isTauriRuntime()) {
      showToast("Import is available in the Orivo desktop app.");
      return;
    }

    try {
      const result = await invoke<unknown>("import_game");
      const importedId = readImportedId(result);
      await refreshLibrary(importedId);
      if (importedId) {
        showToast("Game added to your library.");
        // A manual import rarely ships its own artwork, so look one up in the
        // background and refresh the card once it lands.
        void invoke("fetch_game_artwork", { gameId: importedId, force: false })
          .then(() => refreshLibrary(importedId))
          .catch(() => {
            // Best-effort: a game with no online match keeps its placeholder.
          });
      }
    } catch (error) {
      showToast(messageFromError(error, "Could not import this game."));
    }
  };

  const wineActionButton = (
    action: string,
    label: string,
    className = "steam-secondary-button",
    iconName?: "folder" | "refresh" | "close" | "monitor" | "download" | "search",
  ): HTMLButtonElement => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = className;
    button.dataset.wineAction = action;
    if (iconName) {
      button.innerHTML = icon(iconName) + "<span>" + label + "</span>";
    } else {
      button.textContent = label;
    }
    return button;
  };

  const appendWineNotice = (parent: HTMLElement, message: string, tone: SteamNoticeTone): void => {
    if (!message) {
      return;
    }
    const notice = document.createElement("p");
    notice.className = "steam-notice steam-notice--" + tone;
    notice.setAttribute("role", tone === "error" ? "alert" : "status");
    notice.textContent = message;
    parent.append(notice);
  };

  const appendWineLoadingState = (
    parent: HTMLElement,
    headingText: string,
    messageText: string,
  ): void => {
    const loading = document.createElement("section");
    loading.className = "steam-state steam-state--loading";
    loading.setAttribute("aria-live", "polite");
    const spinner = document.createElement("span");
    spinner.className = "steam-spinner";
    spinner.setAttribute("aria-hidden", "true");
    const heading = document.createElement("h2");
    heading.textContent = headingText;
    const message = document.createElement("p");
    message.textContent = messageText;
    loading.append(spinner, heading, message);
    parent.append(loading);
  };

  const runnerStatusLabel = (runner: WineRunnerStatus | null): string => {
    if (!runner || runner.state === "checking") {
      return "Checking Wine-Staging…";
    }
    if (runner.state === "ready") {
      return runner.version ? "Ready · " + runner.version : "Ready";
    }
    if (runner.state === "unavailable") {
      return "Wine-Staging not found";
    }
    if (runner.state === "invalid") {
      return "Invalid Wine installation";
    }
    return "Wine-Staging unavailable";
  };

  const appendWineRunnerSummary = (
    parent: HTMLElement,
    runner: WineRunnerStatus | null,
    // Inside the Wine-Staging settings card the surrounding header already says
    // "Wine-Staging", so the summary names what it reports instead of repeating
    // the card title.
    headingText = "Wine-Staging",
  ): void => {
    const overview = document.createElement("section");
    overview.className = "wine-runner-summary";
    const mark = document.createElement("span");
    mark.className = "steam-state__icon";
    mark.innerHTML = icon(runner?.state === "ready" ? "monitor" : "alert");
    mark.setAttribute("aria-hidden", "true");
    const copy = document.createElement("div");
    const heading = document.createElement("h2");
    heading.textContent = headingText;
    const message = document.createElement("p");
    message.textContent = runnerStatusLabel(runner);
    copy.append(heading, message);
    overview.append(mark, copy);
    parent.append(overview);
    if (runner?.message) {
      appendWineNotice(parent, runner.message, runner.state === "ready" ? "info" : "error");
    }
  };

  // The plugin registry lives in the host. Settings never learns whether it
  // answered: an empty catalogue and a host binary with no registry render the
  // same panel, which is the panel that shipped before this feature existed.
  const pluginManager = createPluginManagerController(createDefaultPluginManagerClient());
  pluginManager.onChange(() => renderPluginList());

  const renderPluginCatalogRow = (entry: AvailablePluginView): HTMLElement => {
    const row = document.createElement("div");
    row.className = "settings-row plugin-catalog-row";
    row.dataset.pluginRow = entry.id;
    const progress = pluginManager.progressFor(entry.id);
    const busy = isPluginInstallBusy(progress);
    const installed = entry.installed || progress?.phase === "installed";

    const copy = document.createElement("div");
    copy.className = "settings-row__copy";
    const name = document.createElement("strong");
    name.textContent = entry.name;
    const summary = document.createElement("small");
    summary.textContent = [
      entry.summary,
      entry.version ? `v${entry.version}` : "",
      formatPluginSize(entry.sizeBytes),
    ]
      .filter(Boolean)
      .join(" · ");
    copy.append(name, summary);
    if (progress) {
      // The row's own status line, not a toast: an install that takes a minute
      // has to stay legible next to the thing it is installing.
      const status = document.createElement("small");
      status.className = "plugin-catalog-row__status";
      if (progress.phase === "failed") status.classList.add("plugin-catalog-row__status--error");
      status.textContent = progress.message || formatInstallLabel(progress, entry);
      copy.append(status);
    }

    const install = document.createElement("button");
    install.type = "button";
    install.className = "settings-button";
    install.dataset.pluginInstall = entry.id;
    install.disabled = busy || installed;
    const label = formatInstallLabel(progress, { ...entry, installed });
    install.setAttribute("aria-label", `${label} ${entry.name}`);
    install.innerHTML = `${icon(installed ? "check" : "download")}<span></span>`;
    // The label is host copy and plugin names are third-party: neither goes
    // through innerHTML.
    install.lastElementChild!.textContent = label;
    row.append(copy, install);

    if (busy) {
      const track = document.createElement("div");
      track.className = "plugin-progress";
      const bar = document.createElement("div");
      bar.className = "plugin-progress__bar";
      bar.style.width = `${pluginPercent(progress)}%`;
      track.append(bar);
      row.append(track);
    }
    return row;
  };

  const renderInstalledPluginRow = (plugin: InstalledPluginView): HTMLElement => {
    const row = document.createElement("div");
    row.className = "settings-row plugin-row";
    row.dataset.pluginManaged = plugin.id;
    const mark = document.createElement("span");
    mark.className = "settings-card__mark plugin-row__mark";
    mark.setAttribute("aria-hidden", "true");
    mark.innerHTML = icon("puzzle");

    const copy = document.createElement("div");
    copy.className = "settings-row__copy";
    const name = document.createElement("strong");
    name.textContent = plugin.name;
    const details = document.createElement("small");
    details.textContent =
      [
        plugin.version ? `v${plugin.version}` : "",
        plugin.extensions.join(", "),
        plugin.message,
      ]
        .filter(Boolean)
        .join(" · ") || "Plugin tiers";
    copy.append(name, details);

    const state = document.createElement("span");
    state.className = "plugin-row__state";
    // A plugin the host refused to load must not read in the same green as one
    // that works; the copy says so, the colour says it faster.
    if (plugin.state === "incompatible") state.classList.add("plugin-row__state--warn");
    if (plugin.state === "invalid") state.classList.add("plugin-row__state--error");
    state.textContent = formatPluginStatus(plugin);

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "settings-button settings-button--quiet plugin-uninstall-button";
    remove.dataset.pluginUninstall = plugin.id;
    remove.setAttribute("aria-label", `Uninstall ${plugin.name}`);
    remove.textContent = "Uninstall";

    row.append(mark, copy, state, remove);
    return row;
  };

  // Quiky is the Store's installer, not a plugin the user manages: it has no
  // row in Settings and nothing to query on startup. It is subscribed to
  // because the host writes the finished game straight into the catalog, and
  // the shell reloads the library on its own rather than making the user find
  // a refresh.
  const installerClient = createDefaultQuikyClient();
  installerClient.subscribe((progress) => {
    if (progress.phase === "installed") void refreshLibrary();
  });

  /**
   * The Quiky row predates the registry and is discovered through its own
   * command. Once the registry reports the same plugin, the registry wins: it
   * is the row that can be uninstalled, and two rows for one plugin is worse
   * than either of them alone.
   */
  // Third-party plugins are discovered on disk, so the Installed group is the
  // two native runners plus whatever the registry found. Nothing extra renders
  // when the registry is empty: the panel then looks exactly as it did before.
  const renderDiscoveredPlugins = (): void => {
    for (const stale of refs.pluginsInstalledList.querySelectorAll("[data-plugin-managed]")) {
      stale.remove();
    }
    const installed = pluginManager.catalog().installed;
    const rows = installed.map(renderInstalledPluginRow);
    refs.pluginsInstalledList.append(...rows);
  };

  /**
   * What Orivo is being built towards, named rather than promised in a
   * changelog. These are not installable and say so: a row marked "Soon" is an
   * honest roadmap entry, an install button that fails is not.
   */
  const COMING_SOON_PLUGINS: ReadonlyArray<{ icon: IconName; name: string; summary: string }> = [
    { icon: "sparkle", name: "Spotify", summary: "What you are listening to, beside what you are playing." },
    { icon: "monitor", name: "Moonlight / Sunshine", summary: "Stream a game from another machine on your network." },
    { icon: "collections", name: "Playnite", summary: "Import a Playnite library, its metadata and its categories." },
    { icon: "cloud", name: "Ludusavi", summary: "Back up and restore your save games." },
  ];

  const renderComingSoonPlugins = (term: string): HTMLElement[] =>
    COMING_SOON_PLUGINS.filter(
      (entry) => !term || `${entry.name} ${entry.summary}`.toLocaleLowerCase().includes(term),
    ).map((entry) => {
      const row = document.createElement("div");
      row.className = "settings-row plugin-row plugin-row--soon";
      const mark = document.createElement("span");
      mark.className = "settings-card__mark plugin-row__mark";
      mark.setAttribute("aria-hidden", "true");
      mark.innerHTML = icon(entry.icon);
      const copy = document.createElement("div");
      copy.className = "settings-row__copy";
      const name = document.createElement("strong");
      name.textContent = entry.name;
      const summary = document.createElement("small");
      summary.textContent = entry.summary;
      copy.append(name, summary);
      const state = document.createElement("span");
      state.className = "plugin-row__state plugin-row__state--soon";
      state.textContent = "Soon";
      row.append(mark, copy, state);
      return row;
    });

  const renderPluginList = (): void => {
    const showList = state.pluginView === "list";
    renderDiscoveredPlugins();
    refs.pluginsCatalogPanel.hidden = !showList;
    refs.wallpaperPluginPanel.hidden = state.pluginView !== "wallpaper-searcher";
    refs.wineSettingsPanel.hidden = state.pluginView !== "wine";
    if (!showList) return;
    refs.pluginsCatalogSearch.value = state.pluginCatalogSearch;
    const available = pluginManager.catalog().available;
    const term = state.pluginCatalogSearch.trim().toLocaleLowerCase();
    const matches = available.filter(
      (entry) => !term || `${entry.name} ${entry.summary}`.toLocaleLowerCase().includes(term),
    );
    const soon = renderComingSoonPlugins(term);
    refs.pluginsCatalogList.replaceChildren(...matches.map(renderPluginCatalogRow), ...soon);
    refs.pluginsCatalogEmpty.hidden = matches.length + soon.length > 0;
    // An empty registry and a search that matched nothing are different
    // problems, and telling the user to refine a search they never typed is
    // the kind of dead end this panel used to have.
    refs.pluginsCatalogEmpty.textContent =
      available.length === 0
        ? "Aucun plugin à installer pour le moment."
        : "No plugins match that search.";
  };

  const pluginName = (pluginId: string): string => {
    const catalog = pluginManager.catalog();
    const known =
      catalog.installed.find((plugin) => plugin.id === pluginId) ??
      catalog.available.find((plugin) => plugin.id === pluginId);
    return known?.name ?? pluginId;
  };

  // Removal and file installs have no row to print into, so their outcome goes
  // to the toast. A registry install does have a row, and reports there.
  const uninstallPlugin = async (pluginId: string): Promise<void> => {
    const name = pluginName(pluginId);
    try {
      await pluginManager.uninstall(pluginId);
      showToast(`${name} has been uninstalled.`);
    } catch (error) {
      showToast(pluginErrorMessage(error));
    }
  };

  const installPluginFromFile = async (): Promise<void> => {
    if (!isTauriRuntime()) {
      showToast("Installing a plugin from a file needs the Orivo desktop app.");
      return;
    }
    try {
      const installed = await pluginManager.installFromFile();
      // A cancelled picker is not a failure and says nothing.
      if (installed) showToast(`${pluginName(installed)} has been installed.`);
    } catch (error) {
      showToast(pluginErrorMessage(error));
    }
  };

  const openPluginDetail = (id: PluginId): void => {
    state.pluginView = id;
    renderPluginList();
    renderWineSettingsPanel();
    if (id === "wine" && state.wineSettings.runner === null && !state.wineSettings.loading) {
      void refreshWineRunnerSettings();
    }
  };

  const renderWineSettingsPanel = (): void => {
    const settings = state.wineSettings;
    refs.wineSettingsPanel.hidden = state.pluginView !== "wine";
    refs.wineSettingsPanel.setAttribute("aria-busy", String(settings.loading));
    if (state.pluginView !== "wine") {
      return;
    }

    refs.wineSettingsBody.replaceChildren();
    const body = refs.wineSettingsBody;
    if (settings.loading) {
      // Progress messages ("Downloading DXVK-macOS…") are set alongside
      // `loading`, so the notice has to survive the loading state instead of
      // being replaced by a generic spinner.
      appendWineNotice(body, settings.notice, settings.noticeTone);
      appendWineLoadingState(
        body,
        "Loading plugins",
        "Orivo is reading the local Wine runner and its profiles.",
      );
      return;
    }

    appendWineRunnerSummary(body, settings.runner, "Runner status");
    appendWineNotice(body, settings.notice, settings.noticeTone);

    const heading = document.createElement("h2");
    heading.className = "wine-settings-heading";
    heading.textContent = "Wine profiles";
    body.append(heading);

    if (settings.profiles.length === 0) {
      const empty = document.createElement("section");
      empty.className = "wine-directory-empty";
      empty.textContent =
        "No Wine-Staging profile yet. Orivo adds one automatically the first time you import a Windows game while Wine-Staging is installed.";
      body.append(empty);
      return;
    }

    const profiles = document.createElement("div");
    profiles.className = "wine-settings-profiles";
    for (const profile of settings.profiles) {
      const card = document.createElement("section");
      card.className = "wine-profile-card";
      const profileHeader = document.createElement("div");
      profileHeader.className = "wine-profile-card__header";
      const copy = document.createElement("div");
      const profileName = document.createElement("h3");
      profileName.textContent = profile.displayName;
      const runnerLabel = document.createElement("p");
      runnerLabel.textContent = profile.wineLabel || "Wine-Staging";
      copy.append(profileName, runnerLabel);
      const stateLabel = document.createElement("span");
      stateLabel.className = "wine-profile-state";
      stateLabel.classList.toggle("is-disabled", !profile.enabled);
      stateLabel.textContent = profile.enabled ? "Active" : "Disabled";
      profileHeader.append(copy, stateLabel);
      card.append(profileHeader);

      const directoryHeading = document.createElement("p");
      directoryHeading.className = "wine-profile-card__label";
      directoryHeading.textContent = "Allowed folders";
      card.append(directoryHeading);
      const directories = document.createElement("ul");
      directories.className = "wine-profile-directories";
      if (profile.directories.length === 0) {
        const item = document.createElement("li");
        item.textContent = "No allowed folder";
        directories.append(item);
      } else {
        for (const directory of profile.directories) {
          const item = document.createElement("li");
          item.innerHTML = icon("folder") + "<span></span>";
          item.querySelector("span")!.textContent = directory.label;
          directories.append(item);
        }
      }
      card.append(directories);

      const graphicsHeading = document.createElement("p");
      graphicsHeading.className = "wine-profile-card__label";
      graphicsHeading.textContent = "Graphismes";
      const graphics = document.createElement("p");
      graphics.className = "wine-profile-card__graphics";
      graphics.textContent = profile.graphicsSummary;
      card.append(graphicsHeading, graphics);
      if (profile.graphicsBackend === "dxvk_macos") {
        const warning = document.createElement("p");
        warning.className = "wine-profile-card__warning";
        warning.textContent = "Experimental · avoid games protected by anti-cheat.";
        card.append(warning);
      } else {
        const dxvkHint = document.createElement("p");
        dxvkHint.className = "wine-profile-card__hint";
        dxvkHint.textContent =
          "Automatic compatibility · the first time a compatible Wine game launches, Orivo prepares DXVK-macOS for this profile if it is needed, without changing Wine-Staging or any other application’s prefix.";
        card.append(dxvkHint);
      }

      const lastImport = document.createElement("p");
      lastImport.className = "wine-profile-card__import";
      const importDetails = [
        profile.lastImport ? "Last import · " + profile.lastImport : "",
        profile.lastImportSummary,
      ].filter(Boolean);
      lastImport.textContent = importDetails.length > 0 ? importDetails.join(" · ") : "No import yet";
      card.append(lastImport);

      if (settings.pendingDeleteProfileId === profile.id) {
        const confirmation = document.createElement("div");
        confirmation.className = "wine-delete-confirmation";
        const prompt = document.createElement("p");
        prompt.textContent = "Delete this Wine profile? Its Wine games are removed from your library too.";
        const confirm = wineActionButton("confirm-delete-wine-profile", "Delete the profile", "wine-danger-button");
        confirm.dataset.profileId = profile.id;
        const cancel = wineActionButton("cancel-delete-wine-profile", "Conserver", "steam-secondary-button");
        confirmation.append(prompt, confirm, cancel);
        card.append(confirmation);
      } else {
        const actions = document.createElement("div");
        actions.className = "wine-profile-card__actions";
        const dxvk = wineActionButton(
          "install-dxvk-macos",
          profile.graphicsBackend === "dxvk_macos" ? "Reinstall DXVK-macOS" : "Enable DXVK-macOS",
          "steam-secondary-button",
          "monitor",
        );
        dxvk.dataset.profileId = profile.id;
        dxvk.disabled = !profile.enabled;
        if (profile.graphicsBackend === "dxvk_macos") {
          const wine3d = wineActionButton(
            "use-wine-3d",
            "Switch back to Wine 3D",
            "steam-secondary-button",
          );
          wine3d.dataset.profileId = profile.id;
          actions.append(wine3d);
        }
        const toggle = wineActionButton(
          "toggle-wine-profile",
          profile.enabled ? "Disable" : "Enable",
          "steam-secondary-button",
        );
        toggle.dataset.profileId = profile.id;
        toggle.dataset.enabled = String(!profile.enabled);
        const remove = wineActionButton("delete-wine-profile", "Delete", "wine-text-button");
        remove.dataset.profileId = profile.id;
        actions.append(dxvk, toggle, remove);
        card.append(actions);
      }
      profiles.append(card);
    }
    body.append(profiles);
  };

  const refreshWineRunnerSettings = async (render = true): Promise<void> => {
    const settings = state.wineSettings;
    // Settings loaders share one generation counter: a snapshot that resolves
    // after the section changed must never overwrite the fresher one.
    const request = settingsRequest;
    settings.loading = true;
    if (render) {
      renderWineSettingsPanel();
    }
    if (!isTauriRuntime()) {
      settings.loading = false;
      settings.runner = {
        state: "unavailable",
        available: false,
        version: "",
        message: "Wine-Staging settings are available in the Orivo desktop app for macOS.",
      };
      settings.profiles = [];
      settings.notice = "";
      settings.noticeTone = "info";
      if (render) {
        renderWineSettingsPanel();
      }
      return;
    }

    try {
      const snapshot = normaliseWineRunnerSettings(await invoke<unknown>("get_wine_runner_settings"));
      if (request !== settingsRequest) return;
      settings.runner = snapshot.runner;
      settings.profiles = snapshot.profiles;
      settings.notice = "";
      settings.noticeTone = "info";
      settings.pendingDeleteProfileId = "";
    } catch (error) {
      if (request !== settingsRequest) return;
      settings.notice = messageFromError(error, "Wine-Staging settings could not be loaded.");
      settings.noticeTone = "error";
    }
    settings.loading = false;
    if (render) {
      renderWineSettingsPanel();
    }
  };

  const setWineProfileEnabled = async (profileId: string, enabled: boolean): Promise<void> => {
    if (!profileId || !isTauriRuntime()) {
      return;
    }
    state.wineSettings.notice = enabled ? "Enabling the Wine profile…" : "Disabling the Wine profile…";
    state.wineSettings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("set_wine_profile_enabled", { profileId, enabled });
      await refreshWineRunnerSettings(false);
      state.wineSettings.notice = enabled ? "Wine profile enabled." : "Wine profile disabled.";
      state.wineSettings.noticeTone = "success";
    } catch (error) {
      state.wineSettings.notice = messageFromError(error, "The Wine profile state could not be changed.");
      state.wineSettings.noticeTone = "error";
    }
    renderWineSettingsPanel();
  };

  const deleteWineProfile = async (profileId: string): Promise<void> => {
    if (!profileId || !isTauriRuntime()) {
      return;
    }
    state.wineSettings.notice = "Deleting the Wine profile…";
    state.wineSettings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("delete_wine_profile", { profileId });
      state.wineSettings.pendingDeleteProfileId = "";
      await Promise.all([refreshWineRunnerSettings(false), refreshLibrary()]);
      state.wineSettings.notice = "Wine profile deleted, along with its Wine games in your library.";
      state.wineSettings.noticeTone = "success";
    } catch (error) {
      state.wineSettings.notice = messageFromError(error, "The Wine profile could not be deleted.");
      state.wineSettings.noticeTone = "error";
    }
    renderWineSettingsPanel();
  };

  const visibleSteamGames = (): SteamPreviewGame[] => {
    const preview = state.steam.preview;
    if (!preview || preview.status !== "available") {
      return [];
    }

    const term = state.steam.query.trim().toLocaleLowerCase();
    if (!term) {
      return preview.games;
    }

    return preview.games.filter((game) =>
      [game.title, game.appId].join(" ").toLocaleLowerCase().includes(term),
    );
  };

  const focusSteamPanel = (): void => {
    requestAnimationFrame(() => {
      refs.steamPanel.querySelector<HTMLInputElement>("#steam-game-search")?.focus();
    });
  };

  const renderSteamPanel = (): void => {
    const steam = state.steam;
    // Collapsed by default: the scan is reached from the Steam row's "Installed
    // games" button, so an idle pitch for it no longer needs to hold a card.
    refs.steamPanel.hidden = !steam.open;
    refs.steamPanel.setAttribute("aria-busy", String(steam.phase === "scanning" || steam.phase === "importing"));
    const hasAvailablePreview = steam.preview?.status === "available";
    refs.steamRefresh.hidden = !hasAvailablePreview || !steam.open;
    refs.steamRefresh.disabled =
      !hasAvailablePreview || steam.phase === "scanning" || steam.phase === "importing";

    if (!steam.open) {
      refs.steamBody.replaceChildren();
      refs.steamFooter.hidden = true;
      return;
    }

    const detail = refs.steamPanel.querySelector<HTMLElement>("#steam-import-detail");
    if (detail) {
      if (steam.preview?.status === "available") {
        const libraryCount = steam.preview.libraries === 1 ? "1 library" : steam.preview.libraries + " libraries";
        const gameCount = steam.preview.games.length === 1 ? "1 installed game" : steam.preview.games.length + " installed games";
        detail.textContent = "Steam · " + libraryCount + " · " + gameCount;
      } else {
        detail.textContent = "A local Steam source";
      }
    }

    refs.steamBody.replaceChildren();
    refs.steamFooter.hidden = true;
    refs.steamImportButton.disabled = true;
    refs.steamImportButton.textContent = "Import selected";
    refs.steamSelectionSummary.textContent = "";

    if (steam.phase === "idle" || steam.phase === "scanning") {
      const loading = document.createElement("section");
      loading.className = "steam-state steam-state--loading";
      loading.setAttribute("aria-live", "polite");

      const spinner = document.createElement("span");
      spinner.className = "steam-spinner";
      spinner.setAttribute("aria-hidden", "true");

      const heading = document.createElement("h2");
      heading.textContent = "Looking for your Steam library";
      const message = document.createElement("p");
      message.textContent = "Orivo is reading installed games locally. You can keep browsing while this finishes.";
      loading.append(spinner, heading, message);
      refs.steamBody.append(loading);
      return;
    }

    const preview = steam.preview;
    if (!preview || preview.status === "unavailable" || preview.status === "error") {
      const unavailable = document.createElement("section");
      unavailable.className = "steam-state steam-state--unavailable";
      unavailable.setAttribute("aria-live", "polite");

      const badge = document.createElement("span");
      badge.className = "steam-state__icon";
      badge.innerHTML = icon(preview?.status === "error" ? "alert" : "folder");
      badge.setAttribute("aria-hidden", "true");

      const heading = document.createElement("h2");
      heading.textContent = preview?.status === "error" ? "Steam could not be scanned" : "Steam was not found";
      const message = document.createElement("p");
      message.textContent =
        preview?.message ||
        (preview?.status === "error"
          ? "Try again in a moment. Your existing Orivo library is unaffected."
          : "Install Steam or open it once, then try scanning again.");

      const retry = document.createElement("button");
      retry.type = "button";
      retry.className = "steam-secondary-button";
      retry.dataset.steamAction = "retry";
      retry.innerHTML = icon("refresh") + "<span>Scan again</span>";
      unavailable.append(badge, heading, message, retry);
      refs.steamBody.append(unavailable);
      return;
    }

    if (steam.notice) {
      const notice = document.createElement("p");
      notice.className = "steam-notice steam-notice--" + steam.noticeTone;
      notice.setAttribute("role", steam.noticeTone === "error" ? "alert" : "status");
      notice.textContent = steam.notice;
      refs.steamBody.append(notice);
    }

    if (steam.phase === "importing") {
      const progress = document.createElement("p");
      progress.className = "steam-notice steam-notice--info";
      progress.setAttribute("role", "status");
      const count = steam.selectedAppIds.size;
      progress.textContent =
        "Adding " + count.toLocaleString() + (count === 1 ? " game" : " games") + ". You can keep browsing.";
      refs.steamBody.append(progress);
    }

    const matchingGames = visibleSteamGames();
    const games = matchingGames.slice(0, MAX_RENDERED_STEAM_GAMES);
    const selectedMatchingCount = matchingGames.filter((game) => steam.selectedAppIds.has(game.appId)).length;
    const controls = document.createElement("div");
    controls.className = "steam-list-controls";

    const search = document.createElement("label");
    search.className = "steam-search-control";
    search.innerHTML = icon("search");
    const searchInput = document.createElement("input");
    searchInput.id = "steam-game-search";
    searchInput.type = "search";
    searchInput.autocomplete = "off";
    searchInput.spellcheck = false;
    searchInput.placeholder = "Filter installed games";
    searchInput.value = steam.query;
    searchInput.setAttribute("aria-label", "Filter installed Steam games");
    search.append(searchInput);

    const selectAll = document.createElement("label");
    selectAll.className = "steam-select-all";
    const selectAllInput = document.createElement("input");
    selectAllInput.id = "steam-select-all";
    selectAllInput.type = "checkbox";
    selectAllInput.checked = matchingGames.length > 0 && selectedMatchingCount === matchingGames.length;
    selectAllInput.indeterminate = selectedMatchingCount > 0 && selectedMatchingCount < matchingGames.length;
    selectAllInput.disabled = matchingGames.length === 0 || steam.phase === "importing";
    const selectAllText = document.createElement("span");
    selectAllText.textContent = "Select matching";
    selectAll.append(selectAllInput, selectAllText);
    controls.append(search, selectAll);
    refs.steamBody.append(controls);

    const results = document.createElement("p");
    results.className = "steam-results-count";
    const matchingLabel =
      matchingGames.length === preview.games.length
        ? matchingGames.length + " games found"
        : matchingGames.length + " of " + preview.games.length + " games";
    results.textContent =
      games.length < matchingGames.length
        ? matchingLabel + " · Showing the first " + MAX_RENDERED_STEAM_GAMES + "; refine your filter to see the rest."
        : matchingLabel;
    results.setAttribute("aria-live", "polite");
    refs.steamBody.append(results);

    if (matchingGames.length === 0) {
      const empty = document.createElement("section");
      empty.className = "steam-list-empty";
      const heading = document.createElement("h2");
      heading.textContent = preview.games.length === 0 ? "No installed Steam games yet" : "No games match that filter";
      const message = document.createElement("p");
      message.textContent =
        preview.games.length === 0
          ? "When Steam has installed games locally, they will appear here."
          : "Try a game title or clear the filter.";
      empty.append(heading, message);
      refs.steamBody.append(empty);
    } else {
      const list = document.createElement("div");
      list.id = "steam-games";
      list.className = "steam-game-list";
      list.setAttribute("role", "list");

      for (const game of games) {
        const row = document.createElement("label");
        row.className = "steam-game-row";
        row.classList.toggle("is-selected", steam.selectedAppIds.has(game.appId));
        row.classList.toggle("is-imported", game.alreadyImported);
        row.setAttribute("role", "listitem");

        const toggle = document.createElement("input");
        toggle.type = "checkbox";
        toggle.checked = steam.selectedAppIds.has(game.appId);
        toggle.disabled = steam.phase === "importing";
        toggle.dataset.steamAppId = game.appId;
        toggle.setAttribute("aria-label", "Select " + game.title);

        const artwork = document.createElement("span");
        artwork.className = "steam-game-artwork";
        if (game.coverUrl || game.heroUrl) {
          const image = document.createElement("img");
          image.src = game.coverUrl || game.heroUrl;
          image.alt = "";
          image.loading = "lazy";
          image.decoding = "async";
          image.addEventListener("error", () => {
            image.remove();
            artwork.classList.add("steam-game-artwork--fallback");
          });
          artwork.append(image);
        } else {
          artwork.classList.add("steam-game-artwork--fallback");
        }

        const copy = document.createElement("span");
        copy.className = "steam-game-copy";
        const title = document.createElement("strong");
        title.textContent = game.title;
        const metadata = document.createElement("span");
        metadata.className = "steam-game-metadata";
        const metadataParts = ["Steam app " + game.appId];
        if (game.locationLabel) {
          metadataParts.push(game.locationLabel);
        }
        if (game.lastUpdated) {
          metadataParts.push("Updated " + game.lastUpdated);
        }
        metadata.textContent = metadataParts.join(" · ");
        copy.append(title, metadata);

        const status = document.createElement("span");
        status.className = "steam-game-status";
        if (game.alreadyImported) {
          status.textContent = "In library";
        } else {
          status.textContent = "Ready";
        }

        row.append(toggle, artwork, copy, status);
        list.append(row);
      }

      refs.steamBody.append(list);
    }

    refs.steamFooter.hidden = false;
    const selectedCount = preview.games.filter((game) => steam.selectedAppIds.has(game.appId)).length;
    refs.steamSelectionSummary.textContent =
      selectedCount === 0 ? "Choose games to import" : selectedCount === 1 ? "1 game selected" : selectedCount + " games selected";
    refs.steamImportButton.disabled = selectedCount === 0 || steam.phase === "importing";
    refs.steamImportButton.textContent =
      steam.phase === "importing"
        ? "Importing…"
        : selectedCount === 0
          ? "Import selected"
        : selectedCount === 1
          ? "Import 1 game"
          : "Import " + selectedCount + " games";
  };

  // The Steam import list lives inside Settings › Libraries. "Open" only means
  // the scanned list is expanded in that card — there is no modal to dismiss.
  const setSteamPanelOpen = (open: boolean): void => {
    if (open && state.steam.open) {
      focusSteamPanel();
      return;
    }
    state.steam.open = open;

    if (!open) {
      renderSteamPanel();
      return;
    }

    closeLibraryMenu();
    if (state.steam.phase !== "scanning" && state.steam.phase !== "importing") {
      void scanSteamLibrary();
    } else {
      renderSteamPanel();
      focusSteamPanel();
    }
  };

  const focusSteamAccountPanel = (): void => {
    requestAnimationFrame(() => {
      if (!state.steamAccount.open) {
        return;
      }
      const steamId = refs.steamAccountPanel.querySelector<HTMLInputElement>("input[name='steam-id']");
      if (state.steamAccount.phase === "api-key" && steamId) {
        steamId.focus();
        return;
      }
      const primaryAction =
        refs.steamAccountPanel.querySelector<HTMLElement>("[data-steam-account-action='connect']") ??
        refs.steamAccountPanel.querySelector<HTMLElement>("[data-steam-account-action='sync']") ??
        refs.steamAccountPanel.querySelector<HTMLElement>("[data-steam-account-action='cancel-wait']") ??
        refs.steamAccountPanel.querySelector<HTMLElement>("[data-steam-account-action='back']");
      (primaryAction ?? refs.steamAccountPanel).focus();
    });
  };

  const accountActionButton = (
    action: string,
    label: string,
    className = "steam-secondary-button",
    iconName: "library" | "refresh" | "close" | "settings" | "steam" = "library",
  ): HTMLButtonElement => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = className;
    button.dataset.steamAccountAction = action;
    button.innerHTML = icon(iconName) + "<span>" + label + "</span>";
    return button;
  };

  const renderSteamAccountPanel = (): void => {
    const account = state.steamAccount;
    const restoreFocus = refs.steamAccountPanel.contains(document.activeElement);
    // The row states what Steam is doing whether or not the panel is expanded,
    // so it is rendered before the early return below.
    renderSteamSourceRow();
    refs.steamAccountPanel.hidden = !account.open;
    refs.steamAccountPanel.setAttribute(
      "aria-busy",
      String(account.phase === "loading" || account.phase === "connecting" || account.phase === "syncing" || account.phase === "saving-api-key"),
    );
    if (!account.open) {
      return;
    }

    refs.steamAccountBody.replaceChildren();
    const body = refs.steamAccountBody;

    const appendNotice = (): void => {
      if (!account.notice) {
        return;
      }
      const notice = document.createElement("p");
      notice.className = "steam-notice steam-notice--" + account.noticeTone;
      notice.setAttribute("role", account.noticeTone === "error" ? "alert" : "status");
      notice.textContent = account.notice;
      body.append(notice);
    };

    if (account.phase === "loading" || account.phase === "idle") {
      const loading = document.createElement("section");
      loading.className = "steam-state steam-state--loading";
      loading.setAttribute("aria-live", "polite");
      const spinner = document.createElement("span");
      spinner.className = "steam-spinner";
      spinner.setAttribute("aria-hidden", "true");
      const heading = document.createElement("h2");
      heading.textContent = "Checking your Steam connection";
      const message = document.createElement("p");
      message.textContent = "Your account details stay on this Mac.";
      loading.append(spinner, heading, message);
      body.append(loading);
      if (restoreFocus) {
        focusSteamAccountPanel();
      }
      return;
    }

    if (account.phase === "connecting") {
      const waiting = document.createElement("section");
      waiting.className = "steam-state steam-state--loading";
      waiting.setAttribute("aria-live", "polite");
      const spinner = document.createElement("span");
      spinner.className = "steam-spinner";
      spinner.setAttribute("aria-hidden", "true");
      const heading = document.createElement("h2");
      heading.textContent = "Continue in the Steam window";
      const message = document.createElement("p");
      message.textContent = "Sign in directly with Steam, including Steam Guard. Orivo never receives your password.";
      const complete = accountActionButton("complete-login", "I’ve signed in", "steam-import-button", "refresh");
      const cancel = accountActionButton("cancel-wait", "I’ll connect later", "steam-secondary-button", "close");
      waiting.append(spinner, heading, message, complete, cancel);
      body.append(waiting);
      appendNotice();
      if (restoreFocus) {
        focusSteamAccountPanel();
      }
      return;
    }

    if (account.phase === "api-key") {
      const intro = document.createElement("section");
      intro.className = "steam-account-intro";
      const heading = document.createElement("h2");
      heading.textContent = "Use your Steam API key";
      const message = document.createElement("p");
      message.textContent = "This fallback stays encrypted in your macOS Keychain and is sent only to Steam.";
      intro.append(heading, message);

      const form = document.createElement("form");
      form.className = "steam-api-key-form";
      form.dataset.steamAccountForm = "api-key";

      const steamIdLabel = document.createElement("label");
      steamIdLabel.textContent = "SteamID64";
      const steamId = document.createElement("input");
      steamId.name = "steam-id";
      steamId.inputMode = "numeric";
      steamId.autocomplete = "off";
      steamId.required = true;
      steamId.placeholder = "7656119…";
      steamId.value = account.apiKeySteamId;
      steamIdLabel.append(steamId);

      const apiKeyLabel = document.createElement("label");
      apiKeyLabel.textContent = "Steam Web API key";
      const apiKey = document.createElement("input");
      apiKey.name = "steam-api-key";
      apiKey.type = "password";
      apiKey.autocomplete = "off";
      apiKey.spellcheck = false;
      apiKey.required = true;
      apiKey.placeholder = "32-character key";
      apiKeyLabel.append(apiKey);

      const help = document.createElement("a");
      help.className = "steam-api-key-help";
      help.href = "https://steamcommunity.com/dev/apikey";
      help.target = "_blank";
      help.rel = "noreferrer";
      help.textContent = "Get a Steam Web API key";

      const actions = document.createElement("div");
      actions.className = "steam-account-actions";
      const back = accountActionButton("back", "Back", "steam-secondary-button", "close");
      const submit = accountActionButton("save-api-key", "Connect & sync", "steam-import-button", "library");
      submit.type = "submit";
      actions.append(back, submit);
      form.append(steamIdLabel, apiKeyLabel, help, actions);
      body.append(intro);
      appendNotice();
      body.append(form);
      if (restoreFocus) {
        focusSteamAccountPanel();
      }
      return;
    }

    if (account.phase === "saving-api-key" || account.phase === "syncing") {
      const syncing = document.createElement("section");
      syncing.className = "steam-state steam-state--loading";
      syncing.setAttribute("aria-live", "polite");
      const spinner = document.createElement("span");
      spinner.className = "steam-spinner";
      spinner.setAttribute("aria-hidden", "true");
      const heading = document.createElement("h2");
      heading.textContent = account.phase === "saving-api-key" ? "Saving your Steam connection" : "Syncing your Steam library";
      const message = document.createElement("p");
      message.textContent = "Your library will appear here as soon as Steam responds. You can keep using Orivo.";
      syncing.append(spinner, heading, message);
      body.append(syncing);
      if (restoreFocus) {
        focusSteamAccountPanel();
      }
      return;
    }

    const status = account.status;
    const connectedStatus = status?.connected ? status : null;
    const connected = connectedStatus !== null;
    const overview = document.createElement("section");
    overview.className = "steam-account-overview";
    const badge = document.createElement("span");
    badge.className = "steam-state__icon";
    badge.innerHTML = icon(connected ? "steam" : "alert");
    badge.setAttribute("aria-hidden", "true");
    const copy = document.createElement("div");
    const heading = document.createElement("h2");
    heading.textContent = connected ? "Steam is connected" : "Connect your Steam library";
    const message = document.createElement("p");
    if (connectedStatus) {
      const suffix = connectedStatus.steamId.length > 4 ? "••••" + connectedStatus.steamId.slice(-4) : connectedStatus.steamId;
      message.textContent = "Steam account " + suffix + " · " + (connectedStatus.method === "api_key" ? "API key" : "Web login");
    } else {
      message.textContent = "See the games you own, even before they are installed on this Mac.";
    }
    copy.append(heading, message);
    overview.append(badge, copy);
    body.append(overview);
    appendNotice();

    if (connected && account.lastSync) {
      const summary = document.createElement("p");
      summary.className = "steam-account-sync-summary";
      const installed = account.lastSync.installedGames;
      summary.textContent = account.lastSync.totalGames.toLocaleString() + " owned games · " + installed.toLocaleString() + " installed on this Mac";
      body.append(summary);
    }

    const actions = document.createElement("div");
    actions.className = "steam-account-actions";
    if (connected) {
      actions.append(
        accountActionButton("sync", "Sync library", "steam-import-button", "refresh"),
        accountActionButton("disconnect", "Disconnect", "steam-secondary-button", "close"),
      );
    } else {
      actions.append(
        accountActionButton("connect", "Continue with Steam", "steam-import-button", "steam"),
        accountActionButton("api-key", "Use an API key", "steam-secondary-button", "settings"),
      );
    }
    body.append(actions);
    if (restoreFocus) {
      focusSteamAccountPanel();
    }
  };

  const refreshSteamAccountStatus = async (): Promise<void> => {
    // Shares the settings generation counter with every other settings loader:
    // a status that resolves after the section changed is stale and must not
    // overwrite the fresher one (a slow "disconnected" beating a "connected").
    const request = settingsRequest;
    if (!isTauriRuntime()) {
      state.steamAccount.status = { connected: false, steamId: "", method: "" };
      state.steamAccount.phase = "disconnected";
      state.steamAccount.notice = "Steam account connection is available in the Orivo desktop app.";
      state.steamAccount.noticeTone = "info";
      renderSteamAccountPanel();
      return;
    }

    try {
      const status = normaliseSteamAccountStatus(await invoke<unknown>("get_steam_account_status"));
      if (request !== settingsRequest) return;
      if (!status) {
        throw new Error("Steam returned an invalid account status.");
      }
      state.steamAccount.status = status;
      state.steamAccount.phase = status.connected ? "connected" : "disconnected";
      if (state.steamAccount.noticeTone !== "error") {
        state.steamAccount.notice = "";
      }
    } catch (error) {
      if (request !== settingsRequest) return;
      state.steamAccount.phase = "error";
      state.steamAccount.notice = messageFromError(error, "Steam account status could not be loaded.");
      state.steamAccount.noticeTone = "error";
    }
    renderSteamAccountPanel();
  };

  /**
   * Steam's row in the stores list.
   *
   * Steam is not a `ConnectedSource` — it has its own backend, its own sign-in
   * states and a local scan the other stores have no equivalent for — so it
   * cannot come from the same loop. It is rendered to match those rows exactly,
   * because to the person reading the page it is simply another store.
   */
  const renderSteamSourceRow = (): void => {
    const account = state.steamAccount;
    const connected = account.status?.connected === true;
    const busy = account.phase === "connecting" || account.phase === "syncing";
    const row = document.createElement("div");
    row.className = "settings-row source-account-row";
    row.classList.toggle("is-connected", connected);
    row.dataset.sourceRow = "steam";

    const mark = document.createElement("span");
    mark.className = "source-account-row__mark";
    mark.setAttribute("aria-hidden", "true");
    mark.innerHTML = brandIcon("steam");

    const copy = document.createElement("div");
    copy.className = "settings-row__copy source-account-row__copy";
    const name = document.createElement("strong");
    name.textContent = "Steam";
    const detail = document.createElement("small");
    // The Steam ID is the account's own identifier, so it goes in as text.
    detail.textContent = connected
      ? account.status?.steamId
        ? `Connected as ${account.status.steamId}`
        : "Connected"
      : "See the games you own, and import the ones installed on this Mac.";
    copy.append(name, detail);

    // Steam's price-data health rides on its row, the same way every other
    // store's does, instead of being repeated under "Store data only".
    const health = state.providerStatuses.find((provider) => provider.provider === "steam");
    if (health) {
      const dot = document.createElement("span");
      dot.className = `source-account-row__dot source-account-row__dot--${health.health}`;
      dot.title = health.message || `Store data: ${health.health.replace("-", " ")}`;
      dot.setAttribute("role", "img");
      dot.setAttribute("aria-label", `Store data: ${health.health.replace("-", " ")}`);
      name.append(dot);
    }

    const actions = document.createElement("div");
    actions.className = "source-account-row__actions";
    const button = (
      action: string,
      label: string,
      className: string,
      iconName: IconName,
    ): HTMLButtonElement => {
      const element = document.createElement("button");
      element.type = "button";
      element.className = className;
      element.dataset.steamRowAction = action;
      element.innerHTML = icon(iconName);
      element.append(document.createTextNode(label));
      return element;
    };

    if (busy) {
      const spinner = document.createElement("span");
      spinner.className = "steam-spinner";
      spinner.setAttribute("aria-hidden", "true");
      const waiting = document.createElement("span");
      waiting.className = "source-account-row__waiting";
      waiting.setAttribute("role", "status");
      waiting.textContent = account.phase === "syncing" ? "Syncing…" : "Continue in the window…";
      actions.append(spinner, waiting);
    } else if (connected) {
      actions.append(
        button("sync", "Sync", "steam-import-button", "refresh"),
        button("import", "Installed games", "steam-secondary-button", "download"),
        button("manage", "Manage", "steam-secondary-button", "settings"),
      );
    } else {
      actions.append(
        button("connect", "Connect", "steam-import-button", "steam"),
        button("import", "Installed games", "steam-secondary-button", "download"),
      );
    }

    row.append(mark, copy, actions);
    refs.steamSourceRow.replaceChildren(row);
  };

  // The Steam account card is part of Settings › Libraries, so "open" simply
  // tracks whether that section is on screen and its status is worth loading.
  const setSteamAccountPanelOpen = (open: boolean): void => {
    if (!open) {
      if (state.steamAccount.phase === "connecting" && isTauriRuntime()) {
        void invoke("cancel_steam_web_login");
      }
      state.steamAccount.open = false;
      renderSteamAccountPanel();
      return;
    }

    state.steamAccount.open = true;
    state.steamAccount.phase = "loading";
    state.steamAccount.notice = "";
    renderSteamAccountPanel();
    void refreshSteamAccountStatus();
  };

  const syncSteamAccountLibrary = async (): Promise<void> => {
    if (!isTauriRuntime() || state.steamAccount.phase === "syncing") {
      return;
    }
    state.steamAccount.phase = "syncing";
    state.steamAccount.notice = "";
    renderSteamAccountPanel();

    try {
      const result = normaliseSteamAccountSyncResult(
        await invoke<unknown>("sync_steam_account_library"),
      );
      if (!result) {
        throw new Error("Steam returned an invalid library sync.");
      }
      state.steamAccount.lastSync = result;
      state.steamAccount.phase = "connected";
      state.steamAccount.notice = steamAccountSyncSummary(result);
      state.steamAccount.noticeTone = "success";
      await refreshLibrary();
      showToast(state.steamAccount.notice);
    } catch (error) {
      state.steamAccount.phase = "error";
      state.steamAccount.notice = messageFromError(error, "Steam library could not be synced.");
      state.steamAccount.noticeTone = "error";
    }
    renderSteamAccountPanel();
  };

  const startSteamWebLogin = async (): Promise<void> => {
    if (!isTauriRuntime()) {
      state.steamAccount.phase = "disconnected";
      state.steamAccount.notice = "Steam account connection is available in the Orivo desktop app.";
      state.steamAccount.noticeTone = "info";
      renderSteamAccountPanel();
      return;
    }
    state.steamAccount.phase = "connecting";
    state.steamAccount.notice = "";
    renderSteamAccountPanel();
    try {
      await invoke("begin_steam_web_login");
    } catch (error) {
      state.steamAccount.phase = state.steamAccount.status?.connected ? "connected" : "disconnected";
      state.steamAccount.notice = messageFromError(error, "Steam sign-in window could not be opened.");
      state.steamAccount.noticeTone = "error";
      renderSteamAccountPanel();
    }
  };

  const connectSteamWithApiKey = async (steamId: string, apiKey: string): Promise<void> => {
    if (!isTauriRuntime()) {
      return;
    }
    state.steamAccount.phase = "saving-api-key";
    state.steamAccount.notice = "";
    renderSteamAccountPanel();
    try {
      const status = normaliseSteamAccountStatus(
        await invoke<unknown>("connect_steam_with_api_key", { steamId, apiKey }),
      );
      if (!status?.connected) {
        throw new Error("Steam API key could not be saved.");
      }
      state.steamAccount.status = status;
      state.steamAccount.phase = "connected";
      await syncSteamAccountLibrary();
    } catch (error) {
      state.steamAccount.phase = "api-key";
      state.steamAccount.notice = messageFromError(error, "Steam API key could not be connected.");
      state.steamAccount.noticeTone = "error";
      renderSteamAccountPanel();
    }
  };

  const disconnectSteamAccount = async (): Promise<void> => {
    if (!isTauriRuntime()) {
      return;
    }
    try {
      await invoke("disconnect_steam_account");
      state.steamAccount.status = { connected: false, steamId: "", method: "" };
      state.steamAccount.phase = "disconnected";
      state.steamAccount.lastSync = null;
      state.steamAccount.apiKeySteamId = "";
      state.steamAccount.notice = "Steam was disconnected. Games already imported into Orivo remain in your library.";
      state.steamAccount.noticeTone = "info";
    } catch (error) {
      state.steamAccount.phase = "error";
      state.steamAccount.notice = messageFromError(error, "Steam could not be disconnected.");
      state.steamAccount.noticeTone = "error";
    }
    renderSteamAccountPanel();
  };

  // -------------------------------------------------------------------------
  // Connected libraries (Epic, GOG, Ubisoft, Xbox, Microsoft Store, Instant Gaming)
  // -------------------------------------------------------------------------

  const sourceStatus = (provider: ConnectedSource): SourceAccountStatus =>
    state.sourceAccounts.statuses.find((status) => status.provider === provider) ??
    defaultSourceAccounts().find((status) => status.provider === provider)!;

  const setSourceNotice = (
    provider: ConnectedSource | null,
    notice: string,
    tone: SteamNoticeTone = "info",
  ): void => {
    state.sourceAccounts.notice = notice;
    state.sourceAccounts.noticeTone = tone;
    state.sourceAccounts.noticeProvider = provider;
  };

  const setSourceBusy = (provider: ConnectedSource, busy: boolean): void => {
    if (busy) {
      state.sourceAccounts.busy.add(provider);
    } else {
      state.sourceAccounts.busy.delete(provider);
    }
  };

  const sourceActionButton = (
    provider: ConnectedSource,
    action: string,
    label: string,
    className: string,
    iconName: IconName,
  ): HTMLButtonElement => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = className;
    button.dataset.sourceAction = action;
    button.dataset.sourceProvider = provider;
    button.innerHTML = icon(iconName) + "<span>" + label + "</span>";
    return button;
  };

  const renderSourceAccountsPanel = (): void => {
    const body = refs.sourceAccountsBody;
    const restoreFocus = body.contains(document.activeElement);
    const focusedAction = restoreFocus
      ? (document.activeElement as HTMLElement | null)?.dataset.sourceAction ?? ""
      : "";
    const focusedProvider = restoreFocus
      ? (document.activeElement as HTMLElement | null)?.dataset.sourceProvider ?? ""
      : "";
    body.replaceChildren();
    refs.sourceAccountsPanel.setAttribute(
      "aria-busy",
      String(state.sourceAccounts.loading || state.sourceAccounts.busy.size > 0),
    );

    if (!isTauriRuntime()) {
      const hint = document.createElement("p");
      hint.className = "settings-hint";
      hint.textContent = "Connecting another game library is available in the Orivo desktop app.";
      body.append(hint);
      return;
    }

    if (state.sourceAccounts.notice && state.sourceAccounts.noticeProvider === null) {
      body.append(sourceNoticeElement());
    }

    const list = document.createElement("div");
    list.className = "source-account-list";
    for (const descriptor of CONNECTED_SOURCES) {
      const status = sourceStatus(descriptor.provider);
      const busy = state.sourceAccounts.busy.has(descriptor.provider);
      const row = document.createElement("div");
      row.className = "settings-row source-account-row";
      row.classList.toggle("is-connected", status.connected);
      row.dataset.sourceRow = descriptor.provider;

      const mark = document.createElement("span");
      mark.className = "source-account-row__mark";
      mark.setAttribute("aria-hidden", "true");
      // Settings presents each store as itself, in its own colours and at a
      // size where the logo is the logo. The library, the hero badge and the
      // detail page keep the white marks.
      mark.innerHTML = brandIcon(descriptor.icon);

      const copy = document.createElement("div");
      copy.className = "settings-row__copy source-account-row__copy";
      const name = document.createElement("strong");
      name.textContent = status.label;
      const detail = document.createElement("small");
      // A disconnected store needs to say what connecting it gives you. Once
      // it is connected that pitch is spent, and the account is what matters —
      // repeating both made every connected row three lines tall.
      detail.textContent = status.connected
        ? sourceStatusLine(status)
        : status.description || sourceStatusLine(status);
      copy.append(name, detail);
      const sync = state.sourceAccounts.lastSync.get(descriptor.provider);
      if (sync) {
        const summary = document.createElement("small");
        summary.className = "source-account-row__summary";
        summary.textContent = sourceSyncSummary(sync);
        copy.append(summary);
      }

      // This store's price-data health, on the same row as its connection, as
      // a small dot beside the name. It used to be a red "Unavailable" pill on
      // every row, which read as an error about the store itself rather than a
      // note about its price feed.
      const health = state.providerStatuses.find(
        (provider) => providerStatusForSource(provider.provider) === descriptor.provider,
      );
      if (health) {
        const dot = document.createElement("span");
        dot.className = `source-account-row__dot source-account-row__dot--${health.health}`;
        dot.title = health.message || `Store data: ${health.health.replace("-", " ")}`;
        dot.setAttribute("role", "img");
        dot.setAttribute(
          "aria-label",
          `Store data: ${health.health.replace("-", " ")}`,
        );
        name.append(dot);
      }

      const actions = document.createElement("div");
      actions.className = "source-account-row__actions";
      if (busy) {
        const spinner = document.createElement("span");
        spinner.className = "steam-spinner";
        spinner.setAttribute("aria-hidden", "true");
        const waiting = document.createElement("span");
        waiting.className = "source-account-row__waiting";
        waiting.setAttribute("role", "status");
        waiting.textContent = status.connected
          ? "Syncing…"
          : status.style === "session"
            ? "Continue in the window…"
            : "Waiting for sign-in…";
        actions.append(spinner, waiting);
        actions.append(
          sourceActionButton(
            descriptor.provider,
            "cancel",
            "Cancel",
            "steam-secondary-button",
            "close",
          ),
        );
      } else if (state.sourceAccounts.pendingDisconnect === descriptor.provider) {
        // Disconnecting is two decisions, not one: sign out, and optionally
        // forget what was already imported. Never guess the second.
        const confirm = document.createElement("p");
        confirm.className = "source-account-row__confirm";
        confirm.textContent = "Keep the games already imported?";
        copy.append(confirm);
        actions.append(
          sourceActionButton(
            descriptor.provider,
            "disconnect-keep",
            "Keep them",
            "steam-secondary-button",
            "check",
          ),
          sourceActionButton(
            descriptor.provider,
            "disconnect-forget",
            "Remove them",
            "steam-secondary-button",
            "close",
          ),
          sourceActionButton(
            descriptor.provider,
            "disconnect-cancel",
            "Cancel",
            "steam-secondary-button",
            "chevron-left",
          ),
        );
      } else if (status.connected) {
        actions.append(
          sourceActionButton(
            descriptor.provider,
            "sync",
            "Sync",
            "steam-import-button",
            "refresh",
          ),
          sourceActionButton(
            descriptor.provider,
            "disconnect",
            "Disconnect",
            "steam-secondary-button",
            "close",
          ),
        );
      } else {
        // The brand logo already sits at the head of the row, so repeating it
        // inside the button only added noise.
        const connect = document.createElement("button");
        connect.type = "button";
        connect.className = "steam-import-button source-account-row__connect";
        connect.dataset.sourceAction = "connect";
        connect.dataset.sourceProvider = descriptor.provider;
        connect.textContent = "Connect";
        actions.append(connect);
      }

      row.append(mark, copy, actions);
      if (
        state.sourceAccounts.notice &&
        state.sourceAccounts.noticeProvider === descriptor.provider
      ) {
        const wrapper = document.createElement("div");
        wrapper.className = "source-account-entry";
        wrapper.append(row, sourceNoticeElement());
        list.append(wrapper);
      } else {
        list.append(row);
      }

      // Xbox and Microsoft Store are one Microsoft account. Saying so once,
      // under the pair, beats letting someone connect twice and wonder why.
      if (status.sharesSignInWith.length > 0 && descriptor.provider === "microsoft-store") {
        const shared = document.createElement("p");
        shared.className = "settings-hint source-account-shared";
        shared.textContent =
          "Xbox and Microsoft Store share one Microsoft sign-in: connecting or disconnecting either affects both.";
        list.append(shared);
      }
    }
    body.append(list);

    if (restoreFocus && focusedAction && focusedProvider) {
      requestAnimationFrame(() => {
        const next = body.querySelector<HTMLElement>(
          `[data-source-action="${focusedAction}"][data-source-provider="${focusedProvider}"]`,
        );
        (next ?? body.querySelector<HTMLElement>(`[data-source-provider="${focusedProvider}"] button`))?.focus();
      });
    }
  };

  const sourceNoticeElement = (): HTMLParagraphElement => {
    const notice = document.createElement("p");
    notice.className = "steam-notice steam-notice--" + state.sourceAccounts.noticeTone;
    notice.setAttribute(
      "role",
      state.sourceAccounts.noticeTone === "error" ? "alert" : "status",
    );
    notice.textContent = state.sourceAccounts.notice;
    return notice;
  };

  const refreshSourceAccounts = async (): Promise<void> => {
    // Shares the settings generation counter with every other settings loader,
    // so a status that resolves after the section changed cannot overwrite a
    // fresher one.
    const request = settingsRequest;
    if (!isTauriRuntime()) {
      state.sourceAccounts.statuses = defaultSourceAccounts();
      state.sourceAccounts.loading = false;
      renderSourceAccountsPanel();
      return;
    }
    state.sourceAccounts.loading = true;
    renderSourceAccountsPanel();
    try {
      const statuses = normaliseSourceAccounts(await invoke<unknown>("get_source_accounts"));
      if (request !== settingsRequest) return;
      state.sourceAccounts.statuses = statuses;
      if (state.sourceAccounts.noticeTone !== "error") {
        setSourceNotice(null, "");
      }
    } catch (error) {
      if (request !== settingsRequest) return;
      setSourceNotice(
        null,
        messageFromError(error, "Library source connections could not be loaded."),
        "error",
      );
    }
    state.sourceAccounts.loading = false;
    renderSourceAccountsPanel();
  };

  const connectSourceAccount = async (provider: ConnectedSource): Promise<void> => {
    if (!isTauriRuntime() || state.sourceAccounts.busy.has(provider)) {
      return;
    }
    const label = connectedSourceDescriptor(provider).label;
    setSourceBusy(provider, true);
    setSourceNotice(provider, "");
    renderSourceAccountsPanel();
    try {
      await invoke<unknown>("connect_source_account", { provider });
      // The backend is the authority on what a sign-in produced, including for
      // the sibling source that shares the same account.
      await refreshSourceAccounts();
      // A session-style store imports its library as part of connecting, so
      // the Library has to be re-read either way.
      await refreshLibrary();
      setSourceNotice(provider, `${label} is connected.`, "success");
      showToast(`${label} is connected.`);
    } catch (error) {
      setSourceNotice(provider, messageFromError(error, `${label} could not be connected.`), "error");
    }
    setSourceBusy(provider, false);
    renderSourceAccountsPanel();
  };

  const syncSourceLibrary = async (provider: ConnectedSource): Promise<void> => {
    if (!isTauriRuntime() || state.sourceAccounts.busy.has(provider)) {
      return;
    }
    const label = connectedSourceDescriptor(provider).label;
    setSourceBusy(provider, true);
    setSourceNotice(provider, "");
    renderSourceAccountsPanel();
    try {
      const result = normaliseSourceSyncResult(
        await invoke<unknown>("sync_source_library", { provider }),
      );
      if (!result) {
        throw new Error(`${label} returned an invalid library sync.`);
      }
      state.sourceAccounts.lastSync.set(provider, result);
      await refreshLibrary();
      setSourceNotice(provider, sourceSyncSummary(result), "success");
      showToast(sourceSyncSummary(result));
    } catch (error) {
      setSourceNotice(provider, messageFromError(error, `${label} could not be synced.`), "error");
    }
    setSourceBusy(provider, false);
    renderSourceAccountsPanel();
  };

  /**
   * Re-sync every connected store, one after another.
   *
   * A library already in the catalog keeps the artwork it was imported with:
   * a connector that learns to fetch something new — a wordmark, a cleaner
   * wallpaper — changes nothing for the games already there. This is how the
   * user asks for that work to be done again without disconnecting anything.
   *
   * Sequential on purpose: each store is rate-limited on its own account, and
   * a burst of parallel syncs is how a provider decides to stop answering.
   */
  const resyncEveryLibrary = async (): Promise<void> => {
    if (!isTauriRuntime()) return;
    const connected = state.sourceAccounts.statuses
      .filter((status) => status.connected)
      .map((status) => status.provider);
    if (connected.length === 0) {
      showToast("Connect a store first — there is nothing to refresh yet.");
      return;
    }

    showToast(
      connected.length === 1
        ? "Refreshing 1 library…"
        : `Refreshing ${connected.length} libraries…`,
    );
    for (const provider of connected) {
      await syncSourceLibrary(provider);
    }
    showToast("Every connected library has been refreshed.");
  };

  const cancelSourceLogin = async (provider: ConnectedSource): Promise<void> => {
    if (!isTauriRuntime()) {
      return;
    }
    try {
      // Closing the window is what settles the pending connect: the awaiting
      // call resolves on its own, so nothing is cleared here.
      await invoke("cancel_source_login", { provider });
    } catch {
      // A window that is already gone is exactly the outcome asked for.
    }
  };

  const disconnectSourceAccount = async (
    provider: ConnectedSource,
    forgetGames: boolean,
  ): Promise<void> => {
    if (!isTauriRuntime()) {
      return;
    }
    const label = connectedSourceDescriptor(provider).label;
    state.sourceAccounts.pendingDisconnect = null;
    setSourceBusy(provider, true);
    renderSourceAccountsPanel();
    try {
      const removed = await invoke<unknown>("disconnect_source_account", {
        provider,
        forgetGames,
      });
      state.sourceAccounts.lastSync.delete(provider);
      const removedCount = typeof removed === "number" && Number.isFinite(removed) ? removed : 0;
      setSourceNotice(
        provider,
        forgetGames
          ? `${label} was disconnected and ${removedCount === 1 ? "1 game was" : `${removedCount} games were`} removed.`
          : `${label} was disconnected. Games already imported stay in your library.`,
        "info",
      );
      await refreshSourceAccounts();
      await refreshLibrary();
    } catch (error) {
      setSourceNotice(
        provider,
        messageFromError(error, `${label} could not be disconnected.`),
        "error",
      );
    }
    setSourceBusy(provider, false);
    renderSourceAccountsPanel();
  };

  const hydrateSteamPreviewMedia = async (
    preview: SteamPreview,
    request: number,
    candidates: SteamPreviewGame[] = preview.games,
  ): Promise<void> => {
    if (!isTauriRuntime() || preview.status !== "available") {
      return;
    }
    if ([...pendingSteamPreviewMediaIds.values()].some((pendingRequest) => pendingRequest === request)) {
      steamPreviewMediaRefreshQueued = true;
      return;
    }

    const appIds: string[] = [];
    for (const game of candidates) {
      if (
        appIds.length >= MAX_STEAM_PREVIEW_MEDIA ||
        (game.coverUrl || game.heroUrl) ||
        pendingSteamPreviewMediaIds.has(game.appId)
      ) {
        continue;
      }
      pendingSteamPreviewMediaIds.set(game.appId, request);
      appIds.push(game.appId);
    }
    if (appIds.length === 0) {
      return;
    }

    try {
      const media = await normaliseSteamPreviewMedia(
        await invoke<unknown>("get_steam_preview_media", { appIds }),
      );
      const currentPreview = state.steam.preview;
      if (request !== steamRequest || media.size === 0 || currentPreview?.status !== "available") {
        return;
      }

      state.steam.preview = {
        ...currentPreview,
        games: currentPreview.games.map((game) => {
          const cached = media.get(game.appId);
          return cached
            ? {
                ...game,
                coverUrl: game.coverUrl || cached.coverUrl,
                heroUrl: game.heroUrl || cached.heroUrl,
              }
            : game;
        }),
      };
      const activeSearch = refs.steamPanel.querySelector<HTMLInputElement>("#steam-game-search");
      const searchWasFocused = document.activeElement === activeSearch;
      const selectionStart = activeSearch?.selectionStart ?? null;
      const selectionEnd = activeSearch?.selectionEnd ?? null;
      renderSteamPanel();
      if (searchWasFocused) {
        requestAnimationFrame(() => {
          const nextSearch = refs.steamPanel.querySelector<HTMLInputElement>("#steam-game-search");
          nextSearch?.focus();
          if (nextSearch && selectionStart !== null && selectionEnd !== null) {
            nextSearch.setSelectionRange(selectionStart, selectionEnd);
          }
        });
      }
    } catch {
      // Preview artwork is optional. The list remains immediately usable if a
      // cache copy races Steam or cannot be read on this machine.
    } finally {
      for (const appId of appIds) {
        if (pendingSteamPreviewMediaIds.get(appId) === request) {
          pendingSteamPreviewMediaIds.delete(appId);
        }
      }
      if (
        request === steamRequest &&
        steamPreviewMediaRefreshQueued &&
        state.steam.preview?.status === "available"
      ) {
        steamPreviewMediaRefreshQueued = false;
        void hydrateSteamPreviewMedia(state.steam.preview, request, visibleSteamGames());
      }
    }
  };

  const scanSteamLibrary = async (): Promise<void> => {
    const request = ++steamRequest;
    pendingSteamPreviewMediaIds.clear();
    steamPreviewMediaRefreshQueued = false;
    state.steam.phase = "scanning";
    state.steam.notice = "";
    state.steam.query = "";
    renderSteamPanel();

    if (!isTauriRuntime()) {
      if (request !== steamRequest) {
        return;
      }
      state.steam.preview = {
        status: "unavailable",
        libraries: 0,
        games: [],
        message: "Steam scanning is available in the Orivo desktop app.",
      };
      state.steam.phase = "unavailable";
      renderSteamPanel();
      return;
    }

    try {
      const result = await invoke<unknown>("get_steam_import_preview");
      const preview = await normaliseSteamPreview(result);
      if (!preview) {
        throw new Error("Steam returned an invalid import preview.");
      }
      if (request !== steamRequest) {
        return;
      }

      state.steam.preview = preview;
      state.steam.phase = preview.status;
      const initiallySelected = preview.games.filter((game) => game.selected && !game.alreadyImported);
      state.steam.selectedAppIds = new Set(
        initiallySelected.length <= MAX_AUTOMATIC_STEAM_SELECTION
          ? initiallySelected.map((game) => game.appId)
          : [],
      );
      if (initiallySelected.length > MAX_AUTOMATIC_STEAM_SELECTION) {
        state.steam.notice =
          initiallySelected.length.toLocaleString() +
          " installed games found. Filter and choose the games you want to add (up to " +
          MAX_STEAM_IMPORT_SELECTION.toLocaleString() +
          ").";
        state.steam.noticeTone = "info";
      }

      if (!state.steam.open && preview.status === "available") {
        const count = preview.games.length;
        showToast(count === 0 ? "Steam is ready to import when you install a game." : "Steam found " + count + " installed games.");
      }
    } catch (error) {
      if (request !== steamRequest) {
        return;
      }
      state.steam.preview = {
        status: "error",
        libraries: 0,
        games: [],
        message: messageFromError(error, "Steam could not be scanned."),
      };
      state.steam.phase = "error";
      state.steam.selectedAppIds.clear();
    }

    renderSteamPanel();
    if (state.steam.open) {
      focusSteamPanel();
    }
    if (state.steam.preview?.status === "available") {
      void hydrateSteamPreviewMedia(state.steam.preview, request);
    }
  };

  const setSteamSelectionForVisibleGames = (selected: boolean): void => {
    let selectionWasCapped = false;
    for (const game of visibleSteamGames()) {
      if (selected) {
        if (!state.steam.selectedAppIds.has(game.appId) && state.steam.selectedAppIds.size >= MAX_STEAM_IMPORT_SELECTION) {
          selectionWasCapped = true;
          break;
        }
        state.steam.selectedAppIds.add(game.appId);
      } else {
        state.steam.selectedAppIds.delete(game.appId);
      }
    }
    if (selectionWasCapped) {
      state.steam.notice = "Choose up to " + MAX_STEAM_IMPORT_SELECTION.toLocaleString() + " games per import.";
      state.steam.noticeTone = "info";
    }
    renderSteamPanel();
  };

  const importSteamGames = async (): Promise<void> => {
    const preview = state.steam.preview;
    if (!preview || preview.status !== "available" || state.steam.phase === "importing") {
      return;
    }

    const appIds = preview.games
      .filter((game) => state.steam.selectedAppIds.has(game.appId))
      .map((game) => game.appId);
    if (appIds.length === 0) {
      return;
    }
    if (!isTauriRuntime()) {
      state.steam.notice = "Steam importing is available in the Orivo desktop app.";
      state.steam.noticeTone = "error";
      renderSteamPanel();
      return;
    }

    state.steam.phase = "importing";
    state.steam.notice = "";
    renderSteamPanel();

    try {
      const result = normaliseSteamImportResult(
        await invoke<unknown>("import_steam_games", { appIds }),
      );
      const changed = new Set([...result.importedIds, ...result.updatedIds]);
      state.steam.preview = {
        ...preview,
        games: preview.games.map((game) =>
          changed.has(game.appId) ? { ...game, alreadyImported: true, selected: false } : game,
        ),
      };
      for (const appId of [...result.importedIds, ...result.updatedIds, ...result.skippedAppIds]) {
        state.steam.selectedAppIds.delete(appId);
      }
      state.steam.phase = "available";
      state.steam.notice = steamImportSummary(result);
      state.steam.noticeTone = result.importedIds.length + result.updatedIds.length > 0 ? "success" : "info";

      await refreshLibrary();
      showToast(state.steam.notice);
    } catch (error) {
      state.steam.phase = "available";
      state.steam.notice = messageFromError(error, "Could not import the selected Steam games.");
      state.steam.noticeTone = "error";
    }

    renderSteamPanel();
  };

  const installDxvkMacosForProfile = async (profileId: string): Promise<void> => {
    const settings = state.wineSettings;
    if (!profileId || !isTauriRuntime() || settings.loading) {
      return;
    }
    settings.loading = true;
    settings.notice = "Downloading and verifying DXVK-macOS, then preparing the isolated prefix…";
    settings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("install_dxvk_macos_for_profile", { profileId });
      await refreshWineRunnerSettings(false);
      settings.notice = "DXVK-macOS is ready. Orivo uses it automatically for compatible DirectX 10/11 games in this profile.";
      settings.noticeTone = "success";
      showToast(settings.notice);
    } catch (error) {
      settings.notice = messageFromError(error, "DXVK-macOS could not be installed for this profile.");
      settings.noticeTone = "error";
    }
    settings.loading = false;
    renderWineSettingsPanel();
  };

  const useWine3dForProfile = async (profileId: string): Promise<void> => {
    const settings = state.wineSettings;
    if (settings.loading || !isTauriRuntime()) {
      return;
    }
    settings.loading = true;
    settings.notice = "Switching back to Wine 3D…";
    settings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("use_wine_3d_for_profile", { profileId });
      await refreshWineRunnerSettings(false);
      settings.notice = "This profile now uses Wine 3D. DXVK-macOS stays isolated in this profile.";
      settings.noticeTone = "success";
      showToast(settings.notice);
    } catch (error) {
      settings.notice = messageFromError(error, "Switching back to Wine 3D could not be saved.");
      settings.noticeTone = "error";
    }
    settings.loading = false;
    renderWineSettingsPanel();
  };

  const launchGame = async (requestedGameId?: string): Promise<void> => {
    const requested = requestedGameId
      ? state.games.find((candidate) => candidate.id === requestedGameId)
      : undefined;
    // A caller that names a game means *that* game. Falling back to the
    // Library's current selection here would launch a different game than the
    // one on screen, which is exactly what a deep link opened before the
    // library loaded would do.
    if (requestedGameId && !requested) {
      showToast("That game is not in your library yet.");
      return;
    }
    const game = requested ?? selectedGame();
    if (!isTauriRuntime()) {
      showToast("Launch is available in the Orivo desktop app.");
      return;
    }

    try {
      if (!game.launchable) {
        if (game.source === "steam") {
          showToast("Opening Steam to install " + game.title + "…");
          await invoke("install_steam_game", { gameId: game.id });
        } else if (game.source === "epic" && game.installState === "installing") {
          // The download the launcher is already running is the answer here,
          // not a reason to start a second one.
          showToast(game.title + " is already downloading.");
        } else if (game.source === "epic") {
          showToast("Starting the download for " + game.title + "…");
          await invoke("install_epic_game", { gameId: game.id });
          // Epic reports nothing back, so Orivo starts watching the launcher's
          // own manifests and paints the percentage itself. The grace ticks
          // cover the dialog the launcher puts up before the transfer begins.
          startInstallWatch(INSTALL_WATCH_GRACE_TICKS);
        } else if (game.source === "wine") {
          state.launchFeedback = {
            gameId: game.id,
            phase: "failed",
            message: "This Wine game cannot be launched with that profile.",
          };
          renderLaunchFeedback();
        } else {
          showToast("Visual showcase — import a local game to play.");
        }
        return;
      }
      if (game.source === "wine") {
        state.launchFeedback = {
          gameId: game.id,
          phase: "launching",
          message: "Launching with Wine-Staging…",
        };
        renderLaunchFeedback();
      }
      showToast(game.source === "wine" ? "Preparing Wine for " + game.title + "…" : "Launching " + game.title + "…");
      await invoke("launch_game", { gameId: game.id });
    } catch (error) {
      const message = messageFromError(error, "Could not launch " + game.title + ".");
      if (game.source === "wine") {
        state.launchFeedback = {
          gameId: game.id,
          phase: "failed",
          message,
        };
        renderLaunchFeedback();
      }
      showToast(message);
    }
  };

  const retryWineGameInCompatibility = async (gameId: string): Promise<void> => {
    if (!isTauriRuntime()) {
      return;
    }
    state.launchFeedback = {
      gameId,
      phase: "launching",
      message: "Preparing the next Wine compatibility mode…",
    };
    renderLaunchFeedback();
    try {
      await invoke("retry_wine_game_in_compatibility", { gameId });
      await launchGame(gameId);
    } catch (error) {
      const message = messageFromError(error, "That Wine compatibility mode is not available.");
      state.launchFeedback = { gameId, phase: "failed", message };
      renderLaunchFeedback();
      showToast(message);
    }
  };

  const applyMotionPreference = (): void => {
    root.dataset.motion = state.preferences.motion;
  };

  const renderPreferenceControls = (): void => {
    const startPage = root.querySelector<HTMLSelectElement>("#preference-start-page");
    const storeRegion = root.querySelector<HTMLSelectElement>("#preference-store-region");
    if (startPage) startPage.value = state.preferences.startPage;
    if (storeRegion) storeRegion.value = state.preferences.storeRegion;
    for (const input of root.querySelectorAll<HTMLInputElement>("input[name='motion-preference']")) {
      input.checked = input.value === state.preferences.motion;
    }
    const showcase = root.querySelector<HTMLInputElement>("#preference-show-showcase");
    if (showcase) showcase.checked = state.preferences.showShowcaseGames;
    const sampleSocial = root.querySelector<HTMLInputElement>("#preference-debug-social");
    if (sampleSocial) sampleSocial.checked = state.preferences.debugSampleSocial;
    const beta = root.querySelector<HTMLInputElement>("#preference-beta");
    if (beta) beta.checked = state.preferences.betaFeatures;
    applyBetaFeatures();
    applyMotionPreference();
  };

  /**
   * The price-data providers that are *not* a connectable library. A store you
   * can sign into shows its health inline on its own row instead, so nothing is
   * listed twice.
   */
  const renderProviderStatuses = (): void => {
    const list = root.querySelector<HTMLElement>("#provider-status-list");
    const group = root.querySelector<HTMLElement>(".source-providers");
    // Steam's row carries its health dot, so it has to be redrawn whenever
    // these statuses land.
    renderSteamSourceRow();
    if (!list) return;
    // Steam is excluded by name rather than through `providerStatusForSource`:
    // it is a row in this list now, but it is not a `ConnectedSource`, so it
    // has no descriptor for that mapping to return.
    const remaining = state.providerStatuses.filter(
      (provider) =>
        provider.provider !== "steam" && providerStatusForSource(provider.provider) === null,
    );
    if (group) group.hidden = remaining.length === 0;
    const fragment = document.createDocumentFragment();
    for (const provider of remaining) {
      const row = document.createElement("article");
      row.className = "provider-status-row";
      row.dataset.settingsSearchable = "";
      const copy = document.createElement("div");
      const title = document.createElement("strong");
      title.textContent = provider.label;
      const message = document.createElement("p");
      message.textContent = provider.message || "No status details available.";
      copy.append(title, message);
      const status = document.createElement("span");
      status.className = `provider-health provider-health--${provider.health}`;
      status.textContent = provider.health.replace("-", " ");
      row.append(copy, status);
      fragment.append(row);
    }
    list.replaceChildren(fragment);
    // The connectable rows carry the rest of the health chips, so they have to
    // repaint whenever provider status lands.
    renderSourceAccountsPanel();
  };

  /**
   * Map a store-data provider onto the library source it is the same shop as.
   * `microsoft` covers both Microsoft surfaces, and the pairing is what lets
   * one row speak for both meanings.
   */
  const providerStatusForSource = (provider: StoreProvider): ConnectedSource | null => {
    switch (provider) {
      case "epic":
        return "epic";
      case "gog":
        return "gog";
      case "ubisoft":
        return "ubisoft";
      case "instant-gaming":
        return "instant-gaming";
      case "microsoft":
        return "microsoft-store";
      default:
        return null;
    }
  };

  const renderDataUsage = (): void => {
    const size = root.querySelector<HTMLElement>("#derived-cache-size");
    const entries = root.querySelector<HTMLElement>("#derived-cache-entries");
    const freshness = root.querySelector<HTMLElement>("#derived-cache-freshness");
    if (size) size.textContent = formatDataSize(state.dataUsage.derivedCacheBytes);
    if (entries) {
      entries.textContent = `${state.dataUsage.derivedCacheEntries.toLocaleString()} derived ${
        state.dataUsage.derivedCacheEntries === 1 ? "entry" : "entries"
      }`;
    }
    if (freshness) freshness.textContent = formatFreshness(state.dataUsage.refreshedAt);
  };

  const renderUpdatePanel = (): void => {
    const button = root.querySelector<HTMLButtonElement>("#check-updates-button");
    const status = root.querySelector<HTMLElement>("#update-status");
    const progress = root.querySelector<HTMLElement>("#update-progress");
    const fill = progress?.querySelector<HTMLElement>(".update-progress__fill") ?? null;
    if (!button || !status || !progress) return;

    // Outside the desktop app there is no installer to run, so the action is
    // disabled and says why instead of failing on the first click.
    const description = isTauriRuntime()
      ? describeUpdateState(state.update)
      : {
          label: "Updates are installed by the Orivo desktop app.",
          detail: "Open Orivo on your Mac to check for and install a new version.",
          buttonLabel: "Check for updates",
          buttonDisabled: true,
        };

    button.textContent = description.buttonLabel;
    button.disabled = description.buttonDisabled;

    const label = document.createElement("span");
    label.className = "update-status__label";
    label.textContent = description.label;
    const detail = document.createElement("span");
    detail.className = "update-status__detail";
    detail.textContent = description.detail;
    status.replaceChildren(label, detail);

    const downloading = isTauriRuntime() && state.update.status === "downloading";
    progress.hidden = !downloading;
    const percent = downloading ? updateProgressPercent(state.update) : null;
    // A server that sends no content length gives no percentage; the bar runs
    // indeterminate rather than sitting at a false 0%.
    progress.classList.toggle("update-progress--indeterminate", downloading && percent === null);
    if (percent === null) {
      progress.removeAttribute("aria-valuenow");
    } else {
      progress.setAttribute("aria-valuenow", String(percent));
    }
    if (fill) fill.style.width = `${percent ?? 100}%`;
  };

  const renderSettingsSearch = (): void => {
    const term = state.settingsSearch.trim().toLocaleLowerCase();
    const activePanel = refs.settingsPanels.find((panel) => !panel.hidden);
    if (!activePanel) return;
    for (const item of activePanel.querySelectorAll<HTMLElement>("[data-settings-searchable]")) {
      item.hidden = Boolean(term) && !item.textContent?.toLocaleLowerCase().includes(term);
    }
    // The topbar search only ever filters the visible plugin card; it must not
    // reveal a plugin detail view the user did not open.
    renderPluginList();
  };

  const renderSettingsRoute = (route: Extract<AppRoute, { page: "settings" }>): void => {
    const definition = SETTINGS_SECTIONS.find((section) => section.id === route.section)!;
    refs.settingsTitle.textContent = definition.label;
    refs.settingsDescription.textContent = definition.description;
    for (const button of refs.settingsSectionButtons) {
      const active = button.dataset.settingsSection === route.section;
      button.classList.toggle("is-active", active);
      button.setAttribute("aria-selected", String(active));
      // A tablist is one Tab stop: Tab reaches the selected tab, the arrow keys
      // move between them.
      button.tabIndex = active ? 0 : -1;
    }
    for (const panel of refs.settingsPanels) {
      panel.hidden = panel.dataset.settingsPanel !== route.section;
    }
    renderPreferenceControls();
    renderProviderStatuses();
    renderDataUsage();
    renderUpdatePanel();
    renderSettingsSearch();
  };

  const loadPreferences = async (request = settingsRequest): Promise<void> => {
    if (!isTauriRuntime()) {
      renderPreferenceControls();
      return;
    }
    try {
      const preferences = normalisePreferences(await invoke<unknown>("get_preferences"));
      if (request !== settingsRequest) return;
      state.preferences = preferences;
      renderPreferenceControls();
    } catch (error) {
      if (request === settingsRequest) {
        showToast(messageFromError(error, "Preferences could not be loaded."));
      }
    }
  };

  const savePreferences = async (update: PreferencesUpdate): Promise<void> => {
    const previous = state.preferences;
    state.preferences = update.reset
      ? { ...DEFAULT_PREFERENCES }
      : normalisePreferences({ ...previous, ...update });
    renderPreferenceControls();
    if (!isTauriRuntime()) return;
    try {
      state.preferences = normalisePreferences(
        await invoke<unknown>("update_preferences", { update }),
      );
      renderPreferenceControls();
      showToast(update.reset ? "Preferences reset to defaults." : "Preferences saved.");
    } catch (error) {
      state.preferences = previous;
      renderPreferenceControls();
      showToast(messageFromError(error, "Preferences could not be saved."));
    }
  };

  const loadProviderStatuses = async (request = settingsRequest): Promise<void> => {
    if (!isTauriRuntime()) {
      renderProviderStatuses();
      return;
    }
    try {
      const result = await invoke<unknown>("get_store_home");
      if (request !== settingsRequest) return;
      const statuses = normaliseProviderStatuses(result);
      if (statuses.length > 0) state.providerStatuses = statuses;
    } catch {
      // Provider availability remains explicit even when the Store service is
      // offline or has not been integrated yet.
    }
    if (request === settingsRequest) renderProviderStatuses();
  };

  const loadDataUsage = async (request = settingsRequest): Promise<void> => {
    if (!isTauriRuntime()) {
      renderDataUsage();
      return;
    }
    try {
      const usage = normaliseDataUsage(await invoke<unknown>("get_data_usage"));
      if (request !== settingsRequest) return;
      state.dataUsage = usage;
      renderDataUsage();
    } catch (error) {
      if (request === settingsRequest) {
        showToast(messageFromError(error, "Data usage could not be loaded."));
      }
    }
  };

  const renderWallpaperCredentials = (): void => {
    refs.wallpaperIgdbClientId.value = state.wallpaperCredentials.igdbClientId;
    refs.wallpaperIgdbClientSecret.value = state.wallpaperCredentials.igdbClientSecret;
    refs.wallpaperGoogleApiKey.value = state.wallpaperCredentials.googleApiKey;
    refs.wallpaperGoogleCseId.value = state.wallpaperCredentials.googleCseId;
    refs.wallpaperSteamGridDbApiKey.value = state.wallpaperCredentials.steamgriddbApiKey;
    // An empty box is not an unset feature: the placeholder shows the default
    // term the search already uses, so the form reads as "override this".
    refs.wallpaperSearchTermCover.value = state.wallpaperCredentials.searchTermCover;
    refs.wallpaperSearchTermLandscape.value = state.wallpaperCredentials.searchTermLandscape;
    refs.wallpaperSearchTermBackground.value = state.wallpaperCredentials.searchTermBackground;
    refs.wallpaperSearchTermLogo.value = state.wallpaperCredentials.searchTermLogo;
    refs.wallpaperCredentialsSave.disabled = state.wallpaperCredentialsSaving;
    refs.wallpaperCredentialsSave.textContent = state.wallpaperCredentialsSaving
      ? "Saving…"
      : "Save keys";
  };

  const loadWallpaperCredentials = async (request = settingsRequest): Promise<void> => {
    if (!isTauriRuntime()) {
      renderWallpaperCredentials();
      return;
    }
    try {
      const credentials = normaliseWallpaperCredentials(
        await invoke<unknown>("get_wallpaper_credentials"),
      );
      if (request !== settingsRequest) return;
      state.wallpaperCredentials = credentials;
      renderWallpaperCredentials();
    } catch (error) {
      if (request === settingsRequest) {
        showToast(messageFromError(error, "Wallpaper keys could not be loaded."));
      }
    }
  };

  const saveWallpaperCredentials = async (): Promise<void> => {
    const update: WallpaperCredentialsUpdate = {
      igdbClientId: refs.wallpaperIgdbClientId.value.trim(),
      igdbClientSecret: refs.wallpaperIgdbClientSecret.value.trim(),
      googleApiKey: refs.wallpaperGoogleApiKey.value.trim(),
      googleCseId: refs.wallpaperGoogleCseId.value.trim(),
      steamgriddbApiKey: refs.wallpaperSteamGridDbApiKey.value.trim(),
      searchTermCover: refs.wallpaperSearchTermCover.value.trim(),
      searchTermLandscape: refs.wallpaperSearchTermLandscape.value.trim(),
      searchTermBackground: refs.wallpaperSearchTermBackground.value.trim(),
      searchTermLogo: refs.wallpaperSearchTermLogo.value.trim(),
    };
    if (!isTauriRuntime()) {
      state.wallpaperCredentials = { ...EMPTY_WALLPAPER_CREDENTIALS, ...update };
      renderWallpaperCredentials();
      showToast("Wallpaper keys are saved in the Orivo desktop app.");
      return;
    }
    const previous = state.wallpaperCredentials;
    state.wallpaperCredentialsSaving = true;
    renderWallpaperCredentials();
    try {
      state.wallpaperCredentials = normaliseWallpaperCredentials(
        await invoke<unknown>("update_wallpaper_credentials", { update }),
      );
      renderWallpaperCredentials();
      showToast("Wallpaper keys saved.");
    } catch (error) {
      state.wallpaperCredentials = previous;
      renderWallpaperCredentials();
      showToast(messageFromError(error, "Wallpaper keys could not be saved."));
    } finally {
      state.wallpaperCredentialsSaving = false;
      renderWallpaperCredentials();
    }
  };

  const refreshDerivedData = async (): Promise<void> => {
    const refresh = root.querySelector<HTMLButtonElement>("#refresh-derived-data");
    if (refresh) refresh.disabled = true;
    try {
      if (isTauriRuntime()) await invoke("refresh_store_sources");
      await Promise.all([loadProviderStatuses(), loadDataUsage()]);
      showToast("Derived data refreshed.");
    } catch (error) {
      showToast(messageFromError(error, "Derived data could not be refreshed."));
    } finally {
      if (refresh) refresh.disabled = false;
    }
  };

  const clearDerivedData = async (): Promise<void> => {
    const clear = root.querySelector<HTMLButtonElement>("#clear-derived-cache");
    if (clear) clear.disabled = true;
    try {
      if (isTauriRuntime()) {
        state.dataUsage = normaliseDataUsage(await invoke<unknown>("clear_derived_cache"));
      } else {
        state.dataUsage = { derivedCacheBytes: 0, derivedCacheEntries: 0, refreshedAt: null };
      }
      renderDataUsage();
      showToast("Derived cache cleared. Your library and game media were kept.");
    } catch (error) {
      showToast(messageFromError(error, "Derived cache could not be cleared."));
    } finally {
      if (clear) clear.disabled = false;
    }
  };

  // The handle is a reference to a resource held by the Rust side, and only
  // `close()` releases it. Dropping the reference instead would leak one entry
  // per check for the lifetime of the process.
  const releasePendingUpdate = (): void => {
    const previous = pendingUpdate;
    pendingUpdate = null;
    void previous?.close().catch(() => {});
  };

  const checkForUpdates = async (): Promise<void> => {
    state.update = startCheck(state.update);
    releasePendingUpdate();
    renderUpdatePanel();
    try {
      // Imported here, not at the top of the module: the plugin talks to the
      // Tauri IPC on load, which does not exist in the browser preview or in
      // jsdom. Keeping it lazy is what makes the whole page survive without it.
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      pendingUpdate = update;
      state.update = applyCheckResult(state.update, update);
    } catch (error) {
      state.update = applyError(state.update, error);
    }
    renderUpdatePanel();
  };

  const downloadAndInstallUpdate = async (): Promise<void> => {
    const update = pendingUpdate;
    if (!update) {
      // The handle is gone (a failed check cleared it), so re-check rather than
      // leave the button pointing at nothing.
      await checkForUpdates();
      return;
    }
    state.update = startDownload(state.update);
    renderUpdatePanel();
    try {
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          state.update = applyProgress(state.update, 0, event.data.contentLength ?? null);
        } else if (event.event === "Progress") {
          state.update = applyProgress(state.update, event.data.chunkLength);
        } else {
          state.update = markReady(state.update);
        }
        renderUpdatePanel();
      });
      // `Finished` is emitted before the installer returns, so readiness is
      // confirmed here as well: the button must never offer a restart for an
      // install that has not actually completed.
      state.update = markReady(state.update);
      renderUpdatePanel();
      showToast("Update installed. Restart Orivo to finish.");
    } catch (error) {
      state.update = applyError(state.update, error);
      renderUpdatePanel();
      showToast(messageFromError(error, "The update could not be installed."));
    }
  };

  const restartForUpdate = async (): Promise<void> => {
    try {
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
    } catch (error) {
      // Only the restart failed: the update is installed and still waiting, so
      // the row keeps offering it. Falling into the error state would send the
      // retry back to a fresh check, which would download the same build again.
      showToast(
        messageFromError(error, "Orivo could not restart. Quit and reopen it to finish updating."),
      );
    }
  };

  // One button drives the whole flow, so what it does is read off the state it
  // is currently rendering.
  const runUpdateAction = async (): Promise<void> => {
    if (!isTauriRuntime()) return;
    const status = state.update.status;
    if (status === "checking" || status === "downloading") return;
    if (status === "ready") {
      await restartForUpdate();
      return;
    }
    if (status === "available") {
      await downloadAndInstallUpdate();
      return;
    }
    await checkForUpdates();
  };

  root.querySelector<HTMLButtonElement>("#previous-game")?.addEventListener("click", () => moveSelection(-1));
  root.querySelector<HTMLButtonElement>("#next-game")?.addEventListener("click", () => moveSelection(1));
  root.querySelector<HTMLButtonElement>("#play-button")?.addEventListener("click", () => void launchGame());
  refs.launchFeedback.addEventListener("click", (event) => {
    const retry = (event.target as Element | null)?.closest<HTMLButtonElement>("[data-launch-action='retry']");
    const gameId = retry?.dataset.gameId;
    if (!gameId || state.launchFeedback?.gameId !== gameId) {
      return;
    }
    void retryWineGameInCompatibility(gameId);
  });

  refs.libraryMenuButton.addEventListener("click", () => {
    setLibraryMenuOpen(!state.libraryMenuOpen);
  });

  refs.libraryMenuButton.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setLibraryMenuOpen(true, "first");
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setLibraryMenuOpen(true, "last");
    } else if (event.key === "Escape" && state.libraryMenuOpen) {
      event.preventDefault();
      setLibraryMenuOpen(false, undefined, true);
    }
  });

  refs.libraryMenu.addEventListener("click", (event) => {
    const trigger = (event.target as Element | null)?.closest<HTMLButtonElement>("[data-library-action]");
    const action = trigger?.dataset.libraryAction;
    if (!trigger || !action) {
      return;
    }

    if (action === "source-steam") {
      // The connected Steam source goes straight to the existing installed-games
      // import that lives in Settings › Libraries & Sources.
      closeLibraryMenu();
      navigate({ page: "settings", section: "libraries", attachGameId: null });
      setSteamPanelOpen(true);
    } else if (action === "add-source") {
      // "Add a new source" opens the existing library connection flow: the
      // Settings › Libraries page auto-expands the Steam account connect card.
      closeLibraryMenu();
      navigate({ page: "settings", section: "libraries", attachGameId: null });
    } else if (action === "local") {
      void importGame();
    }
  });

  refs.libraryMenu.addEventListener("keydown", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLButtonElement)) {
      return;
    }

    const items = libraryMenuItems();
    const currentIndex = items.indexOf(target);
    if (currentIndex < 0) {
      return;
    }

    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      setLibraryMenuOpen(false, undefined, true);
      return;
    }

    let nextIndex: number | null = null;
    if (event.key === "ArrowDown") {
      nextIndex = (currentIndex + 1) % items.length;
    } else if (event.key === "ArrowUp") {
      nextIndex = (currentIndex - 1 + items.length) % items.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = items.length - 1;
    } else if (event.key === "Tab") {
      closeLibraryMenu();
      return;
    }

    if (nextIndex !== null) {
      event.preventDefault();
      event.stopPropagation();
      items[nextIndex]?.focus();
    }
  });

  refs.wineSettingsPanel.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const button = target?.closest<HTMLButtonElement>("[data-wine-action]");
    const action = button?.dataset.wineAction;
    if (!action) {
      return;
    }
    if (action === "install-dxvk-macos") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        void installDxvkMacosForProfile(profileId);
      }
    } else if (action === "use-wine-3d") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        void useWine3dForProfile(profileId);
      }
    } else if (action === "toggle-wine-profile") {
      const profileId = button?.dataset.profileId;
      const enabled = button?.dataset.enabled === "true";
      if (profileId) {
        void setWineProfileEnabled(profileId, enabled);
      }
    } else if (action === "delete-wine-profile") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        state.wineSettings.pendingDeleteProfileId = profileId;
        renderWineSettingsPanel();
      }
    } else if (action === "cancel-delete-wine-profile") {
      state.wineSettings.pendingDeleteProfileId = "";
      renderWineSettingsPanel();
    } else if (action === "confirm-delete-wine-profile") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        void deleteWineProfile(profileId);
      }
    }
  });

  refs.steamPanel.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const action = target?.closest<HTMLButtonElement>("[data-steam-action]")?.dataset.steamAction;

    if (action === "scan") {
      setSteamPanelOpen(true);
    } else if (action === "retry" || action === "refresh") {
      void scanSteamLibrary();
    } else if (action === "import") {
      void importSteamGames();
    }
  });

  refs.steamPanel.addEventListener("input", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLInputElement) || target.id !== "steam-game-search") {
      return;
    }

    const cursor = target.selectionStart;
    state.steam.query = target.value;
    renderSteamPanel();
    if (state.steam.preview?.status === "available") {
      void hydrateSteamPreviewMedia(state.steam.preview, steamRequest, visibleSteamGames());
    }
    requestAnimationFrame(() => {
      const next = refs.steamPanel.querySelector<HTMLInputElement>("#steam-game-search");
      if (next) {
        next.focus();
        if (cursor !== null) {
          next.setSelectionRange(cursor, cursor);
        }
      }
    });
  });

  refs.steamPanel.addEventListener("change", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLInputElement)) {
      return;
    }

    if (target.id === "steam-select-all") {
      setSteamSelectionForVisibleGames(target.checked);
      return;
    }

    const appId = target.dataset.steamAppId;
    if (!appId) {
      return;
    }

    if (target.checked) {
      if (
        !state.steam.selectedAppIds.has(appId) &&
        state.steam.selectedAppIds.size >= MAX_STEAM_IMPORT_SELECTION
      ) {
        state.steam.notice = "Choose up to " + MAX_STEAM_IMPORT_SELECTION.toLocaleString() + " games per import.";
        state.steam.noticeTone = "info";
      } else {
        state.steam.selectedAppIds.add(appId);
      }
    } else {
      state.steam.selectedAppIds.delete(appId);
    }
    renderSteamPanel();
  });

  refs.steamPanel.addEventListener("keydown", (event) => {
    const target = event.target;
    if (
      (event.key !== "ArrowDown" && event.key !== "ArrowUp") ||
      !(target instanceof HTMLInputElement) ||
      !target.dataset.steamAppId
    ) {
      return;
    }

    const toggles = Array.from(
      refs.steamPanel.querySelectorAll<HTMLInputElement>("input[data-steam-app-id]:not(:disabled)"),
    );
    const index = toggles.indexOf(target);
    if (index < 0) {
      return;
    }

    event.preventDefault();
    const nextIndex =
      event.key === "ArrowDown"
        ? Math.min(toggles.length - 1, index + 1)
        : Math.max(0, index - 1);
    toggles[nextIndex]?.focus();
  });

  // Steam's row and the two panels it expands share one card, so the listener
  // sits on the card rather than on each panel.
  refs.sourceAccountsPanel.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const action = target?.closest<HTMLButtonElement>("[data-steam-row-action]")?.dataset.steamRowAction;
    if (!action) return;

    if (action === "connect" || action === "manage") {
      setSteamAccountPanelOpen(!(state.steamAccount.open && action === "manage"));
    } else if (action === "sync") {
      void syncSteamAccountLibrary();
    } else if (action === "import") {
      setSteamPanelOpen(!state.steam.open);
    } else if (action === "close-import") {
      setSteamPanelOpen(false);
    }
  });

  refs.steamAccountPanel.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const action = target?.closest<HTMLButtonElement>("[data-steam-account-action]")?.dataset.steamAccountAction;
    if (!action) {
      return;
    }

    if (action === "connect") {
      void startSteamWebLogin();
    } else if (action === "cancel-wait") {
      if (isTauriRuntime()) {
        void invoke("cancel_steam_web_login").catch(() => {
          state.steamAccount.phase = state.steamAccount.status?.connected ? "connected" : "disconnected";
          renderSteamAccountPanel();
        });
      }
    } else if (action === "complete-login") {
      if (isTauriRuntime()) {
        void invoke("complete_steam_web_login").catch((error) => {
          state.steamAccount.notice = messageFromError(error, "Steam sign-in could not be checked.");
          state.steamAccount.noticeTone = "error";
          renderSteamAccountPanel();
        });
      }
    } else if (action === "api-key") {
      state.steamAccount.phase = "api-key";
      state.steamAccount.notice = "";
      renderSteamAccountPanel();
      focusSteamAccountPanel();
    } else if (action === "back") {
      state.steamAccount.phase = state.steamAccount.status?.connected ? "connected" : "disconnected";
      state.steamAccount.notice = "";
      renderSteamAccountPanel();
    } else if (action === "sync") {
      void syncSteamAccountLibrary();
    } else if (action === "disconnect") {
      void disconnectSteamAccount();
    }
  });

  refs.sourceAccountsPanel.addEventListener("click", (event) => {
    const trigger = (event.target as Element | null)?.closest<HTMLButtonElement>(
      "[data-source-action]",
    );
    const action = trigger?.dataset.sourceAction;
    if (!trigger || !action) {
      return;
    }
    if (action === "refresh") {
      void refreshSourceAccounts();
      return;
    }
    if (action === "resync") {
      void resyncEveryLibrary();
      return;
    }
    const provider = trigger.dataset.sourceProvider ?? "";
    if (!isConnectedSource(provider)) {
      return;
    }

    if (action === "connect") {
      void connectSourceAccount(provider);
    } else if (action === "sync") {
      void syncSourceLibrary(provider);
    } else if (action === "cancel") {
      void cancelSourceLogin(provider);
    } else if (action === "disconnect") {
      state.sourceAccounts.pendingDisconnect = provider;
      setSourceNotice(provider, "");
      renderSourceAccountsPanel();
    } else if (action === "disconnect-keep") {
      void disconnectSourceAccount(provider, false);
    } else if (action === "disconnect-forget") {
      void disconnectSourceAccount(provider, true);
    } else if (action === "disconnect-cancel") {
      state.sourceAccounts.pendingDisconnect = null;
      renderSourceAccountsPanel();
    }
  });

  refs.steamAccountPanel.addEventListener("submit", (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || form.dataset.steamAccountForm !== "api-key") {
      return;
    }
    event.preventDefault();
    const steamId = form.querySelector<HTMLInputElement>("input[name='steam-id']")?.value ?? "";
    const apiKey = form.querySelector<HTMLInputElement>("input[name='steam-api-key']")?.value ?? "";
    state.steamAccount.apiKeySteamId = steamId.trim();
    void connectSteamWithApiKey(steamId, apiKey);
  });

  if (isTauriRuntime()) {
    void listen<SteamAccountConnectedEvent>(STEAM_ACCOUNT_CONNECTED_EVENT, (event) => {
      const steamId = typeof event.payload?.steamId === "string" ? event.payload.steamId : "";
      state.steamAccount.open = true;
      state.steamAccount.status = { connected: true, steamId, method: "web" };
      // Let `syncSteamAccountLibrary` own the in-flight state. Setting this to
      // `syncing` here would make its guard treat the fresh login as an
      // existing sync and leave the panel waiting forever.
      state.steamAccount.phase = "connected";
      state.steamAccount.notice = "";
      renderSteamAccountPanel();
      void syncSteamAccountLibrary();
    });
    void listen(STEAM_ACCOUNT_LOGIN_CANCELLED_EVENT, () => {
      if (state.steamAccount.phase !== "connecting") {
        return;
      }
      state.steamAccount.phase = state.steamAccount.status?.connected ? "connected" : "disconnected";
      state.steamAccount.notice = "Steam sign-in was cancelled.";
      state.steamAccount.noticeTone = "info";
      renderSteamAccountPanel();
    });
    void listen<string>(STEAM_ACCOUNT_LOGIN_FAILED_EVENT, (event) => {
      state.steamAccount.phase = state.steamAccount.status?.connected ? "connected" : "disconnected";
      state.steamAccount.notice = messageFromError(event.payload, "Steam sign-in could not be saved.");
      state.steamAccount.noticeTone = "error";
      renderSteamAccountPanel();
    });
    void listen(STEAM_ACCOUNT_LOGIN_PENDING_EVENT, () => {
      if (state.steamAccount.phase !== "connecting") {
        return;
      }
      state.steamAccount.notice = "Steam sign-in is not complete yet. Finish in the Steam window, then try again.";
      state.steamAccount.noticeTone = "info";
      renderSteamAccountPanel();
    });
    // The connected-store commands already resolve with their own outcome, so
    // these listeners exist for the paths the awaiting caller cannot see: a
    // sibling source that the same sign-in connected, and a window the user
    // closed or that failed while the panel was on another settings section.
    void listen<{ provider?: string; accountLabel?: string }>(
      SOURCE_ACCOUNT_CONNECTED_EVENT,
      (event) => {
        const provider = event.payload?.provider ?? "";
        if (!isConnectedSource(provider)) return;
        void refreshSourceAccounts();
      },
    );
    void listen<{ provider?: string }>(SOURCE_ACCOUNT_LOGIN_CANCELLED_EVENT, (event) => {
      const provider = event.payload?.provider ?? "";
      if (!isConnectedSource(provider) || !state.sourceAccounts.busy.has(provider)) return;
      setSourceNotice(
        provider,
        `${connectedSourceDescriptor(provider).label} sign-in was cancelled.`,
        "info",
      );
      renderSourceAccountsPanel();
    });
    void listen<{ provider?: string; message?: string }>(
      SOURCE_ACCOUNT_LOGIN_FAILED_EVENT,
      (event) => {
        const provider = event.payload?.provider ?? "";
        if (!isConnectedSource(provider)) return;
        setSourceNotice(
          provider,
          messageFromError(
            event.payload?.message,
            `${connectedSourceDescriptor(provider).label} sign-in could not be saved.`,
          ),
          "error",
        );
        renderSourceAccountsPanel();
      },
    );
    void listen<unknown>(SOURCE_LIBRARY_SYNCED_EVENT, (event) => {
      const result = normaliseSourceSyncResult(event.payload);
      if (!result) return;
      state.sourceAccounts.lastSync.set(result.provider, result);
      renderSourceAccountsPanel();
    });
    void listen<WineLaunchStatusEvent>(WINE_LAUNCH_STATUS_EVENT, (event) => {
      const payload = event.payload;
      if (
        !payload ||
        typeof payload.gameId !== "string" ||
        (payload.phase !== "preparing" && payload.phase !== "started" && payload.phase !== "failed") ||
        typeof payload.message !== "string" ||
        !state.games.some((game) => game.id === payload.gameId && game.source === "wine")
      ) {
        return;
      }
      state.launchFeedback = {
        gameId: payload.gameId,
        phase: payload.phase === "preparing" ? "launching" : payload.phase,
        message: payload.message,
      };
      renderLaunchFeedback();
      if (payload.phase === "failed") {
        showToast(payload.message);
      }
    });
  }

  const loadAboutVersions = async (request = settingsRequest): Promise<void> => {
    const appVersion = root.querySelector<HTMLElement>("#about-app-version");
    const tauriVersion = root.querySelector<HTMLElement>("#about-tauri-version");
    if (!isTauriRuntime()) {
      if (appVersion) appVersion.textContent = "Development build";
      if (tauriVersion) tauriVersion.textContent = "Browser preview";
      renderUpdatePanel();
      return;
    }
    try {
      const [app, tauri] = await Promise.all([getVersion(), getTauriVersion()]);
      if (request !== settingsRequest) return;
      if (appVersion) appVersion.textContent = app;
      if (tauriVersion) tauriVersion.textContent = tauri;
      // The updater copy names the running version, so it is refreshed with the
      // rest of the About metadata rather than read separately.
      state.update = { ...state.update, currentVersion: app };
      renderUpdatePanel();
    } catch {
      // Version metadata is informational. A missing value must never keep the
      // About section from rendering its attributions.
    }
  };

  // ---------------------------------------------------------------------------
  // Shell: one topbar, one router, one host per page.
  // ---------------------------------------------------------------------------

  type NavPage = "library" | "store" | "me" | "settings";

  const navPageForRoute = (route: AppRoute): NavPage => {
    if (route.page === "store") return "store";
    if (route.page === "me") return "me";
    if (route.page === "settings") return "settings";
    if (route.page === "game") return route.from === "store" ? "store" : "library";
    return "library";
  };

  const syncTopbarSearch = (route: AppRoute): void => {
    // The topbar is visually identical on every page — the search is never
    // blanked out, because a 352px hole mid-bar reads as a broken layout. Only
    // what the field searches changes.
    if (route.page === "store") {
      refs.search.placeholder = "Search the store…";
      refs.search.setAttribute("aria-label", "Search the store");
      if (document.activeElement !== refs.search) refs.search.value = route.query;
      return;
    }
    if (route.page === "settings") {
      refs.search.placeholder = "Search settings…";
      refs.search.setAttribute("aria-label", "Search settings");
      if (refs.search.value !== state.settingsSearch) refs.search.value = state.settingsSearch;
      return;
    }
    // Library, game detail, and not-found all search the library. From a page
    // that cannot show the results, Enter takes the query to the Library.
    refs.search.placeholder = "Search games…";
    refs.search.setAttribute("aria-label", "Search your library");
    if (refs.search.value !== state.query) refs.search.value = state.query;
  };

  /**
   * Beta surfaces are hidden, not disabled: the nav link goes away and the route
   * stops being reachable, but the page and its data are untouched. A user who
   * is on Me when the switch goes off is walked back to the Library rather than
   * left on a page that no longer has a way back to it.
   */
  const applyBetaFeatures = (): void => {
    const beta = state.preferences.betaFeatures;
    for (const link of refs.navLinks) {
      if (link.dataset.navPage === "me") link.hidden = !beta;
    }
    if (!beta && currentRoute.page === "me") navigate({ page: "library" }, { replace: true });
  };

  const syncTopbar = (route: AppRoute): void => {
    const current = navPageForRoute(route);
    // The Library and the Store are full-bleed heroes with their own top
    // gradient, so the topbar floats over them completely transparent. Every
    // other page scrolls content underneath the bar and needs an opaque,
    // blurred scrim or that content reads straight through it.
    refs.topbar.classList.toggle(
      "topbar--over-content",
      route.page !== "library" &&
        route.page !== "store" &&
        route.page !== "game" &&
        route.page !== "me",
    );
    // The Library shows its wallpaper at full strength with nothing over it, so
    // the navigation there earns its contrast from a text shadow rather than
    // from a band of darkness across the top of the artwork.
    refs.topbar.classList.toggle("topbar--over-art", route.page === "library");
    for (const link of refs.navLinks) {
      const active = link.dataset.navPage === current;
      link.classList.toggle("is-active", active);
      // Exactly one navigation link is ever marked as the current page.
      if (active) {
        link.setAttribute("aria-current", "page");
      } else {
        link.removeAttribute("aria-current");
      }
    }
    syncTopbarSearch(route);
  };

  const libraryPage: AppPage = {
    mount() {
      // The Library scene is part of the shell markup, so there is nothing to
      // build here. Mount stays a no-op to keep hero images warm across visits.
    },
    activate(activation) {
      const restore = activation.restoreState;
      // The library query is not restored from page state: the topbar owns it
      // and outlives every navigation, so re-applying a snapshot here would
      // discard a query typed on another page on the way back.
      if (restore?.selectedGameId && state.games.some((game) => game.id === restore.selectedGameId)) {
        state.selectedId = restore.selectedGameId;
      }
      renderSelection(true);
      if (!restore) return;
      // The rail is a horizontal scroller, so the restored offset is its
      // `scrollLeft`; `PageRestoreState` keeps a single scroll field.
      refs.cards.scrollLeft = restore.scrollTop;
      const focusKey = restore.focusKey;
      if (!focusKey) return;
      requestAnimationFrame(() => {
        if (!activation.isCurrent()) return;
        Array.from(refs.cards.querySelectorAll<HTMLButtonElement>(".game-card"))
          .find((card) => card.dataset.gameId === focusKey)
          ?.focus({ preventScroll: true });
      });
    },
    deactivate(): PageRestoreState | null {
      closeLibraryMenu();
      const focused = document.activeElement;
      const focusKey =
        focused instanceof HTMLElement && focused.classList.contains("game-card")
          ? focused.dataset.gameId ?? null
          : null;
      return {
        scrollTop: refs.cards.scrollLeft,
        focusKey,
        selectedGameId: state.selectedId,
      };
    },
  };

  const settingsPage: AppPage = {
    mount() {
      // Settings is a normal page rendered from the shell markup: no backdrop,
      // no focus trap, and no Escape-to-close.
    },
    activate(activation) {
      const route = activation.route;
      if (route.page !== "settings") return;
      const request = ++settingsRequest;
      // A deep link that names a game (for example the game detail page's
      // "Configure Wine" action) opens the Wine runner detail directly;
      // otherwise the Plugins section opens its plugin browser.
      state.pluginView = route.section === "plugins" && route.attachGameId ? "wine" : "list";
      renderSettingsRoute(route);
      renderPluginList();
      renderWineSettingsPanel();
      renderSteamPanel();
      void loadPreferences(request);

      if (route.section === "libraries") {
        // The connect panel stays collapsed — the row is what the page shows —
        // but its status still has to be read, or the row cannot say whether
        // Steam is connected.
        renderSteamSourceRow();
        void refreshSteamAccountStatus();
        void refreshSourceAccounts();
        void loadProviderStatuses(request);
      } else if (state.steamAccount.open) {
        setSteamAccountPanelOpen(false);
      }
      if (route.section === "plugins") void refreshWineRunnerSettings();
      // The catalogue is re-read on every visit: a plugin installed from the
      // file picker in a previous session has to show up without a restart.
      if (route.section === "plugins") void pluginManager.load(activation.signal);
      if (route.section === "data") void loadDataUsage(request);
      if (route.section === "about") void loadAboutVersions(request);
      if (route.section === "plugins") void loadWallpaperCredentials(request);
      if (activation.restoreState) refs.settingsPage.scrollTop = activation.restoreState.scrollTop;
    },
    deactivate(): PageRestoreState | null {
      settingsRequest += 1;
      const scrollTop = refs.settingsPage.scrollTop;
      state.pluginView = "list";
      state.wineSettings.pendingDeleteProfileId = "";
      state.steam.open = false;
      renderPluginList();
      renderWineSettingsPanel();
      renderSteamPanel();
      // Closing through the setter is what cancels an in-flight Steam web
      // login; assigning `open` directly orphans the `steam-auth` window.
      setSteamAccountPanelOpen(false);
      return { scrollTop, focusKey: null };
    },
  };

  const notFoundPage: AppPage = {
    mount() {
      // The not-found page is static shell markup with a single way back.
    },
    activate(activation) {
      const route = activation.route;
      refs.notFoundDetail.textContent =
        route.page === "not-found" ? `Orivo has no page at “${route.path}”.` : "";
      requestAnimationFrame(() => {
        if (!activation.isCurrent()) return;
        refs.notFoundPage
          .querySelector<HTMLButtonElement>("[data-app-action='go-library']")
          ?.focus();
      });
    },
    deactivate(): PageRestoreState | null {
      return null;
    },
  };

  const storePage =
    options.storePage ?? createStorePage({ navigate: (route) => navigate(route) });
  const gameDetailPage =
    options.gameDetailPage ??
    createGameDetailPage({
      navigate: (route) => navigate(route),
      // A deep link opened without history still has somewhere to go back to.
      back: () => router.back({ page: "library" }),
      play: (gameId) => {
        void launchGame(gameId);
      },
      // Home art, a refetched cover or a removed game changed the catalog; pull
      // the library again so its cards and hero reflect it on the way back.
      onLibraryChanged: () => {
        void refreshLibrary();
      },
      // Debug overlay: when the Settings toggle is on, the detail page fills in
      // sample achievements, friends and activity for games that ship none.
      sampleSocialEnabled: () => state.preferences.debugSampleSocial,
    });

  const mePage = options.mePage ?? createMePage();

  const pageHosts: Record<AppRoute["page"], PageLifecycleHost> = {
    library: new PageLifecycleHost(refs.libraryPage, libraryPage),
    store: new PageLifecycleHost(refs.storePage, storePage),
    me: new PageLifecycleHost(refs.mePage, mePage),
    game: new PageLifecycleHost(refs.gamePage, gameDetailPage),
    settings: new PageLifecycleHost(refs.settingsPage, settingsPage),
    "not-found": new PageLifecycleHost(refs.notFoundPage, notFoundPage),
  };
  const restoreStates = new Map<AppRoute["page"], PageRestoreState | null>();
  let activePage: AppRoute["page"] | null = null;

  const dispatchRoute = (route: AppRoute): void => {
    currentRoute = route;
    syncTopbar(route);

    const leaving = activePage !== null && activePage !== route.page;
    if (leaving && activePage) {
      restoreStates.set(activePage, pageHosts[activePage].deactivate());
    }
    const returning = activePage !== route.page;
    activePage = route.page;
    const restore = returning ? restoreStates.get(route.page) ?? null : null;
    void pageHosts[route.page].activate(route, restore).catch((error: unknown) => {
      showToast(messageFromError(error, "This page could not be opened."));
    });
  };

  for (const link of refs.navLinks) {
    link.addEventListener("click", () => {
      const page = link.dataset.navPage;
      if (page === "store") {
        if (currentRoute.page === "store") return;
        navigate({ page: "store", category: "for-you", platforms: ["pc"], query: "" });
      } else if (page === "me") {
        if (currentRoute.page === "me") return;
        navigate({ page: "me" });
      } else if (page === "settings") {
        if (currentRoute.page === "settings") return;
        navigate({ page: "settings", section: "general", attachGameId: null });
      } else {
        navigate({ page: "library" });
      }
    });
  }

  refs.browseMode.addEventListener("click", () => cycleBrowseMode());
  refs.rageToggle.addEventListener("click", () => setRageMode(!state.browse.rage));

  refs.notFoundPage.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    if (target?.closest("[data-app-action='go-library']")) {
      navigate({ page: "library" }, { replace: true });
    }
  });

  refs.settingsPage.addEventListener("click", (event) => {
    const target = event.target as Element | null;

    const pluginId = target?.closest<HTMLButtonElement>("[data-plugin-open]")?.dataset.pluginOpen;
    if (pluginId === "wine" || pluginId === "wallpaper-searcher") {
      openPluginDetail(pluginId);
      return;
    }
    if (target?.closest("[data-plugin-back]")) {
      state.pluginView = "list";
      renderPluginList();
      renderWineSettingsPanel();
      return;
    }
    const installId = target?.closest<HTMLButtonElement>("[data-plugin-install]")?.dataset
      .pluginInstall;
    if (installId) {
      // The controller notifies on every phase, and `onChange` repaints the
      // row: nothing here waits for the install to finish.
      void pluginManager.installFromRegistry(installId);
      return;
    }
    const uninstallId = target?.closest<HTMLButtonElement>("[data-plugin-uninstall]")?.dataset
      .pluginUninstall;
    if (uninstallId) {
      void uninstallPlugin(uninstallId);
      return;
    }
    if (target?.closest("[data-plugin-install-file]")) {
      void installPluginFromFile();
      return;
    }

    const section = target?.closest<HTMLButtonElement>("[data-settings-section]")?.dataset
      .settingsSection;
    if (section) {
      navigate({
        page: "settings",
        section: section as SettingsSection,
        attachGameId: null,
      });
      return;
    }

    const action = target?.closest<HTMLButtonElement>("[data-settings-action]")?.dataset
      .settingsAction;
    if (action === "reset-preferences") {
      void savePreferences({ reset: true });
    } else if (action === "refresh-derived") {
      void refreshDerivedData();
    } else if (action === "clear-derived") {
      void clearDerivedData();
    } else if (action === "check-updates") {
      void runUpdateAction();
    }

    if (target?.closest("#wallpaper-credentials-save")) {
      void saveWallpaperCredentials();
    }
  });

  refs.settingsPage.addEventListener("change", (event) => {
    const target = event.target;
    if (target instanceof HTMLSelectElement) {
      if (target.id === "preference-start-page") {
        void savePreferences({ startPage: target.value as StartPage });
      } else if (target.id === "preference-store-region") {
        void savePreferences({ storeRegion: target.value as StoreRegion });
      }
      return;
    }
    if (
      target instanceof HTMLInputElement &&
      target.name === "motion-preference" &&
      target.checked
    ) {
      void savePreferences({ motion: target.value as MotionPreference });
    }
    if (target instanceof HTMLInputElement && target.id === "preference-show-showcase") {
      // Toggling the debug demo games re-seeds (or clears) the library.
      void savePreferences({ showShowcaseGames: target.checked }).then(() => {
        void refreshLibrary();
      });
    }
    if (target instanceof HTMLInputElement && target.id === "preference-debug-social") {
      // Purely a detail-page overlay: no library reload needed.
      void savePreferences({ debugSampleSocial: target.checked });
    }
    if (target instanceof HTMLInputElement && target.id === "preference-beta") {
      void savePreferences({ betaFeatures: target.checked });
    }
  });

  refs.settingsPage.addEventListener("input", (event) => {
    if (event.target === refs.pluginsCatalogSearch) {
      state.pluginCatalogSearch = refs.pluginsCatalogSearch.value;
      renderPluginList();
    }
  });

  refs.settingsPage.addEventListener("keydown", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLButtonElement) || !target.dataset.settingsSection) return;
    const buttons = refs.settingsSectionButtons;
    const index = buttons.indexOf(target);
    if (index < 0) return;

    let nextIndex: number | null = null;
    if (event.key === "ArrowDown" || event.key === "ArrowRight") {
      nextIndex = (index + 1) % buttons.length;
    } else if (event.key === "ArrowUp" || event.key === "ArrowLeft") {
      nextIndex = (index - 1 + buttons.length) % buttons.length;
    } else if (event.key === "Home") {
      nextIndex = 0;
    } else if (event.key === "End") {
      nextIndex = buttons.length - 1;
    }
    if (nextIndex === null) return;

    event.preventDefault();
    const next = buttons[nextIndex];
    next.focus();
    const section = next.dataset.settingsSection;
    if (section) {
      navigate({ page: "settings", section: section as SettingsSection, attachGameId: null });
    }
  });

  let storeSearchTimer: ReturnType<typeof setTimeout> | null = null;

  refs.search.addEventListener("input", () => {
    const route = currentRoute;
    const value = refs.search.value;
    if (route.page === "library") {
      state.query = value;
      const matches = visibleGames();
      if (matches.length > 0 && !matches.some((game) => game.id === state.selectedId)) {
        state.selectedId = matches[0].id;
      }
      renderSelection();
      return;
    }
    if (route.page === "settings") {
      state.settingsSearch = value;
      renderSettingsSearch();
      return;
    }
    if (route.page === "store") {
      // Typing filters the shelf live. Each keystroke replaces the entry rather
      // than pushing one, so Back still leaves the Store in one step.
      if (storeSearchTimer) clearTimeout(storeSearchTimer);
      storeSearchTimer = setTimeout(() => {
        storeSearchTimer = null;
        const current = currentRoute;
        if (current.page !== "store") return;
        navigate(
          {
            page: "store",
            category: current.category,
            platforms: [...current.platforms],
            query: refs.search.value.trim(),
          },
          { replace: true },
        );
      }, 240);
      return;
    }
    // Detail and not-found keep the library query warm so Enter can carry it.
    if (route.page === "game" || route.page === "not-found") {
      state.query = value;
    }
  });

  refs.search.addEventListener("keydown", (event) => {
    const route = currentRoute;
    if (event.key !== "Enter") return;
    if (route.page === "store") {
      event.preventDefault();
      navigate({
        page: "store",
        category: route.category,
        platforms: [...route.platforms],
        query: refs.search.value.trim(),
      });
      return;
    }
    if (route.page === "game" || route.page === "not-found") {
      // The results live in the Library, so Enter goes there with the query.
      event.preventDefault();
      state.query = refs.search.value;
      navigate({ page: "library" });
    }
  });

  document.addEventListener("pointerdown", (event) => {
    const target = event.target as Node;
    if (
      state.libraryMenuOpen &&
      !refs.libraryMenu.contains(target) &&
      !refs.libraryMenuButton.contains(target)
    ) {
      closeLibraryMenu();
    }
  });

  window.addEventListener("keydown", (event) => {
    // `composedTarget` rather than `event.target`: a listener on `window` sees
    // the shadow host, not the field inside it, and Sentry's feedback form
    // lives in a shadow root.
    const target = composedTarget(event);
    if (refs.libraryMenu.contains(target)) {
      return;
    }

    if (state.libraryMenuOpen && event.key === "Escape") {
      event.preventDefault();
      setLibraryMenuOpen(false, undefined, true);
      return;
    }

    const typing = isTypingEvent(event);
    const commandSearch = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k";

    // Focusing the contextual search is the only shortcut shared by every page.
    if ((commandSearch || (!typing && event.key === "/")) && !refs.search.disabled) {
      event.preventDefault();
      closeLibraryMenu();
      refs.search.focus();
      refs.search.select();
      return;
    }

    if (typing) {
      if (event.key === "Escape" && target === refs.search) {
        refs.search.blur();
      }
      return;
    }

    // Everything below belongs to the Library page and must never fire while
    // the Store, a game detail, Settings, or the not-found page is active.
    if (currentRoute.page !== "library") {
      return;
    }

    // With nothing focused there is no geometry to navigate from, so the rail
    // keeps its own 1-D shortcuts. Once a card holds focus, spatial navigation
    // takes over and can also walk off the rail to the rest of the page.
    const onRail = target?.classList.contains("game-card") === true;
    const adrift = target === null || target === document.body;

    switch (event.key) {
      case "ArrowLeft":
      case "ArrowUp":
        if (!adrift) break;
        event.preventDefault();
        moveSelection(-1);
        break;
      case "ArrowRight":
      case "ArrowDown":
        if (!adrift) break;
        event.preventDefault();
        moveSelection(1);
        break;
      case "Enter":
        // Enter launches the selected game outright; A opens its page first.
        if (!adrift && !onRail) break;
        event.preventDefault();
        void launchGame(selectedGame().id);
        break;
      case "a":
      case "A":
        if (!adrift) break;
        event.preventDefault();
        openGameDetail(selectedGame().id);
        break;
      case "i":
      case "I":
        event.preventDefault();
        if (event.shiftKey) {
          void importGame();
        } else {
          navigate({ page: "settings", section: "libraries", attachGameId: null });
        }
        break;
      case "Escape":
        closeLibraryMenu();
        break;
      default:
        break;
    }
  });

  // Arrow keys, then the same verbs on a controller. The engine reads the live
  // DOM, so pages only have to stay focusable — they never register anything.
  const spatialNav = createSpatialNav({
    openGame: (gameId) => {
      openGameDetail(gameId);
    },
    launchGame: (gameId) => {
      void launchGame(gameId);
    },
    back: () => {
      router.back({ page: "library" });
    },
  });

  createGamepadBridge({
    move: (direction) => spatialNav.move(direction),
    activate: () => void spatialNav.activate(),
    back: () => spatialNav.back(),
    launch: () => {
      if (!spatialNav.launchFocused()) spatialNav.activate();
    },
    focusSearch: () => {
      if (refs.search.disabled) return;
      refs.search.focus();
      refs.search.select();
    },
    cycleNav: (delta) => {
      const links = refs.navLinks;
      const index = links.findIndex((link) => link.classList.contains("is-active"));
      links[(index + delta + links.length) % links.length]?.click();
    },
    scroll: (delta) => spatialNav.scrollBy(delta),
    onActivity: () => spatialNav.setInputMode("gamepad"),
  });

  renderSteamPanel();
  renderWineSettingsPanel();
  renderPreferenceControls();
  /**
   * Crash reports and the feedback form.
   *
   * Started before the router so an error thrown during the first render is
   * still caught. Without a DSN this does nothing and the button stays hidden,
   * which is the state every test and every source build runs in.
   */
  if (initErrorReporting(isTauriRuntime() ? "desktop" : "browser")) {
    // The page and the game on screen travel with the report: "the covers are
    // wrong" is a shrug, the same sentence tagged with a title is a lead.
    const attached = attachFeedbackTo(refs.feedbackButton, () => ({
      page: currentRoute.page,
      game: selectedGame().title,
    }));
    refs.feedbackButton.hidden = !attached;
  }

  router.start((route) => {
    dispatchRoute(route);
    spatialNav.enterPage();
  });
  void refreshLibrary();
  void (async () => {
    await loadPreferences();
    // Preferences decide which surfaces exist, so the beta gate is applied the
    // moment they land rather than the first time Settings is opened.
    applyBetaFeatures();
    const hash = window.location.hash;
    const isDefaultEntry = hash === "" || hash === "#" || hash === "#/";
    if (isDefaultEntry && state.preferences.startPage === "store") {
      navigate({ page: "store", category: "for-you", platforms: ["pc"], query: "" }, { replace: true });
    }
  })();

  /**
   * Look for a new release once, shortly after the shell settles.
   *
   * Nothing is downloaded and nothing interrupts: the check writes the About
   * panel's status and stops there, so the only way an update installs is
   * still the user pressing the button. Deferred rather than immediate,
   * because a network round trip has no business competing with the first
   * paint of the library.
   */
  if (isTauriRuntime()) {
    // A shell that was torn down while the check was still waiting has no panel
    // left to write into: `renderUpdatePanel` would paint refs that are no
    // longer in the document, and the round trip would be spent on nothing.
    // Both delays are long enough for that to happen, so both are guarded.
    const stillMounted = (): boolean => root.isConnected;

    // `requestIdleCallback` takes an options object, `setTimeout` takes
    // milliseconds. Passing the object to both meant the fallback coerced it to
    // NaN and fired immediately, which is the opposite of waiting for quiet.
    window.setTimeout(() => {
      if (!stillMounted()) return;
      const idle = window.requestIdleCallback;
      if (idle) {
        idle(
          () => {
            if (stillMounted()) void checkForUpdates();
          },
          { timeout: 2_000 },
        );
      } else void checkForUpdates();
    }, AUTOMATIC_UPDATE_CHECK_DELAY_MS);
  }
}

async function loadLibrary(): Promise<LibraryLoad | null> {
  if (!isTauriRuntime()) {
    return {
      games: fallbackLibrary.map((game) => ({ ...game })),
      mediaTokens: new Map(),
    };
  }

  try {
    // Learn the media directory alongside the library, so cached artwork is
    // already resolvable when the records are normalised and the first paint
    // shows the real covers rather than a placeholder.
    const [result] = await Promise.all([invoke<unknown>("get_library"), primeMediaDirectory()]);
    const records = recordsFromResult(result);
    if (records.length === 0) {
      return {
        games: fallbackLibrary.map((game) => ({ ...game })),
        mediaTokens: new Map(),
      };
    }

    const normalised = records
      .map(normaliseGame)
      .filter((game): game is NormalisedLibraryGame => game !== null);
    return {
      games: normalised.map(({ game }) => game),
      mediaTokens: new Map(normalised.map(({ game, mediaTokens }) => [game.id, mediaTokens])),
    };
  } catch {
    return null;
  }
}

function recordsFromResult(result: unknown): BackendRecord[] {
  if (Array.isArray(result)) {
    return result.filter(isRecord);
  }

  if (isRecord(result) && Array.isArray(result.games)) {
    return result.games.filter(isRecord);
  }

  return [];
}

function normaliseGame(record: BackendRecord): NormalisedLibraryGame | null {
  const id = readString(record, "id");
  const title = readString(record, "title");
  if (!id || !title) {
    return null;
  }

  // Copy (description, genre, play state) may borrow from a matching fixture,
  // and only from one that matches by title.
  //
  // Artwork never does. Falling back to `lastUsedFallback` here is what put
  // Elden Ring's cover on every game whose art lives in the media cache: a
  // `cache:` token is not resolvable synchronously, so the fallback won, and
  // only the first handful of cards were ever hydrated back to the truth.
  const fallback = fallbackLibrary.find((game) => game.title === title) ?? lastUsedFallback;
  const artworkFallback = fallbackLibrary.find((game) => game.title === title) ?? null;
  const rawSource = readString(record, "source");
  const source: LibraryGame["source"] =
    rawSource === "steam" || rawSource === "local"
      ? rawSource
      : isConnectedSource(rawSource)
        ? rawSource
        : rawSource === "wine" || rawSource === "runner"
          ? "wine"
          : rawSource === "wine_staging" || rawSource === "wine-staging"
            ? "wine"
            : id.startsWith("runner:")
              ? "wine"
              : id.startsWith("showcase-")
                ? "showcase"
                : fallback.source ?? "local";
  const heroToken = readString(record, "heroUrl", "hero_url");
  // The wordmark travels the same road as the artwork: a logo a reset wrote to
  // the cache arrives as a `cache:` token, which an <img> cannot load until the
  // media directory has turned it into a real URL.
  const logoToken = readString(record, "logoUrl", "logo_url");
  const coverToken = readString(record, "coverUrl", "cover_url");
  const landscapeToken = readString(record, "landscapeUrl", "landscape_url");
  const rawHostPlatform = readString(record, "hostPlatform", "host_platform");
  const hostPlatform =
    rawHostPlatform === "windows" || rawHostPlatform === "macos" || rawHostPlatform === "linux" || rawHostPlatform === "other"
      ? rawHostPlatform
      : undefined;
  const supportedPlatforms = readStringArray(record, "supportedPlatforms", "supported_platforms")
    .filter((platform): platform is "windows" | "macos" | "linux" =>
      platform === "windows" || platform === "macos" || platform === "linux",
    );
  return {
    game: {
      id,
      title,
      source,
      description: readString(record, "description") || fallback.description,
      metadata: readString(record, "metadata") || fallback.metadata,
      genre: readString(record, "genre") || fallback.genre,
      heroUrl: immediateMediaUrl(heroToken) || artworkFallback?.heroUrl || "",
      coverUrl: immediateMediaUrl(coverToken) || artworkFallback?.coverUrl || "",
      landscapeUrl:
        immediateMediaUrl(landscapeToken) ||
        immediateMediaUrl(heroToken) ||
        artworkFallback?.landscapeUrl ||
        "",
      // An empty value is meaningful for a newly synced Steam game: it has
      // never been launched locally. Do not borrow a fixture's last-played
      // date merely because the backend intentionally returned an empty one.
      lastPlayedAt: readOptionalString(record, "lastPlayedAt", "last_played_at") ?? fallback.lastPlayedAt,
      logoUrl: immediateMediaUrl(logoToken),
      playTimeSeconds: readNumber(record, "playTimeSeconds", "play_time_seconds") ?? fallback.playTimeSeconds,
      launchable: readBoolean(record, "launchable") ?? fallback.launchable,
      hostPlatform,
      supportedPlatforms,
      compatibleWithHost: readBoolean(record, "compatibleWithHost", "compatible_with_host"),
      wineAttachable: readBoolean(record, "wineAttachable", "wine_attachable") ?? false,
      installState: readInstallState(record),
      installPercent: readNumber(record, "installPercent", "install_percent") ?? null,
      macCompatibility: readMacCompatibility(record),
    },
    mediaTokens: {
      heroUrl: heroToken,
      coverUrl: coverToken,
      landscapeUrl: landscapeToken,
      logoUrl: logoToken,
    },
  };
}


/**
 * Resolve an artwork token without awaiting. Remote and bundled URLs are usable
 * as-is; a `cache:` token resolves once the media directory has been learned,
 * which `primeMediaDirectory` does before the first library load.
 */
function immediateMediaUrl(value: string): string {
  if (value.startsWith("https://") || value.startsWith("/media/")) {
    return value;
  }
  return resolveMediaUrlSync(value);
}

function nestedRecord(record: BackendRecord, ...keys: string[]): BackendRecord | null {
  for (const key of keys) {
    const value = record[key];
    if (isRecord(value)) {
      return value;
    }
  }
  return null;
}

function normaliseWineRunnerStatus(value: unknown): WineRunnerStatus {
  const record = isRecord(value) ? nestedRecord(value, "runner", "runnerStatus", "runner_status") ?? value : {};
  const rawState = readString(record, "state", "status", "phase").toLocaleLowerCase();
  const reportedAvailable = readBoolean(record, "available", "installed", "isAvailable", "is_available");
  let state: WineRunnerState;
  if (rawState === "ready" || rawState === "available" || rawState === "installed") {
    state = "ready";
  } else if (rawState === "checking" || rawState === "loading") {
    state = "checking";
  } else if (rawState === "invalid" || rawState === "incompatible") {
    state = "invalid";
  } else if (rawState === "error" || rawState === "failed") {
    state = "error";
  } else if (rawState === "unavailable" || rawState === "missing" || rawState === "not_found") {
    state = "unavailable";
  } else {
    state = reportedAvailable === true ? "ready" : "unavailable";
  }

  return {
    state,
    available: reportedAvailable ?? state === "ready",
    version: readString(record, "version", "wineVersion", "wine_version", "label"),
    message: readString(record, "message"),
  };
}

function normaliseWineDirectory(value: unknown): WineDirectory | null {
  const record = isRecord(value) ? nestedRecord(value, "directory", "gameDirectory", "game_directory") ?? value : null;
  if (!record) {
    return null;
  }
  const id = readString(record, "directoryId", "directory_id", "id");
  if (!id) {
    return null;
  }
  return {
    id,
    label: readString(record, "directoryLabel", "directory_label", "label", "name") || "Game folder",
  };
}

function normaliseWineDirectories(value: unknown): WineDirectory[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const directories: WineDirectory[] = [];
  const seenIds = new Set<string>();
  for (const candidate of value) {
    const directory = normaliseWineDirectory(candidate);
    if (!directory || seenIds.has(directory.id)) {
      continue;
    }
    seenIds.add(directory.id);
    directories.push(directory);
  }
  return directories;
}

function normaliseWineLastImport(record: BackendRecord, fallback = ""): string {
  const supplied = readString(record, "lastImport", "last_import", "lastImportAt", "last_import_at");
  if (supplied) {
    return supplied;
  }
  const rawTimestamp = readNumber(record, "lastImportAt", "last_import_at");
  if (rawTimestamp === undefined || rawTimestamp <= 0) {
    return fallback;
  }
  const timestamp = rawTimestamp < 10_000_000_000 ? rawTimestamp * 1_000 : rawTimestamp;
  const date = new Date(timestamp);
  if (Number.isNaN(date.getTime())) {
    return fallback;
  }
  return date.toLocaleString();
}

function normaliseWineProfile(value: unknown, fallback?: WineProfile): WineProfile | null {
  const record = isRecord(value) ? nestedRecord(value, "profile", "wineProfile", "wine_profile") ?? value : null;
  if (!record) {
    return fallback ?? null;
  }
  const id = readString(record, "profileId", "profile_id", "id") || fallback?.id || "";
  if (!id) {
    return null;
  }
  const rawDirectories =
    record.directories ??
    record.gameDirectories ??
    record.game_directories;
  return {
    id,
    displayName: readString(record, "displayName", "display_name", "name") || fallback?.displayName || "Wine profile",
    enabled: readBoolean(record, "enabled") ?? fallback?.enabled ?? true,
    wineLabel:
      readString(record, "wineLabel", "wine_label", "wineVersion", "wine_version", "version") ||
      fallback?.wineLabel ||
      "Wine-Staging",
    directories: Array.isArray(rawDirectories)
      ? normaliseWineDirectories(rawDirectories)
      : [...(fallback?.directories ?? [])],
    lastImport: normaliseWineLastImport(record, fallback?.lastImport),
    lastImportSummary:
      readString(record, "lastImportSummary", "last_import_summary") ||
      fallback?.lastImportSummary ||
      "",
    graphicsBackend:
      readString(record, "graphicsBackend", "graphics_backend") === "dxvk_macos"
        ? "dxvk_macos"
        : "wine_d3d",
    graphicsSummary:
      readString(record, "graphicsSummary", "graphics_summary") ||
      fallback?.graphicsSummary ||
      "Wine 3D · default compatibility mode",
  };
}

function normaliseWineRunnerSettings(value: unknown): { runner: WineRunnerStatus; profiles: WineProfile[] } {
  const record = isRecord(value) ? value : {};
  const runnerValue = record.runner ?? record.runnerStatus ?? record.runner_status ?? record;
  const rawProfiles = Array.isArray(record.profiles)
    ? record.profiles
    : Array.isArray(record.wineProfiles)
      ? record.wineProfiles
      : [];
  const profiles: WineProfile[] = [];
  const seenIds = new Set<string>();
  for (const candidate of rawProfiles) {
    const profile = normaliseWineProfile(candidate);
    if (!profile || seenIds.has(profile.id)) {
      continue;
    }
    seenIds.add(profile.id);
    profiles.push(profile);
  }
  return {
    runner: normaliseWineRunnerStatus(runnerValue),
    profiles,
  };
}

async function normaliseSteamPreview(result: unknown): Promise<SteamPreview | null> {
  if (!isRecord(result)) {
    return null;
  }

  const rawStatus = readString(result, "status");
  if (rawStatus !== "available" && rawStatus !== "unavailable" && rawStatus !== "error") {
    return null;
  }

  const rawGames = Array.isArray(result.games) ? result.games.filter(isRecord) : [];
  const seenAppIds = new Set<string>();
  const games: SteamPreviewGame[] = [];
  for (const record of rawGames) {
    const appId = readSteamAppId(record);
    const title = readString(record, "title");
    if (!appId || !title || seenAppIds.has(appId)) {
      continue;
    }
    seenAppIds.add(appId);

    const alreadyImported = readBoolean(record, "alreadyImported", "already_imported") ?? false;
    // Resolving cache tokens involves desktop path APIs. Keep the first paint
    // bounded even when a user has already imported a very large library.
    const shouldResolveMedia = games.length < MAX_STEAM_PREVIEW_MEDIA;
    const coverToken = shouldResolveMedia ? readString(record, "coverUrl", "cover_url") : "";
    const heroToken = shouldResolveMedia ? readString(record, "heroUrl", "hero_url") : "";
    const [coverUrl, heroUrl] = await Promise.all([
      coverToken ? resolveMediaUrl(coverToken) : Promise.resolve(""),
      heroToken ? resolveMediaUrl(heroToken) : Promise.resolve(""),
    ]);

    games.push({
      appId,
      title,
      locationLabel: readString(record, "locationLabel", "location_label"),
      lastUpdated: readString(record, "lastUpdated", "last_updated"),
      selected: readBoolean(record, "selected") ?? !alreadyImported,
      alreadyImported,
      coverUrl,
      heroUrl,
    });
  }

  return {
    status: rawStatus,
    libraries: Math.max(0, Math.floor(readNumber(result, "libraries") ?? 0)),
    games,
    message: readString(result, "message"),
  };
}

async function normaliseSteamPreviewMedia(result: unknown): Promise<Map<string, SteamPreviewMedia>> {
  const records = Array.isArray(result) ? result.filter(isRecord) : [];
  const media = new Map<string, SteamPreviewMedia>();

  for (const record of records) {
    const appId = readSteamAppId(record);
    if (!appId || media.has(appId)) {
      continue;
    }
    const coverToken = readString(record, "coverUrl", "cover_url");
    const heroToken = readString(record, "heroUrl", "hero_url");
    const [coverUrl, heroUrl] = await Promise.all([
      coverToken ? resolveMediaUrl(coverToken) : Promise.resolve(""),
      heroToken ? resolveMediaUrl(heroToken) : Promise.resolve(""),
    ]);
    if (!coverUrl && !heroUrl) {
      continue;
    }
    media.set(appId, { appId, coverUrl, heroUrl });
  }

  return media;
}

function normaliseSteamImportResult(result: unknown): SteamImportResult {
  if (!isRecord(result)) {
    throw new Error("Steam returned an invalid import result.");
  }

  return {
    importedIds: readStringArray(result, "importedIds", "imported_ids"),
    updatedIds: readStringArray(result, "updatedIds", "updated_ids"),
    skippedAppIds: readStringArray(result, "skippedAppIds", "skipped_app_ids"),
  };
}

function normaliseSteamAccountStatus(result: unknown): SteamAccountStatus | null {
  if (!isRecord(result)) {
    return null;
  }
  const connected = readBoolean(result, "connected") ?? false;
  const steamId = readString(result, "steamId", "steam_id");
  const rawMethod = readString(result, "method");
  const method = rawMethod === "web" || rawMethod === "api_key" ? rawMethod : "";
  if (connected && (!steamId || !method)) {
    return null;
  }
  return { connected, steamId, method };
}

function normaliseSteamAccountSyncResult(result: unknown): SteamAccountSyncResult | null {
  if (!isRecord(result)) {
    return null;
  }
  const totalGames = readNumber(result, "totalGames", "total_games");
  const importedGames = readNumber(result, "importedGames", "imported_games");
  const updatedGames = readNumber(result, "updatedGames", "updated_games");
  const installedGames = readNumber(result, "installedGames", "installed_games");
  if (
    totalGames === undefined ||
    importedGames === undefined ||
    updatedGames === undefined ||
    installedGames === undefined
  ) {
    return null;
  }
  return {
    totalGames: Math.max(0, Math.floor(totalGames)),
    importedGames: Math.max(0, Math.floor(importedGames)),
    updatedGames: Math.max(0, Math.floor(updatedGames)),
    installedGames: Math.max(0, Math.floor(installedGames)),
  };
}

function steamAccountSyncSummary(result: SteamAccountSyncResult): string {
  const changes = result.importedGames + result.updatedGames;
  if (result.totalGames === 0) {
    return "Steam is connected, but no owned games were returned.";
  }
  if (changes === 0) {
    return result.totalGames.toLocaleString() + " Steam games are already up to date.";
  }
  return (
    result.totalGames.toLocaleString() +
    " owned games synced · " +
    result.installedGames.toLocaleString() +
    " installed on this Mac."
  );
}

function steamImportSummary(result: SteamImportResult): string {
  const parts: string[] = [];
  if (result.importedIds.length > 0) {
    parts.push(result.importedIds.length === 1 ? "1 game imported" : result.importedIds.length + " games imported");
  }
  if (result.updatedIds.length > 0) {
    parts.push(result.updatedIds.length === 1 ? "1 library entry updated" : result.updatedIds.length + " library entries updated");
  }
  if (result.skippedAppIds.length > 0) {
    parts.push(result.skippedAppIds.length === 1 ? "1 game skipped" : result.skippedAppIds.length + " games skipped");
  }

  return parts.length > 0 ? parts.join(" · ") + "." : "No games needed importing.";
}

function readImportedId(result: unknown): string | undefined {
  if (typeof result === "string") {
    return result;
  }

  return isRecord(result)
    ? readString(result, "importedId", "imported_id", "id", "gameId", "game_id") || undefined
    : undefined;
}

function readString(record: BackendRecord, ...keys: string[]): string {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string") {
      return value;
    }
  }
  return "";
}

function readOptionalString(record: BackendRecord, ...keys: string[]): string | undefined {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "string") {
      return value;
    }
  }
  return undefined;
}

function readNumber(record: BackendRecord, ...keys: string[]): number | undefined {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "number" && Number.isFinite(value)) {
      return value;
    }
  }
  return undefined;
}

function readBoolean(record: BackendRecord, ...keys: string[]): boolean | undefined {
  for (const key of keys) {
    const value = record[key];
    if (typeof value === "boolean") {
      return value;
    }
  }
  return undefined;
}

function readInstallState(record: BackendRecord): LibraryGame["installState"] {
  const value = readString(record, "installState", "install_state");
  return value === "installed" ||
    value === "installing" ||
    value === "not-installed"
    ? value
    : "unknown";
}

function readMacCompatibility(
  record: BackendRecord,
): LibraryGame["macCompatibility"] {
  const value = readString(record, "macCompatibility", "mac_compatibility");
  return value === "native" || value === "not-native" ? value : "unknown";
}

function readStringArray(record: BackendRecord, ...keys: string[]): string[] {
  for (const key of keys) {
    const value = record[key];
    if (Array.isArray(value)) {
      return value.filter((item): item is string => typeof item === "string" && item.length > 0);
    }
  }
  return [];
}

function readSteamAppId(record: BackendRecord): string {
  const value = record.appId ?? record.app_id;
  const parsed =
    typeof value === "string" && /^\d+$/.test(value.trim())
      ? Number(value.trim())
      : typeof value === "number"
        ? value
        : Number.NaN;
  return Number.isSafeInteger(parsed) && parsed > 0 && parsed <= 0xffff_ffff ? String(parsed) : "";
}

function isRecord(value: unknown): value is BackendRecord {
  return typeof value === "object" && value !== null;
}

function messageFromError(error: unknown, fallback: string): string {
  if (typeof error === "string" && error.trim()) {
    return error;
  }

  if (error instanceof Error && error.message) {
    return error.message;
  }

  return fallback;
}

function prefersReducedMotion(): boolean {
  // The Appearance preference wins over the system setting; "system" falls back
  // to the media query so the default still honours macOS accessibility.
  if (document.querySelector('[data-motion="reduced"]')) {
    return true;
  }
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function shell(): string {
  return `
      <!-- The topbar is the document banner, so it sits outside the page
           wrapper: a <header> nested in <main> is not a banner, and each page
           owns the only <main> on screen. -->
      <header class="topbar" aria-label="Primary navigation" data-tauri-drag-region>
        <div class="nav-cluster">
          <div class="library-menu-control">
            <button id="library-menu-button" class="brand-mark-button" type="button" aria-label="Open library sources" aria-haspopup="menu" aria-expanded="false" aria-controls="library-source-menu">
              <img id="brand-mark-ring" class="brand-mark" src="/media/orivo-ring-icon.png" alt="" />
              <!-- Rage mode wears the spiral instead of the ring. Both marks
                   live in the markup so the swap is a class, not a fetch. -->
              <img id="brand-mark-spiral" class="brand-mark" src="/media/orivo-spiral-icon.png" alt="" hidden />
            </button>
            <div id="library-source-menu" class="library-source-menu" role="menu" aria-label="Library sources" hidden>
              <p class="library-source-menu__label" id="library-sources-label">Sources</p>
              <!-- The connected sources are rendered on open, so the list always
                   reflects the current library rather than a hardcoded set. -->
              <div id="library-source-list" role="group" aria-labelledby="library-sources-label"></div>
              <button type="button" class="library-source-action" role="menuitem" data-library-action="add-source">
                <span class="library-source-action__icon" aria-hidden="true">${icon("collections")}</span>
                <span class="library-source-action__copy"><strong>Add a new source</strong><small>Connect another library to Orivo</small></span>
                ${icon("chevron-right", "library-source-action__chevron")}
              </button>
              <p class="library-source-menu__label">This Mac</p>
              <button type="button" class="library-source-action" role="menuitem" data-library-action="local">
                <span class="library-source-action__icon" aria-hidden="true">${icon("folder")}</span>
                <span class="library-source-action__copy"><strong>Import a local game</strong><small>Pick an app or executable on this Mac</small></span>
              </button>
            </div>
          </div>
          <span class="top-divider" aria-hidden="true"></span>
          <nav class="primary-nav" aria-label="Orivo navigation">
            <button type="button" class="nav-link is-active" data-nav-page="library" aria-current="page">${icon("library")}<span>Library</span></button>
            <button type="button" class="nav-link" data-nav-page="store">${icon("store")}<span>Store</span></button>
            <button type="button" class="nav-link" data-nav-page="me">${icon("user")}<span>Me</span></button>
            <button type="button" class="nav-link" data-nav-page="settings">${icon("settings")}<span>Settings</span></button>
          </nav>
        </div>

        <label class="search-control" id="topbar-search-control">
          ${icon("search")}
          <input id="topbar-search" type="search" autocomplete="off" spellcheck="false" placeholder="Search games…" aria-label="Search games" />
          <span class="search-shortcut" aria-hidden="true"><kbd>⌘</kbd><kbd>K</kbd></span>
        </label>

        <div class="profile-cluster">
          <!-- Reporting a problem sits beside the profile picture rather than
               buried in Settings: the moment someone wants to complain is the
               moment they are looking at the thing that went wrong. Hidden
               until Sentry is configured, so a source build shows no dead
               control. -->
          <button id="feedback-button" class="feedback-button" type="button" aria-label="Send feedback" title="Send feedback" hidden>${icon("feedback")}</button>
          <img class="avatar" src="/media/steam-avatar.png" alt="Steam profile" />
        </div>
      </header>

      <div class="selector">
      <div id="app-page-library" class="app-page app-page--library">
      <div class="hero-media" aria-hidden="true">
        <img id="hero-a" class="hero-image" alt="" />
        <img id="hero-b" class="hero-image" alt="" />
      </div>
      <!-- Two scrims and no blur: a floor under the rail and the browse bar,
           and a light shading down the left that gives the text block a little
           ground of its own. Both stay far below the wash the scene used to
           carry — the artwork is still the point of the page, and the text
           leans on its shadow in styles.css for the rest. -->
      <div class="scene-overlay scene-overlay--left" aria-hidden="true"></div>
      <div class="scene-overlay scene-overlay--bottom" aria-hidden="true"></div>

      <button id="previous-game" class="scene-arrow scene-arrow--previous" type="button" aria-label="Previous game">${icon("chevron-left")}</button>
      <button id="next-game" class="scene-arrow scene-arrow--next" type="button" aria-label="Next game">${icon("chevron-right")}</button>

      <section class="hero-content" aria-live="polite">
        <!-- The wordmark when a store published one, the title text when it did
             not. Both live in the markup: the logo is a swap, not a rebuild,
             and the title keeps the accessible name either way. The genre pill
             follows the mark rather than leading it — the mark is what the eye
             should land on first. -->
        <img id="hero-logo" class="hero-logo" alt="" hidden />
        <h1 id="hero-title" class="hero-title">Elden Ring</h1>
        <span id="hero-genre" class="genre-chip">RPG</span>
        <div class="hero-meta" aria-label="Game metadata">
          <span>${icon("clock")}<span id="hero-play-time"></span></span>
          <span>${icon("clock")}<span id="hero-last-played"></span></span>
          <span id="hero-source" class="hero-source">
            <span id="hero-source-icon" class="hero-source-icon" aria-hidden="true"></span>
            <span id="hero-source-label">Steam</span>
            <i id="hero-source-divider" class="hero-source-divider" aria-hidden="true">·</i>
            <span id="hero-metadata"></span>
          </span>
          <span id="hero-platform" class="hero-platform" hidden>
            ${icon("monitor")}<span id="hero-platform-label"></span>
          </span>
        </div>
        <!-- What the Play button cannot say: whether the game is actually on
             this machine, and whether it runs natively here. -->
        <div id="hero-status" class="hero-status" hidden></div>
        <!-- Play stands alone. A card opens its own page on a second click and
             the detail route is a link away, so a chevron beside Play was one
             affordance too many for the one thing this scene is for. -->
        <div class="hero-actions">
          <button id="play-button" class="play-button" type="button"><span class="play-button__fill" hidden></span>${icon("play")}<span>Play</span></button>
        </div>
        <div id="launch-feedback" class="launch-feedback" role="status" aria-live="polite" hidden></div>
      </section>

      <section class="recently-played" aria-label="Library games">
        <div class="rail-header">
          <!-- The heading is the active segment: the rail and its title can
               never disagree about what is on screen. -->
          <h2 id="recently-played-title">Recently Played</h2>
        </div>
        <div id="game-cards" class="game-cards" role="list" aria-label="Recently Played"></div>
      </section>

      <!-- The browse bar. One button cycles the mode, the row beside it holds
           that mode's segments, and the mark on the left is the app's own
           mood. -->
      <footer class="browse-bar" aria-label="Library browsing">
        <!-- A switch, not a button that happens to remember a state: the role
             says so, and the track says so. -->
        <button id="rage-toggle" class="browse-bar__mood" type="button" role="switch" aria-checked="false">
          <span class="browse-bar__switch" aria-hidden="true"><i></i></span>
          <span id="rage-label">Orivo</span>
        </button>
        <div id="browse-segments" class="browse-bar__segments" role="group" aria-label="Library segments"></div>
        <button id="browse-mode" class="browse-bar__mode" type="button">
          <span id="browse-mode-label">Activity</span>
        </button>
      </footer>
      </div>

      <div id="app-page-store" class="app-page app-page--store"></div>
      <div id="app-page-me" class="app-page app-page--scroll app-page--overlay"></div>

      <div id="app-page-game" class="app-page app-page--fit app-page--overlay"></div>

      <div id="app-page-settings" class="app-page app-page--scroll app-page--settings">
        <div class="settings-layout">
          <nav class="settings-sidebar" aria-label="Settings sections">
            <p class="settings-sidebar__title">Settings</p>
            <div class="settings-sidebar__list" role="tablist" aria-orientation="vertical" aria-label="Settings sections">
              ${SETTINGS_SECTIONS.map(
                (section) => `
              <button type="button" role="tab" class="settings-section-link" id="settings-tab-${section.id}" data-settings-section="${section.id}" aria-controls="settings-panel-${section.id}" aria-selected="false" tabindex="-1">
                <strong>${section.label}</strong>
                <small>${section.description}</small>
              </button>`,
              ).join("")}
            </div>
          </nav>

          <div class="settings-content">
            <header class="settings-header">
              <h1 id="settings-page-title">General</h1>
              <p id="settings-page-description">Startup and store defaults</p>
            </header>

            <section class="settings-panel" role="tabpanel" id="settings-panel-general" data-settings-panel="general" aria-labelledby="settings-tab-general" tabindex="0">
              <div class="settings-card" data-settings-searchable>
                <div class="settings-row">
                  <label class="settings-row__copy" for="preference-start-page">
                    <strong>Start page</strong>
                    <small>The page Orivo opens on launch.</small>
                  </label>
                  <select id="preference-start-page" class="settings-select">
                    <option value="library">Library</option>
                    <option value="store">Store</option>
                  </select>
                </div>
                <div class="settings-row">
                  <label class="settings-row__copy" for="preference-store-region">
                    <strong>Store region</strong>
                    <small>Chooses which prices and availability Orivo shows.</small>
                  </label>
                  <select id="preference-store-region" class="settings-select">
                    <option value="automatic">Automatic</option>
                    <option value="us">United States</option>
                    <option value="ca">Canada</option>
                    <option value="gb">United Kingdom</option>
                    <option value="fr">France</option>
                    <option value="de">Germany</option>
                    <option value="jp">Japan</option>
                    <option value="au">Australia</option>
                  </select>
                </div>
              </div>
              <div class="settings-card" data-settings-searchable>
                <div class="settings-row">
                  <div class="settings-row__copy">
                    <strong>Reset preferences</strong>
                    <small>Restores the start page, store region, and motion preference. Your library, Wine profiles, Steam connection, wishlist, and media are never touched.</small>
                  </div>
                  <button id="reset-preferences" type="button" class="settings-button settings-button--quiet" data-settings-action="reset-preferences">Reset preferences</button>
                </div>
              </div>
            </section>

            <section class="settings-panel" role="tabpanel" id="settings-panel-libraries" data-settings-panel="libraries" aria-labelledby="settings-tab-libraries" tabindex="0" hidden>
              <!-- Connecting a library and reading that store's prices were two
                   cards saying different things about the same shops. They are
                   one list now: each store shows its connection and, on the same
                   row, whether its price data is reachable. Steam is in that
                   list too — it used to own two cards above it, which made the
                   one store that needs no introduction the loudest thing on the
                   page. Its two extra affordances, signing in and scanning this
                   Mac for installed games, expand under its row. -->
              <section id="source-accounts-panel" class="settings-card" data-settings-searchable aria-labelledby="source-accounts-title">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("collections")}</span>
                  <div class="settings-card__copy">
                    <strong id="source-accounts-title">Game libraries &amp; providers</strong>
                    <small>Connect a store to see the games you own, and check where store data can come from.</small>
                  </div>
                  <!-- Two different refreshes, and the difference matters: the
                       first only re-reads which accounts are connected, the
                       second re-syncs every one of their libraries. -->
                  <button id="source-accounts-resync" type="button" class="steam-header-button steam-header-button--label" data-source-action="resync">Refresh all libraries</button>
                  <button id="source-accounts-refresh" type="button" class="steam-header-button" data-source-action="refresh" aria-label="Refresh library source connections">${icon("refresh")}</button>
                </header>

                <div class="steam-source-block">
                  <div id="steam-source-row"></div>
                  <div id="steam-account-panel" class="steam-inline-panel" aria-labelledby="steam-account-title" hidden>
                    <strong id="steam-account-title" class="steam-inline-panel__title">Steam library</strong>
                    <div id="steam-account-body" class="steam-account-body"></div>
                  </div>
                  <div id="steam-import-panel" class="steam-inline-panel" aria-labelledby="steam-import-title" aria-describedby="steam-import-detail" hidden>
                    <div class="steam-inline-panel__header">
                      <div class="settings-card__copy">
                        <strong id="steam-import-title" class="steam-inline-panel__title">Import installed games</strong>
                        <small id="steam-import-detail">A local Steam source</small>
                      </div>
                      <button id="steam-refresh" type="button" class="steam-header-button" data-steam-action="refresh" aria-label="Refresh Steam library" hidden>${icon("refresh")}</button>
                      <button type="button" class="steam-header-button" data-steam-row-action="close-import" aria-label="Close installed games">${icon("close")}</button>
                    </div>
                    <div id="steam-import-body" class="steam-import-body"></div>
                    <footer id="steam-import-footer" class="steam-import-footer" hidden>
                      <p id="steam-selection-summary"></p>
                      <button id="steam-import-selected" class="steam-import-button" type="button" data-steam-action="import">Import selected</button>
                    </footer>
                  </div>
                </div>

                <div id="source-accounts-body" class="source-accounts-body"></div>
                <div class="source-providers">
                  <p class="source-providers__label">Store data only</p>
                  <div id="provider-status-list" class="provider-status-list"></div>
                </div>
              </section>
            </section>

            <section class="settings-panel" role="tabpanel" id="settings-panel-plugins" data-settings-panel="plugins" aria-labelledby="settings-tab-plugins" tabindex="0" hidden>
              <section id="plugins-catalog-panel" class="settings-card" data-settings-searchable aria-labelledby="plugins-catalog-title">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("grid")}</span>
                  <div class="settings-card__copy">
                    <strong id="plugins-catalog-title">Plugins</strong>
                    <small>Runners that come with Orivo, and emulators you can add.</small>
                  </div>
                </header>

                <div class="plugins-group">
                  <p class="plugins-group__label">Installed</p>
                  <div id="plugins-installed-list" class="plugins-group__list">
                    <div class="settings-row plugin-row">
                      <span class="settings-card__mark plugin-row__mark" aria-hidden="true">${icon("monitor")}</span>
                      <div class="settings-row__copy">
                        <strong>Wine</strong>
                        <small>Wine-Staging runner and isolated profiles</small>
                      </div>
                      <span class="plugin-row__state">Installed</span>
                      <button type="button" class="plugin-open-button" data-plugin-open="wine" aria-label="Open Wine settings">${icon("chevron-right")}</button>
                    </div>
                    <div class="settings-row plugin-row">
                      <span class="settings-card__mark plugin-row__mark" aria-hidden="true">${icon("search")}</span>
                      <div class="settings-row__copy">
                        <strong>Wallpaper Searcher</strong>
                        <small>Finds wallpaper artwork from IGDB and Google Images</small>
                      </div>
                      <span class="plugin-row__state">Installed</span>
                      <button type="button" class="plugin-open-button" data-plugin-open="wallpaper-searcher" aria-label="Open Wallpaper Searcher settings">${icon("chevron-right")}</button>
                    </div>
                  </div>
                </div>

                <div class="plugins-group plugins-group--catalog">
                  <div class="plugins-group__header">
                    <p class="plugins-group__label">Available</p>
                    <button type="button" class="settings-button settings-button--quiet plugins-group__action" data-plugin-install-file>${icon("folder")}<span>Install from file…</span></button>
                  </div>
                  <label class="plugins-search">
                    ${icon("search")}
                    <input id="plugins-catalog-search" type="search" class="plugins-search__input" placeholder="Search available plugins…" aria-label="Search available plugins" />
                  </label>
                  <div id="plugins-catalog-list"></div>
                  <p id="plugins-catalog-empty" class="settings-hint" hidden>No plugins match that search.</p>
                </div>
              </section>

              <section id="wallpaper-plugin-panel" class="settings-card" aria-labelledby="wallpaper-plugin-title" hidden>
                <header class="settings-card__header">
                  <button type="button" class="settings-button settings-button--quiet plugin-back-button" data-plugin-back aria-label="Back to plugins">${icon("chevron-left")}<span>Plugins</span></button>
                  <span class="settings-card__mark" aria-hidden="true">${icon("search")}</span>
                  <div class="settings-card__copy">
                    <strong id="wallpaper-plugin-title">Wallpaper Searcher</strong>
                    <small>Wallpaper search built into Orivo</small>
                  </div>
                </header>
                <div class="settings-row">
                  <div class="settings-row__copy">
                    <strong>How it works</strong>
                    <small>Adds a search panel to the detail page media picker that finds real game artwork from Steam Store, then downloads your choice through Orivo's media pipeline. Steam Store, Wikimedia Commons and Openverse work without any keys; IGDB and Google Images use the keys below.</small>
                  </div>
                </div>
                <div class="credentials-form">
                  <div class="credentials-form__field">
                    <label for="wallpaper-igdb-client-id">IGDB Client ID</label>
                    <input id="wallpaper-igdb-client-id" class="credentials-form__input" type="text" autocomplete="off" spellcheck="false" placeholder="Twitch application client ID" />
                    <small>Optional, but IGDB needs both this and the secret below. Register an application at dev.twitch.tv/console/apps to get the pair.</small>
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-igdb-client-secret">IGDB Client Secret</label>
                    <input id="wallpaper-igdb-client-secret" class="credentials-form__input" type="password" autocomplete="off" spellcheck="false" placeholder="Twitch application client secret" />
                    <small>The other half of the pair. A secret on its own cannot request a token.</small>
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-google-api-key">Google API Key</label>
                    <input id="wallpaper-google-api-key" class="credentials-form__input" type="password" autocomplete="off" spellcheck="false" placeholder="Google Cloud API key" />
                    <small>Optional. Enables the Google Custom Search source.</small>
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-google-cse-id">Google Search Engine ID</label>
                    <input id="wallpaper-google-cse-id" class="credentials-form__input" type="text" autocomplete="off" spellcheck="false" placeholder="Custom search engine ID" />
                    <small>Optional. The programmatic search engine used with the API key.</small>
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-steamgriddb-api-key">SteamGridDB API Key</label>
                    <input id="wallpaper-steamgriddb-api-key" class="credentials-form__input" type="password" autocomplete="off" spellcheck="false" placeholder="From steamgriddb.com/profile/preferences/api" />
                    <small>Optional, and the one worth adding. It is the only source that can be asked for artwork <em>without</em> the game's title painted across it, and the only one publishing 4K key art. With a key, “Reset the covers” uses it for every format; without one it falls back to Steam's official art.</small>
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-search-term-cover">Cover search term</label>
                    <input id="wallpaper-search-term-cover" class="credentials-form__input" type="text" autocomplete="off" spellcheck="false" placeholder='"{name}" box art cover' />
                    <small>Used by the keyword sources (Google, Wikimedia, Openverse). <code>{name}</code> becomes the game. Leave a box empty to keep Orivo's default. Quoting the name is what stops a search for “Doom” answering with Doom Eternal; the noun after it is what separates box art from gameplay.</small>
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-search-term-landscape">Key art search term</label>
                    <input id="wallpaper-search-term-landscape" class="credentials-form__input" type="text" autocomplete="off" spellcheck="false" placeholder='"{name}" key art' />
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-search-term-background">Background search term</label>
                    <input id="wallpaper-search-term-background" class="credentials-form__input" type="text" autocomplete="off" spellcheck="false" placeholder='"{name}" wallpaper' />
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-search-term-logo">Logo search term</label>
                    <input id="wallpaper-search-term-logo" class="credentials-form__input" type="text" autocomplete="off" spellcheck="false" placeholder='"{name}" logo transparent' />
                    <small>Steam Store, SteamGridDB and IGDB ignore these boxes: they resolve the game first and then ask a typed endpoint, so no keyword is involved and nothing has to be guessed.</small>
                  </div>
                  <div class="credentials-form__actions">
                    <button id="wallpaper-credentials-save" type="button" class="settings-button">Save keys</button>
                    <small>Saved keys are picked up immediately — no restart needed.</small>
                  </div>
                </div>
              </section>

              <section id="wine-settings-panel" class="settings-card" aria-labelledby="wine-settings-title" hidden>
                <header class="settings-card__header">
                  <button type="button" class="settings-button settings-button--quiet plugin-back-button" data-plugin-back aria-label="Back to plugins">${icon("chevron-left")}<span>Plugins</span></button>
                  <span class="settings-card__mark" aria-hidden="true">${icon("monitor")}</span>
                  <div class="settings-card__copy">
                    <strong id="wine-settings-title">Wine-Staging</strong>
                    <small>Runner health and isolated Wine profiles</small>
                  </div>
                </header>
                <div id="wine-settings-body" class="wine-settings-body"></div>
              </section>
            </section>

            <section class="settings-panel" role="tabpanel" id="settings-panel-appearance" data-settings-panel="appearance" aria-labelledby="settings-tab-appearance" tabindex="0" hidden>
              <section class="settings-card" data-settings-searchable aria-labelledby="motion-preference-title">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("navigate")}</span>
                  <div class="settings-card__copy">
                    <strong id="motion-preference-title">Motion</strong>
                    <small>Reduced motion turns off hero cross-fades, card transitions, and panel animations.</small>
                  </div>
                </header>
                <div class="settings-choices" role="radiogroup" aria-labelledby="motion-preference-title">
                  <label class="settings-choice">
                    <input type="radio" name="motion-preference" value="system" />
                    <span><strong>System</strong><small>Follow the macOS reduced-motion setting.</small></span>
                  </label>
                  <label class="settings-choice">
                    <input type="radio" name="motion-preference" value="reduced" />
                    <span><strong>Reduced</strong><small>Always keep motion to a minimum in Orivo.</small></span>
                  </label>
                </div>
              </section>
              <section class="settings-card" data-settings-searchable aria-labelledby="showcase-preference-title">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("grid")}</span>
                  <div class="settings-card__copy">
                    <strong id="showcase-preference-title">Demo games (debug)</strong>
                    <small>Seed the library with the bundled showcase games. Off by default — use it only to test the interface without importing real games.</small>
                  </div>
                </header>
                <div class="settings-choices">
                  <label class="settings-choice settings-choice--toggle">
                    <input type="checkbox" id="preference-show-showcase" />
                    <span><strong>Show demo games</strong><small>Adds Elden Ring, Cyberpunk 2077 and other fixtures to your library.</small></span>
                  </label>
                </div>
              </section>
              <section class="settings-card" data-settings-searchable aria-labelledby="beta-preference-title">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("sparkle")}</span>
                  <div class="settings-card__copy">
                    <strong id="beta-preference-title">Beta features</strong>
                    <small>Surfaces that are still being built. They are hidden by default because they are not finished, not because they are broken.</small>
                  </div>
                </header>
                <div class="settings-choices">
                  <label class="settings-choice settings-choice--toggle">
                    <input type="checkbox" id="preference-beta" />
                    <span><strong>Show the Me dashboard</strong><small>Your play habits, scored and charted. Reads your library only.</small></span>
                  </label>
                </div>
              </section>
              <section class="settings-card" data-settings-searchable aria-labelledby="sample-social-preference-title">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("users")}</span>
                  <div class="settings-card__copy">
                    <strong id="sample-social-preference-title">Sample social data (debug)</strong>
                    <small>Fill every game's detail page with placeholder achievements, friends and activity so those sections can be reviewed without a live feed. Off by default.</small>
                  </div>
                </header>
                <div class="settings-choices">
                  <label class="settings-choice settings-choice--toggle">
                    <input type="checkbox" id="preference-debug-social" />
                    <span><strong>Show sample achievements &amp; friends</strong><small>Adds demo trophies, a friends rail and an activity feed to game pages that have none.</small></span>
                  </label>
                </div>
              </section>
            </section>

            <section class="settings-panel" role="tabpanel" id="settings-panel-data" data-settings-panel="data" aria-labelledby="settings-tab-data" tabindex="0" hidden>
              <div class="settings-card" data-settings-searchable>
                <div class="settings-row">
                  <div class="settings-row__copy">
                    <strong>Derived cache</strong>
                    <small><span id="derived-cache-entries">0 derived entries</span> · <span id="derived-cache-freshness">Not refreshed yet</span></small>
                  </div>
                  <span id="derived-cache-size" class="settings-metric">0 B</span>
                </div>
                <div class="settings-row settings-row--actions">
                  <button id="refresh-derived-data" type="button" class="settings-button" data-settings-action="refresh-derived">Refresh now</button>
                  <button id="clear-derived-cache" type="button" class="settings-button settings-button--quiet" data-settings-action="clear-derived">Clear derived cache</button>
                </div>
                <p class="settings-hint">Clearing removes only recomputed store and media data. Your library, Wine profiles, Steam connection, wishlist, and downloaded media stay on this Mac.</p>
              </div>
            </section>

            <section class="settings-panel" role="tabpanel" id="settings-panel-about" data-settings-panel="about" aria-labelledby="settings-tab-about" tabindex="0" hidden>
              <div class="settings-card" data-settings-searchable>
                <div class="settings-row">
                  <div class="settings-row__copy">
                    <strong>Orivo</strong>
                    <small>A focused, local-first game library.</small>
                  </div>
                  <span id="about-app-version" class="settings-metric">—</span>
                </div>
                <div class="settings-row">
                  <div class="settings-row__copy">
                    <strong>Tauri runtime</strong>
                    <small>The desktop shell Orivo runs inside.</small>
                  </div>
                  <span id="about-tauri-version" class="settings-metric">—</span>
                </div>
                <div class="settings-row">
                  <div class="settings-row__copy">
                    <strong>Updates</strong>
                    <small id="update-status" class="update-status"></small>
                    <div id="update-progress" class="update-progress" role="progressbar" aria-label="Update download progress" aria-valuemin="0" aria-valuemax="100" hidden>
                      <span class="update-progress__fill"></span>
                    </div>
                  </div>
                  <button id="check-updates-button" type="button" class="settings-button" data-settings-action="check-updates">Check for updates</button>
                </div>
              </div>
              <div class="settings-card" data-settings-searchable>
                <header class="settings-card__header">
                  <div class="settings-card__copy">
                    <strong>Attributions</strong>
                    <small>Providers and projects Orivo builds on.</small>
                  </div>
                </header>
                <ul class="settings-attributions">
                  <li>Steam and the Steam logo are trademarks of Valve Corporation. Orivo is not affiliated with or endorsed by Valve.</li>
                  <li>Wine and Wine-Staging are provided by the WineHQ project under the LGPL.</li>
                  <li>DXVK-macOS is provided by the DXVK project contributors.</li>
                  <li>Store listings, artwork, and prices remain the property of their respective providers.</li>
                </ul>
              </div>
            </section>
          </div>
        </div>
      </div>

      <div id="app-page-not-found" class="app-page app-page--scroll app-page--not-found">
        <section class="not-found">
          <p class="not-found__code">404</p>
          <h1 class="not-found__title">This page does not exist</h1>
          <p id="not-found-detail" class="not-found__detail"></p>
          <button type="button" class="settings-button settings-button--primary" data-app-action="go-library">Back to Library</button>
        </section>
      </div>

      <p id="toast" class="toast" role="status" aria-live="polite"></p>
      </div>
  `;
}
