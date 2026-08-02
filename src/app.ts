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
import { icon } from "./icons";
import { isTauriRuntime, resolveMediaUrl } from "./media";
import { fallbackLibrary, formatPlayTime, type LibraryGame } from "./mock-library";
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
import { createMePage } from "./me-page";
import { createStorePage } from "./store-page";
import "./game-detail-page.css";
import "./me-page.css";
import "./store-page.css";

type BackendRecord = Record<string, unknown>;

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
type WineSetupStep = "loading" | "wine" | "name" | "directories" | "preview" | "complete" | "error";
type WineScanPhase = "idle" | "starting" | "scanning" | "cancelling" | "ready" | "importing" | "cancelled" | "error";
type WineDetectionState = "detecting" | "ready" | "unavailable" | "cancelled" | "error";
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

interface WineSetupSnapshot {
  setupId: string;
  wineLabel: string;
  detectedWineLabel: string;
  detectionState: WineDetectionState;
  detectionMessage: string;
  directories: WineDirectory[];
}

interface WineScanStatus {
  state: WineScanPhase;
  scannedFiles: number;
  foundGames: number;
  message: string;
}

interface WineScanGame {
  ref: string;
  title: string;
  directoryLabel: string;
  alreadyImported: boolean;
  launchable: boolean;
}

interface WineScanPage {
  games: WineScanGame[];
  nextCursor: string | null;
}

interface WineImportResult {
  importedIds: string[];
  updatedIds: string[];
  skippedRefs: string[];
  message: string;
}

interface WinePanelState {
  open: boolean;
  step: WineSetupStep;
  setup: WineSetupSnapshot | null;
  profile: WineProfile | null;
  profileId: string;
  displayName: string;
  scanPhase: WineScanPhase;
  scanJobId: string;
  scanStatus: WineScanStatus | null;
  scanGames: Map<string, WineScanGame>;
  selectedGameRefs: Set<string>;
  nextCursor: string | null;
  loadingPage: boolean;
  notice: string;
  noticeTone: SteamNoticeTone;
}

interface WineSettingsState {
  loading: boolean;
  runner: WineRunnerStatus | null;
  profiles: WineProfile[];
  notice: string;
  noticeTone: SteamNoticeTone;
  pendingDeleteProfileId: string;
  pendingAttachGameId: string;
}

/** The built-in plugins Orivo ships with; the chevron opens their detail view. */
type PluginId = "wine" | "wallpaper-searcher";
/** `list` shows the plugin browser; a PluginId shows one plugin's detail view. */
type PluginView = "list" | PluginId;

interface PluginCatalogEntry {
  id: string;
  name: string;
  summary: string;
}

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

interface State {
  games: LibraryGame[];
  libraryMediaTokens: Map<string, LibraryMediaTokens>;
  selectedId: string;
  query: string;
  libraryMenuOpen: boolean;
  steam: SteamPanelState;
  steamAccount: SteamAccountState;
  wine: WinePanelState;
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
}

export interface MountAppOptions {
  storePage?: AppPage;
  mePage?: AppPage;
  gameDetailPage?: AppPage;
}

const lastUsedFallback = fallbackLibrary[0];
// The "Available" catalogue is illustrative for now: no emulator ships with
// Orivo yet, so every Install button is a placeholder until a plugin runtime
// can fetch and verify a package.
const AVAILABLE_PLUGINS: readonly PluginCatalogEntry[] = [
  { id: "astris", name: "Astris Emulator", summary: "Sega Dreamcast titles from disc images." },
  { id: "ps2", name: "PlayStation 2 Emulator", summary: "Run PS2 discs and ISO images." },
  { id: "dolphin", name: "Dolphin Emulator", summary: "GameCube and Wii games." },
  { id: "citra", name: "Citra Emulator", summary: "Nintendo 3DS titles." },
  { id: "retroarch", name: "RetroArch", summary: "Multi-system emulation front-end." },
  { id: "mame", name: "MAME", summary: "Classic arcade cabinets." },
];
const MAX_RENDERED_STEAM_GAMES = 120;
const MAX_STEAM_PREVIEW_MEDIA = 16;
const MAX_STEAM_IMPORT_SELECTION = 2_000;
const MAX_AUTOMATIC_STEAM_SELECTION = 50;
const MAX_RENDERED_LIBRARY_CARDS = 48;
const MAX_LIBRARY_MEDIA_HYDRATION = 16;
const MAX_RENDERED_WINE_GAMES = 100;
const WINE_SCAN_PAGE_SIZE = 80;
const MAX_WINE_IMPORT_SELECTION = 2_000;
const STEAM_ACCOUNT_CONNECTED_EVENT = "steam-account-authenticated";
const STEAM_ACCOUNT_LOGIN_CANCELLED_EVENT = "steam-account-login-cancelled";
const STEAM_ACCOUNT_LOGIN_FAILED_EVENT = "steam-account-login-failed";
const STEAM_ACCOUNT_LOGIN_PENDING_EVENT = "steam-account-login-pending";
const WINE_LAUNCH_STATUS_EVENT = "wine-launch-status";

export function mountApp(root: HTMLElement, options: MountAppOptions = {}): void {
  const state: State = {
    games: fallbackLibrary.map((game) => ({ ...game })),
    libraryMediaTokens: new Map(),
    selectedId: fallbackLibrary[0].id,
    query: "",
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
    wine: {
      open: false,
      step: "loading",
      setup: null,
      profile: null,
      profileId: "",
      displayName: "",
      scanPhase: "idle",
      scanJobId: "",
      scanStatus: null,
      scanGames: new Map(),
      selectedGameRefs: new Set(),
      nextCursor: null,
      loadingPage: false,
      notice: "",
      noticeTone: "info",
    },
    wineSettings: {
      loading: false,
      runner: null,
      profiles: [],
      notice: "",
      noticeTone: "info",
      pendingDeleteProfileId: "",
      pendingAttachGameId: "",
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
  };

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
    heroLayers: [get<HTMLImageElement>("#hero-a"), get<HTMLImageElement>("#hero-b")],
    genre: get<HTMLElement>("#hero-genre"),
    title: get<HTMLElement>("#hero-title"),
    description: get<HTMLElement>("#hero-description"),
    playTime: get<HTMLElement>("#hero-play-time"),
    lastPlayed: get<HTMLElement>("#hero-last-played"),
    source: get<HTMLElement>("#hero-source"),
    sourceIcon: get<HTMLElement>("#hero-source-icon"),
    sourceLabel: get<HTMLElement>("#hero-source-label"),
    sourceDivider: get<HTMLElement>("#hero-source-divider"),
    metadata: get<HTMLElement>("#hero-metadata"),
    platform: get<HTMLElement>("#hero-platform"),
    platformLabel: get<HTMLElement>("#hero-platform-label"),
    cards: get<HTMLElement>("#game-cards"),
    mostPlayed: get<HTMLElement>(".most-played"),
    mostPlayedCards: get<HTMLElement>("#most-played-cards"),
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
    playButton: get<HTMLButtonElement>("#play-button"),
    detailsButton: get<HTMLButtonElement>("#details-button"),
    launchFeedback: get<HTMLElement>("#launch-feedback"),
    winePanel: get<HTMLElement>("#wine-setup-panel"),
    wineBody: get<HTMLElement>("#wine-setup-body"),
    wineSettingsPanel: get<HTMLElement>("#wine-settings-panel"),
    wineSettingsBody: get<HTMLElement>("#wine-settings-body"),
    pluginsCatalogPanel: get<HTMLElement>("#plugins-catalog-panel"),
    pluginsCatalogList: get<HTMLElement>("#plugins-catalog-list"),
    pluginsCatalogSearch: get<HTMLInputElement>("#plugins-catalog-search"),
    pluginsCatalogEmpty: get<HTMLElement>("#plugins-catalog-empty"),
    wallpaperPluginPanel: get<HTMLElement>("#wallpaper-plugin-panel"),
    wallpaperCredentialsSave: get<HTMLButtonElement>("#wallpaper-credentials-save"),
    wallpaperIgdbClientId: get<HTMLInputElement>("#wallpaper-igdb-client-id"),
    wallpaperIgdbClientSecret: get<HTMLInputElement>("#wallpaper-igdb-client-secret"),
    wallpaperGoogleApiKey: get<HTMLInputElement>("#wallpaper-google-api-key"),
    wallpaperGoogleCseId: get<HTMLInputElement>("#wallpaper-google-cse-id"),
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
  let toastTimer: number | undefined;
  let steamRequest = 0;
  let libraryRequest = 0;
  // `state.games` starts as the bundled fallback library. Nothing may treat a
  // missing game as "definitely absent" until a real load has landed.
  let libraryLoaded = false;
  const pendingLibraryMediaIds = new Map<string, number>();
  const pendingSteamPreviewMediaIds = new Map<string, number>();
  let steamPreviewMediaRefreshQueued = false;
  let wineScanRequest = 0;
  let wineScanTimer: number | undefined;
  let wineDetectionRequest = 0;
  let wineDetectionTimer: number | undefined;
  let settingsRequest = 0;
  let currentRoute: AppRoute = { page: "library" };
  // A Wine success message has to survive the `replace` navigation that drops
  // `attachGame` from the hash: that navigation re-activates Settings, which
  // reloads the runner and would otherwise wipe the notice it just set.
  let wineNoticeAfterReload: { message: string; tone: SteamNoticeTone } | null = null;

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

  const visibleGames = (): LibraryGame[] => {
    const term = state.query.trim().toLocaleLowerCase();
    if (!term) {
      return state.games;
    }

    return state.games.filter((game) =>
      [game.title, game.genre, game.description, game.metadata]
        .join(" ")
        .toLocaleLowerCase()
        .includes(term),
    );
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

  const updateHeroImage = (game: LibraryGame, immediate = false): void => {
    const fallback = steamAssetUrl(game, "header.jpg");
    const source = game.heroUrl || game.coverUrl || fallback;
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

    const cards = Array.from(refs.cards.querySelectorAll<HTMLButtonElement>(".game-card"));
    const cardsById = new Map(cards.map((card) => [card.dataset.gameId, card]));
    const visibleIds = games.map((game) => game.id);
    const matchesCurrentOrder =
      cards.length === games.length && cards.every((card, index) => card.dataset.gameId === visibleIds[index]);

    if (!matchesCurrentOrder) {
      const fragment = document.createDocumentFragment();

      for (const [index, game] of games.entries()) {
        const card = cardsById.get(game.id) ?? createGameCard();
        syncGameCard(card, game, index, game.id === state.selectedId);
        fragment.append(card);
      }

      refs.cards.replaceChildren(fragment);
      return;
    }

    for (const [index, game] of games.entries()) {
      const card = cards[index];
      syncGameCard(card, game, index, game.id === state.selectedId);
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

    const shade = document.createElement("span");
    shade.className = "card-shade";

    const name = document.createElement("span");
    name.className = "card-title";

    const time = document.createElement("span");
    time.className = "card-time";

    card.append(media, shade, name, time);
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
    const title = card.querySelector<HTMLElement>(".card-title");
    const time = card.querySelector<HTMLElement>(".card-time");
    const portraitSource = game.coverUrl || game.heroUrl;
    const landscapeSource = game.landscapeUrl || game.heroUrl || portraitSource;
    const fallback = steamAssetUrl(game, "header.jpg");

    card.dataset.gameId = game.id;
    card.classList.toggle("is-selected", selected);
    card.setAttribute("aria-pressed", String(selected));
    card.setAttribute("aria-label", `Open details for ${game.title}`);

    assignCardImage(portrait, portraitSource, fallback, index < 7);
    assignCardImage(landscape, landscapeSource, fallback, index < 7);
    if (title) {
      title.textContent = game.title;
    }
    if (time) {
      time.textContent = formatPlayTime(game.playTimeSeconds);
      time.hidden = game.playTimeSeconds <= 0;
    }
  };

  const MAX_MOST_PLAYED_CARDS = 8;

  const mostPlayedGames = (): LibraryGame[] =>
    state.games
      .filter((game) => game.playTimeSeconds > 0)
      .sort((a, b) => b.playTimeSeconds - a.playTimeSeconds)
      .slice(0, MAX_MOST_PLAYED_CARDS);

  const renderMostPlayed = (): void => {
    // The row steps aside during a search so the results stay the only list.
    const games = state.query.trim() ? [] : mostPlayedGames();
    refs.mostPlayed.hidden = games.length === 0;
    if (games.length === 0) {
      refs.mostPlayedCards.replaceChildren();
      return;
    }

    const cards = Array.from(refs.mostPlayedCards.querySelectorAll<HTMLButtonElement>(".game-card"));
    const matchesCurrentOrder =
      cards.length === games.length &&
      cards.every((card, index) => card.dataset.gameId === games[index].id);

    if (!matchesCurrentOrder) {
      const cardsById = new Map(cards.map((card) => [card.dataset.gameId, card]));
      const fragment = document.createDocumentFragment();
      for (const [index, game] of games.entries()) {
        const card = cardsById.get(game.id) ?? createGameCard();
        syncGameCard(card, game, index, game.id === state.selectedId);
        fragment.append(card);
      }
      refs.mostPlayedCards.replaceChildren(fragment);
      return;
    }

    for (const [index, game] of games.entries()) {
      syncGameCard(cards[index], game, index, game.id === state.selectedId);
    }
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
        (!tokens.heroUrl && !tokens.coverUrl && !tokens.landscapeUrl) ||
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
        const [heroUrl, coverUrl, landscapeUrl] = await Promise.all([
          tokens.heroUrl ? resolveMediaUrl(tokens.heroUrl) : Promise.resolve(""),
          tokens.coverUrl ? resolveMediaUrl(tokens.coverUrl) : Promise.resolve(""),
          tokens.landscapeUrl ? resolveMediaUrl(tokens.landscapeUrl) : Promise.resolve(""),
        ]);
        return { id, heroUrl, coverUrl, landscapeUrl };
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
          if (
            heroUrl !== game.heroUrl ||
            coverUrl !== game.coverUrl ||
            landscapeUrl !== game.landscapeUrl
          ) {
            game.heroUrl = heroUrl;
            game.coverUrl = coverUrl;
            game.landscapeUrl = landscapeUrl;
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
    if (titleChanged && !immediateHero && !prefersReducedMotion()) {
      refs.title.classList.remove("is-changing");
      void refs.title.offsetWidth;
      refs.title.classList.add("is-changing");
    }
    refs.description.textContent = game.description || "Ready for your next session.";
    refs.playTime.textContent = formatPlayTime(game.playTimeSeconds);
    if (refs.playTime.parentElement) refs.playTime.parentElement.hidden = game.playTimeSeconds <= 0;
    // A game that was never launched shows no "last played" chip at all rather
    // than a "Not played yet" placeholder.
    refs.lastPlayed.textContent = game.lastPlayedAt ? `Last played ${game.lastPlayedAt}` : "";
    if (refs.lastPlayed.parentElement) refs.lastPlayed.parentElement.hidden = !game.lastPlayedAt;
    const isSteamInstallable = game.source === "steam" && !game.launchable;
    const isWineAttachable = Boolean(game.wineAttachable);
    const sourceName =
      game.source === "steam"
        ? "Steam"
        : game.source === "wine"
          ? "Windows"
          : game.source === "local"
            ? "Local"
            : "";
    const hasSource = sourceName !== "";
    // The Play button already states runnability (Play / Install / Configure
    // Wine / Unavailable), so the meta row never repeats runtime, install or
    // compatibility mentions ("Wine-Staging", "installed", "incompatible…").
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
    // titles, a folder for local games.
    refs.sourceIcon.innerHTML = hasSource
      ? game.source === "steam"
        ? icon("steam")
        : game.source === "wine"
          ? icon("windows")
          : icon("folder")
      : "";
    refs.sourceLabel.hidden = !hasSource;
    refs.sourceLabel.textContent = sourceName;
    refs.metadata.textContent = cleanMetadata;
    refs.metadata.hidden = !cleanMetadata;
    refs.sourceDivider.hidden = !(hasSource && cleanMetadata);
    // Compatibility is conveyed by the Play button, so the platform chip stays
    // hidden.
    refs.platform.hidden = true;
    refs.platformLabel.textContent = "";
    refs.playButton.disabled = !game.launchable && !isSteamInstallable && !isWineAttachable;
    refs.playButton.setAttribute(
      "aria-label",
      game.launchable
        ? "Play " + game.title
        : isSteamInstallable
          ? "Install " + game.title + " in Steam"
          : isWineAttachable
            ? "Configure " + game.title + " with Wine-Staging"
          : game.title + " is unavailable",
    );
    const playLabel = refs.playButton.querySelector<HTMLElement>("span");
    if (playLabel) {
      playLabel.textContent = game.launchable
        ? "Play"
        : isSteamInstallable
          ? "Install"
          : isWineAttachable
            ? "Configure Wine"
            : "Unavailable";
    }
    updateHeroImage(game, immediateHero);
    renderCards();
    renderMostPlayed();
    renderLaunchFeedback();
    hydrateLibraryMedia([game, ...mostPlayedGames(), ...railGames(visibleGames())]);
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

    // The local source is always present: it is this Mac itself. It is a
    // status row, not an action — importing a local game keeps its own entry
    // under "This Mac" below.
    const local = document.createElement("div");
    local.className = "library-source-action library-source-row";
    local.setAttribute("role", "none");
    local.innerHTML =
      `<span class="library-source-action__icon" aria-hidden="true">${icon("folder")}</span>` +
      `<span class="library-source-action__copy"><strong>Local</strong><small>Games on this Mac</small></span>`;
    list.append(local);
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
    libraryLoaded = true;
    pendingLibraryMediaIds.clear();
    state.selectedId = library.games.some((game) => game.id === importedId)
      ? importedId!
      : library.games.some((game) => game.id === state.selectedId)
        ? state.selectedId
        : library.games[0]?.id ?? lastUsedFallback.id;
    renderSelection();
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

  const clearWineScanPolling = (): number => {
    wineScanRequest += 1;
    if (wineScanTimer !== undefined) {
      window.clearTimeout(wineScanTimer);
      wineScanTimer = undefined;
    }
    return wineScanRequest;
  };

  const stopWineDetectionPolling = (): number => {
    wineDetectionRequest += 1;
    if (wineDetectionTimer !== undefined) {
      window.clearTimeout(wineDetectionTimer);
      wineDetectionTimer = undefined;
    }
    return wineDetectionRequest;
  };

  const cancelWineBackgroundWork = (): void => {
    const setup = state.wine.setup;
    const jobId = state.wine.scanJobId;
    const scanPhase = state.wine.scanPhase;
    if (!isTauriRuntime()) {
      return;
    }
    if (setup?.setupId && setup.detectionState === "detecting") {
      void invoke("cancel_wine_detection", { setupId: setup.setupId }).catch(() => {
        // The setup may already have finished or expired. Its result is no
        // longer visible, so no UI update is needed.
      });
    }
    if (
      jobId &&
      (scanPhase === "starting" || scanPhase === "scanning" || scanPhase === "cancelling" || scanPhase === "importing")
    ) {
      void invoke("cancel_wine_scan", { jobId }).catch(() => {
        // A stale scan is intentionally best-effort cancelled when its panel
        // disappears; failures cannot block navigation.
      });
    }
  };

  const resetWineScan = (): void => {
    state.wine.scanPhase = "idle";
    state.wine.scanJobId = "";
    state.wine.scanStatus = null;
    state.wine.scanGames.clear();
    state.wine.selectedGameRefs.clear();
    state.wine.nextCursor = null;
    state.wine.loadingPage = false;
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

  const setWineSetupOpen = (open: boolean): void => {
    // The Wine wizard is the one remaining overlay task: it floats above
    // whichever page is active and never tears the shell down.
    state.wine.open = open;
    refs.winePanel.hidden = !open;
    if (open) {
      closeLibraryMenu();
    } else {
      cancelWineBackgroundWork();
      stopWineDetectionPolling();
      clearWineScanPolling();
    }
    renderWineSetupPanel();
    if (open) {
      requestAnimationFrame(() => {
        const focusTarget =
          refs.winePanel.querySelector<HTMLInputElement>("input[name='wine-profile-name']") ??
          refs.winePanel.querySelector<HTMLButtonElement>("[data-wine-action='select-wine']") ??
          refs.winePanel.querySelector<HTMLButtonElement>("[data-wine-action='close']");
        focusTarget?.focus();
      });
    }
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

  const renderWineGameList = (parent: HTMLElement): void => {
    const wine = state.wine;
    const games = [...wine.scanGames.values()].slice(0, MAX_RENDERED_WINE_GAMES);
    const status = wine.scanStatus;
    const importedCount = games.filter((game) => game.alreadyImported).length;

    if (games.length === 0) {
      if (wine.scanPhase === "importing") {
        appendWineLoadingState(
          parent,
          "Verifying before import",
          "Orivo is verifying the selected executables. You can cancel at any time.",
        );
        return;
      } else if (wine.scanPhase === "scanning" || wine.scanPhase === "starting" || wine.scanPhase === "cancelling") {
        appendWineLoadingState(
          parent,
          "Scanning the game folders",
          "Orivo is looking for Windows executables in the background. You can keep browsing.",
        );
        return;
      }

      const empty = document.createElement("section");
      empty.className = "steam-list-empty";
      const heading = document.createElement("h2");
      heading.textContent =
        wine.scanPhase === "cancelled"
          ? "Import cancelled"
          : wine.scanPhase === "error"
            ? "The scan did not finish"
            : "No Windows game found";
      const message = document.createElement("p");
      message.textContent =
        wine.scanPhase === "cancelled"
          ? "You can run the scan again whenever you like."
          : wine.scanPhase === "error"
            ? status?.message || "Games already in your Orivo library are unaffected."
            : "Add another allowed folder, or try again after checking what it contains.";
      empty.append(heading, message);
      parent.append(empty);
      return;
    }

    const summary = document.createElement("p");
    summary.className = "wine-results-count";
    const foundCount = status?.foundGames ?? games.length;
    const foundLabel = foundCount === 1 ? "1 game found" : foundCount.toLocaleString() + " games found";
    const importedLabel = importedCount > 0 ? " · " + importedCount + " already in your library" : "";
    summary.textContent =
      foundCount > games.length
        ? foundLabel + importedLabel + " · showing the first " + games.length + " of them"
        : foundLabel + importedLabel;
    summary.setAttribute("aria-live", "polite");
    parent.append(summary);

    const controls = document.createElement("div");
    controls.className = "wine-list-controls";
    // A rescan is also the deliberate refresh path for an already-imported
    // executable: its content fingerprint may have changed after an update.
    // The host upserts the same opaque game reference, so this never creates
    // a duplicate library card.
    const selectedReady = games.filter(
      (game) => game.launchable && wine.selectedGameRefs.has(game.ref),
    ).length;
    const selectable = games.filter((game) => game.launchable);
    const selectAll = wineActionButton(
      selectedReady === selectable.length && selectable.length > 0 ? "clear-wine-selection" : "select-visible-wine-games",
      selectedReady === selectable.length && selectable.length > 0 ? "Deselect all" : "Select the ready games",
      "steam-secondary-button",
    );
    selectAll.disabled =
      selectable.length === 0 ||
      wine.scanPhase === "importing" ||
      wine.scanPhase === "cancelling";
    controls.append(selectAll);
    parent.append(controls);

    const list = document.createElement("div");
    list.className = "steam-game-list wine-game-list";
    list.setAttribute("role", "list");
    for (const game of games) {
      const row = document.createElement("label");
      row.className = "steam-game-row wine-game-row";
      const isSelected = wine.selectedGameRefs.has(game.ref);
      row.classList.toggle("is-selected", isSelected);
      row.classList.toggle("is-imported", game.alreadyImported);
      row.setAttribute("role", "listitem");

      const toggle = document.createElement("input");
      toggle.type = "checkbox";
      toggle.checked = isSelected;
      toggle.disabled =
        !game.launchable ||
        wine.scanPhase === "importing" ||
        wine.scanPhase === "cancelling";
      toggle.dataset.wineGameRef = game.ref;
      toggle.setAttribute("aria-label", "Select " + game.title);

      const artwork = document.createElement("span");
      artwork.className = "steam-game-artwork steam-game-artwork--fallback";
      artwork.innerHTML = icon("monitor");
      artwork.setAttribute("aria-hidden", "true");

      const copy = document.createElement("span");
      copy.className = "steam-game-copy";
      const gameTitle = document.createElement("strong");
      gameTitle.textContent = game.title;
      const metadata = document.createElement("span");
      metadata.className = "steam-game-metadata";
      metadata.textContent = game.directoryLabel || "Allowed folder";
      copy.append(gameTitle, metadata);

      const badge = document.createElement("span");
      badge.className = "steam-game-status";
      badge.textContent = game.alreadyImported ? "In your library" : game.launchable ? "Ready" : "Not launchable";
      row.append(toggle, artwork, copy, badge);
      list.append(row);
    }
    parent.append(list);

    if (wine.nextCursor) {
      const more = wineActionButton(
        "load-more-wine-games",
        wine.loadingPage ? "Chargement…" : "Afficher davantage",
        "steam-secondary-button",
        "refresh",
      );
      more.disabled = wine.loadingPage || wine.scanPhase === "importing";
      more.classList.add("wine-load-more");
      parent.append(more);
    }
  };

  const renderWineSetupPanel = (): void => {
    const wine = state.wine;
    refs.winePanel.hidden = !wine.open;
    refs.winePanel.setAttribute(
      "aria-busy",
      String(
        wine.step === "loading" ||
          wine.scanPhase === "starting" ||
          wine.scanPhase === "scanning" ||
          wine.scanPhase === "cancelling" ||
          wine.scanPhase === "importing",
      ),
    );
    if (!wine.open) {
      return;
    }

    const title = refs.winePanel.querySelector<HTMLElement>("#wine-setup-title");
    const detail = refs.winePanel.querySelector<HTMLElement>("#wine-setup-detail");
    if (title) {
      title.textContent =
        wine.step === "preview" || wine.step === "complete" ? "Import with Wine-Staging" : "Add Wine-Staging";
    }
    if (detail) {
      detail.textContent =
        wine.profile?.displayName ||
        wine.setup?.wineLabel ||
        (wine.step === "loading" ? "Preparing the isolated profile" : "An isolated Wine profile for Orivo");
    }

    refs.wineBody.replaceChildren();
    const body = refs.wineBody;

    if (wine.step === "loading") {
      appendWineLoadingState(
        body,
        "Preparing Wine-Staging",
        "Orivo checks the local runner without touching any other application’s prefix.",
      );
      return;
    }

    if (wine.step === "error") {
      const unavailable = document.createElement("section");
      unavailable.className = "steam-state steam-state--unavailable";
      const badge = document.createElement("span");
      badge.className = "steam-state__icon";
      badge.innerHTML = icon("alert");
      badge.setAttribute("aria-hidden", "true");
      const heading = document.createElement("h2");
      heading.textContent = "Wine-Staging could not be set up";
      const message = document.createElement("p");
      message.textContent = wine.notice || "Nothing in your library was changed.";
      const retry = wineActionButton("restart-wine-setup", "Try again", "steam-secondary-button", "refresh");
      unavailable.append(badge, heading, message, retry);
      body.append(unavailable);
      return;
    }

    if (wine.step === "wine") {
      const intro = document.createElement("section");
      intro.className = "wine-step-intro";
      const heading = document.createElement("h2");
      heading.textContent = "1. Choose Wine-Staging";
      const message = document.createElement("p");
      message.textContent =
        "Choose a Wine-Staging installation already present on this Mac. Orivo installs nothing and only uses a dedicated prefix.";
      intro.append(heading, message);
      body.append(intro);
      appendWineRunnerSummary(body, state.wineSettings.runner);

      const setup = wine.setup;
      if (setup?.detectionState === "detecting") {
        const detection = document.createElement("section");
        detection.className = "wine-detection-status";
        detection.setAttribute("aria-live", "polite");
        const spinner = document.createElement("span");
        spinner.className = "steam-spinner";
        spinner.setAttribute("aria-hidden", "true");
        const copy = document.createElement("p");
        copy.textContent =
          setup.detectionMessage ||
          "Looking for a local Wine-Staging installation in the background.";
        detection.append(spinner, copy);
        body.append(detection);
      } else if (setup?.detectionMessage && !setup.wineLabel && !setup.detectedWineLabel) {
        const tone: SteamNoticeTone =
          setup.detectionState === "error" || setup.detectionState === "unavailable" ? "error" : "info";
        appendWineNotice(body, setup.detectionMessage, tone);
      }

      if (setup?.detectedWineLabel && !setup.wineLabel) {
        const detected = document.createElement("p");
        detected.className = "wine-detected-label";
        detected.textContent = "Wine-Staging installation found: " + setup.detectedWineLabel;
        body.append(detected);
      }

      const select = wineActionButton(
        "select-wine",
        setup?.wineLabel ? "Change the Wine-Staging installation" : "Choose Wine-Staging manually",
        setup?.wineLabel ? "steam-secondary-button" : "steam-import-button",
        "monitor",
      );
      const actions = document.createElement("div");
      actions.className = "wine-step-actions";
      if (setup?.wineLabel) {
        actions.append(wineActionButton("continue-from-wine", "Continue", "steam-secondary-button"));
        actions.append(select);
      } else {
        if (setup?.detectedWineLabel) {
          actions.append(
            wineActionButton(
              "confirm-detected-wine",
              "Use the detected Wine-Staging",
              "steam-import-button",
              "monitor",
            ),
          );
        }
        actions.append(select);
        if (setup?.detectionState === "detecting") {
          actions.append(wineActionButton("cancel-wine-detection", "Cancel detection", "steam-secondary-button", "close"));
        }
      }
      body.append(actions);
      appendWineNotice(body, wine.notice, wine.noticeTone);
      return;
    }

    if (wine.step === "name") {
      const intro = document.createElement("section");
      intro.className = "wine-step-intro";
      const heading = document.createElement("h2");
      heading.textContent = "2. Name the profile";
      const message = document.createElement("p");
      message.textContent =
        "Every profile gets its own Wine prefix managed by Orivo. No existing prefix is modified.";
      intro.append(heading, message);
      body.append(intro);

      const form = document.createElement("form");
      form.className = "wine-profile-form";
      form.dataset.wineForm = "profile-name";
      const label = document.createElement("label");
      label.textContent = "Profile name";
      const input = document.createElement("input");
      input.name = "wine-profile-name";
      input.autocomplete = "off";
      input.maxLength = 80;
      input.required = true;
      input.placeholder = "For example, Windows Games";
      input.value = wine.displayName;
      label.append(input);
      const actions = document.createElement("div");
      actions.className = "wine-step-actions";
      actions.append(
        wineActionButton("back-to-wine", "Retour", "steam-secondary-button", "close"),
        wineActionButton("continue-to-directories", "Continue", "steam-import-button"),
      );
      actions.lastElementChild?.setAttribute("type", "submit");
      form.append(label, actions);
      body.append(form);
      appendWineNotice(body, wine.notice, wine.noticeTone);
      return;
    }

    if (wine.step === "directories") {
      const intro = document.createElement("section");
      intro.className = "wine-step-intro";
      const heading = document.createElement("h2");
      heading.textContent = "3. Allow the game folders";
      const message = document.createElement("p");
      message.textContent =
        "Choose the folders to scan. Orivo only shows their labels here and never scans anywhere else.";
      intro.append(heading, message);
      body.append(intro);

      const directories = wine.setup?.directories ?? [];
      if (directories.length === 0) {
        const empty = document.createElement("section");
        empty.className = "wine-directory-empty";
        empty.textContent = "No game folder is allowed for this profile yet.";
        body.append(empty);
      } else {
        const list = document.createElement("div");
        list.className = "wine-directory-list";
        for (const directory of directories) {
          const row = document.createElement("div");
          row.className = "wine-directory-row";
          const label = document.createElement("span");
          label.innerHTML = icon("folder") + "<strong></strong>";
          label.querySelector("strong")!.textContent = directory.label;
          const remove = wineActionButton("remove-wine-directory", "Remove", "wine-text-button");
          remove.dataset.directoryId = directory.id;
          row.append(label, remove);
          list.append(row);
        }
        body.append(list);
      }

      const add = wineActionButton("choose-wine-directory", "Add a game folder", "steam-secondary-button", "folder");
      body.append(add);
      const actions = document.createElement("div");
      actions.className = "wine-step-actions";
      const back = wineActionButton("back-to-name", "Retour", "steam-secondary-button", "close");
      const create = wineActionButton("create-wine-profile", "Scan for games", "steam-import-button", "search");
      create.disabled = directories.length === 0;
      actions.append(back, create);
      body.append(actions);
      appendWineNotice(body, wine.notice, wine.noticeTone);
      return;
    }

    if (wine.step === "preview") {
      const intro = document.createElement("section");
      intro.className = "wine-step-intro wine-step-intro--compact";
      const heading = document.createElement("h2");
      heading.textContent = "4. Review the games found";
      const message = document.createElement("p");
      const scan = wine.scanStatus;
      if (wine.scanPhase === "importing") {
        message.textContent = "Verifying executables before import · you can cancel.";
      } else if (wine.scanPhase === "scanning" || wine.scanPhase === "starting" || wine.scanPhase === "cancelling") {
        const fileCount = scan?.scannedFiles ?? 0;
        message.textContent =
          fileCount > 0
            ? fileCount.toLocaleString() + " files checked · you can keep using Orivo."
            : "The scan continues in the background.";
      } else {
        message.textContent =
          scan?.message || "Choose the executables to add to your library.";
      }
      intro.append(heading, message);
      body.append(intro);
      appendWineNotice(body, wine.notice, wine.noticeTone);
      renderWineGameList(body);

      const actions = document.createElement("div");
      actions.className = "wine-step-actions wine-step-actions--preview";
      if (
        wine.scanPhase === "scanning" ||
        wine.scanPhase === "starting" ||
        wine.scanPhase === "cancelling" ||
        wine.scanPhase === "importing"
      ) {
        const cancel = wineActionButton(
          "cancel-wine-scan",
          wine.scanPhase === "cancelling"
            ? "Annulation…"
            : wine.scanPhase === "importing"
              ? "Cancel the import"
              : "Cancel the scan",
          "steam-secondary-button",
          "close",
        );
        cancel.disabled = wine.scanPhase === "cancelling";
        actions.append(cancel);
      } else if (wine.scanPhase === "error" || wine.scanPhase === "cancelled") {
        actions.append(wineActionButton("retry-wine-scan", "Run the scan again", "steam-secondary-button", "refresh"));
      }
      const selectedCount = wine.selectedGameRefs.size;
      const importButton = wineActionButton(
        "import-wine-games",
        selectedCount === 1 ? "Import 1 game" : "Import " + selectedCount + " games",
        "steam-import-button",
      );
      importButton.disabled = selectedCount === 0 || wine.scanPhase !== "ready";
      actions.append(importButton);
      body.append(actions);
      return;
    }

    const completed = document.createElement("section");
    completed.className = "steam-state steam-state--unavailable";
    const badge = document.createElement("span");
    badge.className = "steam-state__icon";
    badge.innerHTML = icon("library");
    badge.setAttribute("aria-hidden", "true");
    const heading = document.createElement("h2");
    heading.textContent = "Wine import complete";
    const message = document.createElement("p");
    message.textContent = wine.notice || "The imported games are now in your library.";
    const done = wineActionButton("close", "Done", "steam-import-button");
    completed.append(badge, heading, message, done);
    body.append(completed);
  };

  const renderPluginCatalogRow = (entry: PluginCatalogEntry): HTMLElement => {
    const row = document.createElement("div");
    row.className = "settings-row plugin-catalog-row";
    const copy = document.createElement("div");
    copy.className = "settings-row__copy";
    const name = document.createElement("strong");
    name.textContent = entry.name;
    const summary = document.createElement("small");
    summary.textContent = entry.summary;
    copy.append(name, summary);
    const install = document.createElement("button");
    install.type = "button";
    install.className = "settings-button";
    install.dataset.pluginInstall = entry.id;
    install.setAttribute("aria-label", `Install ${entry.name}`);
    install.innerHTML = `${icon("download")}<span>Install</span>`;
    row.append(copy, install);
    return row;
  };

  const renderPluginList = (): void => {
    const showList = state.pluginView === "list";
    refs.pluginsCatalogPanel.hidden = !showList;
    refs.wallpaperPluginPanel.hidden = state.pluginView !== "wallpaper-searcher";
    refs.wineSettingsPanel.hidden = state.pluginView !== "wine";
    if (!showList) return;
    refs.pluginsCatalogSearch.value = state.pluginCatalogSearch;
    const term = state.pluginCatalogSearch.trim().toLocaleLowerCase();
    const matches = AVAILABLE_PLUGINS.filter(
      (entry) => !term || `${entry.name} ${entry.summary}`.toLocaleLowerCase().includes(term),
    );
    refs.pluginsCatalogList.replaceChildren(...matches.map(renderPluginCatalogRow));
    refs.pluginsCatalogEmpty.hidden = matches.length > 0;
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

    const attachedGame = settings.pendingAttachGameId
      ? state.games.find((game) => game.id === settings.pendingAttachGameId && game.wineAttachable)
      : undefined;
    if (attachedGame) {
      const association = document.createElement("section");
      association.className = "wine-direct-association";
      const heading = document.createElement("h2");
      heading.textContent = "Set up with Wine";
      const message = document.createElement("p");
      message.textContent =
        "Choose a profile that already allows the folder for “" +
        attachedGame.title +
        "”. Orivo verifies the executable before launching it and never edits its Direct entry.";
      const cancel = wineActionButton("cancel-direct-wine-association", "Cancel", "wine-text-button");
      association.append(heading, message, cancel);
      body.append(association);
    } else if (settings.pendingAttachGameId && libraryLoaded) {
      // Only a loaded library can prove the requested game does not exist. On a
      // cold start or a reload the deep link arrives before `refreshLibrary`
      // lands, and clearing here would destroy the attachment permanently.
      settings.pendingAttachGameId = "";
    }

    const heading = document.createElement("h2");
    heading.className = "wine-settings-heading";
    heading.textContent = "Wine profiles";
    body.append(heading);

    if (settings.profiles.length === 0) {
      const empty = document.createElement("section");
      empty.className = "wine-directory-empty";
      empty.textContent = attachedGame
        ? "First create a Wine-Staging profile, then allow this Windows game’s folder."
        : "No Wine-Staging profile is configured yet.";
      const add = wineActionButton("add-wine-profile", "Add Wine-Staging", "steam-secondary-button", "monitor");
      empty.append(add);
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
        if (attachedGame) {
          const associate = wineActionButton(
            "associate-direct-game-with-wine",
            "Use for “" + attachedGame.title + "”",
            "steam-import-button",
            "monitor",
          );
          associate.dataset.profileId = profile.id;
          associate.disabled = !profile.enabled;
          actions.append(associate);
        }
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
        const reimport = wineActionButton("reimport-wine-profile", "Import again", "steam-secondary-button", "refresh");
        reimport.dataset.profileId = profile.id;
        reimport.disabled = !profile.enabled;
        const toggle = wineActionButton(
          "toggle-wine-profile",
          profile.enabled ? "Disable" : "Enable",
          "steam-secondary-button",
        );
        toggle.dataset.profileId = profile.id;
        toggle.dataset.enabled = String(!profile.enabled);
        const remove = wineActionButton("delete-wine-profile", "Delete", "wine-text-button");
        remove.dataset.profileId = profile.id;
        actions.append(dxvk, reimport, toggle, remove);
        card.append(actions);
      }
      profiles.append(card);
    }
    body.append(profiles);
  };

  const setWineNotice = (message: string, tone: SteamNoticeTone): void => {
    state.wine.notice = message;
    state.wine.noticeTone = tone;
  };

  const scheduleWineDetectionPoll = (request: number, setupId: string, delay = 650): void => {
    const setup = state.wine.setup;
    if (
      request !== wineDetectionRequest ||
      !state.wine.open ||
      !setup ||
      setup.setupId !== setupId ||
      setup.detectionState !== "detecting"
    ) {
      return;
    }
    window.clearTimeout(wineDetectionTimer);
    wineDetectionTimer = window.setTimeout(() => {
      wineDetectionTimer = undefined;
      void pollWineDetection(request, setupId);
    }, delay);
  };

  const pollWineDetection = async (request: number, setupId: string): Promise<void> => {
    const currentSetup = state.wine.setup;
    if (
      request !== wineDetectionRequest ||
      !state.wine.open ||
      !currentSetup ||
      currentSetup.setupId !== setupId ||
      currentSetup.detectionState !== "detecting" ||
      !isTauriRuntime()
    ) {
      return;
    }

    try {
      const setup = normaliseWineSetup(
        await invoke<unknown>("get_wine_profile_setup", { setupId }),
        currentSetup,
      );
      if (
        request !== wineDetectionRequest ||
        !state.wine.open ||
        state.wine.setup?.setupId !== setupId
      ) {
        return;
      }
      if (!setup) {
        throw new Error("Wine-Staging detection did not return a valid state.");
      }
      state.wine.setup = setup;
      renderWineSetupPanel();
      if (setup.detectionState === "detecting") {
        scheduleWineDetectionPoll(request, setupId);
      }
    } catch (error) {
      if (
        request !== wineDetectionRequest ||
        !state.wine.open ||
        state.wine.setup?.setupId !== setupId
      ) {
        return;
      }
      state.wine.setup = {
        ...currentSetup,
        detectionState: "error",
        detectionMessage: messageFromError(error, "Wine-Staging detection could not be followed."),
      };
      renderWineSetupPanel();
    }
  };

  const cancelWineDetection = async (): Promise<void> => {
    const currentSetup = state.wine.setup;
    if (
      !currentSetup ||
      currentSetup.detectionState !== "detecting" ||
      !isTauriRuntime()
    ) {
      return;
    }
    const setupId = currentSetup.setupId;
    const request = stopWineDetectionPolling();
    state.wine.setup = {
      ...currentSetup,
      detectionState: "cancelled",
      detectionMessage: "Cancelling Wine-Staging detection…",
    };
    renderWineSetupPanel();

    try {
      const result = await invoke<unknown>("cancel_wine_detection", { setupId });
      if (
        request !== wineDetectionRequest ||
        !state.wine.open ||
        state.wine.setup?.setupId !== setupId
      ) {
        return;
      }
      const setup = normaliseWineSetup(result, currentSetup);
      state.wine.setup = setup ?? {
        ...currentSetup,
        detectionState: "cancelled",
        detectionMessage: "Wine-Staging detection cancelled.",
      };
    } catch (error) {
      if (
        request !== wineDetectionRequest ||
        !state.wine.open ||
        state.wine.setup?.setupId !== setupId
      ) {
        return;
      }
      state.wine.setup = {
        ...currentSetup,
        detectionState: "error",
        detectionMessage: messageFromError(error, "Wine-Staging detection could not be cancelled."),
      };
    }
    renderWineSetupPanel();
  };

  const loadWineRunnerStatus = async (): Promise<WineRunnerStatus | null> => {
    if (!isTauriRuntime()) {
      const unavailable: WineRunnerStatus = {
        state: "unavailable",
        available: false,
        version: "",
        message: "Wine-Staging is available in the Orivo desktop app for macOS.",
      };
      state.wineSettings.runner = unavailable;
      return unavailable;
    }

    try {
      const status = normaliseWineRunnerStatus(await invoke<unknown>("get_wine_runner_status"));
      state.wineSettings.runner = status;
      return status;
    } catch (error) {
      const unavailable: WineRunnerStatus = {
        state: "error",
        available: false,
        version: "",
        message: messageFromError(error, "The Wine-Staging status is temporarily unavailable."),
      };
      state.wineSettings.runner = unavailable;
      return unavailable;
    }
  };

  const beginWineProfileSetup = async (): Promise<void> => {
    cancelWineBackgroundWork();
    clearWineScanPolling();
    const detectionRequest = stopWineDetectionPolling();
    const wine = state.wine;
    wine.step = "loading";
    wine.setup = null;
    wine.profile = null;
    wine.profileId = "";
    wine.displayName = "";
    resetWineScan();
    setWineNotice("", "info");
    renderWineSetupPanel();

    if (!isTauriRuntime()) {
      wine.step = "error";
      setWineNotice("Wine-Staging setup is available in the Orivo desktop app for macOS.", "error");
      renderWineSetupPanel();
      return;
    }

    const runnerTask = loadWineRunnerStatus();
    void runnerTask.then(() => {
      if (detectionRequest === wineDetectionRequest && wine.open) {
        renderWineSetupPanel();
      }
    });
    try {
      const setup = normaliseWineSetup(await invoke<unknown>("begin_wine_profile_setup"));
      if (detectionRequest !== wineDetectionRequest || !wine.open) {
        return;
      }
      if (!setup) {
        throw new Error("Wine-Staging returned an invalid profile setup.");
      }
      wine.setup = setup;
      wine.step = "wine";
      if (setup.detectionState === "detecting") {
        scheduleWineDetectionPoll(detectionRequest, setup.setupId, 250);
      }
    } catch (error) {
      if (detectionRequest !== wineDetectionRequest || !wine.open) {
        return;
      }
      wine.step = "error";
      setWineNotice(messageFromError(error, "The Wine profile could not be prepared."), "error");
    }
    renderWineSetupPanel();
  };

  const openWineSetup = async (): Promise<void> => {
    closeLibraryMenu();
    setWineSetupOpen(true);
    await beginWineProfileSetup();
  };

  const selectWineStaging = async (): Promise<void> => {
    const wine = state.wine;
    const currentSetup = wine.setup;
    if (!currentSetup || !isTauriRuntime()) {
      return;
    }
    const setupId = currentSetup.setupId;
    const request = stopWineDetectionPolling();
    const stoppedSetup: WineSetupSnapshot = {
      ...currentSetup,
      detectionState: currentSetup.detectionState === "detecting" ? "cancelled" : currentSetup.detectionState,
      detectionMessage:
        currentSetup.detectionState === "detecting"
          ? "Detection stopped so you can choose Wine-Staging manually."
          : currentSetup.detectionMessage,
    };
    wine.setup = stoppedSetup;
    wine.step = "loading";
    setWineNotice("Opening the Wine-Staging picker…", "info");
    renderWineSetupPanel();

    try {
      const result = await invoke<unknown>("select_wine_staging", { setupId });
      if (
        request !== wineDetectionRequest ||
        !wine.open ||
        wine.setup?.setupId !== setupId
      ) {
        return;
      }
      const setup = normaliseWineSetup(result, stoppedSetup);
      if (!setup || !setup.wineLabel) {
        throw new Error("Wine-Staging did not confirm the installation you chose.");
      }
      wine.setup = setup;
      wine.step = "wine";
      setWineNotice("", "info");
    } catch (error) {
      if (
        request !== wineDetectionRequest ||
        !wine.open ||
        wine.setup?.setupId !== setupId
      ) {
        return;
      }
      wine.setup = stoppedSetup;
      wine.step = "wine";
      setWineNotice(messageFromError(error, "The Wine-Staging installation could not be validated."), "error");
    }
    renderWineSetupPanel();
  };

  const confirmDetectedWineStaging = async (): Promise<void> => {
    const wine = state.wine;
    const currentSetup = wine.setup;
    if (!currentSetup || !currentSetup.detectedWineLabel || !isTauriRuntime()) {
      return;
    }
    const setupId = currentSetup.setupId;
    const request = stopWineDetectionPolling();
    wine.step = "loading";
    setWineNotice("Validating the detected Wine-Staging…", "info");
    renderWineSetupPanel();

    try {
      const result = await invoke<unknown>("confirm_detected_wine_staging", { setupId });
      if (
        request !== wineDetectionRequest ||
        !wine.open ||
        wine.setup?.setupId !== setupId
      ) {
        return;
      }
      const setup = normaliseWineSetup(result, currentSetup);
      if (!setup || !setup.wineLabel) {
        throw new Error("The detected Wine-Staging could not be validated.");
      }
      wine.setup = setup;
      wine.step = "wine";
      setWineNotice("", "info");
    } catch (error) {
      if (
        request !== wineDetectionRequest ||
        !wine.open ||
        wine.setup?.setupId !== setupId
      ) {
        return;
      }
      wine.setup = currentSetup;
      wine.step = "wine";
      setWineNotice(messageFromError(error, "The detected Wine-Staging could not be validated."), "error");
    }
    renderWineSetupPanel();
  };

  const chooseWineGameDirectory = async (): Promise<void> => {
    const wine = state.wine;
    const currentSetup = wine.setup;
    if (!currentSetup || !isTauriRuntime()) {
      return;
    }
    const setupId = currentSetup.setupId;
    setWineNotice("Choose a game folder in the macOS picker.", "info");
    renderWineSetupPanel();
    try {
      const result = await invoke<unknown>("choose_wine_game_directory", { setupId });
      const setup = normaliseWineSetup(result, currentSetup);
      if (setup) {
        wine.setup = setup;
      } else {
        const directory = normaliseWineDirectory(result);
        if (directory && !currentSetup.directories.some((candidate) => candidate.id === directory.id)) {
          wine.setup = { ...currentSetup, directories: [...currentSetup.directories, directory] };
        }
      }
      setWineNotice("", "info");
    } catch (error) {
      setWineNotice(messageFromError(error, "The game folder was not added."), "error");
    }
    renderWineSetupPanel();
  };

  const removeWineSetupDirectory = async (directoryId: string): Promise<void> => {
    const wine = state.wine;
    const currentSetup = wine.setup;
    if (!currentSetup || !directoryId || !isTauriRuntime()) {
      return;
    }
    const setupId = currentSetup.setupId;
    try {
      const result = await invoke<unknown>("remove_wine_setup_directory", { setupId, directoryId });
      const setup = normaliseWineSetup(result, currentSetup);
      wine.setup = setup ?? {
        ...currentSetup,
        directories: currentSetup.directories.filter((directory) => directory.id !== directoryId),
      };
      setWineNotice("", "info");
    } catch (error) {
      setWineNotice(messageFromError(error, "The allowed folder could not be removed."), "error");
    }
    renderWineSetupPanel();
  };

  const continueWineProfileName = (displayName: string): void => {
    const wine = state.wine;
    const trimmed = displayName.trim();
    if (!trimmed) {
      setWineNotice("Give this Wine profile a name.", "error");
      renderWineSetupPanel();
      return;
    }
    wine.displayName = trimmed;
    wine.step = "directories";
    setWineNotice("", "info");
    renderWineSetupPanel();
  };

  const scheduleWineScanPoll = (request: number, delay = 700): void => {
    if (request !== wineScanRequest) {
      return;
    }
    window.clearTimeout(wineScanTimer);
    wineScanTimer = window.setTimeout(() => {
      wineScanTimer = undefined;
      void pollWineScan(request);
    }, delay);
  };

  const loadWineScanPage = async (request: number): Promise<void> => {
    const wine = state.wine;
    const jobId = wine.scanJobId;
    if (
      request !== wineScanRequest ||
      !jobId ||
      wine.loadingPage ||
      !isTauriRuntime()
    ) {
      return;
    }
    wine.loadingPage = true;
    renderWineSetupPanel();
    try {
      const page = normaliseWineScanPage(
        await invoke<unknown>("get_wine_scan_page", {
          jobId,
          cursor: wine.nextCursor,
          limit: WINE_SCAN_PAGE_SIZE,
        }),
      );
      if (request !== wineScanRequest || wine.scanJobId !== jobId) {
        return;
      }
      for (const game of page.games) {
        const previous = wine.scanGames.get(game.ref);
        wine.scanGames.set(game.ref, { ...previous, ...game });
        if (game.launchable && wine.selectedGameRefs.size < MAX_WINE_IMPORT_SELECTION) {
          wine.selectedGameRefs.add(game.ref);
        }
      }
      wine.nextCursor = page.nextCursor;
      if (wine.selectedGameRefs.size >= MAX_WINE_IMPORT_SELECTION) {
        setWineNotice(
          "Choose up to " + MAX_WINE_IMPORT_SELECTION.toLocaleString() + " games per import.",
          "info",
        );
      }
    } catch (error) {
      if (request === wineScanRequest) {
        setWineNotice(messageFromError(error, "The Wine game preview could not be loaded."), "error");
      }
    } finally {
      if (request === wineScanRequest) {
        wine.loadingPage = false;
        renderWineSetupPanel();
      }
    }
  };

  const pollWineScan = async (request: number): Promise<void> => {
    const wine = state.wine;
    const jobId = wine.scanJobId;
    if (
      request !== wineScanRequest ||
      !jobId ||
      wine.scanPhase === "cancelling" ||
      wine.scanPhase === "cancelled" ||
      wine.scanPhase === "error" ||
      !isTauriRuntime()
    ) {
      return;
    }

    try {
      const status = normaliseWineScanStatus(await invoke<unknown>("get_wine_scan_status", { jobId }));
      if (request !== wineScanRequest || wine.scanJobId !== jobId) {
        return;
      }
      wine.scanStatus = status;
      wine.scanPhase = status.state;
      if (status.foundGames > wine.scanGames.size || wine.scanGames.size === 0) {
        void loadWineScanPage(request);
      }
      renderWineSetupPanel();
      if (status.state === "starting" || status.state === "scanning") {
        scheduleWineScanPoll(request);
      } else if (status.state === "ready") {
        void loadWineScanPage(request);
      }
    } catch (error) {
      if (request !== wineScanRequest) {
        return;
      }
      wine.scanPhase = "error";
      wine.scanStatus = {
        state: "error",
        scannedFiles: wine.scanStatus?.scannedFiles ?? 0,
        foundGames: wine.scanStatus?.foundGames ?? wine.scanGames.size,
        message: messageFromError(error, "The Wine scan could not be followed."),
      };
      renderWineSetupPanel();
    }
  };

  const startWineProfileScan = async (profileId: string): Promise<void> => {
    if (!profileId || !isTauriRuntime()) {
      return;
    }
    const request = clearWineScanPolling();
    const wine = state.wine;
    resetWineScan();
    wine.step = "preview";
    wine.profileId = profileId;
    wine.scanPhase = "starting";
    setWineNotice("", "info");
    renderWineSetupPanel();
    try {
      const result = await invoke<unknown>("start_wine_profile_scan", { profileId });
      if (request !== wineScanRequest) {
        return;
      }
      const status = normaliseWineScanStatus(result);
      const jobId = readStringFromUnknown(result, "jobId", "job_id");
      if (!jobId) {
        throw new Error("Wine-Staging did not start the scan that was requested.");
      }
      wine.scanJobId = jobId;
      wine.scanStatus = status;
      wine.scanPhase = status.state;
      renderWineSetupPanel();
      void loadWineScanPage(request);
      if (status.state === "starting" || status.state === "scanning") {
        scheduleWineScanPoll(request, 350);
      }
    } catch (error) {
      if (request !== wineScanRequest) {
        return;
      }
      wine.scanPhase = "error";
      wine.scanStatus = {
        state: "error",
        scannedFiles: 0,
        foundGames: 0,
        message: messageFromError(error, "The Wine game scan could not start."),
      };
      renderWineSetupPanel();
    }
  };

  const createWineProfile = async (): Promise<void> => {
    const wine = state.wine;
    const setup = wine.setup;
    const displayName = wine.displayName.trim();
    if (!setup || !displayName || setup.directories.length === 0 || !isTauriRuntime()) {
      return;
    }
    wine.step = "loading";
    setWineNotice("", "info");
    renderWineSetupPanel();
    try {
      const result = await invoke<unknown>("create_wine_profile", {
        setupId: setup.setupId,
        displayName,
      });
      const profile = normaliseWineProfile(result, {
        id: readStringFromUnknown(result, "profileId", "profile_id"),
        displayName,
        enabled: true,
        wineLabel: setup.wineLabel,
        directories: setup.directories,
        lastImport: "",
        lastImportSummary: "",
        graphicsBackend: "wine_d3d",
        graphicsSummary: "Wine 3D · default compatibility mode",
      });
      if (!profile) {
        throw new Error("Wine-Staging did not create a valid profile.");
      }
      wine.profile = profile;
      wine.profileId = profile.id;
      wine.step = "preview";
      await startWineProfileScan(profile.id);
    } catch (error) {
      wine.step = "directories";
      setWineNotice(messageFromError(error, "The Wine profile could not be created."), "error");
      renderWineSetupPanel();
    }
  };

  const cancelWineScan = async (): Promise<void> => {
    const wine = state.wine;
    const jobId = wine.scanJobId;
    if (!jobId || !isTauriRuntime() || wine.scanPhase === "cancelling") {
      return;
    }
    const request = clearWineScanPolling();
    wine.scanPhase = "cancelling";
    renderWineSetupPanel();
    try {
      await invoke("cancel_wine_scan", { jobId });
      if (request !== wineScanRequest || wine.scanJobId !== jobId) {
        return;
      }
      wine.scanPhase = "cancelled";
      wine.scanStatus = {
        state: "cancelled",
        scannedFiles: wine.scanStatus?.scannedFiles ?? 0,
        foundGames: wine.scanStatus?.foundGames ?? wine.scanGames.size,
        message: "The Wine scan was cancelled.",
      };
    } catch (error) {
      if (request !== wineScanRequest) {
        return;
      }
      wine.scanPhase = "error";
      wine.scanStatus = {
        state: "error",
        scannedFiles: wine.scanStatus?.scannedFiles ?? 0,
        foundGames: wine.scanStatus?.foundGames ?? wine.scanGames.size,
        message: messageFromError(error, "The Wine scan could not be cancelled."),
      };
    }
    renderWineSetupPanel();
  };

  const retryWineScan = async (): Promise<void> => {
    const profileId = state.wine.profileId || state.wine.profile?.id;
    if (profileId) {
      await startWineProfileScan(profileId);
    }
  };

  const updateWineSelection = (gameRef: string, selected: boolean): void => {
    const wine = state.wine;
    const game = wine.scanGames.get(gameRef);
    if (!game || !game.launchable) {
      return;
    }
    if (selected) {
      if (!wine.selectedGameRefs.has(gameRef) && wine.selectedGameRefs.size >= MAX_WINE_IMPORT_SELECTION) {
        setWineNotice(
          "Choose up to " + MAX_WINE_IMPORT_SELECTION.toLocaleString() + " games per import.",
          "info",
        );
      } else {
        wine.selectedGameRefs.add(gameRef);
      }
    } else {
      wine.selectedGameRefs.delete(gameRef);
    }
    renderWineSetupPanel();
  };

  const setVisibleWineSelection = (selected: boolean): void => {
    const wine = state.wine;
    let capped = false;
    for (const game of [...wine.scanGames.values()].slice(0, MAX_RENDERED_WINE_GAMES)) {
      if (!game.launchable) {
        continue;
      }
      if (selected) {
        if (!wine.selectedGameRefs.has(game.ref) && wine.selectedGameRefs.size >= MAX_WINE_IMPORT_SELECTION) {
          capped = true;
          break;
        }
        wine.selectedGameRefs.add(game.ref);
      } else {
        wine.selectedGameRefs.delete(game.ref);
      }
    }
    if (capped) {
      setWineNotice(
        "Choose up to " + MAX_WINE_IMPORT_SELECTION.toLocaleString() + " games per import.",
        "info",
      );
    }
    renderWineSetupPanel();
  };

  const importWineGames = async (): Promise<void> => {
    const wine = state.wine;
    const profileId = wine.profileId || wine.profile?.id;
    const jobId = wine.scanJobId;
    const gameRefs = [...wine.selectedGameRefs].filter((ref) => wine.scanGames.has(ref));
    if (
      !profileId ||
      !jobId ||
      gameRefs.length === 0 ||
      wine.scanPhase !== "ready" ||
      !isTauriRuntime()
    ) {
      return;
    }

    wine.scanPhase = "importing";
    setWineNotice("", "info");
    renderWineSetupPanel();
    try {
      const result = normaliseWineImportResult(
        await invoke<unknown>("import_wine_games", { profileId, jobId, gameRefs }),
      );
      wine.step = "complete";
      wine.scanPhase = "ready";
      wine.selectedGameRefs.clear();
      setWineNotice(wineImportSummary(result), result.importedIds.length + result.updatedIds.length > 0 ? "success" : "info");
      const importedId = result.importedIds[0] || result.updatedIds[0];
      await Promise.all([refreshLibrary(importedId), refreshWineRunnerSettings(false)]);
      showToast(wine.notice);
    } catch (error) {
      const cancelled = wine.scanStatus?.state === "cancelled";
      wine.scanPhase = cancelled ? "cancelled" : "ready";
      setWineNotice(
        cancelled
          ? "The Wine import was cancelled."
          : messageFromError(error, "The selected Wine games could not be imported."),
        cancelled ? "info" : "error",
      );
    }
    renderWineSetupPanel();
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
      settings.notice = wineNoticeAfterReload?.message ?? "";
      settings.noticeTone = wineNoticeAfterReload?.tone ?? "info";
      wineNoticeAfterReload = null;
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
      // A message handed over by the navigation that triggered this reload
      // outlives the reload; anything else starts clean.
      settings.notice = wineNoticeAfterReload?.message ?? "";
      settings.noticeTone = wineNoticeAfterReload?.tone ?? "info";
      wineNoticeAfterReload = null;
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

  const startWineProfileReimport = async (profileId: string): Promise<void> => {
    const profile = state.wineSettings.profiles.find((candidate) => candidate.id === profileId);
    if (!profile || !profile.enabled) {
      return;
    }
    cancelWineBackgroundWork();
    clearWineScanPolling();
    state.wine.profile = profile;
    state.wine.profileId = profile.id;
    state.wine.displayName = profile.displayName;
    state.wine.setup = null;
    state.wine.step = "preview";
    resetWineScan();
    setWineNotice("", "info");
    setWineSetupOpen(true);
    await startWineProfileScan(profile.id);
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
    refs.steamPanel.setAttribute("aria-busy", String(steam.phase === "scanning" || steam.phase === "importing"));
    const hasAvailablePreview = steam.preview?.status === "available";
    refs.steamRefresh.hidden = !hasAvailablePreview || !steam.open;
    refs.steamRefresh.disabled =
      !hasAvailablePreview || steam.phase === "scanning" || steam.phase === "importing";

    if (!steam.open) {
      const idle = document.createElement("section");
      idle.className = "steam-state steam-state--idle";
      const heading = document.createElement("h2");
      heading.textContent = "Import games already installed on this Mac";
      const message = document.createElement("p");
      message.textContent =
        "Orivo reads your local Steam libraries. Nothing leaves this Mac and nothing is imported until you choose.";
      const scan = document.createElement("button");
      scan.type = "button";
      scan.className = "settings-button settings-button--primary";
      scan.dataset.steamAction = "scan";
      scan.textContent = "Scan Steam libraries";
      idle.append(heading, message, scan);
      refs.steamBody.replaceChildren(idle);
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

  const configureDirectGameWithWine = async (gameId: string): Promise<void> => {
    const game = state.games.find((candidate) => candidate.id === gameId);
    if (!game?.wineAttachable) {
      return;
    }
    navigate({ page: "settings", section: "plugins", attachGameId: gameId });
  };

  const associateDirectGameWithWine = async (profileId: string): Promise<void> => {
    const settings = state.wineSettings;
    const gameId = settings.pendingAttachGameId;
    if (!gameId || !profileId || !isTauriRuntime() || settings.loading) {
      return;
    }
    settings.loading = true;
    settings.notice = "Checking this Windows game against that Wine profile…";
    settings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      const result = await invoke<unknown>("associate_direct_game_with_wine_profile", { gameId, profileId });
      const wineGameId = readStringFromUnknown(result, "gameId", "game_id");
      settings.pendingAttachGameId = "";
      await Promise.all([refreshLibrary(wineGameId || undefined), refreshWineRunnerSettings(false)]);
      settings.notice = "This Windows game is ready to launch with Wine-Staging.";
      settings.noticeTone = "success";
      settings.loading = false;
      showToast(settings.notice);
      // Dropping `attachGame` re-activates Settings, which reloads the runner.
      // Hand the message to that reload so it is not erased by its own success.
      wineNoticeAfterReload = { message: settings.notice, tone: settings.noticeTone };
      navigate({ page: "settings", section: "plugins", attachGameId: null }, { replace: true });
      return;
    } catch (error) {
      settings.notice = messageFromError(error, "This Windows game could not be attached to that Wine profile.");
      settings.noticeTone = "error";
    }
    settings.loading = false;
    renderWineSettingsPanel();
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
      if (game.wineAttachable) {
        await configureDirectGameWithWine(game.id);
        return;
      }
      if (!game.launchable) {
        if (game.source === "steam") {
          showToast("Opening Steam to install " + game.title + "…");
          await invoke("install_steam_game", { gameId: game.id });
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
    applyMotionPreference();
  };

  const renderProviderStatuses = (): void => {
    const list = root.querySelector<HTMLElement>("#provider-status-list");
    if (!list) return;
    const fragment = document.createDocumentFragment();
    for (const provider of state.providerStatuses) {
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
    } else if (action === "wine") {
      void openWineSetup();
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

  refs.winePanel.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const button = target?.closest<HTMLButtonElement>("[data-wine-action]");
    const action = button?.dataset.wineAction;
    if (!action) {
      return;
    }

    if (action === "close") {
      setWineSetupOpen(false);
    } else if (action === "restart-wine-setup") {
      void beginWineProfileSetup();
    } else if (action === "select-wine") {
      void selectWineStaging();
    } else if (action === "confirm-detected-wine") {
      void confirmDetectedWineStaging();
    } else if (action === "cancel-wine-detection") {
      void cancelWineDetection();
    } else if (action === "continue-from-wine") {
      state.wine.step = "name";
      setWineNotice("", "info");
      renderWineSetupPanel();
    } else if (action === "back-to-wine") {
      state.wine.step = "wine";
      setWineNotice("", "info");
      renderWineSetupPanel();
    } else if (action === "back-to-name") {
      state.wine.step = "name";
      setWineNotice("", "info");
      renderWineSetupPanel();
    } else if (action === "choose-wine-directory") {
      void chooseWineGameDirectory();
    } else if (action === "remove-wine-directory") {
      const directoryId = button?.dataset.directoryId;
      if (directoryId) {
        void removeWineSetupDirectory(directoryId);
      }
    } else if (action === "create-wine-profile") {
      void createWineProfile();
    } else if (action === "cancel-wine-scan") {
      void cancelWineScan();
    } else if (action === "retry-wine-scan") {
      void retryWineScan();
    } else if (action === "load-more-wine-games") {
      void loadWineScanPage(wineScanRequest);
    } else if (action === "select-visible-wine-games") {
      setVisibleWineSelection(true);
    } else if (action === "clear-wine-selection") {
      setVisibleWineSelection(false);
    } else if (action === "import-wine-games") {
      void importWineGames();
    }
  });

  refs.winePanel.addEventListener("change", (event) => {
    const target = event.target;
    if (!(target instanceof HTMLInputElement) || !target.dataset.wineGameRef) {
      return;
    }
    updateWineSelection(target.dataset.wineGameRef, target.checked);
  });

  refs.winePanel.addEventListener("submit", (event) => {
    const form = event.target;
    if (!(form instanceof HTMLFormElement) || form.dataset.wineForm !== "profile-name") {
      return;
    }
    event.preventDefault();
    const displayName = form.querySelector<HTMLInputElement>("input[name='wine-profile-name']")?.value ?? "";
    continueWineProfileName(displayName);
  });

  refs.winePanel.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      setWineSetupOpen(false);
    }
  });

  refs.wineSettingsPanel.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const button = target?.closest<HTMLButtonElement>("[data-wine-action]");
    const action = button?.dataset.wineAction;
    if (!action) {
      return;
    }
    if (action === "add-wine-profile") {
      void openWineSetup();
    } else if (action === "cancel-direct-wine-association") {
      state.wineSettings.pendingAttachGameId = "";
      state.wineSettings.notice = "";
      renderWineSettingsPanel();
      navigate({ page: "settings", section: "plugins", attachGameId: null }, { replace: true });
    } else if (action === "associate-direct-game-with-wine") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        void associateDirectGameWithWine(profileId);
      }
    } else if (action === "install-dxvk-macos") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        void installDxvkMacosForProfile(profileId);
      }
    } else if (action === "use-wine-3d") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        void useWine3dForProfile(profileId);
      }
    } else if (action === "reimport-wine-profile") {
      const profileId = button?.dataset.profileId;
      if (profileId) {
        void startWineProfileReimport(profileId);
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
      return;
    }
    try {
      const [app, tauri] = await Promise.all([getVersion(), getTauriVersion()]);
      if (request !== settingsRequest) return;
      if (appVersion) appVersion.textContent = app;
      if (tauriVersion) tauriVersion.textContent = tauri;
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

  const syncTopbar = (route: AppRoute): void => {
    const current = navPageForRoute(route);
    // The Library is a full-bleed hero with its own top gradient, so the topbar
    // floats over it. Every other page scrolls content underneath the bar and
    // needs an opaque, blurred scrim or that content reads straight through it.
    refs.topbar.classList.toggle("topbar--over-content", route.page !== "library");
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
      // A deep-linked Wine attachment lands directly on the Wine plugin detail;
      // otherwise the Plugins section opens its plugin browser.
      state.pluginView =
        route.section === "plugins" ? (route.attachGameId ? "wine" : "list") : "list";
      state.wineSettings.pendingAttachGameId =
        route.section === "plugins" ? route.attachGameId ?? "" : "";
      renderSettingsRoute(route);
      renderPluginList();
      renderWineSettingsPanel();
      renderSteamPanel();
      void loadPreferences(request);

      if (route.section === "libraries") {
        setSteamAccountPanelOpen(true);
        void loadProviderStatuses(request);
      } else if (state.steamAccount.open) {
        setSteamAccountPanelOpen(false);
      }
      if (route.section === "plugins") void refreshWineRunnerSettings();
      if (route.section === "data") void loadDataUsage(request);
      if (route.section === "about") void loadAboutVersions(request);
      if (route.section === "plugins") void loadWallpaperCredentials(request);
      if (activation.restoreState) refs.settingsPage.scrollTop = activation.restoreState.scrollTop;
    },
    deactivate(): PageRestoreState | null {
      settingsRequest += 1;
      const scrollTop = refs.settingsPage.scrollTop;
      state.pluginView = "list";
      state.wineSettings.pendingAttachGameId = "";
      state.wineSettings.pendingDeleteProfileId = "";
      state.steam.open = false;
      wineNoticeAfterReload = null;
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
        navigate({ page: "store", category: "for-you", providers: [], query: "" });
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

  refs.detailsButton.addEventListener("click", () => openGameDetail(selectedGame().id));

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
      const entry = AVAILABLE_PLUGINS.find((candidate) => candidate.id === installId);
      showToast(
        isTauriRuntime()
          ? `${entry?.name ?? "That plugin"} is not installable yet.`
          : `${entry?.name ?? "That plugin"} install is available in the Orivo desktop app.`,
      );
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
        providers: [...route.providers],
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
    // A key event can be dispatched straight at `window`, so the target is only
    // usable once it is known to be an element in this document.
    const target = event.target instanceof HTMLElement ? event.target : null;
    // The Wine wizard is an overlay task: Escape closes it from anywhere.
    if (state.wine.open && event.key === "Escape") {
      event.preventDefault();
      setWineSetupOpen(false);
      return;
    }
    if (refs.winePanel.contains(target) || refs.libraryMenu.contains(target)) {
      return;
    }

    if (state.libraryMenuOpen && event.key === "Escape") {
      event.preventDefault();
      setLibraryMenuOpen(false, undefined, true);
      return;
    }

    const typing =
      target?.tagName === "INPUT" ||
      target?.tagName === "TEXTAREA" ||
      target?.tagName === "SELECT" ||
      target?.isContentEditable === true;
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

    switch (event.key) {
      case "ArrowLeft":
      case "ArrowUp":
        event.preventDefault();
        moveSelection(-1);
        break;
      case "ArrowRight":
      case "ArrowDown":
        event.preventDefault();
        moveSelection(1);
        break;
      case "Enter":
        // Enter opens the detail page. Launching stays on the Play button.
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

  renderSteamPanel();
  renderWineSetupPanel();
  renderWineSettingsPanel();
  renderPreferenceControls();
  router.start(dispatchRoute);
  void refreshLibrary();
  void (async () => {
    await loadPreferences();
    const hash = window.location.hash;
    const isDefaultEntry = hash === "" || hash === "#" || hash === "#/";
    if (isDefaultEntry && state.preferences.startPage === "store") {
      navigate({ page: "store", category: "for-you", providers: [], query: "" }, { replace: true });
    }
  })();
}

async function loadLibrary(): Promise<LibraryLoad | null> {
  if (!isTauriRuntime()) {
    return {
      games: fallbackLibrary.map((game) => ({ ...game })),
      mediaTokens: new Map(),
    };
  }

  try {
    const result = await invoke<unknown>("get_library");
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

  const fallback = fallbackLibrary.find((game) => game.title === title) ?? lastUsedFallback;
  const rawSource = readString(record, "source");
  const source: LibraryGame["source"] =
    rawSource === "steam" || rawSource === "local"
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
      heroUrl: immediateMediaUrl(heroToken) || fallback.heroUrl,
      coverUrl: immediateMediaUrl(coverToken) || fallback.coverUrl,
      landscapeUrl: immediateMediaUrl(landscapeToken) || immediateMediaUrl(heroToken) || fallback.landscapeUrl,
      // An empty value is meaningful for a newly synced Steam game: it has
      // never been launched locally. Do not borrow a fixture's last-played
      // date merely because the backend intentionally returned an empty one.
      lastPlayedAt: readOptionalString(record, "lastPlayedAt", "last_played_at") ?? fallback.lastPlayedAt,
      playTimeSeconds: readNumber(record, "playTimeSeconds", "play_time_seconds") ?? fallback.playTimeSeconds,
      launchable: readBoolean(record, "launchable") ?? fallback.launchable,
      hostPlatform,
      supportedPlatforms,
      compatibleWithHost: readBoolean(record, "compatibleWithHost", "compatible_with_host"),
      wineAttachable: readBoolean(record, "wineAttachable", "wine_attachable") ?? false,
    },
    mediaTokens: {
      heroUrl: heroToken,
      coverUrl: coverToken,
      landscapeUrl: landscapeToken,
    },
  };
}


function immediateMediaUrl(value: string): string {
  return value.startsWith("https://") || value.startsWith("/media/") ? value : "";
}

function readStringFromUnknown(value: unknown, ...keys: string[]): string {
  return isRecord(value) ? readString(value, ...keys) : "";
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

function normaliseWineDetectionState(
  value: string,
  fallback: WineDetectionState | undefined,
  hasValidatedWine: boolean,
): WineDetectionState {
  switch (value.toLocaleLowerCase()) {
    case "detecting":
    case "searching":
      return "detecting";
    case "ready":
    case "complete":
    case "completed":
      return "ready";
    case "unavailable":
    case "missing":
      return "unavailable";
    case "cancelled":
    case "canceled":
      return "cancelled";
    case "error":
    case "failed":
      return "error";
    default:
      return hasValidatedWine ? "ready" : fallback ?? "ready";
  }
}

function normaliseWineSetup(
  value: unknown,
  fallback?: WineSetupSnapshot | null,
): WineSetupSnapshot | null {
  const record = isRecord(value) ? nestedRecord(value, "setup", "wineSetup", "wine_setup") ?? value : null;
  if (!record) {
    return fallback ?? null;
  }
  const setupId = readString(record, "setupId", "setup_id") || fallback?.setupId || "";
  if (!setupId) {
    return fallback ?? null;
  }
  const rawDirectories =
    record.directories ??
    record.gameDirectories ??
    record.game_directories;
  const hasDirectoryList = Array.isArray(rawDirectories);
  let directories = hasDirectoryList
    ? normaliseWineDirectories(rawDirectories)
    : [...(fallback?.directories ?? [])];
  const directory = normaliseWineDirectory(record);
  if (directory && !directories.some((candidate) => candidate.id === directory.id)) {
    directories = [...directories, directory];
  }
  const wineLabel =
    readString(record, "wineLabel", "wine_label", "wineVersion", "wine_version", "version") ||
    fallback?.wineLabel ||
    "";
  const detectionState = normaliseWineDetectionState(
    readString(record, "detectionState", "detection_state"),
    fallback?.detectionState,
    Boolean(wineLabel),
  );
  const reportedDetectedWineLabel = readString(record, "detectedWineLabel", "detected_wine_label");
  const detectedWineLabel =
    reportedDetectedWineLabel ||
    (wineLabel || detectionState === "cancelled" || detectionState === "unavailable" || detectionState === "error"
      ? ""
      : fallback?.detectedWineLabel || "");
  return {
    setupId,
    wineLabel,
    detectedWineLabel,
    detectionState,
    detectionMessage:
      readString(record, "detectionMessage", "detection_message") ||
      fallback?.detectionMessage ||
      "",
    directories,
  };
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

function normaliseWineScanStatus(value: unknown): WineScanStatus {
  const record = isRecord(value) ? value : {};
  const rawState = readString(record, "state", "status", "phase").toLocaleLowerCase();
  const complete = readBoolean(record, "complete", "completed") ?? false;
  let state: WineScanPhase;
  if (rawState === "cancelled" || rawState === "canceled") {
    state = "cancelled";
  } else if (rawState === "error" || rawState === "failed") {
    state = "error";
  } else if (rawState === "starting" || rawState === "queued" || rawState === "pending") {
    state = "starting";
  } else if (rawState === "scanning" || rawState === "running" || rawState === "in_progress") {
    state = "scanning";
  } else if (rawState === "cancelling") {
    state = "cancelling";
  } else if (rawState === "ready" || rawState === "complete" || rawState === "completed") {
    state = "ready";
  } else if (!rawState && complete) {
    state = "ready";
  } else {
    state = "scanning";
  }
  return {
    state,
    scannedFiles: Math.max(0, Math.floor(readNumber(record, "scannedFiles", "scanned_files") ?? 0)),
    foundGames: Math.max(0, Math.floor(readNumber(record, "foundGames", "found_games") ?? 0)),
    message: readString(record, "message"),
  };
}

function normaliseWineScanPage(value: unknown): WineScanPage {
  const record = isRecord(value) ? nestedRecord(value, "page", "result") ?? value : {};
  const rawGames = Array.isArray(record.games)
    ? record.games
    : Array.isArray(record.items)
      ? record.items
      : [];
  const games: WineScanGame[] = [];
  const seenRefs = new Set<string>();
  for (const candidate of rawGames) {
    if (!isRecord(candidate)) {
      continue;
    }
    const ref = readString(candidate, "gameRef", "game_ref", "ref", "id");
    const title = readString(candidate, "title", "name");
    if (!ref || !title || seenRefs.has(ref)) {
      continue;
    }
    seenRefs.add(ref);
    games.push({
      ref,
      title,
      directoryLabel: readString(candidate, "directoryLabel", "directory_label", "locationLabel", "location_label"),
      alreadyImported: readBoolean(candidate, "alreadyImported", "already_imported") ?? false,
      launchable: readBoolean(candidate, "launchable") ?? true,
    });
  }
  return {
    games,
    nextCursor: readOptionalString(record, "nextCursor", "next_cursor") || null,
  };
}

function normaliseWineImportResult(value: unknown): WineImportResult {
  if (!isRecord(value)) {
    throw new Error("Wine-Staging returned an invalid import result.");
  }
  return {
    importedIds: readStringArray(value, "importedIds", "imported_ids"),
    updatedIds: readStringArray(value, "updatedIds", "updated_ids"),
    skippedRefs: readStringArray(value, "skippedRefs", "skipped_refs", "skippedGameRefs", "skipped_game_refs"),
    message: readString(value, "message"),
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

function wineImportSummary(result: WineImportResult): string {
  if (result.message) {
    return result.message;
  }
  const parts: string[] = [];
  if (result.importedIds.length > 0) {
    parts.push(result.importedIds.length === 1 ? "1 Wine game imported" : result.importedIds.length + " Wine games imported");
  }
  if (result.updatedIds.length > 0) {
    parts.push(result.updatedIds.length === 1 ? "1 Wine game updated" : result.updatedIds.length + " Wine games updated");
  }
  if (result.skippedRefs.length > 0) {
    parts.push(result.skippedRefs.length === 1 ? "1 game skipped" : result.skippedRefs.length + " games skipped");
  }
  return parts.length > 0 ? parts.join(" · ") + "." : "No Wine game needed importing.";
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
              <img class="brand-mark" src="/media/orivo-ring-icon.png" alt="" />
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
              <button type="button" class="library-source-action" role="menuitem" data-library-action="wine">
                <span class="library-source-action__icon" aria-hidden="true">${icon("monitor")}</span>
                <span class="library-source-action__copy"><strong>Add Wine-Staging</strong><small>Create an isolated Wine profile</small></span>
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
          <button type="button" class="quiet-icon" aria-label="Notifications">${icon("bell")}</button>
          <span class="top-divider top-divider--right" aria-hidden="true"></span>
          <img class="avatar" src="/media/steam-avatar.png" alt="Steam profile" />
        </div>
      </header>

      <div class="selector">
      <div id="app-page-library" class="app-page app-page--library">
      <div class="hero-media" aria-hidden="true">
        <img id="hero-a" class="hero-image" alt="" />
        <img id="hero-b" class="hero-image" alt="" />
      </div>
      <div class="scene-overlay scene-overlay--left" aria-hidden="true"></div>
      <div class="scene-overlay scene-overlay--bottom" aria-hidden="true"></div>
      <div class="scene-overlay scene-overlay--top" aria-hidden="true"></div>

      <button id="previous-game" class="scene-arrow scene-arrow--previous" type="button" aria-label="Previous game">${icon("chevron-left")}</button>
      <button id="next-game" class="scene-arrow scene-arrow--next" type="button" aria-label="Next game">${icon("chevron-right")}</button>

      <section class="hero-content" aria-live="polite">
        <span id="hero-genre" class="genre-chip">RPG</span>
        <h1 id="hero-title" class="hero-title">Elden Ring</h1>
        <p id="hero-description" class="hero-description"></p>
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
        <div class="hero-actions">
          <button id="play-button" class="play-button" type="button">${icon("play")}<span>Play</span></button>
          <button id="details-button" class="round-button" type="button" aria-label="Open game details">${icon("chevron-right")}</button>
        </div>
        <div id="launch-feedback" class="launch-feedback" role="status" aria-live="polite" hidden></div>
      </section>

      <section class="recently-played" aria-label="Library games">
        <section class="most-played" aria-labelledby="most-played-title" hidden>
          <div class="rail-header rail-header--most-played">
            <h2 id="most-played-title">Most Played</h2>
          </div>
          <div id="most-played-cards" class="game-cards most-played-cards" role="list" aria-label="Most played games"></div>
        </section>
        <div class="rail-header">
          <h2 id="recently-played-title">Recently Played</h2>
          <div class="rail-filters" aria-label="Library view controls">
            <button type="button" class="compact-filter">All Games ${icon("chevron-down")}</button>
            <button type="button" class="compact-filter">Recent ${icon("chevron-down")}</button>
            <button type="button" class="compact-icon" aria-label="Grid view">${icon("grid")}</button>
            <button type="button" class="compact-icon" aria-label="Compact grid view">${icon("layout")}</button>
          </div>
        </div>
        <div id="game-cards" class="game-cards" role="list" aria-label="Recently played games"></div>
      </section>

      <footer class="controller-hud" aria-label="Keyboard controls">
        <span class="hud-category"><i></i><span>Popular</span></span>
        <span class="hud-pagination" aria-hidden="true"><i></i><i class="is-active"></i><i></i><i></i><i></i></span>
        <span class="hud-controls">
          <span>${icon("navigate")}<em>Navigate</em></span>
          <span><b class="gamepad-a">A</b><em>Open</em></span>
          <span><b class="gamepad-b">B</b><em>Back</em></span>
        </span>
      </footer>
      </div>

      <div id="app-page-store" class="app-page app-page--scroll"></div>
      <div id="app-page-me" class="app-page app-page--scroll"></div>

      <div id="app-page-game" class="app-page app-page--scroll"></div>

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
              <section id="steam-account-panel" class="settings-card" data-settings-searchable aria-labelledby="steam-account-title">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("steam")}</span>
                  <div class="settings-card__copy">
                    <strong id="steam-account-title">Steam library</strong>
                    <small>Private, local-first connection</small>
                  </div>
                </header>
                <div id="steam-account-body" class="steam-account-body"></div>
              </section>

              <section id="steam-import-panel" class="settings-card" data-settings-searchable aria-labelledby="steam-import-title" aria-describedby="steam-import-detail">
                <header class="settings-card__header">
                  <span class="settings-card__mark" aria-hidden="true">${icon("download")}</span>
                  <div class="settings-card__copy">
                    <strong id="steam-import-title">Import installed games</strong>
                    <small id="steam-import-detail">A local Steam source</small>
                  </div>
                  <button id="steam-refresh" type="button" class="steam-header-button" data-steam-action="refresh" aria-label="Refresh Steam library" hidden>${icon("refresh")}</button>
                </header>
                <div id="steam-import-body" class="steam-import-body"></div>
                <footer id="steam-import-footer" class="steam-import-footer" hidden>
                  <p id="steam-selection-summary"></p>
                  <button id="steam-import-selected" class="steam-import-button" type="button" data-steam-action="import">Import selected</button>
                </footer>
              </section>

              <section class="settings-card" data-settings-searchable aria-labelledby="provider-status-title">
                <header class="settings-card__header">
                  <div class="settings-card__copy">
                    <strong id="provider-status-title">Provider status</strong>
                    <small>Where store data can come from on this Mac.</small>
                  </div>
                </header>
                <div id="provider-status-list" class="provider-status-list"></div>
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
                  <div class="plugins-group__list">
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
                  <p class="plugins-group__label">Available</p>
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
                    <input id="wallpaper-igdb-client-id" class="credentials-form__input" type="text" autocomplete="off" spellcheck="false" placeholder="From the IGDB API site" />
                    <small>Optional. Enables the IGDB artwork source.</small>
                  </div>
                  <div class="credentials-form__field">
                    <label for="wallpaper-igdb-client-secret">IGDB Client Secret</label>
                    <input id="wallpaper-igdb-client-secret" class="credentials-form__input" type="password" autocomplete="off" spellcheck="false" placeholder="From the IGDB API site" />
                    <small>Optional. Used with the client ID to request an access token.</small>
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
                  <button type="button" class="settings-button" data-wine-action="add-wine-profile">Add a profile</button>
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

      <aside id="wine-setup-panel" class="wine-panel" role="complementary" aria-labelledby="wine-setup-title" aria-describedby="wine-setup-detail" hidden>
        <header class="steam-panel-header">
          <div class="steam-panel-title">
            <span class="steam-source-mark wine-source-mark" aria-hidden="true">${icon("monitor")}</span>
            <span>
              <strong id="wine-setup-title">Add Wine-Staging</strong>
              <small id="wine-setup-detail">An isolated Wine profile for Orivo</small>
            </span>
          </div>
          <div class="steam-panel-actions">
            <button type="button" class="steam-header-button" data-wine-action="close" aria-label="Close Wine-Staging setup">${icon("close")}</button>
          </div>
        </header>
        <div id="wine-setup-body" class="wine-panel-body"></div>
      </aside>

      <p id="toast" class="toast" role="status" aria-live="polite"></p>
      </div>
  `;
}
