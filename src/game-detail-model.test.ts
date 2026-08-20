import { describe, expect, it, vi } from "vitest";
import type {
  AppRoute,
  GameMediaView,
  PageRestoreState,
  WallpaperCandidateView,
  WallpaperCategory,
  WallpaperSearchView,
} from "./contracts";
import { WALLPAPER_CATEGORIES } from "./contracts";
import type { PageActivation } from "./page-lifecycle";
import {
  createGameDetailPage,
  type GameDetailPageClient,
  type GameDetailPageOptions,
  type WallpaperRole,
} from "./game-detail-page";
import {
  allWallpaperCandidates,
  availableMediaKinds,
  buildStatFacts,
  canApplyMedia,
  createFallbackGameDetail,
  createFallbackWallpaperSearch,
  createInitialGameDetailState,
  createWallpaperCategoryState,
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
  statusChips,
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
function detailWith(
  overrides: Partial<GameDetailViewModel> = {},
): GameDetailViewModel {
  const detail = createFallbackGameDetail("game_1");
  detail.relatedGames = [];
  delete detail.friends;
  delete detail.activity;
  delete detail.rating;
  delete detail.reviewPercent;
  return { ...detail, ...overrides };
}

function readyState(
  overrides: Partial<GameDetailViewModel> = {},
): GameDetailPageState {
  const detail = detailWith(overrides);
  let state = reduceGameDetailState(createInitialGameDetailState(), {
    type: "activate",
    gameId: detail.id,
    from: "library",
    online: true,
    restore: null,
  });
  state = reduceGameDetailState(state, {
    type: "request-started",
    requestId: 1,
  });
  return reduceGameDetailState(state, {
    type: "detail-loaded",
    requestId: 1,
    detail,
  });
}

describe("primary action resolution", () => {
  it("maps every backend action to a contextual button", () => {
    expect(
      resolvePrimaryAction(detailWith({ primaryAction: "play" }), "game_1"),
    ).toMatchObject({
      label: "Play",
      intent: "play",
      disabled: false,
    });
    expect(
      resolvePrimaryAction(
        detailWith({ primaryAction: "configure-wine" }),
        "game_1",
      ),
    ).toMatchObject({
      label: "Configure Wine",
      intent: "navigate",
      route: { page: "settings", section: "plugins", attachGameId: "game_1" },
    });
    expect(
      resolvePrimaryAction(
        detailWith({ primaryAction: "unavailable" }),
        "game_1",
      ),
    ).toMatchObject({
      label: "Unavailable",
      intent: "none",
      disabled: true,
    });
  });

  it("disables offer actions when no offer exists and enables them when one does", () => {
    expect(
      resolvePrimaryAction(
        detailWith({ primaryAction: "view-offer" }),
        "game_1",
      ),
    ).toMatchObject({
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

    state = reduceGameDetailState(state, {
      type: "media-previewed",
      mediaId: "w2",
    });
    expect(state.previewMediaId).toBe("w2");
    // Nothing was persisted: the applied selection is untouched.
    expect(state.appliedMediaId).toBe("w1");
    expect(canApplyMedia(state)).toBe(true);
    expect(heroImageUrl(state)).toBe("/media/w2.png");
  });

  it("keeps the previous selection and surfaces an inline error when Apply fails", () => {
    let state = readyState({ media: items });
    state = reduceGameDetailState(state, {
      type: "media-previewed",
      mediaId: "w2",
    });
    state = reduceGameDetailState(state, {
      type: "media-busy-changed",
      busy: true,
    });
    state = reduceGameDetailState(state, {
      type: "media-failed",
      message: "Disk is full.",
    });
    expect(state.previewMediaId).toBe("w1");
    expect(state.appliedMediaId).toBe("w1");
    expect(state.mediaBusy).toBe(false);
    expect(state.mediaError).toBe("Disk is full.");
  });

  it("adopts the backend media list after a successful Apply", () => {
    let state = readyState({ media: items });
    state = reduceGameDetailState(state, {
      type: "media-previewed",
      mediaId: "w2",
    });
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
    state = reduceGameDetailState(state, {
      type: "media-kind-changed",
      kind: "cover",
    });
    expect(state.activeMediaKind).toBe("cover");
    expect(state.previewMediaId).toBe("c1");
    expect(canApplyMedia(state)).toBe(false);
  });

  it("uses the video poster for the hero rather than the video stream", () => {
    let state = readyState({ media: items });
    state = reduceGameDetailState(state, {
      type: "media-previewed",
      mediaId: "v1",
    });
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
    state = reduceGameDetailState(state, {
      type: "request-started",
      requestId: 1,
    });
    state = reduceGameDetailState(state, {
      type: "request-started",
      requestId: 2,
    });
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
    state = reduceGameDetailState(state, {
      type: "request-started",
      requestId: 9,
    });
    state = reduceGameDetailState(state, {
      type: "detail-loaded",
      requestId: 9,
      detail: null,
    });
    expect(state.phase).toBe("not-found");
    expect(state.detail).toBeNull();
    expect(state.media).toEqual([]);
  });

  it("keeps rendering the loaded game when a later refresh fails", () => {
    let state = readyState();
    state = reduceGameDetailState(state, {
      type: "request-started",
      requestId: 4,
    });
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
    state = reduceGameDetailState(state, {
      type: "connectivity-changed",
      online: false,
    });
    expect(state.phase).toBe("offline");
    expect(state.errorMessage).not.toBe("");
    state = reduceGameDetailState(state, {
      type: "connectivity-changed",
      online: true,
    });
    expect(state.phase).toBe("ready");
    expect(state.errorMessage).toBe("");
  });

  it("clears a stuck busy flag and wipes media errors on re-activation", () => {
    let state = readyState();
    state = reduceGameDetailState(state, {
      type: "media-busy-changed",
      busy: true,
    });
    state = reduceGameDetailState(state, {
      type: "media-failed",
      message: "boom",
    });
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
    state = reduceGameDetailState(state, {
      type: "wishlist-changed",
      wishlisted: true,
    });
    expect(state.detail?.wishlisted).toBe(true);
    state = reduceGameDetailState(state, { type: "about-toggled" });
    expect(state.aboutExpanded).toBe(true);
  });
});

describe("wallpaper search state", () => {
  const candidate = (
    id: string,
    category: WallpaperCategory,
  ): WallpaperCandidateView => ({
    id,
    title: id,
    thumbnailUrl: `/${id}.png`,
    category,
  });

  const backendResults = (
    category: WallpaperCategory,
    candidateIds: readonly string[],
  ): WallpaperSearchView => ({
    phase: "ready",
    category,
    query: "elden ring",
    message: "",
    candidates: candidateIds.map((id) => candidate(id, category)),
  });

  /** One full round trip for a single row: the request, then the answer. */
  function runSearch(
    state: GameDetailPageState,
    category: WallpaperCategory,
    candidateIds: readonly string[],
    options: { more?: boolean } = {},
  ): GameDetailPageState {
    const started = reduceGameDetailState(state, {
      type: "wallpaper-search-started",
      category,
      more: options.more === true,
    });
    return reduceGameDetailState(started, {
      type: "wallpaper-search-results",
      category,
      results: backendResults(category, candidateIds),
    });
  }

  const rowIds = (
    state: GameDetailPageState,
    category: WallpaperCategory,
  ): string[] =>
    state.wallpaperSearch.categories[category].candidates.map(
      (entry) => entry.id,
    );

  /** A game with no rail wallpapers, so the dialog opens with no active slide. */
  const emptyRail = () => readyState({ media: [] });

  it("opens prefilled with the game title and closes again", () => {
    let state = readyState();
    expect(state.wallpaperSearch.open).toBe(false);

    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.open).toBe(true);
    expect(state.wallpaperSearch.query).toBe("Elden Ring");
    expect(state.wallpaperSearch.focus).toBeNull();

    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    expect(state.wallpaperSearch.open).toBe(false);
  });

  it("keeps a query the user already typed when reopening", () => {
    let state = readyState();
    state = reduceGameDetailState(state, {
      type: "wallpaper-search-query-changed",
      query: "souls",
    });
    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.query).toBe("souls");
  });

  it("fills the searched row and leaves the other two untouched", () => {
    let state = emptyRail();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    const before = state.wallpaperSearch.categories;

    state = reduceGameDetailState(state, {
      type: "wallpaper-search-started",
      category: "cover",
    });
    expect(state.wallpaperSearch.categories.cover).toMatchObject({
      busy: true,
      phase: "idle",
      // A fresh search restarts at zero.
      offset: 0,
    });
    // The rows nobody asked for are the very same objects, not rebuilt copies.
    expect(state.wallpaperSearch.categories.landscape).toBe(before.landscape);
    expect(state.wallpaperSearch.categories.background).toBe(before.background);

    state = reduceGameDetailState(state, {
      type: "wallpaper-search-results",
      category: "cover",
      results: backendResults("cover", ["c1"]),
    });
    expect(state.wallpaperSearch.categories.cover).toMatchObject({
      busy: false,
      phase: "ready",
      offset: 1,
      hasMore: true,
    });
    expect(rowIds(state, "cover")).toEqual(["c1"]);
    // The backend echoes the query it actually ran.
    expect(state.wallpaperSearch.query).toBe("elden ring");
    // No portrait result bled into the wide rows.
    expect(state.wallpaperSearch.categories.landscape).toEqual(
      createWallpaperCategoryState(),
    );
    expect(state.wallpaperSearch.categories.background).toEqual(
      createWallpaperCategoryState(),
    );
    // With no rail wallpaper to sit on, the dialog lands on the first result.
    expect(state.wallpaperSearch.activeId).toBe("c1");
  });

  it("keeps the other rows' results when one row fails", () => {
    let state = emptyRail();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = runSearch(state, "cover", ["c1", "c2"]);
    state = runSearch(state, "landscape", ["l1"]);
    state = runSearch(state, "background", ["b1"]);

    state = reduceGameDetailState(state, {
      type: "wallpaper-search-started",
      category: "landscape",
      more: true,
    });
    state = reduceGameDetailState(state, {
      type: "wallpaper-search-failed",
      category: "landscape",
      message: "IGDB is unreachable.",
    });

    expect(state.wallpaperSearch.categories.landscape).toMatchObject({
      busy: false,
      phase: "error",
      message: "IGDB is unreachable.",
    });
    // A failed "search more" keeps what that row had already shown.
    expect(rowIds(state, "landscape")).toEqual(["l1"]);
    // And the failure stays in its own row: the neighbours still render.
    expect(state.wallpaperSearch.categories.cover).toMatchObject({
      phase: "ready",
      busy: false,
    });
    expect(rowIds(state, "cover")).toEqual(["c1", "c2"]);
    expect(state.wallpaperSearch.categories.background).toMatchObject({
      phase: "ready",
    });
    expect(rowIds(state, "background")).toEqual(["b1"]);
  });

  it("drops every row's results when the source changes, and only then", () => {
    let state = emptyRail();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = reduceGameDetailState(state, {
      type: "wallpaper-search-focus-changed",
      focus: "cover",
    });
    for (const category of WALLPAPER_CATEGORIES) {
      state = runSearch(state, category, [`${category}-1`]);
    }

    expect(state.wallpaperSearch.open).toBe(true);
    expect(state.wallpaperSearch.query).toBe("elden ring");
    expect(state.wallpaperSearch.focus).toBe("cover");
  });

  it("narrows to a single row and back, ignoring a repeated focus", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.focus).toBeNull();

    state = reduceGameDetailState(state, {
      type: "wallpaper-search-focus-changed",
      focus: "landscape",
    });
    expect(state.wallpaperSearch.focus).toBe("landscape");

    // Clicking the chip already selected must not churn state (and re-render).
    expect(
      reduceGameDetailState(state, {
        type: "wallpaper-search-focus-changed",
        focus: "landscape",
      }),
    ).toBe(state);

    state = reduceGameDetailState(state, {
      type: "wallpaper-search-focus-changed",
      focus: null,
    });
    expect(state.wallpaperSearch.focus).toBeNull();
    expect(
      reduceGameDetailState(state, {
        type: "wallpaper-search-focus-changed",
        focus: null,
      }),
    ).toBe(state);
  });

  it("appends when searching more and pages only the row that asked", () => {
    let state = emptyRail();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = runSearch(state, "cover", ["c1", "c2", "c3", "c4"]);
    state = runSearch(state, "landscape", ["l1"]);
    expect(state.wallpaperSearch.categories.cover.offset).toBe(4);
    expect(state.wallpaperSearch.activeId).toBe("c1");

    // "Search more" keeps the page offset, the results and the active slide.
    const more = reduceGameDetailState(state, {
      type: "wallpaper-search-started",
      category: "cover",
      more: true,
    });
    expect(more.wallpaperSearch.categories.cover.offset).toBe(4);
    expect(rowIds(more, "cover")).toEqual(["c1", "c2", "c3", "c4"]);

    state = reduceGameDetailState(more, {
      type: "wallpaper-search-results",
      category: "cover",
      results: backendResults("cover", ["c5", "c6"]),
    });
    expect(rowIds(state, "cover")).toEqual([
      "c1",
      "c2",
      "c3",
      "c4",
      "c5",
      "c6",
    ]);
    expect(state.wallpaperSearch.categories.cover.offset).toBe(6);
    expect(state.wallpaperSearch.categories.cover.hasMore).toBe(true);
    expect(state.wallpaperSearch.activeId).toBe("c1");
    // Paging the Cover row never pages the Landscape row.
    expect(state.wallpaperSearch.categories.landscape.offset).toBe(1);
    expect(rowIds(state, "landscape")).toEqual(["l1"]);

    // The source has nothing else: the merged list stays, hasMore flips off.
    state = runSearch(state, "cover", [], { more: true });
    expect(rowIds(state, "cover")).toHaveLength(6);
    expect(state.wallpaperSearch.categories.cover.hasMore).toBe(false);
  });

  it("replaces a row's results on a fresh search instead of appending", () => {
    let state = emptyRail();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = runSearch(state, "cover", ["c1", "c2"]);

    // A new query empties the row while it loads: no stale art under the spinner.
    const started = reduceGameDetailState(state, {
      type: "wallpaper-search-started",
      category: "cover",
    });
    expect(started.wallpaperSearch.categories.cover.candidates).toEqual([]);
    expect(started.wallpaperSearch.categories.cover.offset).toBe(0);

    state = reduceGameDetailState(started, {
      type: "wallpaper-search-results",
      category: "cover",
      results: backendResults("cover", ["d1"]),
    });
    expect(rowIds(state, "cover")).toEqual(["d1"]);
    expect(state.wallpaperSearch.categories.cover.offset).toBe(1);
  });

  it("lists every row's candidates in cover, landscape, background order", () => {
    let state = emptyRail();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(allWallpaperCandidates(state.wallpaperSearch)).toEqual([]);

    // The three requests can answer in any order; the dialog still reads
    // top row first.
    state = runSearch(state, "background", ["b1"]);
    state = runSearch(state, "landscape", ["l1", "l2"]);
    state = runSearch(state, "cover", ["c1"]);
    expect(
      allWallpaperCandidates(state.wallpaperSearch).map((entry) => entry.id),
    ).toEqual(["c1", "l1", "l2", "b1"]);
    expect(
      allWallpaperCandidates(state.wallpaperSearch).map(
        (entry) => entry.category,
      ),
    ).toEqual(["cover", "landscape", "landscape", "background"]);
  });

  it("keeps in-flight rows busy when the dialog closes, so a reopen cannot double-fire", () => {
    let state = emptyRail();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = runSearch(state, "cover", ["c1"]);
    // Three requests in flight at once, as the dialog fires on open.
    for (const category of WALLPAPER_CATEGORIES) {
      state = reduceGameDetailState(state, {
        type: "wallpaper-search-started",
        category,
        more: category === "cover",
      });
      expect(state.wallpaperSearch.categories[category].busy).toBe(true);
    }

    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    expect(state.wallpaperSearch.open).toBe(false);
    // The requests did not stop just because the dialog did. Clearing `busy`
    // here would let the reopen fire a second request per row, whose results
    // then merge as a "search more" page and duplicate every tile.
    for (const category of WALLPAPER_CATEGORIES) {
      expect(state.wallpaperSearch.categories[category].busy).toBe(true);
    }
    // Closing is not a reset: what the Cover row already found is still there.
    expect(rowIds(state, "cover")).toEqual(["c1"]);

    // Reopening finds the rows still busy, so nothing re-fires; the in-flight
    // answer lands normally and replaces, rather than appending to, the row.
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    for (const category of WALLPAPER_CATEGORIES) {
      expect(state.wallpaperSearch.categories[category].busy).toBe(true);
    }
  });

  it("opens on the wallpaper being previewed, else the first wallpaper", () => {
    let state = readyState({
      media: [media("w1", "wallpaper", true), media("w2", "wallpaper")],
    });
    state = reduceGameDetailState(state, {
      type: "media-previewed",
      mediaId: "w2",
    });
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
    // A search left the user browsing one of its results.
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    state = runSearch(state, "cover", ["c1"]);
    state = reduceGameDetailState(state, {
      type: "wallpaper-slide-changed",
      slideId: "c1",
    });
    state = reduceGameDetailState(state, { type: "wallpaper-search-closed" });
    expect(state.wallpaperSearch.activeId).toBe("c1");
    // The user then previewed a downloaded wallpaper; the dialog must open on it.
    state = reduceGameDetailState(state, {
      type: "media-previewed",
      mediaId: "w2",
    });
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    expect(state.wallpaperSearch.activeId).toBe("w2");
  });

  it("lets the slide controls move the active wallpaper without searching", () => {
    let state = readyState();
    state = reduceGameDetailState(state, { type: "wallpaper-search-opened" });
    const slides =
      state.detail?.media
        .filter((item) => item.kind === "wallpaper")
        .map((item) => item.id) ?? [];
    expect(slides.length).toBeGreaterThan(1);
    state = reduceGameDetailState(state, {
      type: "wallpaper-slide-changed",
      slideId: slides[1],
    });
    expect(state.wallpaperSearch.activeId).toBe(slides[1]);
    for (const category of WALLPAPER_CATEGORIES) {
      expect(state.wallpaperSearch.categories[category].busy).toBe(false);
    }
  });

  it("fills each browser-fallback row with art of that row's shape", () => {
    // The bundled mock art is filed by shape, so a row can only look right if
    // it draws from its own folder.
    const folders: Record<WallpaperCategory, string> = {
      cover: "/media/igdb/covers/",
      landscape: "/media/igdb/landscapes/",
      background: "/media/igdb/heroes/",
      // No wordmarks ship with the app, so this row is empty in the browser.
      logo: "",
    };
    for (const category of WALLPAPER_CATEGORIES.filter(
      (category) => category !== "logo",
    )) {
      const view = createFallbackWallpaperSearch(category, "elden ring");
      expect(view).toMatchObject({
        phase: "ready",
        category,
        query: "elden ring",
      });
      expect(view.candidates).toHaveLength(5);
      expect(view.candidates.map((entry) => entry.category)).toEqual(
        Array(5).fill(category),
      );
      expect(
        view.candidates.filter((entry) =>
          entry.thumbnailUrl.startsWith(folders[category]),
        ),
      ).toHaveLength(5);
    }
  });

  it("pages the fallback rows and never reuses an id across rows", () => {
    expect(
      createFallbackWallpaperSearch("cover", "q", 5).candidates.map(
        (entry) => entry.id,
      ),
    ).toEqual(["candidate-cover-6"]);
    expect(
      createFallbackWallpaperSearch("cover", "q", 6).candidates,
    ).toEqual([]);

    // All three rows live in one dialog, so their ids must not collide.
    const ids = WALLPAPER_CATEGORIES.flatMap((category) =>
      createFallbackWallpaperSearch(category, "q").candidates.map(
        (entry) => entry.id,
      ),
    );
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("restore state", () => {
  it("round-trips scroll, focus and the selected media", () => {
    let state = readyState({
      media: [media("w1", "wallpaper", true), media("c1", "cover", true)],
    });
    state = reduceGameDetailState(state, {
      type: "media-previewed",
      mediaId: "c1",
    });
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
    state = reduceGameDetailState(state, {
      type: "request-started",
      requestId: 1,
    });
    state = reduceGameDetailState(state, {
      type: "detail-loaded",
      requestId: 1,
      detail: detailWith({
        media: [media("w1", "wallpaper", true), media("c1", "cover", true)],
      }),
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
    state = reduceGameDetailState(state, {
      type: "request-started",
      requestId: 1,
    });
    state = reduceGameDetailState(state, {
      type: "detail-loaded",
      requestId: 1,
      detail: detailWith({
        media: [media("w1", "wallpaper", true), media("c1", "cover", true)],
      }),
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
      readGameDetailRestoreState({
        scrollTop: -5,
        focusKey: null,
        filters: ["kind:nope"],
      }),
    ).toMatchObject({
      scrollTop: 0,
      activeMediaKind: null,
      previewMediaId: null,
    });
  });
});

describe("section predicates", () => {
  it("only lists sections that have real content", () => {
    // Game info is the one section a loaded game always fills: at the very
    // least it says where the game came from. It used to vanish for any title
    // whose provider published no developer, date, genre or platform — most of
    // a Microsoft Store or local library — and left a hole in the page.
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
    ).toEqual(["info"]);
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
    expect(
      visibleSections(detailWith({ friends: [] })).includes("friends"),
    ).toBe(false);
    expect(
      visibleSections(
        detailWith({
          friends: [
            { id: "f1", name: "Valkyrie", avatarUrl: "", status: "Online" },
          ],
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

  it("strips runtime-state mentions the Play button already conveys", () => {
    const detail = normaliseGameDetail({
      id: "game_3",
      title: "Wine Game",
      features: ["Single-player", "wine-staging", "Installed"],
      tags: ["Roguelike", "Incompatible for macOS"],
      genres: ["Action", "installed"],
    });
    expect(detail?.features).toEqual(["Single-player"]);
    expect(detail?.tags).toEqual(["Roguelike"]);
    expect(detail?.genres).toEqual(["Action"]);
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
      normaliseWallpaperSearch(
        {
          phase: "ready",
          category: "landscape",
          query: " elden ring ",
          message: "10 results",
          candidates: [
            {
              id: "c1",
              title: "Key art",
              thumbnailUrl: "https://x.test/a.png",
            },
            {
              id: "c1",
              title: "Duplicate",
              thumbnailUrl: "https://x.test/b.png",
            },
            { id: "c2", thumbnailUrl: "https://x.test/c.png" },
            { id: "c3", title: "No thumb" },
            "junk",
          ],
        },
        "landscape",
      ),
    ).toEqual({
      phase: "ready",
      category: "landscape",
      query: " elden ring ",
      message: "10 results",
      candidates: [
        {
          id: "c1",
          title: "Key art",
          thumbnailUrl: "https://x.test/a.png",
          category: "landscape",
        },
        {
          id: "c2",
          title: "Wallpaper",
          thumbnailUrl: "https://x.test/c.png",
          category: "landscape",
        },
      ],
    });
  });

  it("files every candidate under the row that was actually requested", () => {
    const view = normaliseWallpaperSearch(
      {
        phase: "ready",
        source: "igdb",
        query: "elden ring",
        message: "",
        candidates: [
          { id: "c1", title: "Silent", thumbnailUrl: "/a.png" },
          {
            id: "c2",
            title: "Garbage",
            thumbnailUrl: "/b.png",
            category: "screenshot",
          },
          {
            id: "c3",
            title: "Explicit",
            thumbnailUrl: "/c.png",
            category: "background",
          },
        ],
      },
      "cover",
    );
    // The payload never named a shape, so the row we asked for wins.
    expect(view.category).toBe("cover");
    // A silent candidate inherits the row; a nonsense one is coerced to it; a
    // candidate that names a real shape keeps it.
    expect(view.candidates.map((entry) => entry.category)).toEqual([
      "cover",
      "cover",
      "background",
    ]);
  });

  it("falls back to a safe shape for a malformed search payload", () => {
    expect(normaliseWallpaperSearch(null, "landscape")).toEqual({
      phase: "error",
      category: "landscape",
      query: "",
      message: "",
      candidates: [],
    });
    expect(
      normaliseWallpaperSearch(
        {
          phase: "mystery",
          category: "portrait",
          candidates: "x",
        },
        "cover",
      ),
    ).toMatchObject({
      phase: "error",
      category: "cover",
      candidates: [],
    });
  });
});

describe("formatting", () => {
  it("formats play time and last-played facts", () => {
    expect(formatPlayTime(0)).toBe("Not played yet");
    expect(formatPlayTime(1_800)).toBe("Played 30 min");
    expect(formatPlayTime(460_800)).toBe("Played 128h");
    const now = Date.parse("2026-08-01T12:00:00Z");
    expect(formatLastPlayed(null, now)).toBe("Never played");
    expect(formatLastPlayed("2026-07-30T12:00:00Z", now)).toBe(
      "Last played 2 days ago",
    );
    expect(formatLastPlayed("2026-08-01T09:00:00Z", now)).toBe(
      "Last played 3h ago",
    );
  });

  it("formats release dates and achievement progress", () => {
    expect(formatReleaseDate("2022-02-25")).toBe("February 25, 2022");
    expect(formatReleaseDate("not a date")).toBe("not a date");
    expect(
      formatAchievementProgress({ unlocked: 67, total: 82, items: [] }),
    ).toEqual({
      label: "67/82 unlocked",
      percent: 82,
    });
    expect(formatAchievementProgress(null)).toBeNull();
  });

  it("omits the achievements stat when the game has none", () => {
    const facts = buildStatFacts(
      detailWith({ achievements: null }),
      Date.now(),
    );
    expect(facts.map((fact) => fact.id)).toEqual(["playtime", "last-played"]);
  });

  it("shows no playtime facts at all for a never-played game", () => {
    const facts = buildStatFacts(
      detailWith({
        playTimeSeconds: 0,
        lastPlayedAt: null,
        achievements: null,
      }),
      Date.now(),
    );
    expect(facts).toEqual([]);
  });
});

describe("status chips", () => {
  const detailWithStatus = (
    overrides: Partial<
      Pick<
        GameDetailViewModel,
        "installState" | "installPercent" | "macCompatibility"
      >
    >,
  ): GameDetailViewModel => ({
    ...createFallbackGameDetail("epic:abc"),
    installState: "unknown",
    installPercent: null,
    macCompatibility: "unknown",
    ...overrides,
  });

  it("leaves the install chip to the button when the game is not downloaded", () => {
    // The Install button already says this; a chip repeating it is noise.
    expect(
      statusChips(detailWithStatus({ installState: "not-installed" })),
    ).toEqual([]);
  });

  it("says nothing at all when neither answer is known", () => {
    expect(statusChips(detailWithStatus({}))).toEqual([]);
    expect(statusChips(null)).toEqual([]);
  });

  it("names the two Mac answers apart, and never invents the third", () => {
    expect(
      statusChips(detailWithStatus({ macCompatibility: "native" }))[0].label,
    ).toBe("Mac native");
    expect(
      statusChips(detailWithStatus({ macCompatibility: "not-native" }))[0].label,
    ).toBe("Windows only");
    expect(
      statusChips(detailWithStatus({ macCompatibility: "unknown" })),
    ).toEqual([]);
  });

  it("leaves a running download to the button's own progress bar", () => {
    expect(
      statusChips(
        detailWithStatus({ installState: "installing", installPercent: 12 }),
      ),
    ).toEqual([]);
  });

  it("turns the primary action into the download bar while Epic transfers", () => {
    const detail = {
      ...detailWithStatus({ installState: "installing", installPercent: 12 }),
      primaryAction: "install-epic" as const,
    };

    const action = resolvePrimaryAction(detail, "epic:abc");
    expect(action.label).toBe("Downloading 12%");
    expect(action.progress).toBe(12);
    // A transfer already running is not a second install to start.
    expect(action.disabled).toBe(true);
    expect(action.intent).toBe("none");
    // A live measurement beats the catalogue's last-known percentage.
    expect(resolvePrimaryAction(detail, "epic:abc", 63).label).toBe(
      "Downloading 63%",
    );
  });

  it("offers a plain Install button when nothing is downloading", () => {
    const action = resolvePrimaryAction(
      {
        ...detailWithStatus({ installState: "not-installed" }),
        primaryAction: "install-epic" as const,
      },
      "epic:abc",
    );

    expect(action.label).toBe("Install");
    expect(action.progress).toBeNull();
    expect(action.disabled).toBe(false);
  });

  it("shows both facts together once both are known", () => {
    const chips = statusChips(
      detailWithStatus({
        installState: "installed",
        macCompatibility: "native",
      }),
    );

    expect(chips.map((chip) => chip.label)).toEqual(["Installed", "Mac native"]);
  });
});

/* -------------------------------------------------------------------------- */
/* Page lifecycle (DOM)                                                        */
/* -------------------------------------------------------------------------- */

function stubClient(
  overrides: Partial<GameDetailPageClient> = {},
): GameDetailPageClient {
  return {
    getDetail: async () =>
      detailWith({
        media: [media("w1", "wallpaper", true), media("w2", "wallpaper")],
      }),
    setWishlist: async () => undefined,
    selectMedia: async () => [
      media("w1", "wallpaper"),
      media("w2", "wallpaper", true),
    ],
    importMedia: async () => [],
    exportMedia: async () => undefined,
    cancelMediaDownload: async () => undefined,
    searchWallpapers: async (category) =>
      createFallbackWallpaperSearch(category, ""),
    importWallpaper: async () => [],
    openOffer: async () => undefined,
    installEpicGame: async () => undefined,
    uninstallEpicGame: async () => undefined,
    epicInstallStatus: async () => ({
      appName: "Sugar",
      state: "not-installed" as const,
      percent: 0,
      installedBytes: 0,
      totalBytes: 0,
      installPath: null,
    }),
    searchArtwork: async () => undefined,
    resetArtwork: async () => ({
      title: "Test",
      replaced: ["cover", "landscape", "background"] as WallpaperRole[],
    }),
    removeGame: async () => undefined,
    setGameHidden: async () => undefined,
    setHomeImage: async () => undefined,
    ...overrides,
  };
}

function activationFor(
  route: AppRoute,
  options: {
    isCurrent?: () => boolean;
    restoreState?: PageRestoreState | null;
  } = {},
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
): {
  page: ReturnType<typeof createGameDetailPage>;
  host: HTMLElement;
  options: GameDetailPageOptions;
} {
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

/** Opens the wallpaper dialog the way the UI now does: through the "…" menu. */
function openWallpaperDialog(host: HTMLElement): void {
  host
    .querySelector<HTMLButtonElement>("[data-focus-key='more-actions']")
    ?.click();
  host
    .querySelector<HTMLButtonElement>("[data-focus-key='menu-wallpaper']")
    ?.click();
}

describe("game detail page lifecycle", () => {
  it("mounts, paints the loaded game and never throws into the shell", async () => {
    const { page, host } = mountPage(stubClient());
    const { activation } = activationFor(gameRoute);
    await page.activate(activation);
    expect(host.querySelector(".gd-hero__title")?.textContent).toBe(
      "Elden Ring",
    );
    expect(host.querySelector(".gd-primary-action")?.textContent).toContain(
      "Play",
    );
    expect(
      host.querySelectorAll(".gd-gallery__tile:not(.gd-gallery__tile--empty)")
        .length,
    ).toBe(2);
    // Adding a wallpaper lives in the "…" menu now, not on the rail.
    expect(
      host.querySelector("[data-focus-key='menu-wallpaper']"),
    ).not.toBeNull();
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
    const { activation } = activationFor(gameRoute, {
      isCurrent: () => current,
    });
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
    expect(host.querySelector(".gd-notice__title")?.textContent).toBe(
      "Game not found",
    );
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
    expect(
      host.querySelector("[data-focus-key='notice-retry']"),
    ).not.toBeNull();
  });

  it("captures and restores scroll, focus and the previewed media", async () => {
    const { page, host } = mountPage(stubClient());
    await page.activate(activationFor(gameRoute).activation);
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='media-w2']")
      ?.click();
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='more-actions']")
      ?.focus();
    const restore = page.deactivate();
    expect(restore).toMatchObject({
      focusKey: "more-actions",
      selectedGameId: "game_1",
      filters: ["kind:wallpaper", "media:w2"],
    });

    await page.activate(
      activationFor(gameRoute, { restoreState: restore }).activation,
    );
    expect(
      host
        .querySelector("[data-focus-key='media-w2']")
        ?.classList.contains("gd-gallery__tile--selected"),
    ).toBe(true);
  });

  it("chooses a rail wallpaper as the home background and commits it on click", async () => {
    const selectMedia = vi.fn(async () => [
      media("w1", "wallpaper"),
      media("w2", "wallpaper", true),
    ]);
    const setHomeImage = vi.fn(async () => undefined);
    const { page, host } = mountPage(stubClient({ selectMedia, setHomeImage }));
    await page.activate(activationFor(gameRoute).activation);

    // Clicking a rail wallpaper chooses it as the home (Library) background: it
    // persists the selection and promotes the media, with opaque ids only.
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='media-w2']")
      ?.click();
    await vi.waitFor(() =>
      expect(selectMedia).toHaveBeenCalledWith(
        "game_1",
        "w2",
        expect.anything(),
      ),
    );
    await vi.waitFor(() =>
      expect(setHomeImage).toHaveBeenCalledWith(
        "game_1",
        "w2",
        "background",
        expect.anything(),
      ),
    );
  });

  it("commits and promotes a ticked wallpaper as the home background", async () => {
    const selectMedia = vi.fn(async () => [
      media("w1", "wallpaper"),
      media("w2", "wallpaper", true),
    ]);
    const setHomeImage = vi.fn(async () => undefined);
    const { page, host } = mountPage(stubClient({ selectMedia, setHomeImage }));
    await page.activate(activationFor(gameRoute).activation);

    openWallpaperDialog(host);
    // Tick an existing wallpaper in the grid, then apply it.
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='wall-w2']")
      ?.click();
    const apply = host.querySelector<HTMLButtonElement>(
      "[data-focus-key='wallpaper-apply']",
    );
    expect(apply?.disabled).toBe(false);
    apply?.click();
    await vi.waitFor(() =>
      expect(selectMedia).toHaveBeenCalledWith(
        "game_1",
        "w2",
        expect.anything(),
      ),
    );
    await vi.waitFor(() =>
      expect(setHomeImage).toHaveBeenCalledWith(
        "game_1",
        "w2",
        "background",
        expect.anything(),
      ),
    );
    await vi.waitFor(() => expect(host.querySelector(".gd-modal")).toBeNull());
  });

  it("applies each pick to the slot its own row stands for, all three at once", async () => {
    const selectMedia = vi.fn(async () => [
      media("w1", "wallpaper"),
      media("w2", "wallpaper", true),
    ]);
    const setHomeImage = vi.fn(
      async (_gameId: string, _mediaId: string, _role: WallpaperRole) =>
        undefined,
    );
    // Each import echoes back a media id derived from the candidate, so the
    // assertion can tell which row's pick produced which call.
    const importWallpaper = vi.fn(
      async (_gameId: string, candidateId: string) => [
        { ...media(`saved-${candidateId}`, "wallpaper", true) },
      ],
    );
    const { page, host } = mountPage(
      stubClient({ selectMedia, setHomeImage, importWallpaper }),
    );
    await page.activate(activationFor(gameRoute).activation);

    openWallpaperDialog(host);
    await vi.waitFor(() =>
      expect(
        host.querySelector("[data-focus-key='wall-candidate-cover-1']"),
      ).not.toBeNull(),
    );

    // One tile per row — no "Apply as" step in between.
    host
      .querySelector<HTMLButtonElement>(
        "[data-focus-key='wall-candidate-cover-1']",
      )
      ?.click();
    host
      .querySelector<HTMLButtonElement>(
        "[data-focus-key='wall-candidate-landscape-1']",
      )
      ?.click();
    host
      .querySelector<HTMLButtonElement>(
        "[data-focus-key='wall-candidate-background-1']",
      )
      ?.click();
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']")
      ?.click();

    await vi.waitFor(() => expect(setHomeImage).toHaveBeenCalledTimes(3));
    const roles = setHomeImage.mock.calls.map((call) => [call[1], call[2]]);
    expect(roles).toEqual(
      expect.arrayContaining([
        ["saved-candidate-cover-1", "cover"],
        ["saved-candidate-landscape-1", "landscape"],
        ["saved-candidate-background-1", "background"],
      ]),
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
    openWallpaperDialog(host);
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='wall-w2']")
      ?.click();
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']")
      ?.click();
    await vi.waitFor(() =>
      expect(host.querySelector(".gd-modal__error")?.textContent).toContain(
        "read-only",
      ),
    );
    expect(host.querySelector(".gd-modal")).not.toBeNull();
  });

  it("routes the contextual primary action through shell-owned callbacks", async () => {
    const play = vi.fn();
    const { page, host } = mountPage(stubClient(), { play });
    await page.activate(activationFor(gameRoute).activation);
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='primary-action']")
      ?.click();
    expect(play).toHaveBeenCalledWith("game_1");

    const navigate = vi.fn();
    const wine = mountPage(
      stubClient({
        getDetail: async () => detailWith({ primaryAction: "configure-wine" }),
      }),
      { navigate },
    );
    await wine.page.activate(activationFor(gameRoute).activation);
    wine.host
      .querySelector<HTMLButtonElement>("[data-focus-key='primary-action']")
      ?.click();
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
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='primary-action']")
      ?.click();
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
    expect(host.querySelector(".gd-hero__title")?.textContent).toBe(
      "Elden Ring",
    );
    expect(host.querySelector(".gd-modal")).toBeNull();
    // No media yet: the rail is gone entirely, but the "…" menu still offers a
    // way to add a wallpaper.
    expect(host.querySelector(".gd-gallery")).toBeNull();
    expect(host.querySelectorAll(".gd-gallery__tile")).toHaveLength(0);
    expect(
      host.querySelector("[data-focus-key='menu-wallpaper']"),
    ).not.toBeNull();
  });

  it("downloads a ticked search result and applies it to its own row's slot", async () => {
    // Every row is fetched on its own, so each answers with its own art.
    const searchWallpapers = vi.fn(async (category: WallpaperCategory) => ({
        phase: "ready" as const,
        category,
        query: "elden ring",
        message: "",
        candidates: [
          {
            id: `c1-${category}`,
            title: "Key art",
            thumbnailUrl: "/a.png",
            category,
          },
        ],
    }));
    const importWallpaper = vi.fn(async () => [media("w9", "wallpaper", true)]);
    const selectMedia = vi.fn(async () => [media("w9", "wallpaper", true)]);
    const setHomeImage = vi.fn(async () => undefined);
    const { page, host } = mountPage(
      stubClient({
        searchWallpapers,
        importWallpaper,
        selectMedia,
        setHomeImage,
      }),
    );
    await page.activate(activationFor(gameRoute).activation);

    // The search toggle only exists on the wallpaper slot; opening it searches.
    openWallpaperDialog(host);
    await vi.waitFor(() => expect(searchWallpapers).toHaveBeenCalled());
    // The candidate lands as a tile in its own row; tick it and apply.
    await vi.waitFor(() =>
      expect(
        host.querySelector<HTMLButtonElement>(
          "[data-focus-key='wall-c1-cover']",
        ),
      ).not.toBeNull(),
    );
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='wall-c1-cover']")
      ?.click();
    host
      .querySelector<HTMLButtonElement>("[data-focus-key='wallpaper-apply']")
      ?.click();

    await vi.waitFor(() =>
      expect(importWallpaper).toHaveBeenCalledWith(
        "game_1",
        "c1-cover",
        expect.anything(),
      ),
    );
    // It was ticked in the Cover row, so it fills the cover slot — not the
    // background, which used to be the hardcoded default.
    await vi.waitFor(() =>
      expect(setHomeImage).toHaveBeenCalledWith(
        "game_1",
        "w9",
        "cover",
        expect.anything(),
      ),
    );
    await vi.waitFor(() => expect(host.querySelector(".gd-modal")).toBeNull());
  });

  it("shows why a row came back empty rather than a bare 'no results'", async () => {
    const { page, host } = mountPage(
      stubClient({
        searchWallpapers: async (category) => ({
          phase: "ready" as const,
          category,
          query: "elden ring",
          message:
            "No 4K background matched that search. SteamGridDB API Key is missing — add it under Settings.",
          candidates: [],
        }),
      }),
    );
    await page.activate(activationFor(gameRoute).activation);
    openWallpaperDialog(host);
    host.querySelector<HTMLFormElement>(".gd-wallhead__search")?.requestSubmit();
    await vi.waitFor(() =>
      expect(host.querySelector(".gd-search__notice")?.textContent).toContain(
        "SteamGridDB API Key is missing",
      ),
    );
    expect(host.querySelector(".gd-search__status")).toBeNull();
  });
});
