/**
 * Where chosen artwork is persisted, outside any page's lifetime.
 *
 * Saving a wallpaper is a chain — download the candidate, select it, copy it
 * into the artwork cache for each role. The detail page orchestrates that chain,
 * and its requests used to ride the page's abort signal, so leaving the page
 * mid-save dropped the rest of it. The chain runs here instead: it finishes
 * whatever the user navigates to, and only the page's repainting is conditional
 * on the page still being there.
 */

/**
 * Handed to every job in place of a page's signal. A save is the one request
 * that must outlive navigation, so nothing ever aborts this one.
 */
const PERSIST_SIGNAL = new AbortController().signal;

/**
 * Per game, the promise of the last job queued. It never rejects, so the next
 * job can chain onto it without inheriting an earlier failure.
 */
const tails = new Map<string, Promise<void>>();

/**
 * Runs `job` once every earlier save of the same game has finished. Two saves
 * of one game never interleave (the second would select media the first is
 * still downloading), while saves of different games run side by side. The
 * returned promise carries the job's own result or failure.
 */
export function enqueueMediaSave<T>(
  gameId: string,
  job: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const previous = tails.get(gameId) ?? Promise.resolve();
  const result = previous.then(() => job(PERSIST_SIGNAL));
  const tail: Promise<void> = result
    .then(
      () => undefined,
      () => undefined,
    )
    .then(() => {
      if (tails.get(gameId) === tail) tails.delete(gameId);
    });
  tails.set(gameId, tail);
  return result;
}

/** Whether a save of this game is queued or running. */
export function isMediaSaving(gameId: string): boolean {
  return tails.has(gameId);
}

/**
 * Resolves once nothing is left to save for this game, including jobs queued
 * while it waited. This is how a page opened mid-save finds out the result is
 * ready to be read.
 */
export async function whenMediaSaveSettled(gameId: string): Promise<void> {
  for (let tail = tails.get(gameId); tail; tail = tails.get(gameId)) {
    await tail;
  }
}
