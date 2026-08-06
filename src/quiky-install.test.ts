import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute, GameSummary, StoreOffer } from "./contracts";
import { PageLifecycleHost } from "./page-lifecycle";
import {
  createDefaultQuikyClient,
  createQuikyController,
  formatQuikyLabel,
  isQuikyBusy,
  matchTitle,
  normaliseTitle,
  quikyPercent,
  unavailableQuikyStatus,
  type QuikyClient,
  type QuikyProgress,
  type QuikyStatus,
  type QuikyTitle,
} from "./quiky-install";
import type { StoreBrowsePage, StoreHomeView } from "./store-model";
import { createStorePage, type StorePageClient } from "./store-page";

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

function title(overrides: Partial<QuikyTitle> = {}): QuikyTitle {
  return {
    slug: "openttd",
    title: "OpenTTD",
    matchTitles: ["open ttd", "transport tycoon deluxe"],
    downloadBytes: 8_916_160,
    installed: false,
    installPath: null,
    ...overrides,
  };
}

function status(overrides: Partial<QuikyStatus> = {}): QuikyStatus {
  return {
    available: true,
    pluginName: "Quiky",
    version: "0.1.0",
    message: "",
    titles: [title()],
    ...overrides,
  };
}

function progress(overrides: Partial<QuikyProgress> = {}): QuikyProgress {
  return {
    slug: "openttd",
    phase: "downloading",
    percent: 42,
    message: "",
    installPath: null,
    ...overrides,
  };
}

interface RecordedQuiky {
  client: QuikyClient;
  calls: string[];
  /** The Store card id handed to each startInstall, in order. */
  gameIds: Array<string | null>;
  teardowns: string[];
  emit(next: QuikyProgress): void;
}

function createFakeQuiky(overrides: Partial<QuikyClient> = {}): RecordedQuiky {
  const calls: string[] = [];
  const gameIds: Array<string | null> = [];
  const teardowns: string[] = [];
  const listeners = new Set<(next: QuikyProgress) => void>();
  const client: QuikyClient = {
    async getStatus(signal) {
      calls.push("getStatus");
      return overrides.getStatus ? overrides.getStatus(signal) : status();
    },
    async startInstall(slug, gameId, signal) {
      calls.push(`startInstall:${slug}`);
      gameIds.push(gameId);
      if (overrides.startInstall) await overrides.startInstall(slug, gameId, signal);
    },
    async cancelInstall(slug, signal) {
      calls.push(`cancelInstall:${slug}`);
      if (overrides.cancelInstall) await overrides.cancelInstall(slug, signal);
    },
    subscribe(onProgress) {
      calls.push("subscribe");
      listeners.add(onProgress);
      return () => {
        teardowns.push("unsubscribe");
        listeners.delete(onProgress);
      };
    },
  };
  return {
    client,
    calls,
    gameIds,
    teardowns,
    emit(next) {
      for (const listener of [...listeners]) listener(next);
    },
  };
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

const liveSignal = (): AbortSignal => new AbortController().signal;

// ---------------------------------------------------------------------------
// Title matching
// ---------------------------------------------------------------------------

describe("normaliseTitle", () => {
  it("lowercases, drops diacritics and collapses punctuation to single spaces", () => {
    expect(normaliseTitle("Ōkami HD: Rebirth — Édition Deluxe!")).toBe(
      "okami hd rebirth edition deluxe",
    );
    expect(normaliseTitle("  Baldur's   Gate 3  ")).toBe("baldur s gate 3");
    expect(normaliseTitle("OpenTTD")).toBe("openttd");
    expect(normaliseTitle("DOSBox-Staging")).toBe("dosbox staging");
  });

  it("returns an empty string for a title made only of punctuation", () => {
    expect(normaliseTitle("— :: —")).toBe("");
    expect(normaliseTitle("")).toBe("");
  });
});

describe("matchTitle", () => {
  const titles = [
    title(),
    title({ slug: "dosbox-staging", title: "DOSBox Staging", matchTitles: ["dosbox"] }),
  ];

  it("matches the canonical title whatever the store's punctuation", () => {
    expect(matchTitle(titles, "openttd")?.slug).toBe("openttd");
    expect(matchTitle(titles, "  OpenTTD  ")?.slug).toBe("openttd");
    expect(matchTitle(titles, "DOSBox-Staging")?.slug).toBe("dosbox-staging");
  });

  it("matches an alternative title", () => {
    expect(matchTitle(titles, "Transport Tycoon Deluxe")?.slug).toBe("openttd");
    expect(matchTitle(titles, "Open TTD")?.slug).toBe("openttd");
    expect(matchTitle(titles, "DOSBox")?.slug).toBe("dosbox-staging");
  });

  it("returns null rather than guessing", () => {
    expect(matchTitle(titles, "Elden Ring")).toBeNull();
    expect(matchTitle(titles, "Open")).toBeNull();
    expect(matchTitle(titles, "")).toBeNull();
    expect(matchTitle([], "OpenTTD")).toBeNull();
  });
});

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

describe("formatQuikyLabel", () => {
  it("names every phase", () => {
    const entry = title();
    expect(formatQuikyLabel(null, entry)).toBe("Installer");
    expect(formatQuikyLabel(progress({ phase: "queued", percent: 0 }), entry)).toBe("En attente…");
    expect(formatQuikyLabel(progress({ phase: "downloading", percent: 42 }), entry)).toBe(
      "Téléchargement 42%",
    );
    expect(formatQuikyLabel(progress({ phase: "extracting", percent: 78 }), entry)).toBe(
      "Extraction 78%",
    );
    expect(formatQuikyLabel(progress({ phase: "installed", percent: 100 }), entry)).toBe("Installé");
    expect(formatQuikyLabel(progress({ phase: "failed", percent: 61 }), entry)).toBe("Réessayer");
    expect(formatQuikyLabel(progress({ phase: "cancelled", percent: 61 }), entry)).toBe("Installer");
  });

  it("reads a title the plugin already installed as installed", () => {
    const entry = title({ installed: true, installPath: "C:/Games/OpenTTD" });
    expect(formatQuikyLabel(null, entry)).toBe("Installé");
    expect(formatQuikyLabel(progress({ phase: "cancelled" }), entry)).toBe("Installé");
  });

  it("keeps the percentage inside the bar", () => {
    expect(formatQuikyLabel(progress({ percent: 41.6 }), title())).toBe("Téléchargement 42%");
    expect(formatQuikyLabel(progress({ percent: -5 }), title())).toBe("Téléchargement 0%");
    expect(formatQuikyLabel(progress({ percent: 140 }), title())).toBe("Téléchargement 100%");
    expect(quikyPercent(progress({ percent: Number.NaN }))).toBe(0);
    expect(quikyPercent(null)).toBe(0);
  });
});

describe("isQuikyBusy", () => {
  it("is true only while the host still has work to cancel", () => {
    expect(isQuikyBusy(null)).toBe(false);
    expect(isQuikyBusy(progress({ phase: "queued" }))).toBe(true);
    expect(isQuikyBusy(progress({ phase: "downloading" }))).toBe(true);
    expect(isQuikyBusy(progress({ phase: "extracting" }))).toBe(true);
    expect(isQuikyBusy(progress({ phase: "installed" }))).toBe(false);
    expect(isQuikyBusy(progress({ phase: "failed" }))).toBe(false);
    expect(isQuikyBusy(progress({ phase: "cancelled" }))).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Controller
// ---------------------------------------------------------------------------

describe("createQuikyController", () => {
  it("keeps the progress map in step with the host and notifies on every change", async () => {
    const fake = createFakeQuiky();
    const controller = createQuikyController(fake.client);
    const changes: number[] = [];
    controller.onChange(() => changes.push(changes.length));

    await controller.load(liveSignal());
    expect(controller.status().available).toBe(true);
    expect(controller.progressFor("openttd")).toBeNull();
    expect(fake.calls).toEqual(["getStatus", "subscribe"]);

    fake.emit(progress({ phase: "queued", percent: 0 }));
    expect(controller.progressFor("openttd")?.phase).toBe("queued");

    fake.emit(progress({ phase: "downloading", percent: 42 }));
    expect(controller.progressFor("openttd")).toEqual(
      progress({ phase: "downloading", percent: 42 }),
    );

    fake.emit(progress({ slug: "dosbox-staging", phase: "extracting", percent: 78 }));
    expect(controller.progressFor("openttd")?.percent).toBe(42);
    expect(controller.progressFor("dosbox-staging")?.percent).toBe(78);
    expect(controller.progressFor("unknown-slug")).toBeNull();

    // One notification per load and per emitted progress, and no more.
    expect(changes.length).toBe(4);
  });

  it("records a finished install on the title so the button stays on Installé", async () => {
    const fake = createFakeQuiky();
    const controller = createQuikyController(fake.client);
    await controller.load(liveSignal());
    expect(controller.status().titles[0].installed).toBe(false);

    fake.emit(progress({ phase: "installed", percent: 100, installPath: "C:/Games/OpenTTD" }));

    const entry = controller.status().titles[0];
    expect(entry.installed).toBe(true);
    expect(entry.installPath).toBe("C:/Games/OpenTTD");
    expect(formatQuikyLabel(controller.progressFor("openttd"), entry)).toBe("Installé");
  });

  it("queues optimistically and hands the rest of the phases to the host", async () => {
    const fake = createFakeQuiky();
    const controller = createQuikyController(fake.client);
    await controller.load(liveSignal());

    const started = controller.install("openttd", "store:openttd");
    // The phase flips before the command resolves: the shopper clicked now.
    expect(controller.progressFor("openttd")?.phase).toBe("queued");
    await started;
    expect(fake.calls).toContain("startInstall:openttd");
    // The Store card id travels with the request so the installed game keeps
    // the catalogue's artwork instead of the Windows binary's icon.
    expect(fake.gameIds).toEqual(["store:openttd"]);

    fake.emit(progress({ phase: "downloading", percent: 12 }));
    expect(controller.progressFor("openttd")?.phase).toBe("downloading");
  });

  it("turns a refused install into a failed phase carrying the host's message", async () => {
    const fake = createFakeQuiky({
      startInstall: async () => {
        throw "Le disque est plein.";
      },
    });
    const controller = createQuikyController(fake.client);
    await controller.load(liveSignal());
    await controller.install("openttd");

    const failed = controller.progressFor("openttd");
    expect(failed?.phase).toBe("failed");
    expect(failed?.message).toBe("Le disque est plein.");
    expect(formatQuikyLabel(failed, title())).toBe("Réessayer");
  });

  it("leaves the cancelled phase to the host and only records a refusal", async () => {
    const fake = createFakeQuiky();
    const controller = createQuikyController(fake.client);
    await controller.load(liveSignal());
    fake.emit(progress({ phase: "downloading", percent: 42 }));

    await controller.cancel("openttd");
    expect(fake.calls).toContain("cancelInstall:openttd");
    // No optimistic write: a `downloading` tick already in flight would fight it.
    expect(controller.progressFor("openttd")?.phase).toBe("downloading");

    fake.emit(progress({ phase: "cancelled", percent: 42 }));
    expect(controller.progressFor("openttd")?.phase).toBe("cancelled");
    expect(formatQuikyLabel(controller.progressFor("openttd"), title())).toBe("Installer");
  });

  it("records a refused cancel as a failure", async () => {
    const fake = createFakeQuiky({
      cancelInstall: async () => {
        throw new Error("L'installation ne répond plus.");
      },
    });
    const controller = createQuikyController(fake.client);
    await controller.load(liveSignal());
    await controller.cancel("openttd");

    expect(controller.progressFor("openttd")?.phase).toBe("failed");
    expect(controller.progressFor("openttd")?.message).toBe("L'installation ne répond plus.");
  });

  it("drops its subscription and stops notifying once disposed", async () => {
    const fake = createFakeQuiky();
    const controller = createQuikyController(fake.client);
    let changes = 0;
    controller.onChange(() => {
      changes += 1;
    });
    await controller.load(liveSignal());
    fake.emit(progress());
    expect(changes).toBe(2);

    controller.dispose();
    expect(fake.teardowns).toEqual(["unsubscribe"]);
    fake.emit(progress({ percent: 90 }));

    expect(changes).toBe(2);
    expect(controller.progressFor("openttd")).toBeNull();
    expect(controller.status()).toEqual(unavailableQuikyStatus());
  });

  it("stops listening as soon as onChange is unsubscribed", async () => {
    const fake = createFakeQuiky();
    const controller = createQuikyController(fake.client);
    let changes = 0;
    const off = controller.onChange(() => {
      changes += 1;
    });
    await controller.load(liveSignal());
    off();
    fake.emit(progress());
    expect(changes).toBe(1);
  });

  it("drops a status that resolves after the activation was cancelled", async () => {
    const aborted = new AbortController();
    const fake = createFakeQuiky({
      getStatus: async () => {
        aborted.abort();
        return status();
      },
    });
    const controller = createQuikyController(fake.client);
    await controller.load(aborted.signal);

    expect(controller.status().available).toBe(false);
    expect(fake.calls).toEqual(["getStatus"]);
  });
});

describe("an absent Quiky plugin", () => {
  it("yields an unavailable status with no titles when getStatus throws", async () => {
    const fake = createFakeQuiky({
      getStatus: async () => {
        throw new Error("plugin com.orivo.quiky is not installed");
      },
    });
    const controller = createQuikyController(fake.client);
    await controller.load(liveSignal());

    expect(controller.status()).toEqual({
      available: false,
      pluginName: "",
      version: "",
      message: "",
      titles: [],
    });
    expect(controller.status().titles).toEqual([]);
    // Nothing to listen to: no subscription is opened for a feature that will
    // never paint.
    expect(fake.calls).toEqual(["getStatus"]);
  });

  it("never lets the default client throw outside the desktop shell", async () => {
    const client = createDefaultQuikyClient();
    await expect(client.getStatus(liveSignal())).resolves.toEqual({
      available: false,
      pluginName: "",
      version: "",
      message: "",
      titles: [],
    });
    await expect(client.startInstall("openttd", null, liveSignal())).resolves.toBeUndefined();
    await expect(client.cancelInstall("openttd", liveSignal())).resolves.toBeUndefined();
    expect(() => client.subscribe(() => {})()).not.toThrow();
  });
});

// ---------------------------------------------------------------------------
// The Store page's install affordance
// ---------------------------------------------------------------------------

function offer(overrides: Partial<StoreOffer> = {}): StoreOffer {
  return {
    id: "offer_1",
    gameId: "store:1",
    provider: "steam",
    providerLabel: "Steam",
    priceMinor: null,
    currency: null,
    region: "US",
    verifiedAt: null,
    availability: "unknown",
    stale: true,
    ...overrides,
  };
}

function game(overrides: Partial<GameSummary> = {}): GameSummary {
  const id = overrides.id ?? "store:1";
  return {
    id,
    title: "OpenTTD",
    source: "store",
    shortDescription: "A test entry.",
    coverUrl: "/media/cover.jpg",
    heroUrl: "/media/hero.jpg",
    landscapeUrl: "/media/landscape.jpg",
    genres: ["Simulation"],
    tags: ["Short Sessions"],
    supportedPlatforms: ["windows"],
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: [],
    offers: [offer({ gameId: id })],
    ...overrides,
  };
}

function homeView(games: GameSummary[]): StoreHomeView {
  return {
    games,
    providerStatuses: [],
    recommendationMode: "editorial",
    recommendationHeading: "Editorial picks",
    refreshedAt: null,
  };
}

function createFakeStoreClient(games: GameSummary[]): StorePageClient {
  const browse: StoreBrowsePage = { games: [], nextCursor: null, providerStatuses: [] };
  return {
    async getHome() {
      return homeView(games);
    },
    async browse() {
      return browse;
    },
    async refreshSources() {},
    async setWishlist() {},
    async openOffer() {},
    async listOwnedGameIds() {
      return [];
    },
  };
}

const storeRoute = (): AppRoute => ({
  page: "store",
  category: "for-you",
  platforms: [],
  query: "",
});

let host: PageLifecycleHost | null = null;
let container: HTMLElement | null = null;

function mountStore(
  games: GameSummary[],
  quiky: QuikyClient | undefined,
): { container: HTMLElement; host: PageLifecycleHost; navigations: AppRoute[] } {
  const element = document.createElement("section");
  document.body.append(element);
  const navigations: AppRoute[] = [];
  const page = createStorePage({
    navigate: (route) => navigations.push(route),
    client: createFakeStoreClient(games),
    quiky,
  });
  const lifecycle = new PageLifecycleHost(element, page);
  container = element;
  host = lifecycle;
  return { container: element, host: lifecycle, navigations };
}

const railHtml = (root: HTMLElement): string => {
  const track = root.querySelector<HTMLElement>(".store-rail__track");
  if (!track) throw new Error("The shelf did not mount.");
  return track.innerHTML;
};

const installButton = (root: HTMLElement): HTMLButtonElement => {
  const node = root.querySelector<HTMLButtonElement>('[data-focus-key="install-store:1"]');
  if (!node) throw new Error("No install control on the card.");
  return node;
};

beforeEach(() => {
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    callback(0);
    return 1;
  });
});

afterEach(() => {
  host?.deactivate();
  host = null;
  container?.remove();
  container = null;
  vi.unstubAllGlobals();
});

describe("Store page without the Quiky plugin", () => {
  it("paints the same shelf whether the plugin is missing or merely silent", async () => {
    const unavailable = createFakeQuiky({ getStatus: async () => unavailableQuikyStatus() });
    const withPlugin = mountStore([game()], unavailable.client);
    await withPlugin.host.activate(storeRoute());
    await flush();
    const gated = railHtml(withPlugin.container);

    // The shelf really painted, so the comparison below is not two empty strings.
    expect(gated).toContain("store-card__wishlist");
    expect(gated).toContain("store-card__title");
    expect(withPlugin.container.querySelector(".store-card__install")).toBeNull();
    expect(withPlugin.container.querySelector(".store-card__meter")).toBeNull();
    expect(withPlugin.container.querySelector('[data-focus-key^="install-"]')).toBeNull();
    expect(gated).not.toContain("progressbar");
    // No subscription is opened for a feature that renders nothing.
    expect(unavailable.calls).toEqual(["getStatus"]);

    withPlugin.host.deactivate();
    withPlugin.container.remove();

    // The default client with no Tauri runtime is the same path a shipped build
    // takes when the plugin is not installed. Byte for byte, the same shelf.
    const bare = mountStore([game()], undefined);
    await bare.host.activate(storeRoute());
    await flush();
    expect(railHtml(bare.container)).toBe(gated);
  });

  it("keeps a card that no title claims free of any install control", async () => {
    const fake = createFakeQuiky();
    const mounted = mountStore([game({ id: "store:9", title: "Elden Ring" })], fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    expect(mounted.container.querySelector(".store-card__install")).toBeNull();
    expect(mounted.container.querySelector(".store-card__meter")).toBeNull();
  });
});

describe("Store page install affordance", () => {
  it("adds a labelled control to the matching card and installs without navigating", async () => {
    const fake = createFakeQuiky();
    const mounted = mountStore([game()], fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    const button = installButton(mounted.container);
    expect(button.type).toBe("button");
    expect(button.textContent).toBe("Installer");
    expect(button.getAttribute("aria-label")).toBe("Installer OpenTTD");

    const meter = mounted.container.querySelector<HTMLElement>(".store-card__meter");
    expect(meter?.getAttribute("role")).toBe("progressbar");
    expect(meter?.getAttribute("aria-valuemin")).toBe("0");
    expect(meter?.getAttribute("aria-valuemax")).toBe("100");
    expect(meter?.hidden).toBe(true);

    button.click();
    await flush();

    expect(fake.calls).toContain("startInstall:openttd");
    // Clicking the control is not clicking the card.
    expect(mounted.navigations.some((route) => route.page === "game")).toBe(false);
  });

  it("moves the bar in place, without rebuilding a single card", async () => {
    const fake = createFakeQuiky();
    const mounted = mountStore([game()], fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    const button = installButton(mounted.container);
    const meter = mounted.container.querySelector<HTMLElement>(".store-card__meter");
    const fill = mounted.container.querySelector<HTMLElement>(".store-card__fill");
    if (!meter || !fill) throw new Error("No progress bar on the card.");

    fake.emit(progress({ phase: "downloading", percent: 42 }));
    expect(fill.style.width).toBe("42%");
    expect(meter.hidden).toBe(false);
    expect(meter.getAttribute("aria-valuenow")).toBe("42");
    expect(meter.getAttribute("aria-label")).toBe("OpenTTD — Téléchargement 42%");
    expect(button.textContent).toBe("Annuler");
    expect(button.getAttribute("aria-label")).toBe(
      "Annuler l'installation de OpenTTD — Téléchargement 42%",
    );

    fake.emit(progress({ phase: "extracting", percent: 78 }));
    expect(fill.style.width).toBe("78%");
    expect(meter.getAttribute("aria-label")).toBe("OpenTTD — Extraction 78%");

    // The same nodes throughout: a percentage tick never rebuilds the shelf.
    expect(installButton(mounted.container)).toBe(button);
    expect(mounted.container.querySelector(".store-card__fill")).toBe(fill);
  });

  it("cancels through the same control while the host is working", async () => {
    const fake = createFakeQuiky();
    const mounted = mountStore([game()], fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    fake.emit(progress({ phase: "downloading", percent: 42 }));
    const button = installButton(mounted.container);
    expect(button.textContent).toBe("Annuler");

    button.click();
    await flush();
    expect(fake.calls).toContain("cancelInstall:openttd");
    expect(fake.calls).not.toContain("startInstall:openttd");

    fake.emit(progress({ phase: "cancelled", percent: 42 }));
    expect(button.textContent).toBe("Installer");
    expect(mounted.container.querySelector<HTMLElement>(".store-card__meter")?.hidden).toBe(true);
  });

  it("settles on Installé and stops offering the install", async () => {
    const fake = createFakeQuiky();
    const mounted = mountStore([game()], fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    fake.emit(progress({ phase: "installed", percent: 100, installPath: "C:/Games/OpenTTD" }));

    const button = installButton(mounted.container);
    expect(button.textContent).toBe("Installé");
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect(button.getAttribute("aria-label")).toBe("OpenTTD est déjà installé");
    expect(mounted.container.querySelector<HTMLElement>(".store-card__meter")?.hidden).toBe(true);

    button.click();
    await flush();
    expect(fake.calls).not.toContain("startInstall:openttd");
  });

  it("offers a retry and reports the host's reason when an install fails", async () => {
    const fake = createFakeQuiky({
      startInstall: async () => {
        throw "Le disque est plein.";
      },
    });
    const mounted = mountStore([game()], fake.client);
    await mounted.host.activate(storeRoute());
    await flush();

    installButton(mounted.container).click();
    await flush();

    const button = installButton(mounted.container);
    expect(button.textContent).toBe("Réessayer");
    expect(button.dataset.phase).toBe("failed");
    expect(button.title).toBe("Le disque est plein.");
    expect(mounted.container.querySelector(".store-status__copy")?.textContent).toBe(
      "Le disque est plein.",
    );
  });

  it("releases the plugin subscription when the page goes away", async () => {
    const fake = createFakeQuiky();
    const mounted = mountStore([game()], fake.client);
    await mounted.host.activate(storeRoute());
    await flush();
    expect(fake.calls).toContain("subscribe");

    mounted.host.deactivate();
    expect(fake.teardowns).toEqual(["unsubscribe"]);

    // A progress event arriving after teardown reaches nothing.
    expect(() => fake.emit(progress({ percent: 99 }))).not.toThrow();
  });
});
