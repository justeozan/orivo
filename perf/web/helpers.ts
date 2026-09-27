import type { Page } from "@playwright/test";

/**
 * O1's own timing vocabulary. Deliberately not shared with `e2e/helpers.ts`:
 * that file backs the golden-screenshot suite that gates every PR, and this
 * one exists to print numbers, not to assert on them — coupling the two would
 * mean a perf-only change could touch e2e ownership, or vice versa.
 */

export interface Sample {
  ms: number;
}

/** min / median / max over the given samples, printed for docs/performance.md. */
export function report(label: string, samples: number[]): void {
  const sorted = [...samples].sort((a, b) => a - b);
  const min = sorted[0];
  const median = sorted[Math.floor(sorted.length / 2)];
  const max = sorted[sorted.length - 1];
  const fmt = (value: number) => `${value.toFixed(2)}ms`.padStart(10);
  console.log(
    `PERF ${label.padEnd(32)} min=${fmt(min)} median=${fmt(median)} max=${fmt(max)} n=${sorted.length}`,
  );
}

/** Wall-clock time, from the Node side, for a full navigation to a ready page. */
export async function timeNavigation(
  page: Page,
  hash: string,
  readySelector: string,
): Promise<number> {
  const start = performance.now();
  await page.goto(`/${hash}`, { waitUntil: "domcontentloaded" });
  await page.waitForSelector(readySelector, { state: "visible" });
  return performance.now() - start;
}

/** JS heap in use, via CDP — the one memory number Chromium exposes reliably
 * headless, without relying on the non-standard `performance.memory` API. */
export async function jsHeapUsedBytes(page: Page): Promise<number> {
  const session = await page.context().newCDPSession(page);
  await session.send("Performance.enable");
  const { metrics } = await session.send("Performance.getMetrics");
  await session.detach();
  return metrics.find((metric) => metric.name === "JSHeapUsedSize")?.value ?? 0;
}

export interface FramePacing {
  /** Long tasks (Performance API) observed during the sampling window. */
  longTasks: { count: number; totalMs: number };
  /** requestAnimationFrame deltas, ideal is ~16.7ms at 60Hz. */
  frameDeltas: { count: number; maxMs: number; p95Ms: number };
}

/**
 * Starts collecting long tasks and rAF deltas, runs `interact`, then stops and
 * returns the sample. Wrapping the interaction (rather than sampling for a
 * fixed duration and hoping it overlaps) is what makes this deterministic
 * across a fast machine and a loaded one.
 */
export async function measureFramePacing(
  page: Page,
  interact: () => Promise<void>,
): Promise<FramePacing> {
  await page.evaluate(() => {
    const w = window as unknown as Record<string, unknown>;
    const longTasks: number[] = [];
    const observer = new PerformanceObserver((list) => {
      for (const entry of list.getEntries()) longTasks.push(entry.duration);
    });
    observer.observe({ type: "longtask", buffered: false });
    w.__perfLongTasks = longTasks;
    w.__perfObserver = observer;

    const deltas: number[] = [];
    let last = performance.now();
    let raf = 0;
    const tick = (now: number) => {
      deltas.push(now - last);
      last = now;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    w.__perfFrameDeltas = deltas;
    w.__perfRafHandle = raf;
  });

  await interact();

  return page.evaluate(() => {
    const w = window as unknown as Record<string, unknown>;
    (w.__perfObserver as PerformanceObserver).disconnect();
    cancelAnimationFrame(w.__perfRafHandle as number);
    const longTasks = w.__perfLongTasks as number[];
    // The first delta is measured from the observer's own setup, not from a
    // real previous frame, and would otherwise report a bogus multi-ms outlier.
    const frameDeltas = (w.__perfFrameDeltas as number[]).slice(1);
    const sorted = [...frameDeltas].sort((a, b) => a - b);
    const p95Index = Math.min(sorted.length - 1, Math.floor(sorted.length * 0.95));
    return {
      longTasks: {
        count: longTasks.length,
        totalMs: longTasks.reduce((sum, value) => sum + value, 0),
      },
      frameDeltas: {
        count: frameDeltas.length,
        maxMs: sorted[sorted.length - 1] ?? 0,
        p95Ms: sorted[p95Index] ?? 0,
      },
    };
  });
}
