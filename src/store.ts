import "./store.css";
import {
  STORE_GAMES,
  STORE_INFO_ROWS,
  STORE_PLATFORMS,
  STORE_TABS,
  type StoreGame,
  type StoreIconName,
} from "./store-data";

// Self-contained icon set for the Store page so the shared library icon set
// (src/icons.ts) stays untouched. Stroke glyphs share the app's 1.75 weight;
// platform marks are filled for legibility at small sizes.
const STROKE_ICONS: Partial<Record<StoreIconName, string>> = {
  brain:
    '<path d="M9.4 4.6A2.6 2.6 0 0 0 6.9 6.7 2.4 2.4 0 0 0 4.7 10 2.5 2.5 0 0 0 4.2 12a2.5 2.5 0 0 0 .8 4 2.3 2.3 0 0 0 4.4 1.5"/><path d="M14.6 4.6A2.6 2.6 0 0 1 17.1 6.7 2.4 2.4 0 0 1 19.3 10 2.5 2.5 0 0 1 19.8 12a2.5 2.5 0 0 1-.8 4 2.3 2.3 0 0 1-4.4 1.5"/><path d="M12 5v13.6"/>',
  leaf: '<path d="M4.5 19.5s-.4-7.2 5-11.2C13 5.6 19.5 4.5 19.5 4.5s-.4 6.9-4.2 10.6c-4.3 4.2-10.8 4.4-10.8 4.4Z"/><path d="M4.7 19.3C8.5 14 12 11 16.5 8.8"/>',
  clock: '<circle cx="12" cy="12" r="8.6"/><path d="M12 7.2v5l3.4 2"/>',
  book: '<path d="M12 6.4C10.4 5 7.9 4.5 4 5.1v12.6c3.9-.6 6.4-.1 8 1.3 1.6-1.4 4.1-1.9 8-1.3V5.1c-3.9-.6-6.4-.1-8 1.3Z"/><path d="M12 6.4v13"/>',
  heart:
    '<path fill="currentColor" stroke="none" d="M12 20.3S3.6 15.4 3.6 9.6a4.3 4.3 0 0 1 8-2.2 4.3 4.3 0 0 1 8 2.2c0 5.8-8.4 10.7-8.4 10.7Z"/>',
  barchart:
    '<path d="M4 20h16"/><rect x="5" y="12" width="3.2" height="6" rx="1"/><rect x="10.4" y="7.5" width="3.2" height="10.5" rx="1"/><rect x="15.8" y="14.5" width="3.2" height="3.5" rx="1"/>',
  "arrow-right": '<path d="M5 12h13.5M12.5 6l6 6-6 6"/>',
  "chevron-down": '<path d="m6 9.5 6 6 6-6"/>',
  xbox:
    '<circle cx="12" cy="12" r="9"/><path d="M7 5.6c2.9 2.3 7.2 8.4 9.6 12.9M17 5.6c-2.9 2.3-7.2 8.4-9.6 12.9"/>',
  switch:
    '<rect x="4.2" y="3.8" width="6.4" height="16.4" rx="3.2"/><rect x="13.4" y="3.8" width="6.4" height="16.4" rx="3.2"/><circle cx="7.4" cy="8" r="1.1"/><circle cx="16.6" cy="16" r="1.1"/>',
  emulator:
    '<rect x="3" y="6.5" width="18" height="11" rx="2.6"/><path d="M7.3 12H10M8.6 10.6v2.8"/><circle cx="15.4" cy="11.4" r="1"/><circle cx="17.6" cy="13.4" r="1"/>',
};

const FILLED_ICONS: Partial<Record<StoreIconName, string>> = {
  windows:
    '<path fill="currentColor" stroke="none" d="M3 5.7 10.6 4.6v6.7H3zM11.7 4.4 21 3v8.3h-9.3zM3 12.7h7.6v6.6L3 18.2zM11.7 12.7H21V21l-9.3-1.3z"/>',
  playstation:
    '<path fill="currentColor" stroke="none" d="M9.1 4.6 6.6 3.8v14.3l2.5.8V6.9c0-.5.3-.7.7-.6.5.2.7.6.7 1.2v4c1.9.8 3.4.3 3.4-2 0-2.3-1.3-3.4-4.8-4.9-.1 0 0 0 0 0Z"/><path fill="currentColor" stroke="none" d="M13.9 16.2c1.7.6 3.6.6 5.1-.1v-1.9c-1.3.7-3.1.6-4.4.1-.2-.1-.5-.2-.7-.3v2.1c0 .1 0 .1 0 .1Z"/><path fill="currentColor" stroke="none" d="M4.6 15.4c-1.2.6-1.3 1.6.2 2.2l3.3 1.2v-1.6l-2.2-.8c-.4-.1-.4-.4 0-.5v-1.5c-.5.2-1 .4-1.3.6Z"/>',
};

function sicon(name: StoreIconName, className = ""): string {
  if (FILLED_ICONS[name]) {
    return `<svg class="s-icon ${className}" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">${FILLED_ICONS[name]}</svg>`;
  }
  return `<svg class="s-icon ${className}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${STROKE_ICONS[name] ?? ""}</svg>`;
}

const ratingDots = (value: number): string => {
  let dots = "";
  for (let i = 0; i < 5; i += 1) {
    dots += `<i class="s-dot${i < value ? " is-on" : ""}"></i>`;
  }
  return `<span class="s-rating-dots" aria-label="${value} sur 5">${dots}</span>`;
};

const gameCard = (game: StoreGame): string => {
  const fav = game.favorite
    ? `<button type="button" class="s-card-fav is-on" aria-label="Retirer ${game.title} des favoris">${sicon("heart")}</button>`
    : "";
  const overlayTitle = game.titleInArt
    ? ""
    : `<span class="s-card-title">${game.title.toUpperCase()}</span>`;
  const ratings = game.ratings
    .map(
      (rating) =>
        `<div class="s-rating"><span class="s-rating-label">${rating.label}</span>${ratingDots(rating.value)}</div>`,
    )
    .join("");
  return `
    <article class="s-card${game.selected ? " is-selected" : ""}" data-store-game="${game.id}" tabindex="0" aria-label="${game.title}">
      <div class="s-card-art" style="background-image:url('${game.art}')">
        ${fav}
        <span class="s-card-art-fade" aria-hidden="true"></span>
        ${overlayTitle}
      </div>
      <div class="s-card-body">
        <span class="s-card-genre">${game.genre}</span>
        <div class="s-card-chips">
          <span class="s-chip">${game.duration}</span>
          <span class="s-chip">${game.mode}</span>
        </div>
        <div class="s-card-ratings">${ratings}</div>
        <p class="s-card-desc">${game.description}</p>
      </div>
    </article>`;
};

export function renderStore(): string {
  const infoRows = STORE_INFO_ROWS.map(
    (row) => `
      <div class="s-info-row">
        <span class="s-info-ico">${sicon(row.icon)}</span>
        <span class="s-info-copy">
          <strong>${row.title}</strong>
          <small>${row.description}</small>
        </span>
      </div>`,
  ).join("");

  const cards = STORE_GAMES.map(gameCard).join("");

  const tabs = STORE_TABS.map(
    (tab, index) =>
      `<button type="button" class="s-tab${index === 0 ? " is-active" : ""}" data-store-tab="${index}">${tab}</button>`,
  ).join("");

  const platforms = STORE_PLATFORMS.map((platform) => {
    const inner = platform.trailing
      ? `<span>${platform.label}</span>${sicon(platform.icon, "s-plat-chevron")}`
      : `${sicon(platform.icon)}<span>${platform.label}</span>`;
    return `<button type="button" class="s-plat${platform.active ? " is-active" : ""}" data-store-platform="${platform.id}">${inner}</button>`;
  }).join("");

  // Content only: the Store shares the library's top navigation bar. It renders
  // beneath that bar inside the selector shell.
  return `
  <div class="store-content" data-store-root>
    <div class="s-scene" aria-hidden="true">
      <div class="s-scene-img"></div>
      <div class="s-scene-fade s-scene-fade--bottom"></div>
      <div class="s-scene-fade s-scene-fade--left"></div>
    </div>

    <section class="s-hero">
      <div class="s-hero-text">
        <span class="s-eyebrow">Recommandé pour vous</span>
        <h1 class="s-hero-title">Des expériences<br />qui comptent.</h1>
        <p class="s-hero-desc">Des jeux choisis pour nourrir ton esprit, respecter ton temps et t'offrir des moments vrais.</p>
        <button type="button" class="s-hero-cta">Découvrir pourquoi</button>
      </div>

      <aside class="s-info" aria-label="Pourquoi ces jeux">${infoRows}</aside>

      <p class="s-tagline">${sicon("leaf", "s-tagline-leaf")}<span><b>Moins de bruit.</b> Plus de sens.</span></p>
    </section>

    <section class="s-rail" aria-label="Recommandations">
      <div class="s-cards">${cards}</div>
      <button type="button" class="s-rail-next" aria-label="Voir plus de jeux">${sicon("arrow-right")}</button>
    </section>

    <div class="s-filters">
      <div class="s-tabs" role="tablist" aria-label="Filtres d'humeur">${tabs}</div>
      <div class="s-platforms" aria-label="Plateformes">${platforms}</div>
    </div>

    <div class="s-banner">
      <span class="s-banner-leaf">${sicon("leaf")}</span>
      <p class="s-banner-copy"><b>Rappelle-toi :</b> chaque heure de jeu peut t'apporter quelque chose.<br />Choisis la qualité, pas la quantité.</p>
      <button type="button" class="s-habits">${sicon("barchart")}<span>Voir mes habitudes</span></button>
    </div>
  </div>`;
}

export function mountStore(container: HTMLElement): void {
  container.innerHTML = renderStore();

  const activateWithin = (group: HTMLElement, selector: string, target: Element | null): void => {
    if (!target) {
      return;
    }
    group.querySelectorAll(selector).forEach((node) => {
      node.classList.toggle("is-active", node === target);
    });
  };

  // Mood filter tabs.
  const tabs = container.querySelector<HTMLElement>(".s-tabs");
  tabs?.addEventListener("click", (event) => {
    const tab = (event.target as HTMLElement).closest<HTMLButtonElement>(".s-tab");
    activateWithin(tabs, ".s-tab", tab);
  });

  // Platform filters. The trailing "Plus" control is a menu affordance and
  // never becomes the persistent selection.
  const platforms = container.querySelector<HTMLElement>(".s-platforms");
  platforms?.addEventListener("click", (event) => {
    const platform = (event.target as HTMLElement).closest<HTMLButtonElement>(".s-plat");
    if (!platform || platform.dataset.storePlatform === "plus") {
      return;
    }
    activateWithin(platforms, ".s-plat", platform);
  });

  // Card selection mirrors the mock's highlighted "featured" card.
  const cardRail = container.querySelector<HTMLElement>(".s-cards");
  cardRail?.addEventListener("click", (event) => {
    const fav = (event.target as HTMLElement).closest<HTMLButtonElement>(".s-card-fav");
    if (fav) {
      event.stopPropagation();
      fav.classList.toggle("is-on");
      fav.setAttribute(
        "aria-label",
        (fav.classList.contains("is-on") ? "Retirer" : "Ajouter") + " des favoris",
      );
      return;
    }
    const card = (event.target as HTMLElement).closest<HTMLElement>(".s-card");
    if (!card) {
      return;
    }
    cardRail.querySelectorAll(".s-card").forEach((node) => {
      node.classList.toggle("is-selected", node === card);
    });
  });
}
