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

interface SteamAccountConnectedEvent {
  steamId: string;
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
    const sourceName = game.source === "steam" ? "Steam" : game.source === "local" ? "This Mac" : "";
    refs.source.hidden = !sourceName;
    refs.sourceIcon.innerHTML = game.source === "steam" ? icon("steam") : icon("folder");
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
      playLabel.textContent = game.launchable ? "Play" : isSteamInstallable ? "Install" : "Unavailable";
    }
    updateHeroImage(game, immediateHero);
    renderCards();
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

  const launchGame = async (): Promise<void> => {
    const game = selectedGame();
    if (!isTauriRuntime()) {
      showToast("Launch is available in the Orivo desktop app.");
      return;
    }

    try {
      if (!game.launchable) {
        if (game.source === "steam") {
          showToast(`Opening Steam to install ${game.title}…`);
          await invoke("install_steam_game", { gameId: game.id });
        } else {
          showToast("Visual showcase — import a local game to play.");
        }
        return;
      }
      showToast(`Launching ${game.title}…`);
      await invoke("launch_game", { gameId: game.id });
    } catch (error) {
      showToast(messageFromError(error, `Could not launch ${game.title}.`));
    }
  };

  root.querySelector<HTMLButtonElement>("#previous-game")?.addEventListener("click", () => moveSelection(-1));
  root.querySelector<HTMLButtonElement>("#next-game")?.addEventListener("click", () => moveSelection(1));
  root.querySelector<HTMLButtonElement>("#play-button")?.addEventListener("click", () => void launchGame());

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

    if (refs.steamPanel.contains(target)) {
      return;
    }
    if (refs.steamAccountPanel.contains(target)) {
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
            </div>
          </div>
          <span class="top-divider" aria-hidden="true"></span>
          <nav class="primary-nav" aria-label="Orivo navigation">
            <button type="button" class="nav-link is-active" aria-current="page">${icon("library")}<span>Library</span></button>
            <button type="button" class="nav-link">${icon("store")}<span>Store</span></button>
            <button type="button" class="nav-link">${icon("settings")}<span>Settings</span></button>
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
