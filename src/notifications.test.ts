import { describe, expect, it } from "vitest";
import {
  NOTIFICATIONS,
  NOTIFICATION_STORAGE_KEY,
  dismissNotification,
  dueNotifications,
  emptyNotificationRecord,
  loadNotificationRecord,
  markAllRead,
  markDelivered,
  markLibrarySourcesVisited,
  normaliseNotificationRecord,
  notificationsSettled,
  saveNotificationRecord,
  unreadNotificationCount,
  visibleNotifications,
  type NotificationContext,
  type NotificationDefinition,
} from "./notifications";

const context = (overrides: Partial<NotificationContext> = {}): NotificationContext => ({
  hasGames: true,
  hasArtworkKey: false,
  visitedLibrarySources: false,
  ...overrides,
});

/** A minute past the slowest notice, so only the conditions decide. */
const LATE = 60 * 60_000;

const byId = (id: string): NotificationDefinition =>
  NOTIFICATIONS.find((notification) => notification.id === id)!;

describe("the notification catalogue", () => {
  it("gives every notice a title, a body, an action and somewhere to go", () => {
    for (const notification of NOTIFICATIONS) {
      expect(notification.title).not.toBe("");
      expect(notification.body.length).toBeGreaterThan(40);
      expect(notification.actionLabel).not.toBe("");
      expect(notification.target).not.toBe("");
    }
  });

  it("never fires on arrival", () => {
    // The first minutes of a session belong to the library, not to advice.
    for (const notification of NOTIFICATIONS) {
      expect(notification.delayMs).toBeGreaterThanOrEqual(60_000);
    }
    expect(dueNotifications(0, context(), emptyNotificationRecord())).toEqual([]);
  });

  it("keeps quiet about artwork until there is artwork to improve", () => {
    const withoutGames = dueNotifications(LATE, context({ hasGames: false }), emptyNotificationRecord());
    expect(withoutGames.map((notification) => notification.id)).not.toContain("artwork-keys");
  });

  it("stops offering an artwork key once one is set", () => {
    expect(byId("artwork-keys").applies(context({ hasArtworkKey: true }))).toBe(false);
    expect(byId("artwork-keys").applies(context({ hasArtworkKey: false }))).toBe(true);
  });

  it("stops explaining where prices come from once the provider list has been seen", () => {
    expect(byId("store-prices").applies(context({ visitedLibrarySources: true }))).toBe(false);
    expect(byId("store-prices").applies(context({ visitedLibrarySources: false }))).toBe(true);
  });
});

describe("what is due", () => {
  it("holds a notice back until its own delay has passed", () => {
    const artwork = byId("artwork-keys");
    const record = emptyNotificationRecord();
    expect(dueNotifications(artwork.delayMs - 1, context(), record)).toEqual([]);
    expect(dueNotifications(artwork.delayMs, context(), record).map((n) => n.id)).toContain(
      "artwork-keys",
    );
  });

  it("never delivers the same notice twice", () => {
    const record = markDelivered(emptyNotificationRecord(), "artwork-keys");
    expect(dueNotifications(LATE, context(), record).map((n) => n.id)).not.toContain(
      "artwork-keys",
    );
  });

  it("never brings a dismissed notice back", () => {
    const record = dismissNotification(emptyNotificationRecord(), "store-prices");
    expect(dueNotifications(LATE, context(), record).map((n) => n.id)).not.toContain(
      "store-prices",
    );
  });
});

describe("the panel and the dot", () => {
  it("lists what was delivered and not dismissed", () => {
    let record = markDelivered(emptyNotificationRecord(), "artwork-keys");
    record = markDelivered(record, "store-prices");
    expect(visibleNotifications(record).map((n) => n.id)).toEqual([
      "artwork-keys",
      "store-prices",
    ]);

    record = dismissNotification(record, "artwork-keys");
    expect(visibleNotifications(record).map((n) => n.id)).toEqual(["store-prices"]);
  });

  it("counts only what is both visible and unread", () => {
    let record = markDelivered(emptyNotificationRecord(), "artwork-keys");
    expect(unreadNotificationCount(record)).toBe(1);

    record = markAllRead(record);
    expect(unreadNotificationCount(record)).toBe(0);

    // A notice that arrives after the panel was read is unread again.
    record = markDelivered(record, "store-prices");
    expect(unreadNotificationCount(record)).toBe(1);
  });

  it("leaves no dot behind when a notice is dismissed unread", () => {
    const record = dismissNotification(
      markDelivered(emptyNotificationRecord(), "artwork-keys"),
      "artwork-keys",
    );
    expect(unreadNotificationCount(record)).toBe(0);
    expect(visibleNotifications(record)).toEqual([]);
  });

  it("is settled once nothing is left waiting on the clock", () => {
    let record = emptyNotificationRecord();
    expect(notificationsSettled(record)).toBe(false);
    for (const notification of NOTIFICATIONS) {
      record = markDelivered(record, notification.id);
    }
    expect(notificationsSettled(record)).toBe(true);
  });
});

describe("the stored record", () => {
  it("drops ids it does not recognise instead of trusting them", () => {
    const record = normaliseNotificationRecord({
      delivered: ["artwork-keys", "retired-notice", 7],
      read: "not an array",
      dismissed: ["store-prices", "store-prices"],
      visitedLibrarySources: "yes",
    });
    expect(record).toEqual({
      delivered: ["artwork-keys"],
      read: [],
      dismissed: ["store-prices"],
      visitedLibrarySources: false,
    });
  });

  it("survives a corrupt document rather than silencing the bell forever", () => {
    const storage = fakeStorage();
    storage.setItem(NOTIFICATION_STORAGE_KEY, "{not json");
    expect(loadNotificationRecord(storage)).toEqual(emptyNotificationRecord());
  });

  it("round-trips through storage", () => {
    const storage = fakeStorage();
    const record = markLibrarySourcesVisited(
      markDelivered(emptyNotificationRecord(), "artwork-keys"),
    );
    saveNotificationRecord(storage, record);
    expect(loadNotificationRecord(storage)).toEqual(record);
  });

  it("forgets rather than throws when the WebView refuses storage", () => {
    expect(loadNotificationRecord(null)).toEqual(emptyNotificationRecord());
    expect(() => saveNotificationRecord(null, emptyNotificationRecord())).not.toThrow();

    const hostile = {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("quota");
      },
    } as unknown as Storage;
    expect(loadNotificationRecord(hostile)).toEqual(emptyNotificationRecord());
    expect(() => saveNotificationRecord(hostile, emptyNotificationRecord())).not.toThrow();
  });

  it("only flips the visited flag once", () => {
    const visited = markLibrarySourcesVisited(emptyNotificationRecord());
    expect(visited.visitedLibrarySources).toBe(true);
    // Same object back, so the caller can skip a write it does not need.
    expect(markLibrarySourcesVisited(visited)).toBe(visited);
  });
});

function fakeStorage(): Storage {
  const entries = new Map<string, string>();
  return {
    getItem: (key: string) => entries.get(key) ?? null,
    setItem: (key: string, value: string) => {
      entries.set(key, value);
    },
    removeItem: (key: string) => {
      entries.delete(key);
    },
    clear: () => entries.clear(),
    key: (index: number) => [...entries.keys()][index] ?? null,
    get length() {
      return entries.size;
    },
  } as Storage;
}
