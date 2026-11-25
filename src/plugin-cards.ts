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
  /** One or two short lines under the name: state, then who made it. */
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
