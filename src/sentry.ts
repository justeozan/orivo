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
 *
 * The SDK itself is fetched on demand. It was 141 kB of the shell's 377 kB
 * entry chunk — more than any page — and not one byte of it is needed to paint
 * a library. What the first screen loses by waiting is closed below: this
 * module watches for errors from the moment it is initialised and hands
 * whatever it caught to the SDK as soon as the SDK exists.
 */
type SentrySdk = typeof import("./sentry-sdk");

/** Read once: Vite inlines this at build time, so it cannot change at runtime. */
const DSN = import.meta.env.VITE_SENTRY_DSN?.trim() ?? "";

/**
 * How many errors are held while the SDK is in flight. The window is one
 * same-origin chunk long, so a handful is already generous; a cascade of
 * hundreds is one bug reported many times, not a hundred leads.
 */
const MAX_HELD_ERRORS = 8;

/**
 * The build's own version, for grouping a report against a release.
 *
 * `__APP_VERSION__` is substituted by vite.config.ts from package.json. The
 * guard is for the test runner, which imports this module without going
 * through that substitution — a missing version must not throw on import.
 */
const RELEASE = typeof __APP_VERSION__ === "string" ? `orivo@${__APP_VERSION__}` : "orivo@dev";

interface HeldError {
  error: unknown;
  context?: Record<string, unknown>;
}

/** Non-null once `initErrorReporting` has decided Sentry is configured. */
let starting: Promise<SentrySdk | null> | null = null;
/** Non-null once the SDK has arrived and been initialised. */
let sdk: SentrySdk | null = null;
let held: HeldError[] = [];

export interface ErrorReportingOptions {
  /**
   * Overrides the built-in DSN and SDK loader. Tests are the only caller: the
   * DSN is inlined at build time and the suite pins it empty, so without a seam
   * there is no way to exercise the path a release actually takes.
   */
  dsn?: string;
  load?: () => Promise<SentrySdk>;
}

function hold(error: unknown, context?: Record<string, unknown>): void {
  if (held.length >= MAX_HELD_ERRORS) return;
  held.push({ error, context });
}

/**
 * Wire up Sentry. Safe to call when no DSN is configured — it returns false and
 * never fetches the SDK, which is what a source build and every test does.
 *
 * Returns synchronously: whether reports are on depends on the DSN alone, and
 * the caller needs that answer while it is still building the shell.
 */
export function initErrorReporting(
  runtime: "desktop" | "browser",
  options: ErrorReportingOptions = {},
): boolean {
  const dsn = options.dsn ?? DSN;
  if (starting || !dsn) return false;

  // Installed before the SDK is even requested, so the fetch window is not a
  // blind spot. Both are dropped the moment Sentry's own handlers take over,
  // in the same turn, so no event can be seen by both.
  const onError = (event: ErrorEvent): void => hold(event.error ?? event.message);
  const onRejection = (event: PromiseRejectionEvent): void => hold(event.reason);
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onRejection);

  const load = options.load ?? ((): Promise<SentrySdk> => import("./sentry-sdk"));
  starting = load()
    .then((loaded) => {
      loaded.init({
        dsn,
        release: RELEASE,
        environment: import.meta.env.MODE === "production" ? "production" : "development",
        // A library is not a checkout flow: there is no sensitive payload to leak,
        // but there is no reason to send names and IP addresses either.
        sendDefaultPii: false,
        // Traces are on at a low rate — enough to see a slow library load, not
        // enough to make a hobby project's quota a problem.
        tracesSampleRate: 0.1,
        integrations: [
          loaded.feedbackIntegration({
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
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);

      loaded.setTag("runtime", runtime);
      loaded.setTag("platform", navigator.platform || "unknown");
      sdk = loaded;

      const backlog = held;
      held = [];
      for (const entry of backlog) {
        loaded.captureException(entry.error, entry.context ? { extra: entry.context } : undefined);
      }
      return loaded;
    })
    .catch(() => {
      // A chunk that never arrived leaves the app running with no reporting,
      // which is the state a source build is already in. The early handlers
      // stay on so a later attempt still has something to send.
      return null;
    });

  return true;
}

/**
 * Wire an existing button to the feedback form.
 *
 * `attachTo` is the SDK's path for a caller that places its own trigger, which
 * is the whole reason `autoInject` is off. Resolves false when Sentry is not
 * configured or never loaded, so the caller can leave its button hidden — and
 * because it resolves rather than returns, the button appears only once it
 * actually opens something.
 *
 * `context` is read at click time rather than at wire-up time: what the player
 * is looking at when they complain is what the report needs, and that is not
 * known when the shell mounts.
 */
export async function attachFeedbackTo(
  element: Element,
  context: () => Record<string, string>,
): Promise<boolean> {
  if (!starting) return false;
  const loaded = await starting;
  const feedback = loaded?.getFeedback();
  if (!loaded || !feedback) return false;

  // Runs before the SDK's own click handler, so the dialog is built with these
  // tags already on the scope.
  element.addEventListener("click", () => {
    for (const [key, value] of Object.entries(context())) {
      if (value) loaded.setTag(key, value);
    }
  });

  feedback.attachTo(element);
  return true;
}

/** Report a caught error that the app handled but should not have hit. */
export function reportError(error: unknown, context?: Record<string, unknown>): void {
  if (!starting) return;
  if (!sdk) {
    hold(error, context);
    return;
  }
  sdk.captureException(error, context ? { extra: context } : undefined);
}
