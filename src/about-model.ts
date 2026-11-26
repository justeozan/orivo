import type { IconName } from "./icons";
import { firstSentence, releaseBullets, shortenLine, type UpdateState } from "./updater-model";

/**
 * What Settings › About lists under "What's new": one large line per addition,
 * read from the release notes Orivo already has. Pure, so the choice of which
 * version speaks and how each bullet is cut can be tested without a shell.
 */

/** The plugin shelves' colours, reused so a news tile reads like a plugin tile. */
export type NewsCategory = "emulation" | "library" | "visuals" | "saves" | "music" | "streaming";

export interface NewsItem {
  title: string;
  /** The next sentence, shortened; empty when the bullet had only one. */
  detail: string;
  category: NewsCategory;
  icon: IconName;
}

export interface AboutNews {
  /** The version these items describe — not always the one running. */
  version: string;
  items: NewsItem[];
}

/** Written large, so a few: the full notes are the changelog's job. */
const MAX_ITEMS = 4;
const MAX_TITLE_LENGTH = 90;
const MAX_DETAIL_LENGTH = 110;

/**
 * A tile is coloured and drawn after what the line is about, when one of these
 * words says so; the rest take the shelf colours in turn, so neighbours differ.
 */
const TOPICS: ReadonlyArray<{ pattern: RegExp; category: NewsCategory; icon: IconName }> = [
  { pattern: /\b(stream\w*|moonlight|artemis|sunshine)\b/i, category: "streaming", icon: "cast" },
  { pattern: /\b(switch|emulat\w*|ryujinx|console\w*)\b/i, category: "emulation", icon: "gamepad" },
  { pattern: /\b(librar\w*|sources?|sort\w*|rail)\b/i, category: "library", icon: "library" },
  { pattern: /\b(folders?|moved|disk|drive)\b/i, category: "visuals", icon: "folder" },
  { pattern: /\b(artwork|trailers?|screenshots?|wallpapers?|covers?)\b/i, category: "visuals", icon: "landscape" },
  { pattern: /\b(store|prices?|wishlist)\b/i, category: "saves", icon: "store" },
  { pattern: /\b(iphone|ipad|android|phone)\b/i, category: "music", icon: "monitor" },
];
const ROTATION: readonly NewsCategory[] = ["emulation", "library", "visuals", "saves", "music"];

/** The additions and changes a release body names, ready to be shown large. */
export function releaseNews(body: string | null | undefined): NewsItem[] {
  return releaseBullets(body)
    .slice(0, MAX_ITEMS)
    .map((text, index) => {
      const title = shortenLine(firstSentence(text), MAX_TITLE_LENGTH);
      const head = /^.+?[.!?](\s+|$)/.exec(text)?.[0] ?? text;
      const rest = text.slice(head.length);
      const detail = rest ? shortenLine(firstSentence(rest), MAX_DETAIL_LENGTH) : "";
      const topic = TOPICS.find((candidate) => candidate.pattern.test(title));
      return {
        title,
        detail,
        category: topic?.category ?? ROTATION[index % ROTATION.length],
        icon: topic?.icon ?? "sparkle",
      };
    });
}

/**
 * The section of CHANGELOG.md for `version`, or for the newest released version
 * before it when that one has no section of its own (a patch release made of
 * fixes only, or one cut before the notes were written). "Unreleased" is never
 * taken: it is not in anyone's hands yet.
 */
export function changelogSection(
  markdown: string | null | undefined,
  version: string | null | undefined,
): { version: string; body: string } | null {
  if (!markdown) return null;
  const target = parseVersion(version);
  let best: { version: string; parsed: number[]; body: string } | null = null;
  const sections = markdown.split(/^(?=##\s+\[)/m);
  for (const section of sections) {
    const heading = /^##\s+\[([^\]]+)\]/.exec(section);
    if (!heading) continue;
    const parsed = parseVersion(heading[1]);
    if (!parsed) continue;
    if (target && compareVersions(parsed, target) > 0) continue;
    if (!best || compareVersions(parsed, best.parsed) > 0) {
      best = { version: heading[1].trim(), parsed, body: section };
    }
  }
  return best ? { version: best.version, body: best.body } : null;
}

/**
 * Which notes the About page shows. A pending update speaks first, because what
 * it brings is what the button above installs; when its notes say nothing
 * readable, the running version's own changelog section does.
 */
export function aboutNews(
  update: UpdateState,
  changelog: string | null,
  runningVersion: string | null,
): AboutNews | null {
  const pending =
    update.status === "available" || update.status === "downloading" || update.status === "ready";
  if (pending && update.availableVersion) {
    const items = releaseNews(update.notes);
    if (items.length > 0) return { version: update.availableVersion, items };
  }
  const section = changelogSection(changelog, runningVersion);
  if (!section) return null;
  const items = releaseNews(section.body);
  return items.length > 0 ? { version: section.version, items } : null;
}

function parseVersion(value: string | null | undefined): number[] | null {
  const match = /^v?(\d+)\.(\d+)\.(\d+)/.exec(value?.trim() ?? "");
  return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : null;
}

function compareVersions(left: number[], right: number[]): number {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}
