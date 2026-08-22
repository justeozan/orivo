/**
 * The notification bell: the quiet channel for advice that is worth giving
 * once and never worth interrupting for.
 *
 * Everything here is optional to the user and honest about it. Orivo ships no
 * artwork key and no authorised price feed, so the two things it can usefully
 * say to a new library are "here is where a key would help" and "here is what
 * the price providers can actually answer". Neither is a warning, neither
 * blocks anything, and both stop being said the moment they stop being true.
 *
 * Rules this module exists to keep:
 *
 * - **Nothing fires on arrival.** A notice waits out `delayMs` of app time, so
 *   the first minute of a fresh install belongs to the library, not to advice.
 * - **Nothing is said twice.** Delivery, reading and dismissal are persisted,
 *   and a dismissed notice never comes back.
 * - **Nothing is said that is no longer true.** `applies` is re-checked every
 *   tick, so a key added before the timer elapses cancels the notice about it.
 *
 * No DOM and no storage handle of its own: `app.ts` owns both.
 */
import type { IconName } from "./icons";

export type NotificationId = "artwork-keys" | "store-prices";

/**
 * Where a notice's action goes. A route alone cannot say it — the artwork keys
 * live inside one plugin's detail view, which the route grammar does not name —
 * so the shell resolves these tokens itself.
 */
export type NotificationTarget = "wallpaper-keys" | "library-sources";

/** What the app looks like right now, as far as this advice is concerned. */
export interface NotificationContext {
  /** Advice about artwork is noise until there is artwork to improve. */
  hasGames: boolean;
  /** Any wallpaper source key at all, IGDB pair, Google pair or SteamGridDB. */
  hasArtworkKey: boolean;
  /** Whether the provider list has ever been opened. */
  visitedLibrarySources: boolean;
}

export interface NotificationDefinition {
  id: NotificationId;
  icon: IconName;
  title: string;
  body: string;
  actionLabel: string;
  target: NotificationTarget;
  /** How long the app has to have been open before this is worth saying. */
  delayMs: number;
  /** Whether the advice still applies to the app as it stands. */
  applies(context: NotificationContext): boolean;
}

const MINUTE = 60_000;

export const NOTIFICATIONS: readonly NotificationDefinition[] = [
  {
    id: "artwork-keys",
    icon: "palette",
    title: "Sharper artwork, with your own key",
    body:
      "Covers come from whatever each store publishes. SteamGridDB and IGDB return 4K key " +
      "art and clean wordmarks — Orivo ships no key of its own, so add yours in " +
      "Plugins › Wallpaper Searcher.",
    actionLabel: "Open Wallpaper Searcher",
    target: "wallpaper-keys",
    delayMs: 2 * MINUTE,
    applies: (context) => context.hasGames && !context.hasArtworkKey,
  },
  {
    id: "store-prices",
    icon: "store",
    title: "Where store prices come from",
    body:
      "Orivo only shows a price a store actually returned, and most feeds need an authorised " +
      "key it does not ship. Libraries & Sources lists every provider and what each one can " +
      "answer right now.",
    actionLabel: "Open Libraries & Sources",
    target: "library-sources",
    delayMs: 5 * MINUTE,
    applies: (context) => !context.visitedLibrarySources,
  },
] as const;

/**
 * What the bell remembers between launches. `visitedLibrarySources` rides
 * along because it is the one context fact no backend keeps: it is a record of
 * something the user did, not of something the app is.
 */
export interface NotificationRecord {
  delivered: NotificationId[];
  read: NotificationId[];
  dismissed: NotificationId[];
  visitedLibrarySources: boolean;
}

export const NOTIFICATION_STORAGE_KEY = "orivo.notifications.v1";

export function emptyNotificationRecord(): NotificationRecord {
  return { delivered: [], read: [], dismissed: [], visitedLibrarySources: false };
}

const IDS = new Set<string>(NOTIFICATIONS.map((notification) => notification.id));

function isNotificationId(value: unknown): value is NotificationId {
  return typeof value === "string" && IDS.has(value);
}

function readIds(value: unknown): NotificationId[] {
  return Array.isArray(value) ? [...new Set(value.filter(isNotificationId))] : [];
}

/**
 * Read a stored record. Anything unrecognised is dropped rather than trusted:
 * a notice retired from the catalogue must not keep a slot in the list, and a
 * corrupt document must not be able to silence the bell forever.
 */
export function normaliseNotificationRecord(value: unknown): NotificationRecord {
  const record =
    typeof value === "object" && value !== null ? (value as Record<string, unknown>) : {};
  return {
    delivered: readIds(record.delivered),
    read: readIds(record.read),
    dismissed: readIds(record.dismissed),
    visitedLibrarySources: record.visitedLibrarySources === true,
  };
}

/**
 * A notice is due when its timer has elapsed, it has never been delivered, it
 * was never dismissed, and it is still true.
 */
export function dueNotifications(
  elapsedMs: number,
  context: NotificationContext,
  record: NotificationRecord,
  catalog: readonly NotificationDefinition[] = NOTIFICATIONS,
): NotificationDefinition[] {
  return catalog.filter(
    (notification) =>
      elapsedMs >= notification.delayMs &&
      !record.delivered.includes(notification.id) &&
      !record.dismissed.includes(notification.id) &&
      notification.applies(context),
  );
}

/** What the panel lists: everything delivered that has not been dismissed. */
export function visibleNotifications(
  record: NotificationRecord,
  catalog: readonly NotificationDefinition[] = NOTIFICATIONS,
): NotificationDefinition[] {
  return catalog.filter(
    (notification) =>
      record.delivered.includes(notification.id) &&
      !record.dismissed.includes(notification.id),
  );
}

/** What the dot on the bell counts. */
export function unreadNotificationCount(
  record: NotificationRecord,
  catalog: readonly NotificationDefinition[] = NOTIFICATIONS,
): number {
  return visibleNotifications(record, catalog).filter(
    (notification) => !record.read.includes(notification.id),
  ).length;
}

/**
 * Whether anything is still waiting on the clock. Once nothing is, the shell
 * stops ticking rather than re-asking the same settled question forever.
 */
export function notificationsSettled(
  record: NotificationRecord,
  catalog: readonly NotificationDefinition[] = NOTIFICATIONS,
): boolean {
  return catalog.every(
    (notification) =>
      record.delivered.includes(notification.id) ||
      record.dismissed.includes(notification.id),
  );
}

function withId(ids: NotificationId[], id: NotificationId): NotificationId[] {
  return ids.includes(id) ? ids : [...ids, id];
}

export function markDelivered(
  record: NotificationRecord,
  id: NotificationId,
): NotificationRecord {
  return { ...record, delivered: withId(record.delivered, id) };
}

/** Opening the panel is what counts as reading; nothing else marks a notice. */
export function markAllRead(
  record: NotificationRecord,
  catalog: readonly NotificationDefinition[] = NOTIFICATIONS,
): NotificationRecord {
  return {
    ...record,
    read: visibleNotifications(record, catalog).reduce(
      (ids, notification) => withId(ids, notification.id),
      record.read,
    ),
  };
}

/**
 * Dismissing is final. It also marks the notice read, so a bell cleared from
 * the panel cannot leave its own dot behind.
 */
export function dismissNotification(
  record: NotificationRecord,
  id: NotificationId,
): NotificationRecord {
  return {
    ...record,
    read: withId(record.read, id),
    dismissed: withId(record.dismissed, id),
  };
}

export function markLibrarySourcesVisited(record: NotificationRecord): NotificationRecord {
  return record.visitedLibrarySources ? record : { ...record, visitedLibrarySources: true };
}

/**
 * Load and save, guarded.
 *
 * A WebView can refuse storage entirely — a private context, a locked profile,
 * a quota that is already full. None of that is worth an error the user has to
 * read: the bell simply forgets between launches, which is a smaller failure
 * than a crash on boot.
 */
export function loadNotificationRecord(storage: Storage | null): NotificationRecord {
  if (!storage) return emptyNotificationRecord();
  try {
    const raw = storage.getItem(NOTIFICATION_STORAGE_KEY);
    return raw ? normaliseNotificationRecord(JSON.parse(raw)) : emptyNotificationRecord();
  } catch {
    return emptyNotificationRecord();
  }
}

export function saveNotificationRecord(
  storage: Storage | null,
  record: NotificationRecord,
): void {
  if (!storage) return;
  try {
    storage.setItem(NOTIFICATION_STORAGE_KEY, JSON.stringify(record));
  } catch {
    // Storage is a convenience here, never a dependency.
  }
}

/** The browser's storage when it has one, and nothing when it does not. */
export function defaultNotificationStorage(): Storage | null {
  try {
    return window.localStorage ?? null;
  } catch {
    return null;
  }
}
