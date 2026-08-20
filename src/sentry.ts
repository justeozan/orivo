/**
 * Sentry: crash reports, and the feedback button beside the profile picture.
 *
 * The reason Sentry is here at all is the feedback form — a player who hits
 * something wrong should be able to say so from inside the app, and have it
 * land somewhere a fix can start. Errors ride along because the SDK is already
 * loaded and a report with a stack trace beats a report without one.
 *
 * Nothing initialises without a DSN. A contributor building from source gets an
 * app that never opens a socket to Sentry, and the feedback button hides itself
 * rather than dangling as a control that does nothing.
 */
import * as Sentry from "@sentry/browser";

/** Read once: Vite inlines this at build time, so it cannot change at runtime. */
const DSN = import.meta.env.VITE_SENTRY_DSN?.trim() ?? "";

let started = false;

/**
 * The build's own version, for grouping a report against a release.
 *
 * `__APP_VERSION__` is substituted by vite.config.ts from package.json. The
 * guard is for the test runner, which imports this module without going
 * through that substitution — a missing version must not throw on import.
 */
const RELEASE = typeof __APP_VERSION__ === "string" ? `orivo@${__APP_VERSION__}` : "orivo@dev";

/**
 * Wire up Sentry. Safe to call when no DSN is configured — it returns false and
 * leaves the SDK untouched, which is what a source build and every test does.
 */
export function initErrorReporting(runtime: "desktop" | "browser"): boolean {
  if (started || !DSN) return false;
  started = true;

  Sentry.init({
    dsn: DSN,
    release: RELEASE,
    environment: import.meta.env.MODE === "production" ? "production" : "development",
    // A library is not a checkout flow: there is no sensitive payload to leak,
    // but there is no reason to send names and IP addresses either.
    sendDefaultPii: false,
    // Traces are on at a low rate — enough to see a slow library load, not
    // enough to make a hobby project's quota a problem.
    tracesSampleRate: 0.1,
    integrations: [
      Sentry.feedbackIntegration({
        // Orivo places the trigger itself, next to the profile picture. Sentry's
        // own floating button would fight the layout and sit over the rail.
        autoInject: false,
        colorScheme: "dark",
        showBranding: false,
        formTitle: "Tell us what happened",
        buttonLabel: "Feedback",
        submitButtonLabel: "Send feedback",
        messagePlaceholder:
          "What were you doing, and what did Orivo do instead? A store name or a game title helps a lot.",
        namePlaceholder: "Your name (optional)",
        emailPlaceholder: "Your email, if you want an answer",
        isNameRequired: false,
        isEmailRequired: false,
        enableScreenshot: true,
      }),
    ],
  });

  Sentry.setTag("runtime", runtime);
  Sentry.setTag("platform", navigator.platform || "unknown");
  return true;
}

/**
 * Wire an existing button to the feedback form.
 *
 * `attachTo` is the SDK's path for a caller that places its own trigger, which
 * is the whole reason `autoInject` is off. Returns false when Sentry is not
 * configured, so the caller can leave its button hidden.
 *
 * `context` is read at click time rather than at wire-up time: what the player
 * is looking at when they complain is what the report needs, and that is not
 * known when the shell mounts.
 */
export function attachFeedbackTo(
  element: Element,
  context: () => Record<string, string>,
): boolean {
  const feedback = started ? Sentry.getFeedback() : undefined;
  if (!feedback) return false;

  // Runs before the SDK's own click handler, so the dialog is built with these
  // tags already on the scope.
  element.addEventListener("click", () => {
    for (const [key, value] of Object.entries(context())) {
      if (value) Sentry.setTag(key, value);
    }
  });

  feedback.attachTo(element);
  return true;
}

/** Report a caught error that the app handled but should not have hit. */
export function reportError(error: unknown, context?: Record<string, unknown>): void {
  if (!started) return;
  Sentry.captureException(error, context ? { extra: context } : undefined);
}
