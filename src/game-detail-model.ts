import type {
  AppRoute,
  GameDetailView,
  GameId,
  GameMediaKind,
  GameMediaView,
  GameSummary,
  PageRestoreState,
  StoreOffer,
  WallpaperCandidateView,
  WallpaperCategory,
  WallpaperSearchPhase,
  WallpaperSearchView,
  WallpaperSource,
} from "./contracts";
import { WALLPAPER_CATEGORIES } from "./contracts";
import type { IconName } from "./icons";

/* -------------------------------------------------------------------------- */
/* Types                                                                       */
/* -------------------------------------------------------------------------- */

export type GameDetailPhase =
  | "idle"
  | "loading"
  | "ready"
  | "not-found"
  | "offline"
  | "error";

export type GameDetailPrimaryAction = GameDetailView["primaryAction"];

/**
 * Friends and activity are not part of the shipped backend DTO yet. They are
 * modelled as optional extras so the page can render them the day the backend
 * sends them, and render nothing at all until then.
 */
export interface GameDetailFriend {
  id: string;
  name: string;
  avatarUrl: string;
  status: string;
}

export interface GameDetailActivityEntry {
  id: string;
  actorName: string;
  summary: string;
  detail: string;
  avatarUrl: string;
  occurredAt: string | null;
}

export interface GameDetailSocialExtras {
  friends?: GameDetailFriend[];
  activity?: GameDetailActivityEntry[];
  /** Editorial star rating out of 5, shown in the hero meta row. */
  rating?: number;
  /** Aggregate positive-review percentage, shown in the hero meta row. */
  reviewPercent?: number;
}

/**
 * One category row's own results. `phase` is `"idle"` until that row's first
 * search runs, then it mirrors the backend's `ready | not-configured | error`
 * phases. Each row searches, pages and fails independently, so an empty Cover
 * row never blanks the Background row beside it.
 */
export interface WallpaperCategoryState {
  phase: "idle" | WallpaperSearchPhase;
  message: string;
  candidates: WallpaperCandidateView[];
  busy: boolean;
  /** Offset the last search started from; also the next "search more" offset. */
  offset: number;
  /** Whether another "search more" could still yield new candidates. */
  hasMore: boolean;
}

/**
 * Wallpaper search, split by the shape of artwork each row wants. The dialog
 * shows one row per category and fills each from its own scoped request, which
 * is what keeps a 16:9 screenshot out of the portrait Cover row.
 */
export interface WallpaperSearchState {
  open: boolean;
  source: WallpaperSource;
  query: string;
  /**
   * The row the user has narrowed to via the chips, or `null` for all three.
   * "Voir tout" sets this, so expanding a row and filtering to it are the
   * same state — one concept, not two.
   */
  focus: WallpaperCategory | null;
  categories: Record<WallpaperCategory, WallpaperCategoryState>;
  /** The slide currently shown in the dialog: a candidate id or a media id. */
  activeId: string | null;
}

export function createWallpaperCategoryState(): WallpaperCategoryState {
  return {
    phase: "idle",
    message: "",
    candidates: [],
    busy: false,
    offset: 0,
    hasMore: false,
  };
}

function createWallpaperCategories(): Record<WallpaperCategory, WallpaperCategoryState> {
  return {
    cover: createWallpaperCategoryState(),
    landscape: createWallpaperCategoryState(),
    background: createWallpaperCategoryState(),
  };
}

/** Replaces one row, leaving the other two identical by reference. */
function updateWallpaperCategory(
  search: WallpaperSearchState,
  category: WallpaperCategory,
  row: WallpaperCategoryState,
): WallpaperSearchState {
  return { ...search, categories: { ...search.categories, [category]: row } };
}

/** Every candidate the dialog currently holds, in row order. */
export function allWallpaperCandidates(
  search: WallpaperSearchState,
): WallpaperCandidateView[] {
  return WALLPAPER_CATEGORIES.flatMap((category) => search.categories[category].candidates);
}

export type GameDetailViewModel = GameDetailView & GameDetailSocialExtras;

export interface GameDetailPageState {
  phase: GameDetailPhase;
  gameId: GameId | null;
  from: "library" | "store" | null;
  detail: GameDetailViewModel | null;
  media: GameMediaView[];
  activeMediaKind: GameMediaKind;
  /** Preview-only selection. Never persisted until `Apply` succeeds. */
  previewMediaId: string | null;
  /** Last selection known to be persisted by the backend. */
  appliedMediaId: string | null;
  mediaBusy: boolean;
  mediaError: string;
  wallpaperSearch: WallpaperSearchState;
  aboutExpanded: boolean;
  statusMessage: string;
  errorMessage: string;
  activeRequestId: number;
  pendingRestore: PageRestoreState | null;
}

export type GameDetailPageAction =
  | {
      type: "activate";
      gameId: GameId;
      from: "library" | "store" | null;
      online: boolean;
      restore: PageRestoreState | null;
    }
  | { type: "request-started"; requestId: number }
  | { type: "detail-loaded"; requestId: number; detail: GameDetailViewModel | null }
  | { type: "request-failed"; requestId: number; message: string; offline: boolean }
  | { type: "media-kind-changed"; kind: GameMediaKind }
  | { type: "media-previewed"; mediaId: string }
  | { type: "media-busy-changed"; busy: boolean }
  | { type: "media-committed"; media: GameMediaView[] }
  | { type: "media-imported"; media: GameMediaView[] }
  | { type: "media-failed"; message: string }
  | { type: "wallpaper-search-opened" }
  | { type: "wallpaper-search-closed" }
  | { type: "wallpaper-search-source-changed"; source: WallpaperSource }
  | { type: "wallpaper-search-query-changed"; query: string }
  | { type: "wallpaper-search-focus-changed"; focus: WallpaperCategory | null }
  | { type: "wallpaper-search-started"; category: WallpaperCategory; more?: boolean }
  | { type: "wallpaper-search-results"; category: WallpaperCategory; results: WallpaperSearchView }
  | { type: "wallpaper-search-failed"; category: WallpaperCategory; message: string }
  | { type: "wallpaper-slide-changed"; slideId: string | null }
  | { type: "wishlist-changed"; wishlisted: boolean }
  | { type: "about-toggled" }
  | { type: "status-changed"; message: string }
  | { type: "connectivity-changed"; online: boolean };

export interface GameDetailMediaKindOption {
  id: GameMediaKind;
  label: string;
}

export interface GameDetailFact {
  id: string;
  icon: IconName | null;
  text: string;
  /** `accent` tints the icon and value with the lavender accent (rating, score). */
  tone?: "default" | "accent";
}

export interface PrimaryActionDescriptor {
  kind: GameDetailPrimaryAction;
  label: string;
  icon: IconName;
  intent: "play" | "navigate" | "open-offer" | "none";
  route: AppRoute | null;
  offerId: string | null;
  disabled: boolean;
  hint: string;
}

export type GameDetailSection =
  | "gallery"
  | "about"
  | "info"
  | "features"
  | "achievements"
  | "friends"
  | "activity"
  | "related";

/* -------------------------------------------------------------------------- */
/* Constants                                                                   */
/* -------------------------------------------------------------------------- */

export const GAME_MEDIA_KINDS: readonly GameDetailMediaKindOption[] = [
  { id: "wallpaper", label: "Wallpaper" },
  { id: "video", label: "Video" },
  { id: "icon", label: "Icon" },
  { id: "cover", label: "Cover" },
];

const MEDIA_KIND_IDS: readonly GameMediaKind[] = GAME_MEDIA_KINDS.map((option) => option.id);
const MEDIA_ORIGINS: readonly GameMediaView["origin"][] = [
  "bundled",
  "provider",
  "imported",
  "downloaded",
];
const PRIMARY_ACTIONS: readonly GameDetailPrimaryAction[] = [
  "play",
  "install-steam",
  "configure-wine",
  "view-offer",
  "unavailable",
];

/** Restore-state encoding. `PageRestoreState.filters` is a plain string list. */
export const RESTORE_KIND_PREFIX = "kind:";
export const RESTORE_MEDIA_PREFIX = "media:";

/** About copy longer than this gets a "Read more" disclosure. */
export const ABOUT_CLAMP_THRESHOLD = 260;

/* -------------------------------------------------------------------------- */
/* Defensive normalisation                                                     */
/* -------------------------------------------------------------------------- */

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown, fallback = ""): string {
  return typeof value === "string" && value.trim() ? value : fallback;
}

function optionalText(value: unknown): string | null {
  return typeof value === "string" && value.trim() ? value : null;
}

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((entry): entry is string => typeof entry === "string" && entry.trim() !== "");
}

/**
 * Runtime/install-state mentions ("wine-staging", "incompatible for macos",
 * "installed") that the primary Play button already conveys. They are stripped
 * during normalisation so no badge or feature row ever repeats them.
 */
const RUNTIME_STATE_LABEL = /wine[\s_-]?staging|incompatible|installed/i;

function visibleLabels(values: string[]): string[] {
  return values.filter((value) => !RUNTIME_STATE_LABEL.test(value));
}

function wholeNumber(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.floor(value) : 0;
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return typeof value === "string" && (allowed as readonly string[]).includes(value)
    ? (value as T)
    : fallback;
}

export function normaliseGameMedia(value: unknown): GameMediaView[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const media: GameMediaView[] = [];
  for (const entry of value) {
    if (!isRecord(entry)) continue;
    const id = text(entry.id);
    const previewUrl = text(entry.previewUrl);
    if (!id || !previewUrl || seen.has(id)) continue;
    if (typeof entry.kind !== "string" || !(MEDIA_KIND_IDS as readonly string[]).includes(entry.kind)) {
      continue;
    }
    seen.add(id);
    const item: GameMediaView = {
      id,
      kind: entry.kind as GameMediaKind,
      title: text(entry.title, "Untitled media"),
      previewUrl,
      origin: oneOf(entry.origin, MEDIA_ORIGINS, "provider"),
      selected: entry.selected === true,
      availableOffline: entry.availableOffline === true,
    };
    const posterUrl = optionalText(entry.posterUrl);
    if (posterUrl) item.posterUrl = posterUrl;
    media.push(item);
  }
  return media;
}

function normaliseWallpaperCategory(
  value: unknown,
  fallback: WallpaperCategory,
): WallpaperCategory {
  return oneOf(value, WALLPAPER_CATEGORIES, fallback);
}

function normaliseWallpaperCandidates(
  value: unknown,
  category: WallpaperCategory,
): WallpaperCandidateView[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const candidates: WallpaperCandidateView[] = [];
  for (const entry of value) {
    if (!isRecord(entry)) continue;
    const id = text(entry.id);
    const thumbnailUrl = text(entry.thumbnailUrl);
    if (!id || !thumbnailUrl || seen.has(id)) continue;
    seen.add(id);
    candidates.push({
      id,
      title: text(entry.title, "Wallpaper"),
      thumbnailUrl,
      // A candidate inherits the row it was fetched for unless it says
      // otherwise, so a backend that omits the field still lands correctly.
      category: normaliseWallpaperCategory(entry.category, category),
    });
  }
  return candidates;
}

export function normaliseWallpaperSearch(
  value: unknown,
  category: WallpaperCategory = "background",
): WallpaperSearchView {
  if (!isRecord(value)) {
    return {
      phase: "error",
      source: "steam-store",
      category,
      query: "",
      message: "",
      candidates: [],
    };
  }
  const resolved = normaliseWallpaperCategory(value.category, category);
  return {
    phase: oneOf(value.phase, ["ready", "not-configured", "error"] as const, "error"),
    source: oneOf(
      value.source,
      ["steam-store", "wikimedia", "openverse", "igdb", "google-images"] as const,
      "steam-store",
    ),
    category: resolved,
    query: text(value.query),
    message: text(value.message),
    candidates: normaliseWallpaperCandidates(value.candidates, resolved),
  };
}

function normaliseSummary(value: unknown): GameSummary | null {
  if (!isRecord(value)) return null;
  const id = text(value.id);
  if (!id) return null;
  const platforms = stringList(value.supportedPlatforms).filter(
    (platform): platform is GameSummary["supportedPlatforms"][number] =>
      ["windows", "macos", "linux", "ios", "android"].includes(platform),
  );
  return {
    id,
    title: text(value.title, "Untitled game"),
    source: oneOf(value.source, ["steam", "wine", "local", "showcase", "store"] as const, "local"),
    shortDescription: text(value.shortDescription),
    coverUrl: text(value.coverUrl),
    heroUrl: text(value.heroUrl),
    landscapeUrl: text(value.landscapeUrl),
    genres: visibleLabels(stringList(value.genres)),
    tags: visibleLabels(stringList(value.tags)),
    supportedPlatforms: platforms,
    owned: value.owned === true,
    launchable: value.launchable === true,
    wishlisted: value.wishlisted === true,
    playTimeSeconds: wholeNumber(value.playTimeSeconds),
    lastPlayedAt: optionalText(value.lastPlayedAt),
    recommendationReasons: stringList(value.recommendationReasons),
    offers: normaliseOffers(value.offers),
  };
}

function normaliseOffers(value: unknown): StoreOffer[] {
  if (!Array.isArray(value)) return [];
  const offers: StoreOffer[] = [];
  for (const entry of value) {
    if (!isRecord(entry)) continue;
    const id = text(entry.id);
    if (!id) continue;
    offers.push({
      id,
      gameId: text(entry.gameId),
      provider: oneOf(
        entry.provider,
        ["steam", "ubisoft", "microsoft", "apple", "google-play", "instant-gaming"] as const,
        "steam",
      ),
      providerLabel: text(entry.providerLabel, "Store"),
      priceMinor: typeof entry.priceMinor === "number" && Number.isFinite(entry.priceMinor)
        ? entry.priceMinor
        : null,
      currency: optionalText(entry.currency),
      region: text(entry.region),
      verifiedAt: optionalText(entry.verifiedAt),
      availability: oneOf(entry.availability, ["available", "unavailable", "unknown"] as const, "unknown"),
      stale: entry.stale === true,
    });
  }
  return offers;
}

function normaliseAchievements(value: unknown): GameDetailView["achievements"] {
  if (!isRecord(value)) return null;
  const total = wholeNumber(value.total);
  if (total === 0) return null;
  const unlocked = Math.min(wholeNumber(value.unlocked), total);
  const items = Array.isArray(value.items)
    ? value.items
        .filter(isRecord)
        .map((item) => ({
          id: text(item.id),
          title: text(item.title, "Achievement"),
          iconUrl: text(item.iconUrl),
        }))
        .filter((item) => item.id !== "")
    : [];
  return { unlocked, total, items };
}

function normaliseFriends(value: unknown): GameDetailFriend[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(isRecord)
    .map((entry) => ({
      id: text(entry.id),
      name: text(entry.name),
      avatarUrl: text(entry.avatarUrl),
      status: text(entry.status),
    }))
    .filter((entry) => entry.id !== "" && entry.name !== "");
}

function normaliseActivity(value: unknown): GameDetailActivityEntry[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter(isRecord)
    .map((entry) => ({
      id: text(entry.id),
      actorName: text(entry.actorName),
      summary: text(entry.summary),
      detail: text(entry.detail),
      avatarUrl: text(entry.avatarUrl),
      occurredAt: optionalText(entry.occurredAt),
    }))
    .filter((entry) => entry.id !== "" && entry.actorName !== "");
}

/**
 * Turns an unknown IPC payload into a renderable view model. Missing sections
 * become empty rather than throwing, which is what "partial sections" means
 * for this page.
 */
export function normaliseGameDetail(value: unknown): GameDetailViewModel | null {
  if (!isRecord(value)) return null;
  const summary = normaliseSummary(value);
  if (!summary) return null;
  const detail: GameDetailViewModel = {
    ...summary,
    about: text(value.about),
    developer: optionalText(value.developer),
    publisher: optionalText(value.publisher),
    releaseDate: optionalText(value.releaseDate),
    features: visibleLabels(stringList(value.features)),
    achievements: normaliseAchievements(value.achievements),
    media: normaliseGameMedia(value.media),
    relatedGames: Array.isArray(value.relatedGames)
      ? value.relatedGames
          .map(normaliseSummary)
          .filter((game): game is GameSummary => game !== null)
      : [],
    primaryAction: oneOf(value.primaryAction, PRIMARY_ACTIONS, "unavailable"),
  };
  const friends = normaliseFriends(value.friends);
  if (friends.length > 0) detail.friends = friends;
  const activity = normaliseActivity(value.activity);
  if (activity.length > 0) detail.activity = activity;
  if (typeof value.rating === "number" && Number.isFinite(value.rating) && value.rating > 0) {
    detail.rating = value.rating;
  }
  if (
    typeof value.reviewPercent === "number" &&
    Number.isFinite(value.reviewPercent) &&
    value.reviewPercent > 0
  ) {
    detail.reviewPercent = value.reviewPercent;
  }
  return detail;
}

/* -------------------------------------------------------------------------- */
/* Media grouping and selection                                                */
/* -------------------------------------------------------------------------- */

export function groupMediaByKind(media: GameMediaView[]): Record<GameMediaKind, GameMediaView[]> {
  const groups: Record<GameMediaKind, GameMediaView[]> = {
    wallpaper: [],
    video: [],
    icon: [],
    cover: [],
  };
  for (const item of media) groups[item.kind].push(item);
  return groups;
}

export function mediaForKind(media: GameMediaView[], kind: GameMediaKind): GameMediaView[] {
  return media.filter((item) => item.kind === kind);
}

export function availableMediaKinds(media: GameMediaView[]): GameMediaKind[] {
  const groups = groupMediaByKind(media);
  return MEDIA_KIND_IDS.filter((kind) => groups[kind].length > 0);
}

/** The kind the page should open on: the first kind that actually has media. */
export function defaultMediaKind(media: GameMediaView[]): GameMediaKind {
  return availableMediaKinds(media)[0] ?? "wallpaper";
}

export function selectedMediaId(media: GameMediaView[], kind: GameMediaKind): string | null {
  const group = mediaForKind(media, kind);
  return group.find((item) => item.selected)?.id ?? group[0]?.id ?? null;
}

export function findMedia(media: GameMediaView[], mediaId: string | null): GameMediaView | null {
  if (!mediaId) return null;
  return media.find((item) => item.id === mediaId) ?? null;
}

export function previewedMedia(state: GameDetailPageState): GameMediaView | null {
  return findMedia(state.media, state.previewMediaId);
}

/** `Apply` only makes sense when the preview differs from what is persisted. */
export function canApplyMedia(state: GameDetailPageState): boolean {
  if (state.mediaBusy || !state.previewMediaId) return false;
  return state.previewMediaId !== state.appliedMediaId;
}

export function canExportMedia(state: GameDetailPageState): boolean {
  return !state.mediaBusy && previewedMedia(state) !== null;
}

/** The image the hero should paint: previewed wallpaper first, then fallbacks. */
export function heroImageUrl(state: GameDetailPageState): string {
  const preview = previewedMedia(state);
  if (preview) {
    if (preview.kind === "video") return preview.posterUrl ?? state.detail?.heroUrl ?? "";
    if (preview.kind === "wallpaper" || preview.kind === "cover") return preview.previewUrl;
  }
  const wallpaper = mediaForKind(state.media, "wallpaper").find((item) => item.selected);
  return wallpaper?.previewUrl ?? state.detail?.heroUrl ?? state.detail?.landscapeUrl ?? "";
}

/* -------------------------------------------------------------------------- */
/* Offers and primary action                                                   */
/* -------------------------------------------------------------------------- */

export function isOfferUsable(offer: StoreOffer): boolean {
  return offer.availability !== "unavailable";
}

/**
 * Picks the offer a primary action should open. Deliberately independent from
 * the Store page so this page keeps working when Store internals change.
 */
export function selectActionOffer(
  detail: GameDetailView | null,
  preferred?: StoreOffer["provider"],
): StoreOffer | null {
  if (!detail || detail.offers.length === 0) return null;
  const ranked = [...detail.offers].sort((left, right) => {
    if (preferred) {
      const preference = Number(right.provider === preferred) - Number(left.provider === preferred);
      if (preference !== 0) return preference;
    }
    const usable = Number(isOfferUsable(right)) - Number(isOfferUsable(left));
    if (usable !== 0) return usable;
    const fresh = Number(left.stale) - Number(right.stale);
    if (fresh !== 0) return fresh;
    if (left.priceMinor === null) return 1;
    if (right.priceMinor === null) return -1;
    return left.priceMinor - right.priceMinor;
  });
  return ranked[0] ?? null;
}

export function resolvePrimaryAction(
  detail: GameDetailView | null,
  gameId: GameId | null,
): PrimaryActionDescriptor {
  const base: PrimaryActionDescriptor = {
    kind: "unavailable",
    label: "Unavailable",
    icon: "alert",
    intent: "none",
    route: null,
    offerId: null,
    disabled: true,
    hint: "This game cannot be started on this device yet.",
  };
  if (!detail || !gameId) return base;
  switch (detail.primaryAction) {
    case "play":
      return {
        ...base,
        kind: "play",
        label: "Play",
        icon: "play",
        intent: "play",
        disabled: false,
        hint: `Start ${detail.title}`,
      };
    case "install-steam": {
      const offer = selectActionOffer(detail, "steam");
      return {
        ...base,
        kind: "install-steam",
        label: "Install via Steam",
        icon: "steam",
        intent: offer ? "open-offer" : "none",
        offerId: offer?.id ?? null,
        disabled: !offer,
        hint: offer
          ? "Opens the Steam store page for this game."
          : "No Steam offer is available for this game.",
      };
    }
    case "configure-wine":
      return {
        ...base,
        kind: "configure-wine",
        label: "Configure Wine",
        icon: "settings",
        intent: "navigate",
        route: { page: "settings", section: "plugins", attachGameId: gameId },
        disabled: false,
        hint: "Set up a Wine profile before this game can run.",
      };
    case "view-offer": {
      const offer = selectActionOffer(detail);
      return {
        ...base,
        kind: "view-offer",
        label: "View offer",
        icon: "store",
        intent: offer ? "open-offer" : "none",
        offerId: offer?.id ?? null,
        disabled: !offer,
        hint: offer ? `Opens the ${offer.providerLabel} offer.` : "No offer is available yet.",
      };
    }
    case "unavailable":
      return base;
  }
}

/* -------------------------------------------------------------------------- */
/* Section predicates                                                          */
/* -------------------------------------------------------------------------- */

export function hasGalleryContent(detail: GameDetailViewModel | null): boolean {
  return Boolean(detail && detail.media.length > 0);
}

export function hasAboutContent(detail: GameDetailViewModel | null): boolean {
  return Boolean(detail && detail.about.trim().length > 0);
}

export function hasGameInfoContent(detail: GameDetailViewModel | null): boolean {
  if (!detail) return false;
  return Boolean(
    detail.developer ||
      detail.publisher ||
      detail.releaseDate ||
      detail.genres.length > 0 ||
      detail.supportedPlatforms.length > 0,
  );
}

export function hasFeaturesContent(detail: GameDetailViewModel | null): boolean {
  return Boolean(detail && detail.features.length > 0);
}

export function hasAchievementsContent(detail: GameDetailViewModel | null): boolean {
  return Boolean(detail?.achievements && detail.achievements.total > 0);
}

export function hasFriendsContent(detail: GameDetailViewModel | null): boolean {
  return Array.isArray(detail?.friends) && detail.friends.length > 0;
}

export function hasActivityContent(detail: GameDetailViewModel | null): boolean {
  return Array.isArray(detail?.activity) && detail.activity.length > 0;
}

export function hasRelatedContent(detail: GameDetailViewModel | null): boolean {
  return Boolean(detail && detail.relatedGames.length > 0);
}

const SECTION_PREDICATES: ReadonlyArray<
  [GameDetailSection, (detail: GameDetailViewModel | null) => boolean]
> = [
  ["gallery", hasGalleryContent],
  ["about", hasAboutContent],
  ["info", hasGameInfoContent],
  ["features", hasFeaturesContent],
  ["achievements", hasAchievementsContent],
  ["friends", hasFriendsContent],
  ["activity", hasActivityContent],
  ["related", hasRelatedContent],
];

export function shouldRenderSection(
  detail: GameDetailViewModel | null,
  section: GameDetailSection,
): boolean {
  return SECTION_PREDICATES.find(([id]) => id === section)?.[1](detail) ?? false;
}

export function visibleSections(detail: GameDetailViewModel | null): GameDetailSection[] {
  return SECTION_PREDICATES.filter(([, predicate]) => predicate(detail)).map(([id]) => id);
}

export function shouldOfferAboutToggle(about: string): boolean {
  return about.trim().length > ABOUT_CLAMP_THRESHOLD || about.trim().split(/\n/).length > 3;
}

/* -------------------------------------------------------------------------- */
/* Formatting                                                                  */
/* -------------------------------------------------------------------------- */

export function formatPlayTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "Not played yet";
  if (seconds < 3_600) return `Played ${Math.max(1, Math.round(seconds / 60))} min`;
  return `Played ${Math.round(seconds / 3_600)}h`;
}

export function formatLastPlayed(value: string | null, now = Date.now()): string {
  if (!value) return "Never played";
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "Last played recently";
  const elapsed = Math.max(0, now - timestamp);
  const minutes = Math.round(elapsed / 60_000);
  if (minutes < 2) return "Last played just now";
  if (minutes < 60) return `Last played ${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `Last played ${hours}h ago`;
  const days = Math.round(hours / 24);
  if (days === 1) return "Last played yesterday";
  if (days < 30) return `Last played ${days} days ago`;
  return `Last played ${formatReleaseDate(value)}`;
}

export function formatReleaseDate(value: string | null): string {
  if (!value) return "Release date unknown";
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return value;
  try {
    return new Intl.DateTimeFormat("en-US", {
      year: "numeric",
      month: "long",
      day: "numeric",
      timeZone: "UTC",
    }).format(new Date(timestamp));
  } catch {
    return value;
  }
}

export function releaseYear(value: string | null): string | null {
  if (!value) return null;
  const timestamp = Date.parse(value);
  if (Number.isFinite(timestamp)) return String(new Date(timestamp).getUTCFullYear());
  const match = /\b(\d{4})\b/.exec(value);
  return match ? match[1] : null;
}

export function formatAchievementProgress(
  achievements: GameDetailView["achievements"],
): { label: string; percent: number } | null {
  if (!achievements || achievements.total <= 0) return null;
  const percent = Math.round((achievements.unlocked / achievements.total) * 100);
  return {
    label: `${achievements.unlocked}/${achievements.total} unlocked`,
    percent: Math.min(100, Math.max(0, percent)),
  };
}

export function platformLabels(detail: GameDetailView | null): string[] {
  if (!detail) return [];
  const names: Record<GameSummary["supportedPlatforms"][number], string> = {
    windows: "Windows",
    macos: "macOS",
    linux: "Linux",
    ios: "iOS",
    android: "Android",
  };
  return detail.supportedPlatforms.map((platform) => names[platform]);
}

/** A genre string trimmed to its final word, e.g. "Action RPG" → "RPG". */
export function shortGenre(genre: string): string {
  const parts = genre.trim().split(/\s+/);
  return parts.length > 1 ? parts[parts.length - 1] : genre.trim();
}

/**
 * The dot-separated metadata row directly under the title, matching the design
 * reference: studio · year · genre · ★ rating · 👍 review score.
 */
export function buildMetaFacts(detail: GameDetailViewModel | null): GameDetailFact[] {
  if (!detail) return [];
  const facts: GameDetailFact[] = [];
  if (detail.developer) facts.push({ id: "developer", icon: null, text: detail.developer });
  const year = releaseYear(detail.releaseDate);
  if (year) facts.push({ id: "year", icon: "clock", text: year });
  if (detail.genres.length > 0) {
    facts.push({ id: "genre", icon: null, text: shortGenre(detail.genres[0]) });
  }
  if (typeof detail.rating === "number" && detail.rating > 0) {
    facts.push({
      id: "rating",
      icon: "star",
      text: detail.rating.toFixed(1),
      tone: "accent",
    });
  }
  if (typeof detail.reviewPercent === "number" && detail.reviewPercent > 0) {
    facts.push({
      id: "review",
      icon: "thumbs-up",
      text: `${Math.round(detail.reviewPercent)}%`,
      tone: "accent",
    });
  }
  return facts;
}

/** Maps a feature label to the outline glyph shown beside it in the reference. */
export function featureIcon(feature: string): IconName {
  const label = feature.toLowerCase();
  if (label.includes("co-op") || label.includes("coop") || label.includes("multiplayer")) {
    return "users";
  }
  if (label.includes("controller") || label.includes("gamepad")) return "gamepad";
  if (label.includes("cloud")) return "cloud";
  if (label.includes("achievement")) return "trophy";
  if (label.includes("single") || label.includes("player") || label.includes("pvp")) return "user";
  return "navigate";
}

/**
 * The stats row under the primary action. Each fact appears only when it has
 * real data: a never-played game shows neither a playtime nor a last-played
 * entry rather than "Not played yet" placeholders.
 */
export function buildStatFacts(
  detail: GameDetailViewModel | null,
  now = Date.now(),
): GameDetailFact[] {
  if (!detail) return [];
  const facts: GameDetailFact[] = [];
  if (detail.playTimeSeconds > 0) {
    facts.push({ id: "playtime", icon: "clock", text: formatPlayTime(detail.playTimeSeconds) });
  }
  if (detail.lastPlayedAt) {
    facts.push({ id: "last-played", icon: "clock", text: formatLastPlayed(detail.lastPlayedAt, now) });
  }
  const progress = formatAchievementProgress(detail.achievements);
  if (progress) {
    facts.push({
      id: "achievements",
      icon: "trophy",
      text: `Achievements ${progress.label.replace(" unlocked", "")}`,
    });
  }
  return facts;
}

export function formatRelativeTime(value: string | null, now = Date.now()): string {
  if (!value) return "";
  const timestamp = Date.parse(value);
  if (!Number.isFinite(timestamp)) return "";
  const elapsed = Math.max(0, now - timestamp);
  const minutes = Math.round(elapsed / 60_000);
  if (minutes < 60) return `${Math.max(1, minutes)}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.round(hours / 24)}d ago`;
}

/* -------------------------------------------------------------------------- */
/* Restore state                                                               */
/* -------------------------------------------------------------------------- */

export interface GameDetailRestoreSelection {
  scrollTop: number;
  focusKey: string | null;
  selectedGameId: GameId | null;
  activeMediaKind: GameMediaKind | null;
  previewMediaId: string | null;
}

export function toGameDetailRestoreState(
  state: GameDetailPageState,
  scrollTop: number,
  focusKey: string | null,
): PageRestoreState {
  const filters = [`${RESTORE_KIND_PREFIX}${state.activeMediaKind}`];
  if (state.previewMediaId) filters.push(`${RESTORE_MEDIA_PREFIX}${state.previewMediaId}`);
  const restore: PageRestoreState = {
    scrollTop: Number.isFinite(scrollTop) ? Math.max(0, Math.round(scrollTop)) : 0,
    focusKey,
    filters,
  };
  if (state.gameId) restore.selectedGameId = state.gameId;
  return restore;
}

export function readGameDetailRestoreState(
  restore: PageRestoreState | null,
): GameDetailRestoreSelection {
  const empty: GameDetailRestoreSelection = {
    scrollTop: 0,
    focusKey: null,
    selectedGameId: null,
    activeMediaKind: null,
    previewMediaId: null,
  };
  if (!restore) return empty;
  const filters = Array.isArray(restore.filters) ? restore.filters : [];
  const kindEntry = filters.find((entry) => entry.startsWith(RESTORE_KIND_PREFIX));
  const mediaEntry = filters.find((entry) => entry.startsWith(RESTORE_MEDIA_PREFIX));
  const kind = kindEntry?.slice(RESTORE_KIND_PREFIX.length) ?? "";
  return {
    scrollTop: Number.isFinite(restore.scrollTop) ? Math.max(0, restore.scrollTop) : 0,
    focusKey: restore.focusKey ?? null,
    selectedGameId: restore.selectedGameId ?? null,
    activeMediaKind: (MEDIA_KIND_IDS as readonly string[]).includes(kind)
      ? (kind as GameMediaKind)
      : null,
    previewMediaId: mediaEntry ? mediaEntry.slice(RESTORE_MEDIA_PREFIX.length) || null : null,
  };
}

/* -------------------------------------------------------------------------- */
/* Reducer                                                                     */
/* -------------------------------------------------------------------------- */

export function createInitialGameDetailState(): GameDetailPageState {
  return {
    phase: "idle",
    gameId: null,
    from: null,
    detail: null,
    media: [],
    activeMediaKind: "wallpaper",
    previewMediaId: null,
    appliedMediaId: null,
    mediaBusy: false,
    mediaError: "",
    wallpaperSearch: {
      open: false,
      source: "steam-store",
      query: "",
      focus: null,
      categories: createWallpaperCategories(),
      activeId: null,
    },
    aboutExpanded: false,
    statusMessage: "",
    errorMessage: "",
    activeRequestId: 0,
    pendingRestore: null,
  };
}

function withMedia(
  state: GameDetailPageState,
  media: GameMediaView[],
  kind: GameMediaKind,
): GameDetailPageState {
  const applied = selectedMediaId(media, kind);
  return {
    ...state,
    media,
    detail: state.detail ? { ...state.detail, media } : state.detail,
    activeMediaKind: kind,
    previewMediaId: applied,
    appliedMediaId: applied,
  };
}

function applyRestore(
  state: GameDetailPageState,
  restore: PageRestoreState | null,
): GameDetailPageState {
  const selection = readGameDetailRestoreState(restore);
  if (!selection.selectedGameId || selection.selectedGameId !== state.gameId) return state;
  const kind =
    selection.activeMediaKind && mediaForKind(state.media, selection.activeMediaKind).length > 0
      ? selection.activeMediaKind
      : state.activeMediaKind;
  const previewed = findMedia(state.media, selection.previewMediaId);
  return {
    ...state,
    activeMediaKind: kind,
    previewMediaId: previewed && previewed.kind === kind ? previewed.id : selectedMediaId(state.media, kind),
    appliedMediaId: selectedMediaId(state.media, kind),
  };
}

export function reduceGameDetailState(
  state: GameDetailPageState,
  action: GameDetailPageAction,
): GameDetailPageState {
  switch (action.type) {
    case "activate": {
      const sameGame = state.gameId === action.gameId && state.detail !== null;
      const base = sameGame ? state : createInitialGameDetailState();
      return {
        ...base,
        gameId: action.gameId,
        from: action.from,
        phase: action.online ? (sameGame ? base.phase : "loading") : "offline",
        mediaBusy: false,
        mediaError: "",
        statusMessage: "",
        errorMessage: action.online ? "" : "You are offline. Showing the last saved details.",
        pendingRestore: action.restore,
      };
    }
    case "request-started":
      return {
        ...state,
        activeRequestId: action.requestId,
        phase: state.detail ? state.phase : "loading",
        errorMessage: "",
      };
    case "detail-loaded": {
      if (action.requestId !== state.activeRequestId) return state;
      if (!action.detail) {
        return {
          ...state,
          phase: "not-found",
          detail: null,
          media: [],
          previewMediaId: null,
          appliedMediaId: null,
          pendingRestore: null,
          errorMessage: "",
        };
      }
      const media = action.detail.media;
      const kind = defaultMediaKind(media);
      const loaded: GameDetailPageState = {
        ...state,
        phase: "ready",
        detail: action.detail,
        errorMessage: "",
        mediaError: "",
        aboutExpanded: false,
      };
      const withSelection = withMedia(loaded, media, kind);
      const restored = applyRestore(withSelection, state.pendingRestore);
      return { ...restored, pendingRestore: null };
    }
    case "request-failed":
      if (action.requestId !== state.activeRequestId) return state;
      return {
        ...state,
        phase: action.offline ? "offline" : state.detail ? "ready" : "error",
        errorMessage: action.message,
      };
    case "media-kind-changed": {
      if (action.kind === state.activeMediaKind) return state;
      const applied = selectedMediaId(state.media, action.kind);
      return {
        ...state,
        activeMediaKind: action.kind,
        previewMediaId: applied,
        appliedMediaId: applied,
        mediaError: "",
      };
    }
    case "media-previewed": {
      const media = findMedia(state.media, action.mediaId);
      if (!media) return state;
      return {
        ...state,
        activeMediaKind: media.kind,
        previewMediaId: media.id,
        appliedMediaId:
          media.kind === state.activeMediaKind
            ? state.appliedMediaId
            : selectedMediaId(state.media, media.kind),
        mediaError: "",
      };
    }
    case "media-busy-changed":
      return { ...state, mediaBusy: action.busy, mediaError: action.busy ? "" : state.mediaError };
    case "media-committed":
      return {
        ...withMedia(state, action.media, state.activeMediaKind),
        mediaBusy: false,
        mediaError: "",
      };
    case "media-imported": {
      const kind = mediaForKind(action.media, state.activeMediaKind).length > 0
        ? state.activeMediaKind
        : defaultMediaKind(action.media);
      return { ...withMedia(state, action.media, kind), mediaBusy: false, mediaError: "" };
    }
    case "media-failed":
      // The previous selection stands; only the inline error changes.
      return {
        ...state,
        mediaBusy: false,
        previewMediaId: state.appliedMediaId ?? state.previewMediaId,
        mediaError: action.message,
      };
    case "wallpaper-search-opened": {
      const wallpapers = mediaForKind(state.media, "wallpaper");
      const preview = findMedia(state.media, state.previewMediaId);
      return {
        ...state,
        wallpaperSearch: {
          ...state.wallpaperSearch,
          open: true,
          // A fresh session starts on the game title, but never clobbers what
          // the user already typed.
          query: state.wallpaperSearch.query || state.detail?.title || "",
          // Open on the wallpaper being previewed (the most recent intent),
          // else the last slide the user was on, else the first wallpaper.
          activeId:
            (preview && preview.kind === "wallpaper" ? preview.id : null) ??
            state.wallpaperSearch.activeId ??
            wallpapers[0]?.id ??
            null,
        },
      };
    }
    // `busy` deliberately survives a close: the request is genuinely still in
    // flight, and its result still lands in the row. Clearing it here would let
    // a reopen fire a second request for the same row, which then merges as a
    // "search more" page and duplicates every tile — two tiles sharing one id,
    // so ticking either lights up both.
    case "wallpaper-search-closed":
      return { ...state, wallpaperSearch: { ...state.wallpaperSearch, open: false } };
    case "wallpaper-search-source-changed":
      // A different source has different artwork, so every row's results are
      // stale the moment the source changes.
      return state.wallpaperSearch.source === action.source
        ? state
        : {
            ...state,
            wallpaperSearch: {
              ...state.wallpaperSearch,
              source: action.source,
              categories: createWallpaperCategories(),
            },
          };
    case "wallpaper-search-query-changed":
      return { ...state, wallpaperSearch: { ...state.wallpaperSearch, query: action.query } };
    case "wallpaper-search-focus-changed":
      return state.wallpaperSearch.focus === action.focus
        ? state
        : { ...state, wallpaperSearch: { ...state.wallpaperSearch, focus: action.focus } };
    case "wallpaper-search-started": {
      const row = state.wallpaperSearch.categories[action.category];
      // A fresh search restarts at zero; "search more" keeps the page offset.
      return {
        ...state,
        wallpaperSearch: updateWallpaperCategory(state.wallpaperSearch, action.category, {
          ...row,
          busy: true,
          phase: "idle",
          message: "",
          offset: action.more ? row.offset : 0,
          candidates: action.more ? row.candidates : [],
        }),
      };
    }
    case "wallpaper-search-results": {
      const row = state.wallpaperSearch.categories[action.category];
      // A "search more" search starts at a non-zero offset; a fresh search at
      // zero, so the two merge differently.
      const more = row.offset > 0;
      const incoming = action.results.candidates;
      const candidates = more ? [...row.candidates, ...incoming] : incoming;
      return {
        ...state,
        wallpaperSearch: {
          ...updateWallpaperCategory(state.wallpaperSearch, action.category, {
            ...row,
            busy: false,
            phase: action.results.phase,
            message: action.results.message,
            candidates,
            hasMore: incoming.length > 0,
            offset: row.offset + incoming.length,
          }),
          query: action.results.query,
          activeId: state.wallpaperSearch.activeId ?? candidates[0]?.id ?? null,
        },
      };
    }
    case "wallpaper-search-failed": {
      const row = state.wallpaperSearch.categories[action.category];
      return {
        ...state,
        wallpaperSearch: updateWallpaperCategory(state.wallpaperSearch, action.category, {
          ...row,
          busy: false,
          phase: "error",
          message: action.message,
        }),
      };
    }
    case "wallpaper-slide-changed":
      return {
        ...state,
        wallpaperSearch: { ...state.wallpaperSearch, activeId: action.slideId },
      };
    case "wishlist-changed":
      return state.detail
        ? { ...state, detail: { ...state.detail, wishlisted: action.wishlisted } }
        : state;
    case "about-toggled":
      return { ...state, aboutExpanded: !state.aboutExpanded };
    case "status-changed":
      return { ...state, statusMessage: action.message };
    case "connectivity-changed":
      return action.online
        ? {
            ...state,
            phase: state.phase === "offline" ? (state.detail ? "ready" : "loading") : state.phase,
            errorMessage: state.phase === "offline" ? "" : state.errorMessage,
          }
        : {
            ...state,
            phase: "offline",
            errorMessage: "You are offline. Showing the last saved details.",
          };
  }
}

/* -------------------------------------------------------------------------- */
/* Browser-only fallback                                                       */
/* -------------------------------------------------------------------------- */

const FALLBACK_HERO = "/media/igdb/heroes/elden-ring-wallpaper.png";
const FALLBACK_LANDSCAPE = "/media/igdb/landscapes/elden-ring.jpg";

/**
 * A cinematic set that mirrors the design reference's vertical media rail: a
 * trailer, the key art (applied to the hero), a landscape, and enough extra
 * wallpapers that the rail closes on a "+12" browse-more tile.
 */
const FALLBACK_MEDIA: GameMediaView[] = [
  {
    id: "media_fallback_trailer",
    kind: "video",
    title: "Official trailer",
    previewUrl: FALLBACK_LANDSCAPE,
    posterUrl: FALLBACK_LANDSCAPE,
    origin: "bundled",
    selected: false,
    availableOffline: true,
  },
  {
    id: "media_fallback_wallpaper_hero",
    kind: "wallpaper",
    title: "Key art",
    previewUrl: FALLBACK_HERO,
    origin: "bundled",
    selected: true,
    availableOffline: true,
  },
  {
    id: "media_fallback_wallpaper_landscape",
    kind: "wallpaper",
    title: "Landscape",
    previewUrl: FALLBACK_LANDSCAPE,
    origin: "bundled",
    selected: false,
    availableOffline: true,
  },
  // Twelve more wallpapers so the rail's fourth tile reads "+12", like the
  // reference. They alternate the two bundled Elden Ring plates.
  ...Array.from({ length: 12 }, (_, index) => ({
    id: `media_fallback_wallpaper_extra_${index + 1}`,
    kind: "wallpaper" as const,
    title: `Wallpaper ${index + 1}`,
    previewUrl: index % 2 === 0 ? FALLBACK_HERO : FALLBACK_LANDSCAPE,
    origin: "bundled" as const,
    selected: false,
    availableOffline: true,
  })),
  {
    id: "media_fallback_cover",
    kind: "cover",
    title: "Standard cover",
    previewUrl: "/media/igdb/covers/elden-ring.jpg",
    origin: "bundled",
    selected: true,
    availableOffline: true,
  },
  {
    id: "media_fallback_icon",
    kind: "icon",
    title: "Ring icon",
    previewUrl: "/media/orivo-ring-icon.png",
    origin: "bundled",
    selected: true,
    availableOffline: true,
  },
];

const FALLBACK_AVATARS = ["/media/steam-avatar.png", "/media/avatar-reference.png"];

/** The four named friends the reference shows, then anonymous extras up to +24. */
const FALLBACK_FRIENDS: GameDetailFriend[] = Array.from({ length: 28 }, (_, index) => {
  const named = ["Valkyrie", "PixelNinja", "StormRider", "Ashen"];
  return {
    id: `friend_${index + 1}`,
    name: named[index] ?? `Tarnished ${index + 1}`,
    avatarUrl: FALLBACK_AVATARS[index % FALLBACK_AVATARS.length],
    status: index % 2 === 0 ? "In game" : "Online",
  };
});

function fallbackRelated(
  id: string,
  title: string,
  landscape: string,
  cover: string,
): GameSummary {
  return {
    id,
    title,
    source: "local",
    shortDescription: "",
    coverUrl: cover,
    heroUrl: landscape,
    landscapeUrl: landscape,
    genres: ["Action"],
    tags: [],
    supportedPlatforms: ["windows"],
    owned: false,
    launchable: false,
    wishlisted: false,
    playTimeSeconds: 0,
    lastPlayedAt: null,
    recommendationReasons: [],
    offers: [],
  };
}

/**
 * Used only outside the Tauri runtime so `pnpm dev` renders something real.
 * It never reaches production because `isTauriRuntime()` gates it.
 */
/**
 * Used only outside the Tauri runtime so `pnpm dev` renders something real.
 * It never reaches production because `isTauriRuntime()` gates it.
 */
/**
 * The browser fallback, one deterministic page per category. The bundled mock
 * art is already filed by shape — `covers/` is portrait, `landscapes/` and
 * `heroes/` are wide — so the three rows render at their real proportions
 * without a backend.
 */
export function createFallbackWallpaperSearch(
  source: WallpaperSource,
  category: WallpaperCategory,
  query: string,
  offset = 0,
): WallpaperSearchView {
  const stems: Record<WallpaperCategory, ReadonlyArray<readonly [string, string]>> = {
    cover: [
      ["Elden Ring", "/media/igdb/covers/elden-ring.jpg"],
      ["Baldur's Gate 3", "/media/igdb/covers/baldurs-gate-3.jpg"],
      ["Red Dead Redemption 2", "/media/igdb/covers/red-dead-redemption-2.jpg"],
      ["The Witcher 3", "/media/igdb/covers/the-witcher-3-wild-hunt.jpg"],
      ["God of War", "/media/igdb/covers/god-of-war.jpg"],
      ["Hades II", "/media/igdb/covers/hades-2.jpg"],
    ],
    landscape: [
      ["Elden Ring", "/media/igdb/landscapes/elden-ring.jpg"],
      ["Cyberpunk 2077", "/media/igdb/landscapes/cyberpunk-2077.webp"],
      ["Horizon Forbidden West", "/media/igdb/landscapes/horizon-forbidden-west.jpg"],
      ["God of War", "/media/igdb/landscapes/god-of-war.jpg"],
      ["Baldur's Gate 3", "/media/igdb/landscapes/baldurs-gate-3.jpg"],
      ["Unrailed!", "/media/igdb/landscapes/unrailed.jpg"],
    ],
    background: [
      ["Elden Ring", "/media/igdb/heroes/elden-ring-wallpaper.png"],
      ["Cyberpunk 2077", "/media/igdb/heroes/cyberpunk-2077.webp"],
      ["Horizon Forbidden West", "/media/igdb/heroes/horizon-forbidden-west.jpg"],
      ["Red Dead Redemption 2", "/media/igdb/heroes/red-dead-redemption-2.jpg"],
      ["The Witcher 3", "/media/igdb/heroes/the-witcher-3-wild-hunt.jpg"],
      ["Hades II", "/media/igdb/heroes/hades-2.jpg"],
    ],
  };
  return {
    phase: "ready",
    source,
    category,
    query,
    message: "",
    // 5 results, then 1, then none — enough to exercise "Voir tout".
    candidates: stems[category]
      .slice(offset, offset + 5)
      .map(([title, thumbnailUrl], index) => ({
        id: `candidate-${category}-${offset + index + 1}`,
        title,
        thumbnailUrl,
        category,
      })),
  };
}

/** The browser fallback stands in for the download+register command. */
export function createFallbackImportedWallpaper(): GameMediaView {
  return {
    id: "media_fallback_wallpaper_searched",
    kind: "wallpaper",
    title: "Searched wallpaper",
    previewUrl: "/media/igdb/landscapes/cyberpunk-2077.webp",
    origin: "imported",
    selected: true,
    availableOffline: true,
  };
}

export function createFallbackGameDetail(gameId: GameId): GameDetailViewModel {
  return {
    id: gameId,
    title: "Elden Ring",
    source: "local",
    shortDescription: "A vast world full of mystery and peril. What will you discover?",
    coverUrl: "/media/igdb/covers/elden-ring.jpg",
    heroUrl: "/media/igdb/heroes/elden-ring-wallpaper.png",
    landscapeUrl: "/media/igdb/landscapes/elden-ring.jpg",
    genres: ["Action RPG"],
    tags: ["Open world", "Souls-like"],
    supportedPlatforms: ["windows"],
    owned: true,
    launchable: true,
    wishlisted: false,
    playTimeSeconds: 460_800,
    lastPlayedAt: new Date(Date.now() - 2 * 24 * 60 * 60 * 1_000).toISOString(),
    recommendationReasons: [],
    offers: [],
    about:
      "The Golden Order has been broken.\nRise, Tarnished, and be guided by grace to brandish the power of the Elden Ring and become an Elden Lord in the Lands Between.\nA vast world where open fields with a variety of situations and huge dungeons with complex designs are seamlessly connected. As you explore, the joy of discovery unfolds before you.",
    developer: "FromSoftware Inc.",
    publisher: "Bandai Namco Entertainment",
    releaseDate: "2022-02-25",
    features: [
      "Single-player",
      "Online PvP",
      "Co-op",
      "Steam Achievements",
      "Full Controller Support",
      "Cloud Saves",
    ],
    rating: 4.8,
    reviewPercent: 97,
    achievements: {
      unlocked: 67,
      total: 82,
      items: [
        { id: "ach_1", title: "Elden Lord", iconUrl: "/media/igdb/covers/elden-ring.jpg" },
        { id: "ach_2", title: "Shardbearer", iconUrl: "/media/igdb/covers/god-of-war.jpg" },
        { id: "ach_3", title: "Great Rune", iconUrl: "/media/igdb/covers/hades-2.jpg" },
        {
          id: "ach_4",
          title: "Legendary Armaments",
          iconUrl: "/media/igdb/covers/horizon-forbidden-west.jpg",
        },
        {
          id: "ach_5",
          title: "Roundtable Hold",
          iconUrl: "/media/igdb/covers/the-witcher-3-wild-hunt.jpg",
        },
      ],
    },
    media: FALLBACK_MEDIA.map((item) => ({ ...item })),
    relatedGames: [
      fallbackRelated(
        "related_sekiro",
        "Sekiro: Shadows Die Twice",
        "/media/igdb/landscapes/god-of-war.jpg",
        "/media/igdb/covers/god-of-war.jpg",
      ),
      fallbackRelated(
        "related_dark_souls",
        "Dark Souls III",
        "/media/igdb/landscapes/red-dead-redemption-2.jpg",
        "/media/igdb/covers/red-dead-redemption-2.jpg",
      ),
      fallbackRelated(
        "related_bloodborne",
        "Bloodborne",
        "/media/igdb/landscapes/the-witcher-3-wild-hunt.jpg",
        "/media/igdb/covers/the-witcher-3-wild-hunt.jpg",
      ),
      fallbackRelated(
        "related_lies_of_p",
        "Lies of P",
        "/media/igdb/landscapes/horizon-forbidden-west.jpg",
        "/media/igdb/covers/horizon-forbidden-west.jpg",
      ),
    ],
    friends: FALLBACK_FRIENDS.map((friend) => ({ ...friend })),
    activity: [
      {
        id: "activity_1",
        actorName: "Valkyrie",
        summary: "Earned achievement",
        detail: "Shardbearer Godrick",
        avatarUrl: FALLBACK_AVATARS[0],
        occurredAt: new Date(Date.now() - 2 * 60 * 60 * 1_000).toISOString(),
      },
      {
        id: "activity_2",
        actorName: "PixelNinja",
        summary: "Reached 100 hours played",
        detail: "",
        avatarUrl: FALLBACK_AVATARS[1],
        occurredAt: new Date(Date.now() - 24 * 60 * 60 * 1_000).toISOString(),
      },
      {
        id: "activity_3",
        actorName: "StormRider",
        summary: "Defeated Starscourge Radahn",
        detail: "",
        avatarUrl: FALLBACK_AVATARS[0],
        occurredAt: new Date(Date.now() - 3 * 24 * 60 * 60 * 1_000).toISOString(),
      },
    ],
    primaryAction: "play",
  };
}

/**
 * Debug overlay: grafts the fallback's achievements, friends, activity and
 * related games onto a real detail that ships none, so the social sections can
 * be reviewed without a backend feed. Anything the game already carries is left
 * untouched. Gated behind a Settings toggle; never runs in a normal session.
 */
export function withSampleSocialData(detail: GameDetailViewModel): GameDetailViewModel {
  const sample = createFallbackGameDetail(detail.id);
  const next: GameDetailViewModel = { ...detail };
  if (!hasAchievementsContent(detail)) next.achievements = sample.achievements;
  if (!hasFriendsContent(detail)) next.friends = sample.friends;
  if (!hasActivityContent(detail)) next.activity = sample.activity;
  if (!hasRelatedContent(detail)) next.relatedGames = sample.relatedGames;
  return next;
}
