import type { GameSummary, StoreBrand, StoreTabId } from "./contracts";

/**
 * The Store's tabs: the built-in brands, and the ones plugins add.
 *
 * A plugin tab is registered here and nowhere else. The page reads this list on
 * every render and listens for changes, so a plugin that loads after the Store
 * opened, or one that is removed while it is open, shows up or goes without a
 * reload. Everything a plugin hands over is untrusted: the label is cut and
 * stripped of control characters, the colour must be a plain `#rrggbb`, and a
 * picture must be served over https or from the app itself. Nothing from here
 * is ever written with `innerHTML`.
 */

/** The brands, in the order the bar shows them, after "Tout". */
export const STORE_BRANDS: readonly StoreBrand[] = ["playstation", "xbox", "nintendo", "steam", "epic"];

export interface PluginStoreTab {
  /** The `?store=` value: `plugin:<plugin id>` or `plugin:<plugin id>:<key>`. */
  id: `plugin:${string}`;
  pluginId: string;
  label: string;
  /** `#rrggbb`. A plugin that sent nothing usable gets Orivo's violet. */
  accent: string;
  games: GameSummary[];
}

export interface StoreTabInput {
  pluginId: string;
  /** Tells apart two tabs of the same plugin. Optional for a plugin with one. */
  key?: string;
  label: string;
  accent?: string;
  games: GameSummary[];
}

export const DEFAULT_PLUGIN_ACCENT = "#7d54f4";
const MAX_LABEL = 32;
const MAX_GAMES = 120;

// The manifest's grammar (plugin_manifest.rs `valid_plugin_id`): lowercase
// reverse-DNS, three segments or more. A key is one more such segment.
const PLUGIN_ID = /^[a-z][a-z0-9-]{0,62}(?:\.[a-z][a-z0-9-]{0,62}){2,}$/;
const TAB_KEY = /^[a-z0-9][a-z0-9-]{0,31}$/;
const ACCENT = /^#[0-9a-f]{6}$/i;

const tabs = new Map<string, PluginStoreTab>();
const listeners = new Set<() => void>();

export function isStoreBrand(value: string): value is StoreBrand {
  return (STORE_BRANDS as readonly string[]).includes(value);
}

/** Whether `value` is a tab id the route may carry, registered or not yet. */
export function isStoreTabId(value: string): value is StoreTabId {
  if (value === "all" || isStoreBrand(value)) return true;
  const match = /^plugin:([^:]+)(?::([^:]+))?$/.exec(value);
  if (!match) return false;
  return PLUGIN_ID.test(match[1]) && match[1].length <= 128 && (match[2] === undefined || TAB_KEY.test(match[2]));
}

/** A colour a plugin may paint the page with, or `null`. */
export function safeAccent(value: unknown): string | null {
  return typeof value === "string" && ACCENT.test(value.trim()) ? value.trim().toLowerCase() : null;
}

function safeLabel(value: unknown): string {
  if (typeof value !== "string") return "";
  // Control and bidi-override characters could reorder what the bar prints.
  const clean = value.replace(/[\u0000-\u001f\u007f-\u009f‪-‮⁦-⁩]/g, "").trim();
  return clean.length > MAX_LABEL ? `${clean.slice(0, MAX_LABEL - 1).trimEnd()}…` : clean;
}

/** A picture a plugin may show: https, or a path the app serves itself. */
export function safeImageUrl(value: unknown): string {
  if (typeof value !== "string") return "";
  const url = value.trim();
  if (/["'()\\\s<>]/.test(url)) return "";
  if (/^https:\/\/[^/]/i.test(url)) return url;
  if (/^\/[^/]/.test(url)) return url;
  return "";
}

function safeGame(game: GameSummary, pluginId: string): GameSummary | null {
  if (!game || typeof game.id !== "string" || !game.id || typeof game.title !== "string") return null;
  const title = safeLabel(game.title) || "";
  if (!title) return null;
  return {
    ...game,
    // A plugin's ids live in its own namespace, so one can never stand in for a
    // catalogue game or another plugin's.
    id: game.id.startsWith(`${pluginId}:`) ? game.id : `${pluginId}:${game.id}`,
    title,
    coverUrl: safeImageUrl(game.coverUrl),
    heroUrl: safeImageUrl(game.heroUrl),
    landscapeUrl: safeImageUrl(game.landscapeUrl),
    logoUrl: safeImageUrl(game.logoUrl) || undefined,
    genres: Array.isArray(game.genres) ? game.genres.filter((genre) => typeof genre === "string").slice(0, 4) : [],
    tags: Array.isArray(game.tags) ? game.tags.filter((tag) => typeof tag === "string").slice(0, 8) : [],
    offers: Array.isArray(game.offers) ? game.offers : [],
    // A plugin tab is the plugin's own shelf; it does not get to call a game
    // a brand's exclusive.
    exclusiveTo: undefined,
  };
}

function notify(): void {
  for (const listener of [...listeners]) listener();
}

/** Adds or replaces a plugin's tab. Returns it, or `null` when the input is unusable. */
export function registerStoreTab(input: StoreTabInput): PluginStoreTab | null {
  if (!input || typeof input.pluginId !== "string" || !PLUGIN_ID.test(input.pluginId)) return null;
  if (input.key !== undefined && (typeof input.key !== "string" || !TAB_KEY.test(input.key))) return null;
  const label = safeLabel(input.label);
  if (!label) return null;
  const id = (input.key ? `plugin:${input.pluginId}:${input.key}` : `plugin:${input.pluginId}`) as PluginStoreTab["id"];
  const games = (Array.isArray(input.games) ? input.games : [])
    .slice(0, MAX_GAMES)
    .map((game) => safeGame(game, input.pluginId))
    .filter((game): game is GameSummary => game !== null);
  const tab: PluginStoreTab = {
    id,
    pluginId: input.pluginId,
    label,
    accent: safeAccent(input.accent) ?? DEFAULT_PLUGIN_ACCENT,
    games,
  };
  tabs.set(id, tab);
  notify();
  return tab;
}

export function unregisterStoreTab(id: string): void {
  if (tabs.delete(id)) notify();
}

/** Removes every tab a plugin added: what uninstalling or disabling it does. */
export function unregisterPluginStoreTabs(pluginId: string): void {
  let changed = false;
  for (const [id, tab] of tabs) {
    if (tab.pluginId !== pluginId) continue;
    tabs.delete(id);
    changed = true;
  }
  if (changed) notify();
}

/** Plugin tabs in the order they were first registered. */
export function listPluginStoreTabs(): readonly PluginStoreTab[] {
  return [...tabs.values()];
}

export function pluginStoreTab(id: string): PluginStoreTab | null {
  return tabs.get(id) ?? null;
}

export function onStoreTabsChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Test seam: forget every plugin tab. */
export function resetStoreTabsForTests(): void {
  tabs.clear();
  listeners.clear();
}
