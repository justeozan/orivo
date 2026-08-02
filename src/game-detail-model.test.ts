import { describe, expect, it, vi } from "vitest";
import type { AppRoute, GameMediaView, PageRestoreState } from "./contracts";
import type { PageActivation } from "./page-lifecycle";
import {
  createGameDetailPage,
  type GameDetailPageClient,
  type GameDetailPageOptions,
} from "./game-detail-page";
import {
  availableMediaKinds,
  buildStatFacts,
  canApplyMedia,
  createFallbackGameDetail,
  createFallbackWallpaperSearch,
  createInitialGameDetailState,
  defaultMediaKind,
  formatAchievementProgress,
  formatLastPlayed,
  formatPlayTime,
  formatReleaseDate,
  groupMediaByKind,
  heroImageUrl,
  normaliseGameDetail,
  normaliseGameMedia,
  normaliseWallpaperSearch,
  readGameDetailRestoreState,
  reduceGameDetailState,
  resolvePrimaryAction,
  selectActionOffer,
  shouldOfferAboutToggle,
  toGameDetailRestoreState,
  visibleSections,
  type GameDetailPageState,
  type GameDetailViewModel,
} from "./game-detail-model";

const media = (
  id: string,
  kind: GameMediaView["kind"],
  selected = false,
  extra: Partial<GameMediaView> = {},
): GameMediaView => ({
  id,
  kind,
  title: `${kind} ${id}`,
  previewUrl: `/media/${id}.png`,
  origin: "provider",
  selected,
  availableOffline: true,
  ...extra,
});

// A minimal base: the editorial fallback with its social/related showcase and
// rating stripped, so a test opts into that content explicitly via overrides.
function detailWith(overrides: Partial<GameDetailViewModel> = {}): GameDetailViewModel {
  const detail = createFallbackGameDetail("game_1");
  detail.relatedGames = [];
  delete detail.friends;
  delete detail.activity;
  delete detail.rating;
  delete detail.reviewPercent;
  return { ...detail, ...overrides };
}

function readyState(overrides: Partial<GameDetailViewModel> = {}): GameDetailPageState {
  const detail = detailWith(overrides);
  let state = reduceGameDetailState(createInitialGameDetailState(), {
    type: "activate",
    gameId: detail.id,
    from: "library",
    online: true,
    restore: null,
  });
  state = reduceGameDetailState(state, { type: "request-started", requestId: 1 });
  return reduceGameDetailState(state, { type: "detail-loaded", requestId: 1, detail });
}

describe("primary action resolution", () => {
  it("maps every backend action to a contextual button", () => {
    expect(resolvePrimaryAction(detailWith({ primaryAction: "play" }), "game_1")).toMatchObject({
      label: "Play",
      intent: "play",
      disabled: false,
    });
    expect(resolvePrimaryAction(detailWith({ primaryAction: "configure-wine" }), "game_1")).toMatchObject({
      label: "Configure Wine",
      intent: "navigate",
      route: { page: "settings", section: "plugins", attachGameId: "game_1" },
    });
    expect(resolvePrimaryAction(detailWith({ primaryAction: "unavailable" }), "game_1")).toMatchObject({
      label: "Unavailable",
      intent: "none",
      disabled: true,
    });
  });

  it("disables offer actions when no offer exists and enables them when one does", () => {
    expect(resolvePrimaryAction(detailWith({ primaryAction: "view-offer" }), "game_1")).toMatchObject({
      intent: "none",
      disabled: true,
      offerId: null,
    });
    const withOffer = detailWith({
      primaryAction: "install-steam",
      offers: [
        {
          id: "offer_ms",
          gameId: "game_1",
          provider: "microsoft",
          providerLabel: "Microsoft",
          priceMinor: 1_000,
          currency: "USD",
          region: "us",
          verifiedAt: null,
          availability: "available",
          stale: false,
        },
        {
          id: "offer_steam",
          gameId: "game_1",
          provider: "steam",
          providerLabel: "Steam",
          priceMinor: 5_999,
          currency: "USD",
          region: "us",
          verifiedAt: null,
          availability: "available",
          stale: false,
        },
      ],
    });
    // Steam wins on provider preference even though it is the pricier offer.
    expect(selectActionOffer(withOffer, "steam")?.id).toBe("offer_steam");
    expect(resolvePrimaryAction(withOffer, "game_1")).toMatchObject({
      label: "Install via Steam",
      intent: "open-offer",
      offerId: "offer_steam",
      disabled: false,
    });
  });

  it("falls back to unavailable when there is no detail yet", () => {
    expect(resolvePrimaryAction(null, "game_1").disabled).toBe(true);
    expect(resolvePrimaryAction(detailWith(), null).disabled).toBe(true);
  });
});

describe("media grouping and preview", () => {
  const items = [
    media("w1", "wallpaper", true),
    media("w2", "wallpaper"),
    media("v1", "video", true, { posterUrl: "/poster.png" }),
    media("c1", "cover", true),
  ];

  it("groups media by kind and lists only kinds with content", () => {
    const groups = groupMediaByKind(items);
    expect(groups.wallpaper.map((item) => item.id)).toEqual(["w1", "w2"]);
    expect(groups.icon).toEqual([]);
    expect(availableMediaKinds(items)).toEqual(["wallpaper", "video", "cover"]);
    expect(defaultMediaKind([media("i1", "icon")])).toBe("icon");
    expect(defaultMediaKind([])).toBe("wallpaper");
  });

  it("treats a thumbnail click as preview only and keeps Apply gated on a change", () => {
    let state = readyState({ media: items });
    expect(state.previewMediaId).toBe("w1");
    expect(state.appliedMediaId).toBe("w1");
    expect(canApplyMedia(state)).toBe(false);

    state = reduceGameDetailState(state, { type: "media-previewed", mediaId: "w2" });
    expect(state.previewMediaId).toBe("w2");
    // Nothing was persisted: the applied selection is untouched.
    expect(state.appliedMediaId).toBe("w1");
    expect(canApplyMedia(state)).toBe(true);
    expect(heroImageUrl(state)).toBe("/media/w2.png");
  });

  it("keeps the previous selection and surfaces an inline error when Apply fails", () => {
    let state = readyState({ media: items });
    state = reduceGameDetailState(state, { type: "media-previewed", mediaId: "w2" });
    state = reduceGameDetailState(state, { type: "media-busy-changed", busy: true });
    state = reduceGameDetailState(state, { type: "media-failed", message: "Disk is full." });
    expect(state.previewMediaId).toBe("w1");
    expect(state.appliedMediaId).toBe("w1");
    expect(state.mediaBusy).toBe(false);
    expect(state.mediaError).toBe("Disk is full.");
  });

  it("adopts the backend media list after a successful Apply", () => {
    let state = readyState({ media: items });
    state = reduceGameDetailState(state, { type: "media-previewed", mediaId: "w2" });
    state = reduceGameDetailState(state, {
      type: "media-committed",
      media: [media("w1", "wallpaper"), media("w2", "wallpaper", true)],
    });
    expect(state.previewMediaId).toBe("w2");
    expect(state.appliedMediaId).toBe("w2");
    expect(state.mediaError).toBe("");
    expect(state.detail?.media.map((item) => item.id)).toEqual(["w1", "w2"]);
  });

  it("switching kind resets the preview to that kind's applied item", () => {
    let state = readyState({ media: items });
    state = reduceGameDetailState(state, { type: "media-kind-changed", kind: "cover" });
    expect(state.activeMediaKind).toBe("cover");
    expect(state.previewMediaId).toBe("c1");
    expect(canApplyMedia(state)).toBe(false);
  });

  it("uses the video poster for the hero rather than the video stream", () => {
    let state = readyState({ media: items });
    state = reduceGameDetailState(state, { type: "media-previewed", mediaId: "v1" });
    expect(state.activeMediaKind).toBe("video");
    expect(heroImageUrl(state)).toBe("/poster.png");
  });
});

describe("state reducer", () => {
  it("discards a response whose request id is no longer current", () => {
    let state = reduceGameDetailState(createInitialGameDetailState(), {
      type: "activate",
      gameId: "game_1",
      from: null,
      online: true,
      restore: null,
    });
    state = reduceGameDetailState(state, { type: "request-started", requestId: 1 });
    state = reduceGameDetailState(state, { type: "request-started", requestId: 2 });
    const stale = reduceGameDetailState(state, {
      type: "detail-loaded",
      requestId: 1,
      detail: detailWith({ title: "Stale game" }),
    });
    expect(stale.detail).toBeNull();
    const fresh = reduceGameDetailState(state, {
      type: "detail-loaded",
      requestId: 2,
      detail: detailWith({ title: "Fresh game" }),
    });
    expect(fresh.detail?.title).toBe("Fresh game");
    expect(fresh.phase).toBe("ready");
  });

  it("reports not-found without throwing when the backend returns nothing", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "request-started", requestId: 9 });
    state = reduceGameDetailState(state, { type: "detail-loaded", requestId: 9, detail: null });
    expect(state.phase).toBe("not-found");
    expect(state.detail).toBeNull();
    expect(state.media).toEqual([]);
  });

  it("keeps rendering the loaded game when a later refresh fails", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "request-started", requestId: 4 });
    state = reduceGameDetailState(state, {
      type: "request-failed",
      requestId: 4,
      message: "Network unreachable.",
      offline: false,
    });
    expect(state.phase).toBe("ready");
    expect(state.detail).not.toBeNull();
    expect(state.errorMessage).toBe("Network unreachable.");
  });

  it("marks the page offline and clears the notice when connectivity returns", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "connectivity-changed", online: false });
    expect(state.phase).toBe("offline");
    expect(state.errorMessage).not.toBe("");
    state = reduceGameDetailState(state, { type: "connectivity-changed", online: true });
    expect(state.phase).toBe("ready");
    expect(state.errorMessage).toBe("");
  });

  it("clears a stuck busy flag and wipes media errors on re-activation", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "media-busy-changed", busy: true });
    state = reduceGameDetailState(state, { type: "media-failed", message: "boom" });
    state = reduceGameDetailState(state, {
      type: "activate",
      gameId: "game_1",
      from: "store",
      online: true,
      restore: null,
    });
    expect(state.mediaBusy).toBe(false);
    expect(state.mediaError).toBe("");
    expect(state.from).toBe("store");
  });

  it("toggles wishlist and about without touching anything else", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wishlist-changed", wishlisted: true });
    expect(state.detail?.wishlisted).toBe(true);
    state = reduceGameDetailState(state, { type: "about-toggled" });
    expect(state.aboutExpanded).toBe(true);
  });
});

describe("wallpaper search state", () => {
  it("opens prefilled with the game title and closes again", () => {
    let state = readyState();
    expect(state.wallpaperSearch.open).toBe(false);

    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.open).toBe(true);
    expect(state.wallpaperSearch.query).toBe("Elden Ring");

    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    expect(state.wallpaperSearch.open).toBe(false);
  });

  it("keeps a query the user already typed when reopening", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-query-changed", query: "souls" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.query).toBe("souls");
  });

  it("switches the source without clobbering the rest of the search", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-source-changed", source: "google-images" });
    expect(state.wallpaperSearch.source).toBe("google-images");
  });

  it("tracks a running search and adopts the backend result", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-started" });
    expect(state.wallpaperSearch.busy).toBe(true);
    expect(state.wallpaperSearch.phase).toBe("idle");
    // A fresh search restarts at zero.
    expect(state.wallpaperSearch.offset).toBe(0);

    state = reduceGameDetailState(state, {
      type: "wallpaper-search-results",
      results: {
        phase: "ready",
        source: "igdb",
        query: "elden ring",
        message: "",
        candidates: [{ id: "c1", title: "Key art", thumbnailUrl: "/a.png" }],
      },
    });
    expect(state.wallpaperSearch.busy).toBe(false);
    expect(state.wallpaperSearch.phase).toBe("ready");
    expect(state.wallpaperSearch.candidates.map((candidate) => candidate.id)).toEqual(["c1"]);
    expect(state.wallpaperSearch.activeId).toBe("c1");
    expect(state.wallpaperSearch.offset).toBe(1);
    expect(state.wallpaperSearch.hasMore).toBe(true);
  });

  it("merges results when searching more and stops when the source runs dry", () => {
    const candidate = (id: string) => ({ id, title: id, thumbnailUrl: "/a.png" });
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-started" });
    state = reduceGameDetailState(state, {
      type: "wallpaper-search-results",
      results: {
        phase: "ready",
        source: "igdb",
        query: "elden ring",
        message: "",
        candidates: ["c1", "c2", "c3", "c4"].map(candidate),
      },
    });
    expect(state.wallpaperSearch.offset).toBe(4);
    expect(state.wallpaperSearch.activeId).toBe("c1");

    // "Search more" keeps the page offset and the slide the user is on.
    state = reduceGameDetailState(state, { type: "wallpaper-search-started", more: true });
    expect(state.wallpaperSearch.offset).toBe(4);
    state = reduceGameDetailState(state, {
      type: "wallpaper-search-results",
      results: {
        phase: "ready",
        source: "igdb",
        query: "elden ring",
        message: "",
        candidates: ["c5", "c6"].map(candidate),
      },
    });
    expect(state.wallpaperSearch.candidates.map((candidate) => candidate.id)).toEqual([
      "c1", "c2", "c3", "c4", "c5", "c6",
    ]);
    expect(state.wallpaperSearch.offset).toBe(6);
    expect(state.wallpaperSearch.activeId).toBe("c1");
    expect(state.wallpaperSearch.hasMore).toBe(true);

    // The source has nothing else: the merged list stays, hasMore flips off.
    state = reduceGameDetailState(state, { type: "wallpaper-search-started", more: true });
    state = reduceGameDetailState(state, {
      type: "wallpaper-search-results",
      results: {
        phase: "ready",
        source: "igdb",
        query: "elden ring",
        message: "",
        candidates: [],
      },
    });
    expect(state.wallpaperSearch.candidates.length).toBe(6);
    expect(state.wallpaperSearch.hasMore).toBe(false);
  });

  it("opens on the wallpaper being previewed, else the first wallpaper", () => {
    let state = readyState({
      media: [media("w1", "wallpaper", true), media("w2", "wallpaper")],
    });
    state = reduceGameDetailState(state, { type: "media-previewed", mediaId: "w2" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.activeId).toBe("w2");

    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.activeId).toBe("w2");
  });

  it("reopens on a wallpaper previewed in the rail, not a stale search candidate", () => {
    let state = readyState({
      media: [media("w1", "wallpaper", true), media("w2", "wallpaper")],
    });
    // A search left the last result active.
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-started" });
    state = reduceGameDetailState(state, {
      type: "wallpaper-search-results",
      results: {
        phase: "ready",
        source: "steam-store",
        query: "elden ring",
        message: "",
        candidates: [{ id: "c1", title: "Key art", thumbnailUrl: "/a.png" }],
      },
    });
    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    expect(state.wallpaperSearch.activeId).toBe("c1");
    // The user then previewed a downloaded wallpaper; the dialog must open on it.
    state = reduceGameDetailState(state, { type: "media-previewed", mediaId: "w2" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.activeId).toBe("w2");
  });

  it("lets the slide controls move the active wallpaper without searching", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    const slides = state.detail?.media
      .filter((item) => item.kind === "wallpaper")
      .map((item) => item.id) ?? [];
    expect(slides.length).toBeGreaterThan(1);
    state = reduceGameDetailState(state, {
      type: "wallpaper-slide-changed",
      slideId: slides[1],
    });
    expect(state.wallpaperSearch.activeId).toBe(slides[1]);
  });

  it("surfaces a failure and clears the busy flag", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-started" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-failed", message: "nope" });
    expect(state.wallpaperSearch.busy).toBe(false);
    expect(state.wallpaperSearch.phase).toBe("error");
    expect(state.wallpaperSearch.message).toBe("nope");
  });
});

describe("restore state", () => {
  it("round-trips scroll, focus and the selected media", () => {
    let state = readyState({ media: [media("w1", "wallpaper", true), media("c1", "cover", true)] });
    state = reduceGameDetailState(state, { type: "media-previewed", mediaId: "c1" });
    const restore = toGameDetailRestoreState(state, 412.6, "media-apply");
    expect(restore).toEqual({
      scrollTop: 413,
      focusKey: "media-apply",
      filters: ["kind:cover", "media:c1"],
      selectedGameId: "game_1",
    });
    expect(readGameDetailRestoreState(restore)).toEqual({
      scrollTop: 413,
      focusKey: "media-apply",
      selectedGameId: "game_1",
      activeMediaKind: "cover",
      previewMediaId: "c1",
    });
  });

  it("reapplies the saved media once the detail for the same game arrives", () => {
    const restore: PageRestoreState = {
      scrollTop: 120,
      focusKey: "wishlist",
      selectedGameId: "game_1",
      filters: ["kind:cover", "media:c1"],
    };
    let state = reduceGameDetailState(createInitialGameDetailState(), {
      type: "activate",
      gameId: "game_1",
      from: null,
      online: true,
      restore,
    });
    state = reduceGameDetailState(state, { type: "request-started", requestId: 1 });
    state = reduceGameDetailState(state, {
      type: "detail-loaded",
      requestId: 1,
      detail: detailWith({ media: [media("w1", "wallpaper", true), media("c1", "cover", true)] }),
    });
    expect(state.activeMediaKind).toBe("cover");
    expect(state.previewMediaId).toBe("c1");
    expect(state.pendingRestore).toBeNull();
  });

  it("ignores restore state that belongs to another game", () => {
    const restore: PageRestoreState = {
      scrollTop: 10,
      focusKey: null,
      selectedGameId: "game_other",
      filters: ["kind:cover", "media:c1"],
    };
    let state = reduceGameDetailState(createInitialGameDetailState(), {
      type: "activate",
      gameId: "game_1",
      from: null,
      online: true,
      restore,
    });
    state = reduceGameDetailState(state, { type: "request-started", requestId: 1 });
    state = reduceGameDetailState(state, {
      type: "detail-loaded",
      requestId: 1,
      detail: detailWith({ media: [media("w1", "wallpaper", true), media("c1", "cover", true)] }),
    });
    expect(state.activeMediaKind).toBe("wallpaper");
    expect(state.previewMediaId).toBe("w1");
  });

  it("tolerates missing or malformed restore state", () => {
    expect(readGameDetailRestoreState(null)).toEqual({
      scrollTop: 0,
      focusKey: null,
      selectedGameId: null,
      activeMediaKind: null,
      previewMediaId: null,
    });
    expect(
      readGameDetailRestoreState({ scrollTop: -5, focusKey: null, filters: ["kind:nope"] }),
    ).toMatchObject({ scrollTop: 0, activeMediaKind: null, previewMediaId: null });
  });
});

describe("section predicates", () => {
  it("only lists sections that have real content", () => {
    expect(
      visibleSections(
        detailWith({
          about: "",
          features: [],
          achievements: null,
          media: [],
          relatedGames: [],
          developer: null,
          publisher: null,
          releaseDate: null,
          genres: [],
          supportedPlatforms: [],
        }),
      ),
    ).toEqual([]);
    expect(visibleSections(detailWith())).toEqual([
      "gallery",
      "about",
      "info",
      "features",
      "achievements",
    ]);
  });

  it("never renders friends or activity without real data", () => {
    expect(visibleSections(detailWith()).includes("friends")).toBe(false);
    expect(visibleSections(detailWith({ friends: [] })).includes("friends")).toBe(false);
    expect(
      visibleSections(
        detailWith({
          friends: [{ id: "f1", name: "Valkyrie", avatarUrl: "", status: "Online" }],
          activity: [
            {
              id: "a1",
              actorName: "PixelNinja",
              summary: "Reached 100 hours played",
              detail: "",
              avatarUrl: "",
              occurredAt: null,
            },
          ],
        }),
      ),
    ).toEqual(expect.arrayContaining(["friends", "activity"]));
  });

  it("offers a Read more toggle only for long copy", () => {
    expect(shouldOfferAboutToggle("Short blurb.")).toBe(false);
    expect(shouldOfferAboutToggle("x".repeat(300))).toBe(true);
    expect(shouldOfferAboutToggle("a\nb\nc\nd")).toBe(true);
  });
});

describe("normalisation", () => {
  it("keeps a partial payload renderable instead of throwing", () => {
    const detail = normaliseGameDetail({
      id: "game_2",
      title: "Partial",
      features: ["Single-player", 42, ""],
      achievements: { unlocked: 5 },
      media: null,
      relatedGames: "nope",
      primaryAction: "teleport",
    });
    expect(detail).toMatchObject({
      id: "game_2",
      title: "Partial",
      about: "",
      features: ["Single-player"],
      achievements: null,
      media: [],
      relatedGames: [],
      primaryAction: "unavailable",
    });
  });

  it("rejects payloads without an opaque id", () => {
    expect(normaliseGameDetail({ title: "No id" })).toBeNull();
    expect(normaliseGameDetail(null)).toBeNull();
    expect(normaliseGameDetail("game_1")).toBeNull();
  });

  it("drops media entries with an unknown kind, no url or a duplicate id", () => {
    expect(
      normaliseGameMedia([
        { id: "a", kind: "wallpaper", previewUrl: "/a.png", title: "A" },
        { id: "a", kind: "wallpaper", previewUrl: "/a2.png" },
        { id: "b", kind: "sticker", previewUrl: "/b.png" },
        { id: "c", kind: "cover" },
        null,
      ]).map((item) => item.id),
    ).toEqual(["a"]);
  });

  it("keeps only well-formed wallpaper search results", () => {
    expect(
      normaliseWallpaperSearch({
        phase: "ready",
        source: "google-images",
        query: " elden ring ",
        message: "10 results",
        candidates: [
          { id: "c1", title: "Key art", thumbnailUrl: "https://x.test/a.png" },
          { id: "c1", title: "Duplicate", thumbnailUrl: "https://x.test/b.png" },
          { id: "c2", thumbnailUrl: "https://x.test/c.png" },
          { id: "c3", title: "No thumb" },
          "junk",
        ],
      }),
    ).toEqual({
      phase: "ready",
      source: "google-images",
      query: " elden ring ",
      message: "10 results",
      candidates: [
        { id: "c1", title: "Key art", thumbnailUrl: "https://x.test/a.png" },
        { id: "c2", title: "Wallpaper", thumbnailUrl: "https://x.test/c.png" },
      ],
    });
  });

  it("falls back to a safe shape for a malformed search payload", () => {
    expect(normaliseWallpaperSearch(null)).toEqual({
      phase: "error",
      source: "steam-store",
      query: "",
      message: "",
      candidates: [],
    });
    expect(
      normaliseWallpaperSearch({ phase: "mystery", source: "nope", candidates: "x" }),
    ).toMatchObject({ phase: "error", source: "steam-store", candidates: [] });
  });
});

describe("formatting", () => {
  it("formats play time and last-played facts", () => {
    expect(formatPlayTime(0)).toBe("Not played yet");
    expect(formatPlayTime(1_800)).toBe("Played 30 min");
    expect(formatPlayTime(460_800)).toBe("Played 128h");
    const now = Date.parse("2026-08-01T12:00:00Z");
    expect(formatLastPlayed(null, now)).toBe("Never played");
    expect(formatLastPlayed("2026-07-30T12:00:00Z", now)).toBe("Last played 2 days ago");
    expect(formatLastPlayed("2026-08-01T09:00:00Z", now)).toBe("Last played 3h ago");
  });

  it("formats release dates and achievement progress", () => {
    expect(formatReleaseDate("2022-02-25")).toBe("February 25, 2022");
    expect(formatReleaseDate("not a date")).toBe("not a date");
    expect(formatAchievementProgress({ unlocked: 67, total: 82, items: [] })).toEqual({
      label: "67/82 unlocked",
      percent: 82,
    });
    expect(formatAchievementProgress(null)).toBeNull();
  });

  it("omits the achievements stat when the game has none", () => {
    const facts = buildStatFacts(detailWith({ achievements: null }), Date.now());
    expect(facts.map((fact) => fact.id)).toEqual(["playtime", "last-played"]);
  });
});

/* -------------------------------------------------------------------------- */
/* Page lifecycle (DOM)                                                        */
/* -------------------------------------------------------------------------- */

function stubClient(overrides: Partial<GameDetailPageClient> = {}): GameDetailPageClient {
  return {
    getDetail: async () => detailWith({ media: [media("w1", "wallpaper", true), media("w2", "wallpaper")] }),
    setWishlist: async () => undefined,
    selectMedia: async () => [media("w1", "wallpaper"), media("w2", "wallpaper", true)],
    importMedia: async () => [],
    exportMedia: async () => undefined,
    cancelMediaDownload: async () => undefined,
    searchWallpapers: async () => createFallbackWallpaperSearch("igdb", ""),
    importWallpaper: async () => [],
    openOffer: async () => undefined,
    searchArtwork: async () => undefined,
    removeGame: async () => undefined,
    setHomeImage: async () => undefined,
    ...overrides,
  };
}

function activationFor(
  route: AppRoute,
  options: { isCurrent?: () => boolean; restoreState?: PageRestoreState | null } = {},
): { activation: PageActivation; controller: AbortController } {
  const controller = new AbortController();
  return {
    controller,
    activation: {
      route,
      signal: controller.signal,
      restoreState: options.restoreState ?? null,
      isCurrent: options.isCurrent ?? (() => !controller.signal.aborted),
    },
  };
}

const gameRoute: AppRoute = { page: "game", gameId: "game_1", from: "library" };

function mountPage(
  client: GameDetailPageClient,
  overrides: Partial<GameDetailPageOptions> = {},
): { page: ReturnType<typeof createGameDetailPage>; host: HTMLElement; options: GameDetailPageOptions } {
  const host = document.createElement("div");
  document.body.replaceChildren(host);
  const options: GameDetailPageOptions = {
    navigate: vi.fn(),
    back: vi.fn(),
    play: vi.fn(),
    client,
    ...overrides,
  };
  const page = createGameDetailPage(options);
  page.mount(host);
  return { page, host, options };
}

describe("game detail page lifecycle", () => {
  it("mounts, paints the loaded game and never throws into the shell", async () => {
    const { page, host } = mountPage(stubClient());
    const { activation } = activationFor(gameRoute);
    await page.activate(activation);
    expect(host.querySelector(".gd-hero__title")?.textContent).toBe("Elden Ring");
    expect(host.querySelector(".gd-primary-action")?.textContent).toContain("Play");
    expect(host.querySelectorAll(".gd-gallery__tile:not(.gd-gallery__tile--empty)").length).toBe(2);
    expect(host.querySelector("[data-focus-key='wallpaper-search-toggle']")).not.toBeNull();
  });

  it("discards a late response once the activation is no longer current", async () => {
    let current = true;
    const { page, host } = mountPage(
      stubClient({
        getDetail: async () => {
          current = false;
          return detailWith({ title: "Stale paint" });
        },
      }),
    );
    const { activation } = activationFor(gameRoute, { isCurrent: () => current });
    await page.activate(activation);
    expect(host.textContent).not.toContain("Stale paint");
    expect(host.querySelector(".gd-skeleton")).not.toBeNull();
  });

  it("discards a response that arrives after the signal was aborted", async () => {
    const { page, host } = mountPage(stubClient());
    const { activation, controller } = activationFor(gameRoute);
    const pending = page.activate(activation);
    controller.abort();
    await pending;
    expect(host.querySelector(".gd-hero__title")).toBeNull();
  });

  it("renders a not-found notice rather than a broken page", async () => {
    const { page, host } = mountPage(
      stubClient({ getDetail: async () => null as never }),
    );
    await page.activate(activationFor(gameRoute).activation);
    expect(host.querySelector(".gd-notice__title")?.textContent).toBe("Game not found");
  });

  it("survives a rejected detail request and offers a retry", async () => {
    const { page, host } = mountPage(
      stubClient({
        getDetail: async () => {
          throw new Error("backend exploded");
        },
      }),
    );
    await page.activate(activationFor(gameRoute).activation);
    expect(host.textContent).toContain("backend exploded");
    expect(host.querySelector("[data-focus-key='notice-retry']")).not.toBeNull();
  });

  it("captures and restores scroll, focus and the previewed media", async () => {
    const { page, host } = mountPage(stubClient());
    await page.activate(activationFor(gameRoute).activation);
    host.querySelector<HTMLButtonElement>("[data-focus-key='media-w2']")?.click();
    host.querySelector<HTMLButtonElement>("[data-focus-key='more-actions']")?.focus();
    const restore = page.deactivate();
    expect(restore).toMatchObject({
      focusKey: "more-actions",
      selectedGameId: "game_1",
      filters: ["kind:wallpaper", "media:w2"],
    });

    await page.activate(activationFor(gameRoute, { restoreState: restore }).activation);
    expect(
      host.querySelector("[data-focus-key='media-w2']")?.classList.contains("gd-gallery__tile--selected"),
    ).toBe(true);
  });

  it("chooses a rail wallpaper as the home background and commits it on click", async () => {
    const selectMedia = vi.fn(async () => [media("w1", "wallpaper"), media("w2", "wallpaper", true)]);
    const setHomeImage = vi.fn(async () => undefined);
    const { page, host } = mountPage(stubClient({ selectMedia, setHomeImage }));
    await page.activate(activationFor(gameRoute).activation);

    // Clicking a rail wallpaper chooses it as the home (Library) background: it
    // persists the selection and promotes the media, with opaque ids only.
    host.querySelector<HTMLButtonElement>("[data-focus-key='media-w2']")?.click();
    await vi.waitFor(() => expect(selectMedia).toHaveBeenCalledWith("game_1", "w2", expect.anything()));
    await vi.waitFor(() =>
      expect(setHomeImage).toHaveBeenCalledWith("game_1", "w2", "background", expect.anything()),
    );
  });

  it("commits and promotes a ticked wallpaper as the home background", async () => {
    const selectMedia = vi.fn(async () => [media("w1", "wallpaper"), media("w2", "wallpaper", true)]);
    const setHomeImage = vi.fn(async () => undefined);
    const { page, host } = mountPage(stubClient({ selectMedia, setHomeImage }));
    await page.activate(activationFor(gameRoute).activation);

    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-search-toggle']")?.click();
    // Tick an existing wallpaper in the grid, then apply it.
    host.querySelector<HTMLButtonElement>("[data-focus-key='wall-w2']")?.click();
    const apply = host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']");
    expect(apply?.disabled).toBe(false);
    apply?.click();
    await vi.waitFor(() => expect(selectMedia).toHaveBeenCalledWith("game_1", "w2", expect.anything()));
    await vi.waitFor(() =>
      expect(setHomeImage).toHaveBeenCalledWith("game_1", "w2", "background", expect.anything()),
    );
    await vi.waitFor(() => expect(host.querySelector(".gd-modal")).toBeNull());
  });

  it("applies a single wallpaper to the chosen card role", async () => {
    const selectMedia = vi.fn(async () => [media("w1", "wallpaper"), media("w2", "wallpaper", true)]);
    const setHomeImage = vi.fn(async () => undefined);
    const { page, host } = mountPage(stubClient({ selectMedia, setHomeImage }));
    await page.activate(activationFor(gameRoute).activation);

    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-search-toggle']")?.click();
    host.querySelector<HTMLButtonElement>("[data-focus-key='wall-w2']")?.click();
    // Target the landscape card instead of the default background.
    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-role-landscape']")?.click();
    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']")?.click();
    await vi.waitFor(() =>
      expect(setHomeImage).toHaveBeenCalledWith("game_1", "w2", "landscape", expect.anything()),
    );
  });

  it("shows an inline error and keeps the modal open when applying fails", async () => {
    const { page, host } = mountPage(
      stubClient({
        selectMedia: async () => {
          throw new Error("Media store is read-only.");
        },
      }),
    );
    await page.activate(activationFor(gameRoute).activation);
    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-search-toggle']")?.click();
    host.querySelector<HTMLButtonElement>("[data-focus-key='wall-w2']")?.click();
    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']")?.click();
    await vi.waitFor(() =>
      expect(host.querySelector(".gd-modal__error")?.textContent).toContain("read-only"),
    );
    expect(host.querySelector(".gd-modal")).not.toBeNull();
  });

  it("routes the contextual primary action through shell-owned callbacks", async () => {
    const play = vi.fn();
    const { page, host } = mountPage(stubClient(), { play });
    await page.activate(activationFor(gameRoute).activation);
    host.querySelector<HTMLButtonElement>("[data-focus-key='primary-action']")?.click();
    expect(play).toHaveBeenCalledWith("game_1");

    const navigate = vi.fn();
    const wine = mountPage(
      stubClient({ getDetail: async () => detailWith({ primaryAction: "configure-wine" }) }),
      { navigate },
    );
    await wine.page.activate(activationFor(gameRoute).activation);
    wine.host.querySelector<HTMLButtonElement>("[data-focus-key='primary-action']")?.click();
    expect(navigate).toHaveBeenCalledWith({
      page: "settings",
      section: "plugins",
      attachGameId: "game_1",
    });
  });

  it("opens an offer for view-offer and hands back to the shell", async () => {
    const openOffer = vi.fn(async () => undefined);
    const { page, host } = mountPage(
      stubClient({
        openOffer,
        getDetail: async () =>
          detailWith({
            primaryAction: "view-offer",
            offers: [
              {
                id: "offer_1",
                gameId: "game_1",
                provider: "steam",
                providerLabel: "Steam",
                priceMinor: 1_999,
                currency: "USD",
                region: "us",
                verifiedAt: null,
                availability: "available",
                stale: false,
              },
            ],
          }),
      }),
    );
    await page.activate(activationFor(gameRoute).activation);
    host.querySelector<HTMLButtonElement>("[data-focus-key='primary-action']")?.click();
    await vi.waitFor(() => expect(openOffer).toHaveBeenCalledTimes(1));
    expect(openOffer).toHaveBeenCalledWith("offer_1", expect.anything());
  });

  it("never renders empty friends or activity placeholders", async () => {
    const { page, host } = mountPage(stubClient());
    await page.activate(activationFor(gameRoute).activation);
    expect(host.querySelector(".gd-friends")).toBeNull();
    expect(host.querySelector(".gd-activity")).toBeNull();
    expect(host.querySelector(".gd-related")).toBeNull();
  });

  it("renders a game with no media at all without breaking", async () => {
    const { page, host } = mountPage(
      stubClient({ getDetail: async () => detailWith({ media: [] }) }),
    );
    await page.activate(activationFor(gameRoute).activation);
    expect(host.querySelector(".gd-hero__title")?.textContent).toBe("Elden Ring");
    expect(host.querySelector(".gd-modal")).toBeNull();
    // The rail still offers a way to add a wallpaper even with no media yet.
    expect(host.querySelector(".gd-gallery--empty")).not.toBeNull();
    expect(host.querySelectorAll(".gd-gallery__tile")).toHaveLength(0);
    expect(host.querySelector(".gd-gallery__add")).not.toBeNull();
  });

  it("downloads a ticked search result and promotes it to the background", async () => {
    const searchWallpapers = vi.fn(async () => ({
      phase: "ready" as const,
      source: "steam-store" as const,
      query: "elden ring",
      message: "",
      candidates: [{ id: "c1", title: "Key art", thumbnailUrl: "/a.png" }],
    }));
    const importWallpaper = vi.fn(async () => [media("w9", "wallpaper", true)]);
    const selectMedia = vi.fn(async () => [media("w9", "wallpaper", true)]);
    const setHomeImage = vi.fn(async () => undefined);
    const { page, host } = mountPage(
      stubClient({ searchWallpapers, importWallpaper, selectMedia, setHomeImage }),
    );
    await page.activate(activationFor(gameRoute).activation);

    // The search toggle only exists on the wallpaper slot; opening it searches.
    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-search-toggle']")?.click();
    await vi.waitFor(() => expect(searchWallpapers).toHaveBeenCalled());
    // The candidate lands as a grid tile; tick it and apply.
    await vi.waitFor(() =>
      expect(host.querySelector<HTMLButtonElement>("[data-focus-key='wall-c1']")).not.toBeNull(),
    );
    host.querySelector<HTMLButtonElement>("[data-focus-key='wall-c1']")?.click();
    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']")?.click();

    await vi.waitFor(() =>
      expect(importWallpaper).toHaveBeenCalledWith("game_1", "c1", expect.anything()),
    );
    // The downloaded wallpaper is promoted to the home background, and the
    // dialog closes.
    await vi.waitFor(() =>
      expect(setHomeImage).toHaveBeenCalledWith("game_1", "w9", "background", expect.anything()),
    );
    await vi.waitFor(() => expect(host.querySelector(".gd-modal")).toBeNull());
  });

  it("shows a not-configured notice instead of a broken search", async () => {
    const { page, host } = mountPage(
      stubClient({
        searchWallpapers: async () => ({
          phase: "not-configured",
          source: "igdb",
          query: "elden ring",
          message: "IGDB needs a client id and secret.",
          candidates: [],
        }),
      }),
    );
    await page.activate(activationFor(gameRoute).activation);
    host.querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-search-toggle']")?.click();
    host.querySelector<HTMLFormElement>(".gd-search__form")?.requestSubmit();
    await vi.waitFor(() =>
      expect(host.querySelector(".gd-search__notice")?.textContent).toContain("client id and secret"),
    );
    expect(host.querySelector(".gd-search__status")).toBeNull();
  });
});
