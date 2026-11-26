import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { en, fr } from "./locales/highlights";
import { NEXT_RELEASE, RELEASE_HIGHLIGHTS } from "./release-highlights";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const SCRIPT = join(ROOT, "scripts/release-highlights.mjs");
const DATA = join(ROOT, "src/release-highlights.ts");
const LOCALE = join(ROOT, "src/locales/highlights.ts");

/** Drawn at 19px beside a tile: anything longer wraps or reads as a sentence. */
const MAX_TITLE = 40;
const MAX_LINE = 56;

const parsed = (version: string) => version.split(/[.-]/).slice(0, 3).map(Number);

describe("RELEASE_HIGHLIGHTS", () => {
  it("opens with the one 'next' bucket, then releases newest first, each once", () => {
    expect(RELEASE_HIGHLIGHTS[0].version).toBe(NEXT_RELEASE);
    const released = RELEASE_HIGHLIGHTS.slice(1).map((release) => release.version);
    expect(released).not.toContain(NEXT_RELEASE);
    expect(new Set(released).size).toBe(released.length);
    for (const version of released) expect(version).toMatch(/^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$/);
    const order = released.map(parsed);
    for (let index = 1; index < order.length; index += 1) {
      const [previous, current] = [order[index - 1], order[index]];
      const newer = previous.findIndex((part, at) => part !== current[at]);
      expect(newer === -1 || previous[newer] > current[newer], `${released[index - 1]} before ${released[index]}`).toBe(true);
    }
  });

  it("says each highlight in a few words, in English and in French", () => {
    for (const { items } of RELEASE_HIGHLIGHTS) {
      for (const { titleKey, lineKey } of items) {
        for (const catalogue of [en, fr] as Array<Record<string, string | undefined>>) {
          expect(catalogue[titleKey], titleKey).toBeTruthy();
          expect(catalogue[lineKey], lineKey).toBeTruthy();
          expect(catalogue[titleKey]!.length, titleKey).toBeLessThanOrEqual(MAX_TITLE);
          expect(catalogue[lineKey]!.length, lineKey).toBeLessThanOrEqual(MAX_LINE);
        }
      }
    }
  });

  it("leaves no highlight text without a release", () => {
    const used = new Set(RELEASE_HIGHLIGHTS.flatMap(({ items }) => items.flatMap((item) => [item.titleKey, item.lineKey])));
    expect(Object.keys(en).filter((key) => !used.has(key as keyof typeof en))).toEqual([]);
  });
});

// Each case starts Node a few times; a busy machine needs more than 5 s.
describe("scripts/release-highlights.mjs", { timeout: 30_000 }, () => {
  let folder: string;
  const run = (...args: string[]) => execFileSync("node", [SCRIPT, ...args], { encoding: "utf8", stdio: "pipe" });
  const fixture = (next: string) => {
    const path = join(folder, "data.ts");
    writeFileSync(
      path,
      `export const RELEASE_HIGHLIGHTS = [
  {
    version: "next",
    items: [${next}],
  },
  {
    version: "0.4.3",
    items: [
      { icon: "switch", tone: "emulation", titleKey: "highlights.switchPlugin.title", lineKey: "highlights.switchPlugin.line" },
    ],
  },
];
`,
    );
    return path;
  };
  const movedGames = `
      { icon: "folder", tone: "visuals", titleKey: "highlights.movedGames.title", lineKey: "highlights.movedGames.line" },
    `;

  beforeEach(() => {
    folder = mkdtempSync(join(tmpdir(), "orivo-highlights-"));
  });
  afterEach(() => {
    rmSync(folder, { recursive: true, force: true });
  });

  it("stamps the waiting highlights with the version and opens an empty 'next'", () => {
    const path = fixture(movedGames);
    run("stamp", "0.4.4", path);
    const source = readFileSync(path, "utf8");
    expect(source).toMatch(/version: "next",\n {4}items: \[\],\n {2}\},\n {2}\{\n {4}version: "0\.4\.4",\n {4}items: \[\n.*movedGames/);
    expect(run("notes", "v0.4.4", path, LOCALE)).toBe(
      "## Highlights\n\n- **A moved game is found again**: Otherwise it dims, with Locate\n",
    );
    expect(run("notes", "0.4.4", path, LOCALE)).toBe(run("notes", "v0.4.4", path, LOCALE));
  });

  it("stamps nothing when no PR added highlights", () => {
    const path = fixture("");
    const before = readFileSync(path, "utf8");
    run("stamp", "0.4.4", path);
    expect(readFileSync(path, "utf8")).toBe(before);
    expect(run("notes", "0.4.4", path, LOCALE)).toBe("");
  });

  it("refuses a version that already has highlights, and a bad version", () => {
    const path = fixture(movedGames);
    expect(() => run("stamp", "0.4.3", path)).toThrow(/already has highlights for 0\.4\.3/);
    expect(() => run("stamp", "latest", path)).toThrow(/not a version/);
  });

  it("can stamp the real data file, whatever is waiting in it", () => {
    const copy = join(folder, "release-highlights.ts");
    copyFileSync(DATA, copy);
    run("stamp", "999.0.0", copy);
    const waiting = RELEASE_HIGHLIGHTS[0].items.length;
    expect(readFileSync(copy, "utf8").includes('version: "999.0.0"')).toBe(waiting > 0);
  });

  it("writes the English highlights of a released version for the GitHub release", () => {
    const notes = run("notes", "0.4.3", DATA, LOCALE);
    expect(notes).toBe(`## Highlights\n\n- **${en["highlights.switchPlugin.title"]}**: ${en["highlights.switchPlugin.line"]}\n`);
  });
});
