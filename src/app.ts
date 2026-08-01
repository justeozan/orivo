import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { icon } from "./icons";
import { isTauriRuntime, resolveMediaUrl } from "./media";
import { fallbackLibrary, formatPlayTime, type LibraryGame } from "./mock-library";

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
  open: boolean;
  loading: boolean;
  runner: WineRunnerStatus | null;
  profiles: WineProfile[];
  notice: string;
  noticeTone: SteamNoticeTone;
  pendingDeleteProfileId: string;
  pendingAttachGameId: string;
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
}

const lastUsedFallback = fallbackLibrary[0];
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

export function mountApp(root: HTMLElement): void {
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
      open: false,
      loading: false,
      runner: null,
      profiles: [],
      notice: "",
      noticeTone: "info",
      pendingDeleteProfileId: "",
      pendingAttachGameId: "",
    },
    launchFeedback: null,
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
    metadata: get<HTMLElement>("#hero-metadata"),
    platform: get<HTMLElement>("#hero-platform"),
    platformLabel: get<HTMLElement>("#hero-platform-label"),
    cards: get<HTMLElement>("#game-cards"),
    search: get<HTMLInputElement>("#library-search"),
    libraryMenu: get<HTMLElement>("#library-source-menu"),
    libraryMenuButton: get<HTMLButtonElement>("#library-menu-button"),
    toast: get<HTMLElement>("#toast"),
    steamPanel: get<HTMLElement>("#steam-import-panel"),
    steamBody: get<HTMLElement>("#steam-import-body"),
    steamFooter: get<HTMLElement>("#steam-import-footer"),
    steamSelectionSummary: get<HTMLElement>("#steam-selection-summary"),
    steamImportButton: get<HTMLButtonElement>("#steam-import-selected"),
    steamRefresh: get<HTMLButtonElement>("#steam-refresh"),
    steamAccountPanel: get<HTMLElement>("#steam-account-panel"),
    steamAccountBackdrop: get<HTMLElement>("#steam-account-backdrop"),
    selectorContent: get<HTMLElement>("#selector-content"),
    steamAccountBody: get<HTMLElement>("#steam-account-body"),
    playButton: get<HTMLButtonElement>("#play-button"),
    launchFeedback: get<HTMLElement>("#launch-feedback"),
    winePanel: get<HTMLElement>("#wine-setup-panel"),
    wineBody: get<HTMLElement>("#wine-setup-body"),
    wineSettingsPanel: get<HTMLElement>("#wine-settings-panel"),
    wineSettingsBody: get<HTMLElement>("#wine-settings-body"),
    settingsNav: get<HTMLButtonElement>("#settings-nav-button"),
  };

  let activeHero = 0;
  let heroRequest = 0;
  let toastTimer: number | undefined;
  let steamRequest = 0;
  let libraryRequest = 0;
  const pendingLibraryMediaIds = new Map<string, number>();
  const pendingSteamPreviewMediaIds = new Map<string, number>();
  let steamPreviewMediaRefreshQueued = false;
  let steamReturnFocus: HTMLElement | null = null;
  let steamAccountReturnFocus: HTMLElement | null = null;
  let wineScanRequest = 0;
  let wineScanTimer: number | undefined;
  let wineDetectionRequest = 0;
  let wineDetectionTimer: number | undefined;

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
      retry.textContent = "Réessayer en mode compatibilité";
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
    card.addEventListener("click", () => {
      const id = card.dataset.gameId;
      if (id) {
        selectGame(id);
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
    card.setAttribute("aria-label", `Select ${game.title}`);

    assignCardImage(portrait, portraitSource, fallback, index < 7);
    assignCardImage(landscape, landscapeSource, fallback, index < 7);
    if (title) {
      title.textContent = game.title;
    }
    if (time) {
      time.textContent = formatPlayTime(game.playTimeSeconds);
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
    refs.lastPlayed.textContent = game.lastPlayedAt
      ? `Last played ${game.lastPlayedAt}`
      : "Not played yet";
    const isSteamInstallable = game.source === "steam" && !game.launchable;
    const isWineAttachable = Boolean(game.wineAttachable);
    const sourceName =
      game.source === "steam"
        ? "Steam"
        : game.source === "wine"
          ? "Wine-Staging"
          : game.source === "local"
            ? "This Mac"
            : "";
    refs.source.hidden = !sourceName;
    refs.sourceIcon.innerHTML =
      game.source === "steam" ? icon("steam") : game.source === "wine" ? icon("monitor") : icon("folder");
    refs.sourceLabel.textContent = sourceName;
    refs.metadata.textContent = game.metadata || "Ready to play";
    const hostName = platformName(game.hostPlatform);
    const compatibility = game.compatibleWithHost;
    const compatibilityLabel =
      compatibility === true && hostName
        ? `${hostName} compatible`
        : compatibility === false && hostName
          ? `Not supported on ${hostName}`
          : "";
    refs.platform.hidden = !compatibilityLabel;
    refs.platform.classList.toggle("is-compatible", compatibility === true);
    refs.platform.classList.toggle("is-incompatible", compatibility === false);
    refs.platformLabel.textContent = compatibilityLabel;
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
            ? "Configurer Wine"
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

  const setLibraryMenuOpen = (open: boolean, focus?: "first" | "last", restoreFocus = false): void => {
    state.libraryMenuOpen = open;
    refs.libraryMenu.hidden = !open;
    refs.libraryMenuButton.setAttribute("aria-expanded", String(open));
    refs.topbar.classList.toggle("is-library-menu-open", open);

    if (open && focus) {
      requestAnimationFrame(() => focusLibraryMenuItem(focus));
    } else if (!open && restoreFocus) {
      refs.libraryMenuButton.focus();
    }
  };

  const refreshLibrary = async (importedId?: string): Promise<void> => {
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
    state.wine.open = open;
    refs.winePanel.hidden = !open;
    if (open) {
      closeLibraryMenu();
      state.wineSettings.open = false;
      renderWineSettingsPanel();
      if (state.steam.open) {
        state.steam.open = false;
        renderSteamPanel();
      }
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
      return "Vérification de Wine-Staging…";
    }
    if (runner.state === "ready") {
      return runner.version ? "Prêt · " + runner.version : "Prêt";
    }
    if (runner.state === "unavailable") {
      return "Wine-Staging introuvable";
    }
    if (runner.state === "invalid") {
      return "Installation Wine non valide";
    }
    return "Wine-Staging indisponible";
  };

  const appendWineRunnerSummary = (parent: HTMLElement, runner: WineRunnerStatus | null): void => {
    const overview = document.createElement("section");
    overview.className = "wine-runner-summary";
    const mark = document.createElement("span");
    mark.className = "steam-state__icon";
    mark.innerHTML = icon(runner?.state === "ready" ? "monitor" : "alert");
    mark.setAttribute("aria-hidden", "true");
    const copy = document.createElement("div");
    const heading = document.createElement("h2");
    heading.textContent = "Wine-Staging";
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
          "Validation avant l’import",
          "Orivo vérifie les exécutables sélectionnés. Vous pouvez annuler.",
        );
        return;
      } else if (wine.scanPhase === "scanning" || wine.scanPhase === "starting" || wine.scanPhase === "cancelling") {
        appendWineLoadingState(
          parent,
          "Analyse des dossiers de jeux",
          "Orivo recherche des exécutables Windows en arrière-plan. Vous pouvez continuer à naviguer.",
        );
        return;
      }

      const empty = document.createElement("section");
      empty.className = "steam-list-empty";
      const heading = document.createElement("h2");
      heading.textContent =
        wine.scanPhase === "cancelled"
          ? "Import annulé"
          : wine.scanPhase === "error"
            ? "L’analyse n’a pas abouti"
            : "Aucun jeu Windows trouvé";
      const message = document.createElement("p");
      message.textContent =
        wine.scanPhase === "cancelled"
          ? "Vous pouvez relancer l’analyse quand vous le souhaitez."
          : wine.scanPhase === "error"
            ? status?.message || "Vos jeux déjà présents dans Orivo ne sont pas affectés."
            : "Ajoutez un autre dossier autorisé ou réessayez après avoir vérifié son contenu.";
      empty.append(heading, message);
      parent.append(empty);
      return;
    }

    const summary = document.createElement("p");
    summary.className = "wine-results-count";
    const foundCount = status?.foundGames ?? games.length;
    const foundLabel = foundCount === 1 ? "1 jeu détecté" : foundCount.toLocaleString() + " jeux détectés";
    const importedLabel = importedCount > 0 ? " · " + importedCount + " déjà dans la bibliothèque" : "";
    summary.textContent =
      foundCount > games.length
        ? foundLabel + importedLabel + " · aperçu des " + games.length + " premiers"
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
      selectedReady === selectable.length && selectable.length > 0 ? "Tout désélectionner" : "Sélectionner les jeux prêts",
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
      toggle.setAttribute("aria-label", "Sélectionner " + game.title);

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
      metadata.textContent = game.directoryLabel || "Dossier autorisé";
      copy.append(gameTitle, metadata);

      const badge = document.createElement("span");
      badge.className = "steam-game-status";
      badge.textContent = game.alreadyImported ? "Dans la bibliothèque" : game.launchable ? "Prêt" : "Non lançable";
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
        wine.step === "preview" || wine.step === "complete" ? "Importer avec Wine-Staging" : "Ajouter Wine-Staging";
    }
    if (detail) {
      detail.textContent =
        wine.profile?.displayName ||
        wine.setup?.wineLabel ||
        (wine.step === "loading" ? "Préparation du profil isolé" : "Un profil Wine isolé pour Orivo");
    }

    refs.wineBody.replaceChildren();
    const body = refs.wineBody;

    if (wine.step === "loading") {
      appendWineLoadingState(
        body,
        "Préparation de Wine-Staging",
        "Orivo vérifie le runner local sans modifier les préfixes d’autres applications.",
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
      heading.textContent = "Wine-Staging ne peut pas être configuré";
      const message = document.createElement("p");
      message.textContent = wine.notice || "Aucun changement n’a été apporté à votre bibliothèque.";
      const retry = wineActionButton("restart-wine-setup", "Réessayer", "steam-secondary-button", "refresh");
      unavailable.append(badge, heading, message, retry);
      body.append(unavailable);
      return;
    }

    if (wine.step === "wine") {
      const intro = document.createElement("section");
      intro.className = "wine-step-intro";
      const heading = document.createElement("h2");
      heading.textContent = "1. Choisir Wine-Staging";
      const message = document.createElement("p");
      message.textContent =
        "Sélectionnez une installation Wine-Staging déjà présente sur ce Mac. Orivo n’installe rien et n’utilisera qu’un préfixe dédié.";
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
          "Recherche d’une installation Wine-Staging locale en arrière-plan.";
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
        detected.textContent = "Installation Wine-Staging détectée : " + setup.detectedWineLabel;
        body.append(detected);
      }

      const select = wineActionButton(
        "select-wine",
        setup?.wineLabel ? "Changer l’installation Wine-Staging" : "Sélectionner manuellement Wine-Staging",
        setup?.wineLabel ? "steam-secondary-button" : "steam-import-button",
        "monitor",
      );
      const actions = document.createElement("div");
      actions.className = "wine-step-actions";
      if (setup?.wineLabel) {
        actions.append(wineActionButton("continue-from-wine", "Continuer", "steam-secondary-button"));
        actions.append(select);
      } else {
        if (setup?.detectedWineLabel) {
          actions.append(
            wineActionButton(
              "confirm-detected-wine",
              "Utiliser Wine-Staging détecté",
              "steam-import-button",
              "monitor",
            ),
          );
        }
        actions.append(select);
        if (setup?.detectionState === "detecting") {
          actions.append(wineActionButton("cancel-wine-detection", "Annuler la détection", "steam-secondary-button", "close"));
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
      heading.textContent = "2. Nommer le profil";
      const message = document.createElement("p");
      message.textContent =
        "Chaque profil possède son propre préfixe Wine géré par Orivo. Aucun préfixe existant ne sera modifié.";
      intro.append(heading, message);
      body.append(intro);

      const form = document.createElement("form");
      form.className = "wine-profile-form";
      form.dataset.wineForm = "profile-name";
      const label = document.createElement("label");
      label.textContent = "Nom du profil";
      const input = document.createElement("input");
      input.name = "wine-profile-name";
      input.autocomplete = "off";
      input.maxLength = 80;
      input.required = true;
      input.placeholder = "Par exemple, Jeux Windows";
      input.value = wine.displayName;
      label.append(input);
      const actions = document.createElement("div");
      actions.className = "wine-step-actions";
      actions.append(
        wineActionButton("back-to-wine", "Retour", "steam-secondary-button", "close"),
        wineActionButton("continue-to-directories", "Continuer", "steam-import-button"),
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
      heading.textContent = "3. Autoriser les dossiers de jeux";
      const message = document.createElement("p");
      message.textContent =
        "Choisissez les dossiers à analyser. Orivo n’affichera ici que leurs libellés et n’analysera jamais d’autres emplacements.";
      intro.append(heading, message);
      body.append(intro);

      const directories = wine.setup?.directories ?? [];
      if (directories.length === 0) {
        const empty = document.createElement("section");
        empty.className = "wine-directory-empty";
        empty.textContent = "Aucun dossier de jeux autorisé pour ce profil.";
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
          const remove = wineActionButton("remove-wine-directory", "Retirer", "wine-text-button");
          remove.dataset.directoryId = directory.id;
          row.append(label, remove);
          list.append(row);
        }
        body.append(list);
      }

      const add = wineActionButton("choose-wine-directory", "Ajouter un dossier de jeux", "steam-secondary-button", "folder");
      body.append(add);
      const actions = document.createElement("div");
      actions.className = "wine-step-actions";
      const back = wineActionButton("back-to-name", "Retour", "steam-secondary-button", "close");
      const create = wineActionButton("create-wine-profile", "Analyser les jeux", "steam-import-button", "search");
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
      heading.textContent = "4. Aperçu des jeux trouvés";
      const message = document.createElement("p");
      const scan = wine.scanStatus;
      if (wine.scanPhase === "importing") {
        message.textContent = "Validation des exécutables avant l’import · vous pouvez annuler.";
      } else if (wine.scanPhase === "scanning" || wine.scanPhase === "starting" || wine.scanPhase === "cancelling") {
        const fileCount = scan?.scannedFiles ?? 0;
        message.textContent =
          fileCount > 0
            ? fileCount.toLocaleString() + " fichiers vérifiés · vous pouvez continuer à utiliser Orivo."
            : "L’analyse se poursuit en arrière-plan.";
      } else {
        message.textContent =
          scan?.message || "Sélectionnez les exécutables à ajouter à votre bibliothèque.";
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
              ? "Annuler l’import"
              : "Annuler l’analyse",
          "steam-secondary-button",
          "close",
        );
        cancel.disabled = wine.scanPhase === "cancelling";
        actions.append(cancel);
      } else if (wine.scanPhase === "error" || wine.scanPhase === "cancelled") {
        actions.append(wineActionButton("retry-wine-scan", "Relancer l’analyse", "steam-secondary-button", "refresh"));
      }
      const selectedCount = wine.selectedGameRefs.size;
      const importButton = wineActionButton(
        "import-wine-games",
        selectedCount === 1 ? "Importer 1 jeu" : "Importer " + selectedCount + " jeux",
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
    heading.textContent = "Import Wine terminé";
    const message = document.createElement("p");
    message.textContent = wine.notice || "Les jeux importés sont disponibles dans votre bibliothèque.";
    const done = wineActionButton("close", "Terminer", "steam-import-button");
    completed.append(badge, heading, message, done);
    body.append(completed);
  };

  const renderWineSettingsPanel = (): void => {
    const settings = state.wineSettings;
    refs.wineSettingsPanel.hidden = !settings.open;
    refs.wineSettingsPanel.setAttribute("aria-busy", String(settings.loading));
    if (!settings.open) {
      return;
    }

    refs.wineSettingsBody.replaceChildren();
    const body = refs.wineSettingsBody;
    if (settings.loading) {
      appendWineLoadingState(
        body,
        "Chargement des plugins",
        "Orivo lit l’état local du runner Wine et de ses profils.",
      );
      return;
    }

    appendWineRunnerSummary(body, settings.runner);
    appendWineNotice(body, settings.notice, settings.noticeTone);

    const attachedGame = settings.pendingAttachGameId
      ? state.games.find((game) => game.id === settings.pendingAttachGameId && game.wineAttachable)
      : undefined;
    if (attachedGame) {
      const association = document.createElement("section");
      association.className = "wine-direct-association";
      const heading = document.createElement("h2");
      heading.textContent = "Configurer avec Wine";
      const message = document.createElement("p");
      message.textContent =
        "Choisissez un profil qui autorise déjà le dossier de “" +
        attachedGame.title +
        "”. Orivo validera l’exécutable avant de le lancer, sans modifier sa fiche Direct.";
      const cancel = wineActionButton("cancel-direct-wine-association", "Annuler", "wine-text-button");
      association.append(heading, message, cancel);
      body.append(association);
    } else if (settings.pendingAttachGameId) {
      settings.pendingAttachGameId = "";
    }

    const heading = document.createElement("h2");
    heading.className = "wine-settings-heading";
    heading.textContent = "Profils Wine";
    body.append(heading);

    if (settings.profiles.length === 0) {
      const empty = document.createElement("section");
      empty.className = "wine-directory-empty";
      empty.textContent = attachedGame
        ? "Créez d’abord un profil Wine-Staging et autorisez le dossier de ce jeu Windows."
        : "Aucun profil Wine-Staging n’est encore configuré.";
      const add = wineActionButton("add-wine-profile", "Ajouter Wine-Staging", "steam-secondary-button", "monitor");
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
      stateLabel.textContent = profile.enabled ? "Actif" : "Désactivé";
      profileHeader.append(copy, stateLabel);
      card.append(profileHeader);

      const directoryHeading = document.createElement("p");
      directoryHeading.className = "wine-profile-card__label";
      directoryHeading.textContent = "Dossiers autorisés";
      card.append(directoryHeading);
      const directories = document.createElement("ul");
      directories.className = "wine-profile-directories";
      if (profile.directories.length === 0) {
        const item = document.createElement("li");
        item.textContent = "Aucun dossier autorisé";
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
        warning.textContent = "Expérimental · évitez les jeux protégés par anti-cheat.";
        card.append(warning);
      } else {
        const dxvkHint = document.createElement("p");
        dxvkHint.className = "wine-profile-card__hint";
        dxvkHint.textContent =
          "Compatibilité automatique · au premier lancement d’un jeu Wine compatible, Orivo prépare DXVK-macOS si nécessaire pour ce profil, sans modifier Wine-Staging ni les préfixes d’autres applications.";
        card.append(dxvkHint);
      }

      const lastImport = document.createElement("p");
      lastImport.className = "wine-profile-card__import";
      const importDetails = [
        profile.lastImport ? "Dernier import · " + profile.lastImport : "",
        profile.lastImportSummary,
      ].filter(Boolean);
      lastImport.textContent = importDetails.length > 0 ? importDetails.join(" · ") : "Aucun import effectué";
      card.append(lastImport);

      if (settings.pendingDeleteProfileId === profile.id) {
        const confirmation = document.createElement("div");
        confirmation.className = "wine-delete-confirmation";
        const prompt = document.createElement("p");
        prompt.textContent = "Supprimer ce profil Wine ? Ses jeux Wine seront retirés de la bibliothèque.";
        const confirm = wineActionButton("confirm-delete-wine-profile", "Supprimer le profil", "wine-danger-button");
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
            "Utiliser pour “" + attachedGame.title + "”",
            "steam-import-button",
            "monitor",
          );
          associate.dataset.profileId = profile.id;
          associate.disabled = !profile.enabled;
          actions.append(associate);
        }
        const dxvk = wineActionButton(
          "install-dxvk-macos",
          profile.graphicsBackend === "dxvk_macos" ? "Réinstaller DXVK-macOS" : "Activer DXVK-macOS",
          "steam-secondary-button",
          "monitor",
        );
        dxvk.dataset.profileId = profile.id;
        dxvk.disabled = !profile.enabled;
        if (profile.graphicsBackend === "dxvk_macos") {
          const wine3d = wineActionButton(
            "use-wine-3d",
            "Revenir à Wine 3D",
            "steam-secondary-button",
          );
          wine3d.dataset.profileId = profile.id;
          actions.append(wine3d);
        }
        const reimport = wineActionButton("reimport-wine-profile", "Relancer l’import", "steam-secondary-button", "refresh");
        reimport.dataset.profileId = profile.id;
        reimport.disabled = !profile.enabled;
        const toggle = wineActionButton(
          "toggle-wine-profile",
          profile.enabled ? "Désactiver" : "Activer",
          "steam-secondary-button",
        );
        toggle.dataset.profileId = profile.id;
        toggle.dataset.enabled = String(!profile.enabled);
        const remove = wineActionButton("delete-wine-profile", "Supprimer", "wine-text-button");
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
        throw new Error("La détection Wine-Staging n’a pas renvoyé d’état valide.");
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
        detectionMessage: messageFromError(error, "La détection Wine-Staging n’a pas pu être suivie."),
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
      detectionMessage: "Annulation de la détection Wine-Staging…",
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
        detectionMessage: "Détection Wine-Staging annulée.",
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
        detectionMessage: messageFromError(error, "La détection Wine-Staging n’a pas pu être annulée."),
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
        message: "Wine-Staging est disponible dans l’app Orivo pour macOS.",
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
        message: messageFromError(error, "L’état de Wine-Staging est temporairement indisponible."),
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
      setWineNotice("La configuration Wine-Staging est disponible dans l’app Orivo pour macOS.", "error");
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
        throw new Error("Wine-Staging a renvoyé une préparation de profil invalide.");
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
      setWineNotice(messageFromError(error, "Le profil Wine n’a pas pu être préparé."), "error");
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
          ? "Détection interrompue pour choisir Wine-Staging manuellement."
          : currentSetup.detectionMessage,
    };
    wine.setup = stoppedSetup;
    wine.step = "loading";
    setWineNotice("Ouverture du sélecteur Wine-Staging…", "info");
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
        throw new Error("Wine-Staging n’a pas confirmé l’installation sélectionnée.");
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
      setWineNotice(messageFromError(error, "L’installation Wine-Staging n’a pas pu être validée."), "error");
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
    setWineNotice("Validation de Wine-Staging détecté…", "info");
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
        throw new Error("Wine-Staging détecté n’a pas pu être validé.");
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
      setWineNotice(messageFromError(error, "Wine-Staging détecté n’a pas pu être validé."), "error");
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
    setWineNotice("Choisissez un dossier de jeux dans le sélecteur macOS.", "info");
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
      setWineNotice(messageFromError(error, "Le dossier de jeux n’a pas été ajouté."), "error");
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
      setWineNotice(messageFromError(error, "Le dossier autorisé n’a pas pu être retiré."), "error");
    }
    renderWineSetupPanel();
  };

  const continueWineProfileName = (displayName: string): void => {
    const wine = state.wine;
    const trimmed = displayName.trim();
    if (!trimmed) {
      setWineNotice("Donnez un nom à ce profil Wine.", "error");
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
          "Choisissez jusqu’à " + MAX_WINE_IMPORT_SELECTION.toLocaleString() + " jeux par import.",
          "info",
        );
      }
    } catch (error) {
      if (request === wineScanRequest) {
        setWineNotice(messageFromError(error, "L’aperçu des jeux Wine n’a pas pu être chargé."), "error");
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
        message: messageFromError(error, "L’analyse Wine n’a pas pu être suivie."),
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
        throw new Error("Wine-Staging n’a pas démarré l’analyse demandée.");
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
        message: messageFromError(error, "L’analyse des jeux Wine n’a pas pu démarrer."),
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
        graphicsSummary: "Wine 3D · mode de compatibilité par défaut",
      });
      if (!profile) {
        throw new Error("Wine-Staging n’a pas créé de profil valide.");
      }
      wine.profile = profile;
      wine.profileId = profile.id;
      wine.step = "preview";
      await startWineProfileScan(profile.id);
    } catch (error) {
      wine.step = "directories";
      setWineNotice(messageFromError(error, "Le profil Wine n’a pas pu être créé."), "error");
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
        message: "L’analyse Wine a été annulée.",
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
        message: messageFromError(error, "L’analyse Wine n’a pas pu être annulée."),
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
          "Choisissez jusqu’à " + MAX_WINE_IMPORT_SELECTION.toLocaleString() + " jeux par import.",
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
        "Choisissez jusqu’à " + MAX_WINE_IMPORT_SELECTION.toLocaleString() + " jeux par import.",
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
          ? "L’import Wine a été annulé."
          : messageFromError(error, "Les jeux Wine sélectionnés n’ont pas pu être importés."),
        cancelled ? "info" : "error",
      );
    }
    renderWineSetupPanel();
  };

  const refreshWineRunnerSettings = async (render = true): Promise<void> => {
    const settings = state.wineSettings;
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
        message: "Les réglages Wine-Staging sont disponibles dans l’app Orivo pour macOS.",
      };
      settings.profiles = [];
      settings.notice = "";
      if (render) {
        renderWineSettingsPanel();
      }
      return;
    }

    try {
      const snapshot = normaliseWineRunnerSettings(await invoke<unknown>("get_wine_runner_settings"));
      settings.runner = snapshot.runner;
      settings.profiles = snapshot.profiles;
      settings.notice = "";
      settings.pendingDeleteProfileId = "";
    } catch (error) {
      settings.notice = messageFromError(error, "Les réglages Wine-Staging n’ont pas pu être chargés.");
      settings.noticeTone = "error";
    }
    settings.loading = false;
    if (render) {
      renderWineSettingsPanel();
    }
  };

  const setWineSettingsOpen = (open: boolean): void => {
    state.wineSettings.open = open;
    refs.wineSettingsPanel.hidden = !open;
    if (!open) {
      state.wineSettings.pendingAttachGameId = "";
      state.wineSettings.pendingDeleteProfileId = "";
      renderWineSettingsPanel();
      return;
    }
    closeLibraryMenu();
    if (state.wine.open) {
      cancelWineBackgroundWork();
      stopWineDetectionPolling();
      clearWineScanPolling();
      state.wine.open = false;
      renderWineSetupPanel();
    }
    if (state.steam.open) {
      state.steam.open = false;
      renderSteamPanel();
    }
    state.wineSettings.notice = "";
    renderWineSettingsPanel();
    void refreshWineRunnerSettings();
    requestAnimationFrame(() => {
      refs.wineSettingsPanel.querySelector<HTMLButtonElement>("[data-wine-action='add-wine-profile']")?.focus();
    });
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
    state.wineSettings.notice = enabled ? "Activation du profil Wine…" : "Désactivation du profil Wine…";
    state.wineSettings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("set_wine_profile_enabled", { profileId, enabled });
      await refreshWineRunnerSettings(false);
      state.wineSettings.notice = enabled ? "Profil Wine activé." : "Profil Wine désactivé.";
      state.wineSettings.noticeTone = "success";
    } catch (error) {
      state.wineSettings.notice = messageFromError(error, "L’état du profil Wine n’a pas pu être modifié.");
      state.wineSettings.noticeTone = "error";
    }
    renderWineSettingsPanel();
  };

  const deleteWineProfile = async (profileId: string): Promise<void> => {
    if (!profileId || !isTauriRuntime()) {
      return;
    }
    state.wineSettings.notice = "Suppression du profil Wine…";
    state.wineSettings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("delete_wine_profile", { profileId });
      state.wineSettings.pendingDeleteProfileId = "";
      await Promise.all([refreshWineRunnerSettings(false), refreshLibrary()]);
      state.wineSettings.notice = "Profil Wine supprimé, ainsi que ses jeux Wine de la bibliothèque.";
      state.wineSettings.noticeTone = "success";
    } catch (error) {
      state.wineSettings.notice = messageFromError(error, "Le profil Wine n’a pas pu être supprimé.");
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
      const focusTarget =
        refs.steamPanel.querySelector<HTMLInputElement>("#steam-game-search") ??
        refs.steamPanel.querySelector<HTMLButtonElement>("[data-steam-action='close']");
      focusTarget?.focus();
    });
  };

  const renderSteamPanel = (): void => {
    const steam = state.steam;
    refs.steamPanel.hidden = !steam.open;
    refs.steamPanel.setAttribute("aria-busy", String(steam.phase === "scanning" || steam.phase === "importing"));
    const hasAvailablePreview = steam.preview?.status === "available";
    refs.steamRefresh.hidden = !hasAvailablePreview;
    refs.steamRefresh.disabled =
      !hasAvailablePreview || steam.phase === "scanning" || steam.phase === "importing";

    if (!steam.open) {
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

  const setSteamPanelOpen = (open: boolean, trigger?: HTMLElement): void => {
    if (open && state.steamAccount.open) {
      closeSteamAccountPanel();
    }
    if (open && state.steam.open) {
      closeLibraryMenu();
      focusSteamPanel();
      return;
    }
    state.steam.open = open;

    if (!open) {
      renderSteamPanel();
      if (steamReturnFocus?.isConnected && steamReturnFocus.getClientRects().length > 0) {
        steamReturnFocus.focus();
      } else {
        refs.libraryMenuButton.focus();
      }
      steamReturnFocus = null;
      return;
    }

    const activeTrigger = trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    steamReturnFocus = activeTrigger && refs.libraryMenu.contains(activeTrigger)
      ? refs.libraryMenuButton
      : activeTrigger;
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
        refs.steamAccountPanel.querySelector<HTMLElement>("[data-steam-account-action='back']") ??
        refs.steamAccountPanel.querySelector<HTMLElement>("[data-steam-account-action='close']");
      (primaryAction ?? refs.steamAccountPanel).focus();
    });
  };

  const closeSteamAccountPanel = (restoreFocus = false): void => {
    state.steamAccount.open = false;
    refs.steamAccountPanel.hidden = true;
    refs.steamAccountBackdrop.hidden = true;
    refs.selectorContent.removeAttribute("inert");
    refs.selectorContent.removeAttribute("aria-hidden");
    if (restoreFocus) {
      if (steamAccountReturnFocus?.isConnected && steamAccountReturnFocus.getClientRects().length > 0) {
        steamAccountReturnFocus.focus();
      } else {
        refs.libraryMenuButton.focus();
      }
    }
    steamAccountReturnFocus = null;
  };

  const accountActionButton = (
    action: string,
    label: string,
    className = "steam-secondary-button",
    iconName: "library" | "refresh" | "close" | "settings" = "library",
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
    refs.steamAccountBackdrop.hidden = !account.open;
    if (account.open) {
      refs.selectorContent.setAttribute("inert", "");
      refs.selectorContent.setAttribute("aria-hidden", "true");
    } else {
      refs.selectorContent.removeAttribute("inert");
      refs.selectorContent.removeAttribute("aria-hidden");
    }
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
    badge.innerHTML = icon(connected ? "library" : "alert");
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
        accountActionButton("connect", "Continue with Steam", "steam-import-button", "library"),
        accountActionButton("api-key", "Use an API key", "steam-secondary-button", "settings"),
      );
    }
    body.append(actions);
    if (restoreFocus) {
      focusSteamAccountPanel();
    }
  };

  const refreshSteamAccountStatus = async (): Promise<void> => {
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
      if (!status) {
        throw new Error("Steam returned an invalid account status.");
      }
      state.steamAccount.status = status;
      state.steamAccount.phase = status.connected ? "connected" : "disconnected";
      if (state.steamAccount.noticeTone !== "error") {
        state.steamAccount.notice = "";
      }
    } catch (error) {
      state.steamAccount.phase = "error";
      state.steamAccount.notice = messageFromError(error, "Steam account status could not be loaded.");
      state.steamAccount.noticeTone = "error";
    }
    renderSteamAccountPanel();
  };

  const setSteamAccountPanelOpen = (open: boolean, trigger?: HTMLElement): void => {
    if (!open) {
      if (state.steamAccount.phase === "connecting" && isTauriRuntime()) {
        void invoke("cancel_steam_web_login");
      }
      closeSteamAccountPanel(true);
      return;
    }

    if (state.steam.open) {
      state.steam.open = false;
      renderSteamPanel();
    }
    const activeTrigger = trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null);
    steamAccountReturnFocus = activeTrigger && refs.libraryMenu.contains(activeTrigger)
      ? refs.libraryMenuButton
      : activeTrigger;
    closeLibraryMenu();
    state.steamAccount.open = true;
    state.steamAccount.phase = "loading";
    state.steamAccount.notice = "";
    renderSteamAccountPanel();
    void refreshSteamAccountStatus();
    focusSteamAccountPanel();
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
    state.wineSettings.pendingAttachGameId = gameId;
    state.wineSettings.pendingDeleteProfileId = "";
    setWineSettingsOpen(true);
  };

  const associateDirectGameWithWine = async (profileId: string): Promise<void> => {
    const settings = state.wineSettings;
    const gameId = settings.pendingAttachGameId;
    if (!gameId || !profileId || !isTauriRuntime() || settings.loading) {
      return;
    }
    settings.loading = true;
    settings.notice = "Validation du jeu Windows avec ce profil Wine…";
    settings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      const result = await invoke<unknown>("associate_direct_game_with_wine_profile", { gameId, profileId });
      const wineGameId = readStringFromUnknown(result, "gameId", "game_id");
      settings.pendingAttachGameId = "";
      await Promise.all([refreshLibrary(wineGameId || undefined), refreshWineRunnerSettings(false)]);
      settings.notice = "Le jeu Windows est prêt à être lancé avec Wine-Staging.";
      settings.noticeTone = "success";
      showToast(settings.notice);
    } catch (error) {
      settings.notice = messageFromError(error, "Ce jeu Windows n’a pas pu être associé à ce profil Wine.");
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
    settings.notice = "Téléchargement et vérification de DXVK-macOS, puis préparation du préfixe isolé…";
    settings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("install_dxvk_macos_for_profile", { profileId });
      await refreshWineRunnerSettings(false);
      settings.notice = "DXVK-macOS est prêt. Orivo l’utilisera automatiquement pour les jeux DirectX 10/11 compatibles de ce profil.";
      settings.noticeTone = "success";
      showToast(settings.notice);
    } catch (error) {
      settings.notice = messageFromError(error, "DXVK-macOS n’a pas pu être installé pour ce profil.");
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
    settings.notice = "Retour à Wine 3D…";
    settings.noticeTone = "info";
    renderWineSettingsPanel();
    try {
      await invoke("use_wine_3d_for_profile", { profileId });
      await refreshWineRunnerSettings(false);
      settings.notice = "Ce profil utilise maintenant Wine 3D. DXVK-macOS reste isolé dans ce profil.";
      settings.noticeTone = "success";
      showToast(settings.notice);
    } catch (error) {
      settings.notice = messageFromError(error, "Le retour à Wine 3D n’a pas pu être enregistré.");
      settings.noticeTone = "error";
    }
    settings.loading = false;
    renderWineSettingsPanel();
  };

  const launchGame = async (requestedGameId?: string): Promise<void> => {
    const game =
      (requestedGameId ? state.games.find((candidate) => candidate.id === requestedGameId) : undefined) ??
      selectedGame();
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
            message: "Ce jeu Wine ne peut pas être lancé avec ce profil.",
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
          message: "Lancement avec Wine-Staging…",
        };
        renderLaunchFeedback();
      }
      showToast(game.source === "wine" ? "Préparation de Wine pour " + game.title + "…" : "Launching " + game.title + "…");
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
      message: "Préparation du prochain mode de compatibilité Wine…",
    };
    renderLaunchFeedback();
    try {
      await invoke("retry_wine_game_in_compatibility", { gameId });
      await launchGame(gameId);
    } catch (error) {
      const message = messageFromError(error, "Ce mode de compatibilité Wine n’est pas disponible.");
      state.launchFeedback = { gameId, phase: "failed", message };
      renderLaunchFeedback();
      showToast(message);
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

    if (action === "steam-local") {
      setSteamPanelOpen(true, trigger);
    } else if (action === "steam-account") {
      setSteamAccountPanelOpen(true, trigger);
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
    const settingsAction =
      target?.closest<HTMLButtonElement>("[data-wine-settings-action]")?.dataset.wineSettingsAction;
    if (settingsAction === "close") {
      setWineSettingsOpen(false);
      return;
    }

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

  refs.wineSettingsPanel.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      setWineSettingsOpen(false);
    }
  });

  refs.settingsNav.addEventListener("click", () => {
    setWineSettingsOpen(!state.wineSettings.open);
  });

  refs.steamPanel.addEventListener("click", (event) => {
    const target = event.target as Element | null;
    const action = target?.closest<HTMLButtonElement>("[data-steam-action]")?.dataset.steamAction;

    if (action === "close") {
      setSteamPanelOpen(false);
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
    if (event.key === "Escape") {
      event.preventDefault();
      setSteamPanelOpen(false);
      return;
    }

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

    if (action === "close") {
      setSteamAccountPanelOpen(false);
    } else if (action === "connect") {
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

  refs.steamAccountPanel.addEventListener("keydown", (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      setSteamAccountPanelOpen(false);
    }
  });

  refs.steamAccountBackdrop.addEventListener("click", () => {
    setSteamAccountPanelOpen(false);
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

  refs.search.addEventListener("input", () => {
    state.query = refs.search.value;
    const matches = visibleGames();
    if (matches.length > 0 && !matches.some((game) => game.id === state.selectedId)) {
      state.selectedId = matches[0].id;
    }
    renderSelection();
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
    const target = event.target as HTMLElement | null;
    if (state.steamAccount.open) {
      if (event.key === "Escape") {
        event.preventDefault();
        setSteamAccountPanelOpen(false);
        return;
      }

      if (event.key === "Tab") {
        const focusable = Array.from(
          refs.steamAccountPanel.querySelectorAll<HTMLElement>(
            "button:not(:disabled), input:not(:disabled), a[href]",
          ),
        ).filter((element) => element.getClientRects().length > 0);
        event.preventDefault();
        if (focusable.length === 0) {
          refs.steamAccountPanel.focus();
          return;
        }
        const current = focusable.indexOf(document.activeElement as HTMLElement);
        const next = event.shiftKey
          ? current <= 0 ? focusable.length - 1 : current - 1
          : current >= focusable.length - 1 ? 0 : current + 1;
        focusable[next]?.focus();
        return;
      }

      if (!refs.steamAccountPanel.contains(target)) {
        event.preventDefault();
        focusSteamAccountPanel();
      }
      return;
    }

    if (state.wine.open && event.key === "Escape") {
      event.preventDefault();
      setWineSetupOpen(false);
      return;
    }
    if (state.wineSettings.open && event.key === "Escape") {
      event.preventDefault();
      setWineSettingsOpen(false);
      return;
    }
    if (refs.steamPanel.contains(target)) {
      return;
    }
    if (refs.steamAccountPanel.contains(target)) {
      return;
    }
    if (refs.winePanel.contains(target) || refs.wineSettingsPanel.contains(target)) {
      return;
    }
    if (refs.libraryMenu.contains(target)) {
      return;
    }

    if (state.steam.open && event.key === "Escape") {
      event.preventDefault();
      setSteamPanelOpen(false);
      return;
    }

    if (state.libraryMenuOpen && event.key === "Escape") {
      event.preventDefault();
      setLibraryMenuOpen(false, undefined, true);
      return;
    }

    const typing = target?.tagName === "INPUT" || target?.tagName === "TEXTAREA";
    const commandSearch = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k";

    if (commandSearch || (!typing && event.key === "/")) {
      event.preventDefault();
      closeLibraryMenu();
      refs.search.focus();
      refs.search.select();
      return;
    }

    if (typing) {
      if (event.key === "Escape") {
        refs.search.blur();
      }
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
        event.preventDefault();
        void launchGame();
        break;
      case "i":
      case "I":
        event.preventDefault();
        if (event.shiftKey) {
          void importGame();
        } else {
          setSteamPanelOpen(true, target ?? undefined);
        }
        break;
      case "Escape":
        closeLibraryMenu();
        break;
      default:
        break;
    }
  });

  renderSelection(true);
  renderSteamPanel();
  renderWineSetupPanel();
  renderWineSettingsPanel();
  void refreshLibrary();
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

function platformName(platform: LibraryGame["hostPlatform"]): string {
  switch (platform) {
    case "windows":
      return "Windows";
    case "macos":
      return "macOS";
    case "linux":
      return "Linux";
    default:
      return "";
  }
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
    label: readString(record, "directoryLabel", "directory_label", "label", "name") || "Dossier de jeux",
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
    displayName: readString(record, "displayName", "display_name", "name") || fallback?.displayName || "Profil Wine",
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
      "Wine 3D · mode de compatibilité par défaut",
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
    throw new Error("Wine-Staging a renvoyé un résultat d’import invalide.");
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
    parts.push(result.importedIds.length === 1 ? "1 jeu Wine importé" : result.importedIds.length + " jeux Wine importés");
  }
  if (result.updatedIds.length > 0) {
    parts.push(result.updatedIds.length === 1 ? "1 jeu Wine mis à jour" : result.updatedIds.length + " jeux Wine mis à jour");
  }
  if (result.skippedRefs.length > 0) {
    parts.push(result.skippedRefs.length === 1 ? "1 jeu ignoré" : result.skippedRefs.length + " jeux ignorés");
  }
  return parts.length > 0 ? parts.join(" · ") + "." : "Aucun jeu Wine n’avait besoin d’être importé.";
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
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function shell(): string {
  return `
    <main class="selector" aria-label="Orivo game selector">
      <div id="selector-content">
      <div class="hero-media" aria-hidden="true">
        <img id="hero-a" class="hero-image" alt="" />
        <img id="hero-b" class="hero-image" alt="" />
      </div>
      <div class="scene-overlay scene-overlay--left" aria-hidden="true"></div>
      <div class="scene-overlay scene-overlay--bottom" aria-hidden="true"></div>
      <div class="scene-overlay scene-overlay--top" aria-hidden="true"></div>

      <header class="topbar" aria-label="Primary navigation" data-tauri-drag-region>
        <div class="nav-cluster">
          <div class="library-menu-control">
            <button id="library-menu-button" class="brand-mark-button" type="button" aria-label="Ouvrir les sources de bibliothèque" aria-haspopup="menu" aria-expanded="false" aria-controls="library-source-menu">
              <img class="brand-mark" src="/media/orivo-ring-icon.png" alt="" />
            </button>
            <div id="library-source-menu" class="library-source-menu" role="menu" aria-label="Sources de bibliothèque" hidden>
              <button type="button" class="library-source-action" role="menuitem" data-library-action="steam-local">
                <span class="library-source-action__icon" aria-hidden="true">${icon("download")}</span>
                <span class="library-source-action__copy"><strong>Importer les jeux installés</strong><small>Depuis Steam</small></span>
              </button>
              <button type="button" class="library-source-action library-source-action--connect" role="menuitem" data-library-action="steam-account">
                <span class="library-source-action__icon library-source-action__icon--library" aria-hidden="true">${icon("library")}</span>
                <span class="library-source-action__copy"><strong>Se connecter à une bibliothèque</strong><small>Votre compte Steam</small></span>
                ${icon("chevron-right", "library-source-action__chevron")}
              </button>
              <button type="button" class="library-source-action" role="menuitem" data-library-action="local">
                <span class="library-source-action__icon" aria-hidden="true">${icon("folder")}</span>
                <span class="library-source-action__copy"><strong>Importer un jeu local</strong><small>Depuis ce Mac</small></span>
              </button>
              <button type="button" class="library-source-action" role="menuitem" data-library-action="wine">
                <span class="library-source-action__icon" aria-hidden="true">${icon("monitor")}</span>
                <span class="library-source-action__copy"><strong>Ajouter Wine-Staging</strong><small>Créer un profil Wine isolé</small></span>
              </button>
            </div>
          </div>
          <span class="top-divider" aria-hidden="true"></span>
          <nav class="primary-nav" aria-label="Orivo navigation">
            <button type="button" class="nav-link is-active" aria-current="page">${icon("library")}<span>Library</span></button>
            <button type="button" class="nav-link">${icon("store")}<span>Store</span></button>
            <button id="settings-nav-button" type="button" class="nav-link">${icon("settings")}<span>Settings</span></button>
          </nav>
        </div>

        <label class="search-control">
          ${icon("search")}
          <input id="library-search" type="search" autocomplete="off" spellcheck="false" placeholder="Search games…" aria-label="Search games" />
          <span class="search-shortcut" aria-hidden="true"><kbd>⌘</kbd><kbd>K</kbd></span>
        </label>

        <div class="profile-cluster">
          <button type="button" class="quiet-icon" aria-label="Notifications">${icon("bell")}</button>
          <span class="top-divider top-divider--right" aria-hidden="true"></span>
          <img class="avatar" src="/media/steam-avatar.png" alt="Steam profile" />
        </div>
      </header>

      <button id="previous-game" class="scene-arrow scene-arrow--previous" type="button" aria-label="Previous game">${icon("chevron-left")}</button>
      <button id="next-game" class="scene-arrow scene-arrow--next" type="button" aria-label="Next game">${icon("chevron-right")}</button>

      <section class="hero-content" aria-live="polite">
        <span id="hero-genre" class="genre-chip">RPG</span>
        <h1 id="hero-title">Elden Ring</h1>
        <p id="hero-description" class="hero-description"></p>
        <div class="hero-meta" aria-label="Game metadata">
          <span>${icon("clock")}<span id="hero-play-time"></span></span>
          <span>${icon("clock")}<span id="hero-last-played"></span></span>
          <span id="hero-source" class="hero-source">
            <span id="hero-source-icon" class="hero-source-icon" aria-hidden="true"></span>
            <span id="hero-source-label">Steam</span>
            <i class="hero-source-divider" aria-hidden="true">·</i>
            <span id="hero-metadata"></span>
          </span>
          <span id="hero-platform" class="hero-platform" hidden>
            ${icon("monitor")}<span id="hero-platform-label"></span>
          </span>
        </div>
        <div class="hero-actions">
          <button id="play-button" class="play-button" type="button">${icon("play")}<span>Play</span></button>
          <button class="round-button" type="button" aria-label="Bookmark game">${icon("bookmark")}</button>
        </div>
        <div id="launch-feedback" class="launch-feedback" role="status" aria-live="polite" hidden></div>
      </section>

      <section class="recently-played" aria-labelledby="recently-played-title">
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
          <span><b class="gamepad-a">A</b><em>Select</em></span>
          <span><b class="gamepad-b">B</b><em>Back</em></span>
        </span>
      </footer>

      <aside id="steam-import-panel" class="steam-import-panel" role="complementary" aria-labelledby="steam-import-title" aria-describedby="steam-import-detail" hidden>
        <header class="steam-panel-header">
          <div class="steam-panel-title">
            <span class="steam-source-mark" aria-hidden="true">${icon("download")}</span>
            <span>
              <strong id="steam-import-title">Import from Steam</strong>
              <small id="steam-import-detail">A local Steam source</small>
            </span>
          </div>
          <div class="steam-panel-actions">
            <button id="steam-refresh" type="button" class="steam-header-button" data-steam-action="refresh" aria-label="Refresh Steam library" hidden>${icon("refresh")}</button>
            <button type="button" class="steam-header-button" data-steam-action="close" aria-label="Close Steam import">${icon("close")}</button>
          </div>
        </header>
        <div id="steam-import-body" class="steam-import-body"></div>
        <footer id="steam-import-footer" class="steam-import-footer">
          <p id="steam-selection-summary"></p>
          <button id="steam-import-selected" class="steam-import-button" type="button" data-steam-action="import">Import selected</button>
        </footer>
      </aside>

      <aside id="wine-setup-panel" class="wine-panel" role="complementary" aria-labelledby="wine-setup-title" aria-describedby="wine-setup-detail" hidden>
        <header class="steam-panel-header">
          <div class="steam-panel-title">
            <span class="steam-source-mark wine-source-mark" aria-hidden="true">${icon("monitor")}</span>
            <span>
              <strong id="wine-setup-title">Ajouter Wine-Staging</strong>
              <small id="wine-setup-detail">Un profil Wine isolé pour Orivo</small>
            </span>
          </div>
          <div class="steam-panel-actions">
            <button type="button" class="steam-header-button" data-wine-action="close" aria-label="Fermer Wine-Staging">${icon("close")}</button>
          </div>
        </header>
        <div id="wine-setup-body" class="wine-panel-body"></div>
      </aside>

      <aside id="wine-settings-panel" class="wine-settings-panel" role="complementary" aria-labelledby="wine-settings-title" hidden>
        <header class="steam-panel-header">
          <div class="steam-panel-title">
            <span class="steam-source-mark wine-source-mark" aria-hidden="true">${icon("settings")}</span>
            <span>
              <strong id="wine-settings-title">Plugins et runners</strong>
              <small>Wine-Staging sur ce Mac</small>
            </span>
          </div>
          <div class="steam-panel-actions">
            <button type="button" class="steam-header-button" data-wine-settings-action="close" aria-label="Fermer les réglages Wine">${icon("close")}</button>
          </div>
        </header>
        <div id="wine-settings-body" class="wine-settings-body"></div>
      </aside>

      <p id="toast" class="toast" role="status" aria-live="polite"></p>
      </div>

      <div id="steam-account-backdrop" class="steam-account-backdrop" aria-hidden="true" hidden></div>
      <aside id="steam-account-panel" class="steam-account-panel" role="dialog" aria-modal="true" aria-labelledby="steam-account-title" tabindex="-1" hidden>
        <header class="steam-panel-header">
          <div class="steam-panel-title">
            <span class="steam-source-mark" aria-hidden="true">${icon("library")}</span>
            <span>
              <strong id="steam-account-title">Steam library</strong>
              <small>Private, local-first connection</small>
            </span>
          </div>
          <div class="steam-panel-actions">
            <button type="button" class="steam-header-button" data-steam-account-action="close" aria-label="Close Steam connection">${icon("close")}</button>
          </div>
        </header>
        <div id="steam-account-body" class="steam-account-body"></div>
      </aside>
    </main>
  `;
}
