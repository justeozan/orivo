import type { MessageKey } from "./i18n";
import type { IconName } from "./icons";

/**
 * What each release brings, in a few words, for Settings › About and the top
 * of the GitHub release notes. Bundled rather than read from the release body:
 * the body is PR titles in English, and these are short and translated.
 *
 * A PR with something a player would notice adds its items to the "next"
 * bucket (text in `src/locales/highlights.ts`); `tag.yml` renames that bucket
 * to the version it cuts, through `scripts/release-highlights.mjs stamp`. A PR
 * with nothing to show adds nothing. Newest first, at most four per release.
 *
 * Only type imports here and plain object literals below, in this layout: Node
 * loads this file with its type stripping, and the stamp rewrites the
 * `version: "next",` line in place.
 */

/** A plugin shelf's colour, so a highlight's tile reads like a plugin tile. */
export type HighlightTone =
  | "emulation"
  | "library"
  | "visuals"
  | "saves"
  | "music"
  | "streaming"
  | "compatibility"
  | "tools";

export interface Highlight {
  icon: IconName;
  tone: HighlightTone;
  titleKey: MessageKey;
  lineKey: MessageKey;
}

export interface ReleaseHighlights {
  /** A released version, or "next" for what is merged but not yet released. */
  version: string;
  items: readonly Highlight[];
}

export const NEXT_RELEASE = "next";

export const RELEASE_HIGHLIGHTS: readonly ReleaseHighlights[] = [
  {
    version: "next",
    items: [],
  },
  {
    version: "0.4.25",
    items: [
      { icon: "play", tone: "library", titleKey: "highlights.iphoneVersion.title", lineKey: "highlights.iphoneVersion.line" },
    ],
  },
  {
    version: "0.4.12",
    items: [
      { icon: "sparkle", tone: "saves", titleKey: "highlights.aboutHighlights.title", lineKey: "highlights.aboutHighlights.line" },
    ],
  },
  {
    version: "0.4.9",
    items: [
      { icon: "puzzle", tone: "library", titleKey: "highlights.pluginStates.title", lineKey: "highlights.pluginStates.line" },
      { icon: "cast", tone: "streaming", titleKey: "highlights.streamingFlow.title", lineKey: "highlights.streamingFlow.line" },
      { icon: "folder", tone: "visuals", titleKey: "highlights.storage.title", lineKey: "highlights.storage.line" },
      { icon: "star", tone: "saves", titleKey: "highlights.about.title", lineKey: "highlights.about.line" },
    ],
  },
  {
    version: "0.4.8",
    items: [
      { icon: "users", tone: "library", titleKey: "highlights.accounts.title", lineKey: "highlights.accounts.line" },
    ],
  },
  {
    version: "0.4.7",
    items: [
      { icon: "sliders", tone: "library", titleKey: "highlights.general.title", lineKey: "highlights.general.line" },
      { icon: "sparkle", tone: "saves", titleKey: "highlights.motionAll.title", lineKey: "highlights.motionAll.line" },
    ],
  },
  {
    version: "0.4.6",
    items: [
      { icon: "download", tone: "music", titleKey: "highlights.officialPlugins.title", lineKey: "highlights.officialPlugins.line" },
    ],
  },
  {
    version: "0.4.5",
    items: [
      { icon: "cast", tone: "streaming", titleKey: "highlights.moonlightPlugin.title", lineKey: "highlights.moonlightPlugin.line" },
    ],
  },
  {
    version: "0.4.4",
    items: [
      { icon: "refresh", tone: "music", titleKey: "highlights.pluginUpdates.title", lineKey: "highlights.pluginUpdates.line" },
      { icon: "grid", tone: "library", titleKey: "highlights.pluginCategories.title", lineKey: "highlights.pluginCategories.line" },
    ],
  },
  {
    version: "0.4.3",
    items: [
      { icon: "switch", tone: "emulation", titleKey: "highlights.switchPlugin.title", lineKey: "highlights.switchPlugin.line" },
    ],
  },
  {
    version: "0.4.2",
    items: [
      { icon: "folder", tone: "visuals", titleKey: "highlights.movedGames.title", lineKey: "highlights.movedGames.line" },
      { icon: "library", tone: "library", titleKey: "highlights.sourceBar.title", lineKey: "highlights.sourceBar.line" },
      { icon: "windows", tone: "compatibility", titleKey: "highlights.windowsAdmin.title", lineKey: "highlights.windowsAdmin.line" },
    ],
  },
];
