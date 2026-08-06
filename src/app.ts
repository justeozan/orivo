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
import { createDefaultQuikyClient, normaliseTitle, type QuikyClient } from "./quiky-install";
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
const MAX_RENDERED_STEAM_GAMES = 120;
const MAX_STEAM_PREVIEW_MEDIA = 16;
const MAX_STEAM_IMPORT_SELECTION = 2_000;
const MAX_AUTOMATIC_STEAM_SELECTION = 50;
const MAX_RENDERED_LIBRARY_CARDS = 48;
const MAX_LIBRARY_MEDIA_HYDRATION = 16;
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

    // The art stands alone: no name overlay and no darkening gradient. The
    // title lives in the aria-label, and the playtime keeps its own corner.
    const time = document.createElement("span");
    time.className = "card-time";

    card.append(media, time);
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
    if (time) {
      time.textContent = formatPlayTime(game.playTimeSeconds);
      time.hidden = game.playTimeSeconds <= 0;
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
    const sourceName =
      game.source === "steam"
        ? "Steam"
        : game.source === "wine"
          ? "Windows"
          : game.source === "local"
            ? "Local"
            : "";
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
    refs.playButton.disabled = !game.launchable && !isSteamInstallable;
    refs.playButton.setAttribute(
      "aria-label",
      game.launchable
        ? "Play " + game.title
        : isSteamInstallable
          ? "Install " + game.title + " in Steam"
          : game.title + " is unavailable",
    );
    const playLabel = refs.playButton.querySelector<HTMLElement>("span");
    if (playLabel) {
      playLabel.textContent = game.launchable
        ? "Play"
        : isSteamInstallable
          ? "Install"
          : "Unavailable";
    }
    updateHeroImage(game, immediateHero);
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

  let discoveredInstaller: Awaited<ReturnType<QuikyClient["getStatus"]>> | null = null;
  const installerClient = createDefaultQuikyClient();
  void installerClient
    .getStatus(new AbortController().signal)
    .then((status) => {
      discoveredInstaller = status;
      renderPluginList();
    })
    .catch(() => {});
  // The host writes the finished game straight into the catalog, so the shell
  // reloads the library on its own rather than making the user find a refresh.
  installerClient.subscribe((progress) => {
    if (progress.phase === "installed") void refreshLibrary();
  });

  /**
   * The Quiky row predates the registry and is discovered through its own
   * command. Once the registry reports the same plugin, the registry wins: it
   * is the row that can be uninstalled, and two rows for one plugin is worse
   * than either of them alone.
   */
  const discoveredInstallerRow = (installed: InstalledPluginView[]): HTMLElement | null => {
    const status = discoveredInstaller;
    if (!status?.available) return null;
    const name = normaliseTitle(status.pluginName);
    if (name && installed.some((plugin) => normaliseTitle(plugin.name) === name)) return null;
    const row = document.createElement("div");
    row.className = "settings-row plugin-row";
    row.dataset.pluginManaged = "installer";
    row.dataset.pluginDiscovered = "installer";
    const installable = status.titles.length;
    row.innerHTML = `
      <span class="settings-card__mark plugin-row__mark" aria-hidden="true">${icon("download")}</span>
      <div class="settings-row__copy">
        <strong></strong>
        <small>Installe les jeux du Store — ${installable} titre${installable > 1 ? "s" : ""} disponible${installable > 1 ? "s" : ""}</small>
      </div>
      <span class="plugin-row__state">Installed</span>`;
    // The plugin names itself; that name never reaches innerHTML.
    row.querySelector("strong")!.textContent = status.pluginName;
    return row;
  };

  // Third-party plugins are discovered on disk, so the Installed group is the
  // two native runners plus whatever the registry found. Nothing extra renders
  // when the registry is empty: the panel then looks exactly as it did before.
  const renderDiscoveredPlugins = (): void => {
    for (const stale of refs.pluginsInstalledList.querySelectorAll("[data-plugin-managed]")) {
      stale.remove();
    }
    const installed = pluginManager.catalog().installed;
    const rows = installed.map(renderInstalledPluginRow);
    const discovered = discoveredInstallerRow(installed);
    if (discovered) rows.push(discovered);
    refs.pluginsInstalledList.append(...rows);
  };

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
    refs.pluginsCatalogList.replaceChildren(...matches.map(renderPluginCatalogRow));
    refs.pluginsCatalogEmpty.hidden = matches.length > 0;
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
      refs.search.placeholder = "Search games…";
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
        setSteamAccountPanelOpen(true);
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
    // A key event can be dispatched straight at `window`, so the target is only
    // usable once it is known to be an element in this document.
    const target = event.target instanceof HTMLElement ? event.target : null;
    if (refs.libraryMenu.contains(target)) {
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
  renderWineSettingsPanel();
  renderPreferenceControls();
  router.start(dispatchRoute);
  void refreshLibrary();
  void (async () => {
    await loadPreferences();
    const hash = window.location.hash;
    const isDefaultEntry = hash === "" || hash === "#" || hash === "#/";
    if (isDefaultEntry && state.preferences.startPage === "store") {
      navigate({ page: "store", category: "for-you", platforms: ["pc"], query: "" }, { replace: true });
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

      <div id="app-page-store" class="app-page app-page--store"></div>
      <div id="app-page-me" class="app-page app-page--scroll app-page--overlay"></div>

      <div id="app-page-game" class="app-page app-page--scroll app-page--overlay"></div>

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
