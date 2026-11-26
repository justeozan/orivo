/* ---------------------------------------------------------------------------
   The bridge between plugins' Store tabs in the host and the Store's registry.

   The host asks each installed plugin that declares a Store tab, validates its
   answer and hands over only that (`plugin_store_tabs.rs`). This module keeps
   `store-tabs.ts` equal to it: what the host lists is registered, and a tab
   this module registered that the host no longer lists — its plugin was
   uninstalled, replaced, or parked after repeated failures — is removed. Tabs
   registered by anyone else are left alone.

   Outside the desktop shell (`pnpm dev`, a browser) there is no host to ask,
   and every function here does nothing.
   --------------------------------------------------------------------------- */

import { invoke } from "./host";
import { isTauriRuntime } from "./media";
import {
  listPluginStoreTabs,
  registerStoreTab,
  unregisterStoreTab,
  type PluginStoreGame,
  type PluginStorePrice,
  type StoreTabInput,
} from "./store-tabs";

/** The host's event when an installed plugin changed or went away. */
export const PLUGIN_STORE_TABS_EVENT = "plugin-store-tabs-changed";

/** One tab as `list_plugin_store_tabs` returns it (`StoreTabView`). */
interface StoreTabWire {
  pluginId: string;
  key: string | null;
  label: string;
  accent: string | null;
  games: StoreTabGameWire[];
}

interface StoreTabGameWire {
  id: string;
  title: string;
  shortDescription: string;
  coverUrl: string;
  heroUrl: string;
  landscapeUrl: string;
  logoUrl: string;
  genres: string[];
  price: PluginStorePrice | null;
  hasPage: boolean;
}

type ListenFn = (event: string, handler: () => void) => Promise<() => void>;

/** Ids of the tabs this module registered, so a sync removes only its own. */
const registered = new Map<string, string>();
/** Each sync's number: an answer that arrives after a newer one is dropped. */
let latest = 0;

function text(value: unknown): string {
  return typeof value === "string" ? value : "";
}

/** A host game as the registry takes it. The registry checks it all again. */
function toGame(raw: StoreTabGameWire): PluginStoreGame {
  return {
    id: text(raw.id),
    title: text(raw.title),
    source: "store",
    shortDescription: text(raw.shortDescription),
    coverUrl: text(raw.coverUrl),
    heroUrl: text(raw.heroUrl),
    landscapeUrl: text(raw.landscapeUrl),
    logoUrl: text(raw.logoUrl) || undefined,
    genres: Array.isArray(raw.genres) ? raw.genres : [],
    tags: [],
    supportedPlatforms: [],
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: [],
    offers: [],
    pluginPrice: raw.price ?? undefined,
    hasPluginPage: raw.hasPage === true,
  };
}

function toInput(raw: StoreTabWire): StoreTabInput | null {
  if (!raw || typeof raw !== "object" || typeof raw.pluginId !== "string") return null;
  return {
    pluginId: raw.pluginId,
    key: typeof raw.key === "string" ? raw.key : undefined,
    label: text(raw.label),
    accent: typeof raw.accent === "string" ? raw.accent : undefined,
    games: Array.isArray(raw.games) ? raw.games.filter((game) => game && typeof game === "object").map(toGame) : [],
  };
}

/**
 * Ask the host for the plugins' Store tabs and bring the registry in line.
 *
 * Safe to call as often as a page likes: the host keeps each plugin's answer
 * for as long as the same package is installed, so a call after the first
 * reads the plugin folder and compiles nothing. Never rejects — a host that
 * cannot answer leaves the registry as it was.
 */
export async function syncPluginStoreTabs(): Promise<void> {
  if (!isTauriRuntime()) return;
  const generation = ++latest;
  let answer: unknown;
  try {
    answer = await invoke<StoreTabWire[]>("list_plugin_store_tabs");
  } catch {
    return;
  }
  if (generation !== latest || !Array.isArray(answer)) return;

  const seen = new Set<string>();
  for (const raw of answer as StoreTabWire[]) {
    const input = toInput(raw);
    if (!input) continue;
    // Re-registering an identical tab would repaint the Store for nothing.
    const signature = JSON.stringify(input);
    const id = input.key ? `plugin:${input.pluginId}:${input.key}` : `plugin:${input.pluginId}`;
    if (registered.get(id) === signature && listPluginStoreTabs().some((tab) => tab.id === id)) {
      seen.add(id);
      continue;
    }
    const tab = registerStoreTab(input);
    if (!tab) continue;
    registered.set(tab.id, signature);
    seen.add(tab.id);
  }
  for (const id of [...registered.keys()]) {
    if (seen.has(id)) continue;
    registered.delete(id);
    unregisterStoreTab(id);
  }
}

/**
 * Open the web page a plugin gave for one of its games, in the player's
 * browser. The host resolves the address from the game's id and refuses an id
 * it did not list; the WebView never holds the address. Rejects with the
 * host's message.
 */
export async function openPluginStoreGamePage(gameId: string): Promise<void> {
  if (!isTauriRuntime()) return;
  await invoke("open_plugin_store_page", { gameId });
}

/**
 * Sync once the first frame is painted, then whenever the host says a plugin
 * changed. Returns the disposer.
 */
export function watchPluginStoreTabs(listenFn: ListenFn): () => void {
  if (!isTauriRuntime()) return () => {};
  const idle = window.requestIdleCallback ?? window.setTimeout;
  idle(() => void syncPluginStoreTabs());
  const pending = listenFn(PLUGIN_STORE_TABS_EVENT, () => void syncPluginStoreTabs());
  return () => {
    void pending.then((unlisten) => unlisten()).catch(() => undefined);
  };
}

/** Test seam: forget which tabs this module registered. */
export function resetPluginStoreTabsForTests(): void {
  registered.clear();
  latest = 0;
}
