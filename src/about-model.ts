import { NEXT_RELEASE, type ReleaseHighlights } from "./release-highlights";

/**
 * Which release Settings › About shows under "What's new". Pure, so the choice
 * of version can be tested without a shell.
 */

/** Written large, so a few: the full list is the GitHub release's job. */
export const MAX_HIGHLIGHTS = 4;

/**
 * The running version's highlights, or, when it has none (a patch release made
 * of fixes only), the newest earlier release that has some, named with its own
 * version. "next" is never taken: nobody has it yet. A pending update is not
 * shown either: its highlights ship inside it, not in the copy running now.
 */
export function aboutHighlights(
  releases: readonly ReleaseHighlights[],
  runningVersion: string | null | undefined,
): ReleaseHighlights | null {
  const running = runningVersion?.trim().replace(/^v/, "") ?? "";
  const target = parseVersion(running);
  let best: { release: ReleaseHighlights; parsed: number[] } | null = null;
  for (const release of releases) {
    if (release.version === NEXT_RELEASE || release.items.length === 0) continue;
    if (release.version === running) return trimmed(release);
    const parsed = parseVersion(release.version);
    if (!parsed) continue;
    // Strictly older: a pre-release of the running version (0.5.0-rc.1 under
    // 0.5.0) parses equal and is not what this copy is.
    if (target && compareVersions(parsed, target) >= 0) continue;
    if (!best || compareVersions(parsed, best.parsed) > 0) best = { release, parsed };
  }
  return best ? trimmed(best.release) : null;
}

function trimmed(release: ReleaseHighlights): ReleaseHighlights {
  return { version: release.version, items: release.items.slice(0, MAX_HIGHLIGHTS) };
}

function parseVersion(value: string): number[] | null {
  const match = /^(\d+)\.(\d+)\.(\d+)/.exec(value);
  return match ? [Number(match[1]), Number(match[2]), Number(match[3])] : null;
}

function compareVersions(left: number[], right: number[]): number {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}
