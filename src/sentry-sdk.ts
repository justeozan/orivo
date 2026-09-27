/**
 * The slice of `@sentry/browser` Orivo uses, in its own module.
 *
 * `sentry.ts` imports this on demand. Naming the five exports here rather than
 * dynamically importing the package keeps the chunk tree-shaken: a dynamic
 * `import("@sentry/browser")` pulls in the whole namespace — 471 kB instead of
 * 141 kB — because every export stays reachable.
 */
export {
  captureException,
  feedbackIntegration,
  getFeedback,
  init,
  setTag,
} from "@sentry/browser";
