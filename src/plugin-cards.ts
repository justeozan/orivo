/**
 * The cards of Settings › Plugins: one per plugin, installed or offered, with
 * the coloured icon, the category pill and the corner mark. Plain DOM, no
 * state: app.ts decides what each card says and which one is open.
 */
import { icon, type IconName } from "./icons";
import { t } from "./i18n";

/** The shelf a plugin sits on, shown as the pill beside its name. */
export type PluginCategory =
  | "emulation"
  | "streaming"
  | "compatibility"
  | "visuals"
  | "library"
  | "music"
  | "saves"
  | "tools";

const PLUGIN_CATEGORIES: readonly PluginCategory[] = [
  "emulation",
  "streaming",
  "compatibility",
  "visuals",
  "library",
  "music",
  "saves",
  "tools",
];

/**
 * Plugins ship a name and a sentence-long summary but no category, so it is
 * read from what the plugin is, falling back on the kind of extension it
 * provides.
 */
/** The registry's own category wins; an older index has none and falls back. */
export function pluginCategory(
  id: string,
  name: string,
  extensions: readonly string[] = [],
  declared?: string | null,
): PluginCategory {
  if (declared && (PLUGIN_CATEGORIES as readonly string[]).includes(declared)) {
    return declared as PluginCategory;
  }
  const key = `${id} ${name}`.toLocaleLowerCase();
  if (/stream|moonlight|sunshine/.test(key)) return "streaming";
  if (/ryujinx|switch|ppsspp|psp|retroarch|emul|dolphin|pcsx|duckstation/.test(key)) return "emulation";
  if (/winlator|wine/.test(key)) return "compatibility";
  if (/wallpaper|artwork|theme/.test(key)) return "visuals";
  if (/spotify|music/.test(key)) return "music";
  if (/ludusavi|save/.test(key)) return "saves";
  if (/playnite/.test(key)) return "library";
  if (/quiky|install/.test(key)) return "tools";
  const kinds = extensions.join(" ").toLocaleLowerCase();
  if (kinds.includes("runner")) return "emulation";
  if (kinds.includes("source") || kinds.includes("library")) return "library";
  return "tools";
}

/**
 * The icon a plugin card opens with. Plugins ship no icon of their own, so
 * the glyph is chosen from what the plugin is and the colour from its shelf.
 */
/**
 * The plugins that have a real logo of their own, matched on id or name. The
 * rest get a glyph on their shelf's colour.
 */
const PLUGIN_LOGOS: ReadonlyArray<{ match: RegExp; src: string }> = [
  { match: /\bwine\b/, src: "/media/plugins/wine.png" },
  { match: /moonlight|gamestream/, src: "/media/plugins/moonlight.png" },
  { match: /quiky/, src: "/media/plugins/quiky.png" },
];

export function pluginIcon(id: string, name: string, category: PluginCategory): HTMLElement {
  const key = `${id} ${name}`.toLocaleLowerCase();
  const logo = PLUGIN_LOGOS.find((entry) => entry.match.test(key));
  if (logo) {
    const tile = document.createElement("span");
    tile.className = "plugin-icon plugin-icon--logo";
    tile.dataset.category = category;
    tile.setAttribute("aria-hidden", "true");
    const image = document.createElement("img");
    image.src = logo.src;
    image.alt = "";
    image.decoding = "async";
    image.draggable = false;
    tile.append(image);
    return tile;
  }
  const glyph: IconName =
    key.includes("winlator") || key.includes("android")
      ? "android"
      : (
          {
            emulation: "emulator",
            streaming: "cast",
            compatibility: "windows",
            visuals: "landscape",
            library: "library",
            music: "sparkle",
            saves: "cloud",
            tools: "puzzle",
          } satisfies Record<PluginCategory, IconName>
        )[category];
  const tile = document.createElement("span");
  tile.className = "plugin-icon";
  tile.dataset.category = category;
  tile.setAttribute("aria-hidden", "true");
  tile.innerHTML = icon(glyph);
  return tile;
}

export function pluginPill(category: PluginCategory): HTMLElement {
  const pill = document.createElement("span");
  pill.className = "plugin-pill";
  pill.dataset.category = category;
  pill.textContent = t(`app.pluginCategory.${category}`);
  return pill;
}

export interface PluginCardOptions {
  /** The plugin's id, or a built-in's view name. */
  id: string;
  name: string;
  category: PluginCategory;
  /**
   * How the plugin stands, in a word or two, coloured by its tone: the first
   * line under the name. Ready is green, something missing is amber.
   */
  status?: { text: string; tone: "ok" | "warn" | "muted" } | null;
  /** Short lines under the name (or under the state): a fact each. */
  lines: readonly string[];
  /** The view the whole card opens, with the label a screen reader hears. */
  open?: { view: string; label: string } | null;
  /** The small mark in the corner: something to know, or something wrong. */
  flag?: { tone: "info" | "warn"; label: string } | null;
  /** The card's own control (Install, a percentage), above the open target. */
  action?: HTMLElement | null;
  selected?: boolean;
  /** 0..100 while something downloads; draws a bar along the bottom edge. */
  progress?: number | null;
}

/**
 * One plugin, as a card. The whole card is one button (stretched under the
 * content) so it opens from anywhere, while a control of its own — Install —
 * sits above that button and keeps its own click.
 */
export function pluginCard(options: PluginCardOptions): HTMLElement {
  const card = document.createElement("article");
  card.className = "plugin-card";
  card.dataset.pluginCard = options.id;
  card.classList.toggle("is-selected", Boolean(options.selected));
  card.classList.toggle("is-openable", Boolean(options.open));

  if (options.open) {
    const open = document.createElement("button");
    open.type = "button";
    open.className = "plugin-card__open";
    open.dataset.pluginOpen = options.open.view;
    open.setAttribute("aria-label", options.open.label);
    if (options.selected) open.setAttribute("aria-current", "true");
    card.append(open);
  }

  const copy = document.createElement("div");
  copy.className = "plugin-card__copy";
  const title = document.createElement("div");
  title.className = "plugin-card__title";
  const name = document.createElement("strong");
  name.textContent = options.name;
  title.append(name, pluginPill(options.category));
  copy.append(title);
  if (options.status?.text) {
    const status = document.createElement("small");
    status.className = "plugin-card__status";
    status.dataset.tone = options.status.tone;
    status.textContent = options.status.text;
    copy.append(status);
  }
  for (const text of options.lines.filter(Boolean)) {
    const line = document.createElement("small");
    line.textContent = text;
    copy.append(line);
  }
  card.append(pluginIcon(options.id, options.name, options.category), copy);

  if (options.action) {
    options.action.classList.add("plugin-card__action");
    card.append(options.action);
  } else if (options.open) {
    const chevron = document.createElement("span");
    chevron.className = "plugin-card__chevron";
    chevron.setAttribute("aria-hidden", "true");
    chevron.innerHTML = icon("chevron-right");
    card.append(chevron);
  }

  if (options.flag) {
    const flag = document.createElement("span");
    flag.className = `plugin-card__flag plugin-card__flag--${options.flag.tone}`;
    flag.title = options.flag.label;
    flag.setAttribute("role", "img");
    flag.setAttribute("aria-label", options.flag.label);
    flag.innerHTML = icon(options.flag.tone === "warn" ? "alert" : "info");
    card.append(flag);
  }

  if (options.progress !== null && options.progress !== undefined) {
    const track = document.createElement("div");
    track.className = "plugin-progress";
    const bar = document.createElement("div");
    bar.className = "plugin-progress__bar";
    bar.style.width = `${options.progress}%`;
    track.append(bar);
    card.append(track);
  }
  return card;
}


/** Categories actually on offer, in the fixed shelf order. */
export function catalogCategories(
  entries: ReadonlyArray<{ id: string; name: string; category?: string | null }>,
): PluginCategory[] {
  const present = new Set(entries.map((entry) => pluginCategory(entry.id, entry.name, [], entry.category)));
  return PLUGIN_CATEGORIES.filter((category) => present.has(category));
}

/** Search term and category chip, both optional. */
export function matchesCatalog(
  entry: { id: string; name: string; summary: string; category?: string | null },
  term: string,
  category: PluginCategory | "",
): boolean {
  if (category && pluginCategory(entry.id, entry.name, [], entry.category) !== category) return false;
  return !term || `${entry.name} ${entry.summary}`.toLocaleLowerCase().includes(term);
}

/** One toggle per category, built from the shared pill. */
export function categoryFilterChips(
  categories: readonly PluginCategory[],
  active: PluginCategory | "",
): HTMLButtonElement[] {
  return categories.map((category) => {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "plugins-filter";
    chip.dataset.pluginFilter = category;
    chip.setAttribute("aria-pressed", String(active === category));
    chip.append(pluginPill(category));
    return chip;
  });
}

/** The count pill on the Plugins entry; `null` when nothing waits. */
export function updateBadge(count: number, title: string): HTMLElement | null {
  if (count <= 0) return null;
  const badge = document.createElement("span");
  badge.className = "plugin-pill plugins-update-badge";
  badge.dataset.category = "tools";
  badge.textContent = String(count);
  badge.title = title;
  return badge;
}

/* ---------------------------------------------------------------------------
   The ⋯ menu of a plugin's detail: the rare actions, out of the way.
   --------------------------------------------------------------------------- */

export type MoreMenuEntry =
  | {
      kind: "item";
      label: string;
      icon?: IconName;
      danger?: boolean;
      disabled?: boolean;
      /** Data attributes for the page's own click handler (`pluginUninstall: id`). */
      data?: Record<string, string>;
      run?: () => void;
    }
  | { kind: "separator" }
  /** A small grey line of facts at the bottom: version, source, updates. */
  | { kind: "meta"; text: string };

/** The menu open right now, closed when another one opens. */
let closeOpenMenu: (() => void) | null = null;

/**
 * A ⋯ button and the menu it opens, in the glass of Play's launch menu. The
 * menu closes on a choice, a click elsewhere or Escape. A view that repaints
 * while it is open passes `open` back in so it stays open.
 */
export function pluginMoreMenu(options: {
  label: string;
  entries: readonly MoreMenuEntry[];
  open?: boolean;
  onToggle?: (open: boolean) => void;
}): HTMLElement {
  const wrap = document.createElement("div");
  wrap.className = "plugin-more-menu";
  const button = document.createElement("button");
  button.type = "button";
  button.className = "plugin-dots";
  button.dataset.pluginMore = "";
  button.setAttribute("aria-label", options.label);
  button.title = options.label;
  button.setAttribute("aria-haspopup", "menu");
  button.innerHTML = icon("more");
  const menu = document.createElement("div");
  menu.className = "play-launch-menu plugin-menu";
  menu.setAttribute("role", "menu");
  menu.setAttribute("aria-label", options.label);

  let detach = (): void => {};
  const setOpen = (open: boolean, notify = true): void => {
    menu.hidden = !open;
    button.setAttribute("aria-expanded", String(open));
    detach();
    detach = () => {};
    if (open) {
      // Opened by a press, another open menu closes. Opened again by a repaint,
      // the copy it replaces is already on its way out and detaches itself.
      if (notify && closeOpenMenu && closeOpenMenu !== close) closeOpenMenu();
      closeOpenMenu = close;
      const onClick = (event: MouseEvent): void => {
        if (!wrap.isConnected) return detach();
        if (!wrap.contains(event.target as Node)) close();
      };
      const onKey = (event: KeyboardEvent): void => {
        if (!wrap.isConnected) return detach();
        if (event.key !== "Escape") return;
        // Closing the menu is what the key was for, not leaving the page too.
        event.preventDefault();
        event.stopPropagation();
        close();
        button.focus();
      };
      document.addEventListener("click", onClick, true);
      document.addEventListener("keydown", onKey, true);
      detach = () => {
        document.removeEventListener("click", onClick, true);
        document.removeEventListener("keydown", onKey, true);
      };
    } else if (closeOpenMenu === close) {
      closeOpenMenu = null;
    }
    if (notify) options.onToggle?.(open);
  };
  const close = (): void => setOpen(false);

  for (const entry of options.entries) {
    if (entry.kind === "separator") {
      const line = document.createElement("hr");
      line.className = "plugin-menu__separator";
      menu.append(line);
      continue;
    }
    if (entry.kind === "meta") {
      const meta = document.createElement("p");
      meta.className = "plugin-menu__meta";
      meta.textContent = entry.text;
      menu.append(meta);
      continue;
    }
    const item = document.createElement("button");
    item.type = "button";
    item.className = "play-launch-option plugin-menu__item";
    item.classList.toggle("plugin-menu__item--danger", Boolean(entry.danger));
    item.setAttribute("role", "menuitem");
    item.disabled = Boolean(entry.disabled);
    for (const [key, value] of Object.entries(entry.data ?? {})) item.dataset[key] = value;
    if (entry.icon) item.insertAdjacentHTML("beforeend", icon(entry.icon));
    const label = document.createElement("span");
    label.textContent = entry.label;
    item.append(label);
    item.addEventListener("click", () => {
      close();
      entry.run?.();
    });
    menu.append(item);
  }

  button.addEventListener("click", () => setOpen(Boolean(menu.hidden)));
  setOpen(Boolean(options.open), false);
  wrap.append(button, menu);
  return wrap;
}
