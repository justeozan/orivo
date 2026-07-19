import { invoke } from "@tauri-apps/api/core";
import { icon } from "./icons";
import { isTauriRuntime, resolveMediaUrl } from "./media";
import { fallbackLibrary, formatPlayTime, type LibraryGame } from "./mock-library";

type BackendRecord = Record<string, unknown>;

interface State {
  games: LibraryGame[];
  selectedId: string;
  query: string;
  menuOpen: boolean;
}

const lastUsedFallback = fallbackLibrary[0];

export function mountApp(root: HTMLElement): void {
  const state: State = {
    games: fallbackLibrary.map((game) => ({ ...game })),
    selectedId: fallbackLibrary[0].id,
    query: "",
    menuOpen: false,
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
    heroLayers: [get<HTMLImageElement>("#hero-a"), get<HTMLImageElement>("#hero-b")],
    genre: get<HTMLElement>("#hero-genre"),
    title: get<HTMLElement>("#hero-title"),
    description: get<HTMLElement>("#hero-description"),
    playTime: get<HTMLElement>("#hero-play-time"),
    lastPlayed: get<HTMLElement>("#hero-last-played"),
    metadata: get<HTMLElement>("#hero-metadata"),
    cards: get<HTMLElement>("#game-cards"),
    search: get<HTMLInputElement>("#library-search"),
    menu: get<HTMLElement>("#overflow-menu"),
    toast: get<HTMLElement>("#toast"),
  };

  let activeHero = 0;
  let heroRequest = 0;
  let toastTimer: number | undefined;

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

  const updateHeroImage = (game: LibraryGame, immediate = false): void => {
    const source = game.heroUrl || game.coverUrl;
    const current = refs.heroLayers[activeHero];

    if (current.getAttribute("src") === source) {
      return;
    }

    if (immediate) {
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

    next.onload = reveal;
    next.onerror = () => {
      next.src = lastUsedFallback.heroUrl;
      reveal();
    };
    next.src = source;

    if (next.complete) {
      reveal();
    }
  };

  const renderCards = (): void => {
    const games = visibleGames();

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

    card.dataset.gameId = game.id;
    card.classList.toggle("is-selected", selected);
    card.setAttribute("aria-pressed", String(selected));
    card.setAttribute("aria-label", `Select ${game.title}`);

    for (const [image, source] of [
      [portrait, portraitSource],
      [landscape, landscapeSource],
    ] as const) {
      if (image && image.getAttribute("src") !== source) {
        image.src = source;
        image.loading = index < 7 ? "eager" : "lazy";
      }
    }
    if (title) {
      title.textContent = game.title;
    }
    if (time) {
      time.textContent = formatPlayTime(game.playTimeSeconds);
    }
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
    refs.metadata.textContent = game.metadata || "Ready to play";
    updateHeroImage(game, immediateHero);
    renderCards();
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

  const setMenuOpen = (open: boolean): void => {
    state.menuOpen = open;
    refs.menu.hidden = !open;
    root.querySelector<HTMLButtonElement>("#more-button")?.setAttribute("aria-expanded", String(open));
  };

  const refreshLibrary = async (importedId?: string): Promise<void> => {
    const games = await loadLibrary();
    state.games = games;
    state.selectedId = games.some((game) => game.id === importedId)
      ? importedId!
      : games.some((game) => game.id === state.selectedId)
        ? state.selectedId
        : games[0]?.id ?? lastUsedFallback.id;
    renderSelection();
  };

  const importGame = async (): Promise<void> => {
    setMenuOpen(false);
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

  const launchGame = async (): Promise<void> => {
    const game = selectedGame();
    if (!isTauriRuntime()) {
      showToast("Launch is available in the Orivo desktop app.");
      return;
    }

    try {
      showToast(`Launching ${game.title}…`);
      if (!game.launchable) {
        showToast("Visual showcase — import a local game to play.");
        return;
      }
      await invoke("launch_game", { gameId: game.id });
    } catch (error) {
      showToast(messageFromError(error, `Could not launch ${game.title}.`));
    }
  };

  root.querySelector<HTMLButtonElement>("#previous-game")?.addEventListener("click", () => moveSelection(-1));
  root.querySelector<HTMLButtonElement>("#next-game")?.addEventListener("click", () => moveSelection(1));
  root.querySelector<HTMLButtonElement>("#play-button")?.addEventListener("click", () => void launchGame());
  root.querySelector<HTMLButtonElement>("#more-button")?.addEventListener("click", () => setMenuOpen(!state.menuOpen));
  root.querySelector<HTMLButtonElement>("#import-game")?.addEventListener("click", () => void importGame());

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
    if (state.menuOpen && !refs.menu.contains(target) && !root.querySelector("#more-button")?.contains(target)) {
      setMenuOpen(false);
    }
  });

  window.addEventListener("keydown", (event) => {
    const target = event.target as HTMLElement | null;
    const typing = target?.tagName === "INPUT" || target?.tagName === "TEXTAREA";
    const commandSearch = (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k";

    if (commandSearch || (!typing && event.key === "/")) {
      event.preventDefault();
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
        void importGame();
        break;
      case "Escape":
        setMenuOpen(false);
        break;
      default:
        break;
    }
  });

  renderSelection(true);
  void refreshLibrary();
}

async function loadLibrary(): Promise<LibraryGame[]> {
  if (!isTauriRuntime()) {
    return fallbackLibrary.map((game) => ({ ...game }));
  }

  try {
    const result = await invoke<unknown>("get_library");
    const records = recordsFromResult(result);
    if (records.length === 0) {
      return fallbackLibrary.map((game) => ({ ...game }));
    }

    const games = await Promise.all(records.map(normaliseGame));
    return games.filter((game): game is LibraryGame => game !== null);
  } catch {
    return fallbackLibrary.map((game) => ({ ...game }));
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

async function normaliseGame(record: BackendRecord): Promise<LibraryGame | null> {
  const id = readString(record, "id");
  const title = readString(record, "title");
  if (!id || !title) {
    return null;
  }

  const fallback = fallbackLibrary.find((game) => game.title === title) ?? lastUsedFallback;
  const [heroUrl, coverUrl, landscapeUrl] = await Promise.all([
    resolveMediaUrl(readString(record, "heroUrl", "hero_url")),
    resolveMediaUrl(readString(record, "coverUrl", "cover_url")),
    resolveMediaUrl(readString(record, "landscapeUrl", "landscape_url")),
  ]);

  return {
    id,
    title,
    description: readString(record, "description") || fallback.description,
    metadata: readString(record, "metadata") || fallback.metadata,
    genre: readString(record, "genre") || fallback.genre,
    heroUrl: heroUrl || fallback.heroUrl,
    coverUrl: coverUrl || heroUrl || fallback.coverUrl,
    landscapeUrl: landscapeUrl || heroUrl || fallback.landscapeUrl,
    lastPlayedAt: readString(record, "lastPlayedAt", "last_played_at") || fallback.lastPlayedAt,
    playTimeSeconds: readNumber(record, "playTimeSeconds", "play_time_seconds") ?? fallback.playTimeSeconds,
    launchable: readBoolean(record, "launchable") ?? fallback.launchable,
  };
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
      <div class="hero-media" aria-hidden="true">
        <img id="hero-a" class="hero-image" alt="" />
        <img id="hero-b" class="hero-image" alt="" />
      </div>
      <div class="scene-overlay scene-overlay--left" aria-hidden="true"></div>
      <div class="scene-overlay scene-overlay--bottom" aria-hidden="true"></div>
      <div class="scene-overlay scene-overlay--top" aria-hidden="true"></div>

      <header class="topbar" aria-label="Primary navigation" data-tauri-drag-region>
        <div class="nav-cluster">
          <img class="brand-mark" src="/media/orivo-ring-icon.png" alt="Orivo" />
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
          <span>${icon("trophy")}<span id="hero-metadata"></span></span>
        </div>
        <div class="hero-actions">
          <button id="play-button" class="play-button" type="button">${icon("play")}<span>Play</span></button>
          <button class="round-button" type="button" aria-label="Bookmark game">${icon("bookmark")}</button>
          <div class="more-control">
            <button id="more-button" class="round-button" type="button" aria-label="More game actions" aria-haspopup="menu" aria-expanded="false">${icon("more")}</button>
            <div id="overflow-menu" class="overflow-menu" role="menu" hidden>
              <button id="import-game" type="button" role="menuitem">Import a local game</button>
            </div>
          </div>
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

      <p id="toast" class="toast" role="status" aria-live="polite"></p>
    </main>
  `;
}
