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
 * Two kinds of error reach it. The ones nobody caught, which the SDK's own
 * handlers see. And the ones the app caught and showed as a toast — a launch
 * that was refused, a sync that failed, an update that would not install —
 * which are the failures a player actually meets and which never throw at the
 * window: `host.ts` and the updater hand those to `reportFailure`.
 *
 * The SDK itself is fetched on demand. It was 141 kB of the shell's 377 kB
 * entry chunk — more than any page — and not one byte of it is needed to paint
 * a library. What the first screen loses by waiting is closed below: this
 * module watches for errors from the moment it is initialised and hands
 * whatever it caught to the SDK as soon as the SDK exists.
 */
import type { ErrorEvent as SentryEvent } from "@sentry/browser";
import { t } from "./i18n";

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

/**
 * How many failures one session reports, and how often each. A command that
 * fails because the player is offline fails again on every screen that asks;
 * that is one lead, and a free quota is not there to count it a hundred times.
 * Sentry already counts how many sessions hit it.
 */
const MAX_FAILURE_KINDS = 20;

/** What `captureException` takes as its second argument, in the part Orivo uses. */
interface CaptureScope {
  extra?: Record<string, unknown>;
  tags?: Record<string, string>;
  fingerprint?: string[];
}

interface HeldError {
  error: unknown;
  scope?: CaptureScope;
}

/** Non-null once `initErrorReporting` has decided Sentry is configured. */
let starting: Promise<SentrySdk | null> | null = null;
/** Non-null once the SDK has arrived and been initialised. */
let sdk: SentrySdk | null = null;
let held: HeldError[] = [];
/** Failures already sent this session, by fingerprint. */
const reportedFailures = new Set<string>();

export interface ErrorReportingOptions {
  /**
   * Overrides the built-in DSN and SDK loader. Tests are the only caller: the
   * DSN is inlined at build time and the suite pins it empty, so without a seam
   * there is no way to exercise the path a release actually takes.
   */
  dsn?: string;
  load?: () => Promise<SentrySdk>;
}

function hold(error: unknown, scope?: CaptureScope): void {
  if (held.length >= MAX_HELD_ERRORS) return;
  held.push({ error, scope });
}

/**
 * Takes the player's account name out of a path.
 *
 * A failure's text is the host's own message, and the host quotes the file it
 * could not open. `sendDefaultPii: false` does not touch that, so the folders
 * under the home directory are shortened here, before anything leaves.
 */
export function scrubHomePaths(text: string): string {
  return text
    .replace(/\/Users\/[^/\s"'`]+/g, "/Users/~")
    .replace(/\/home\/[^/\s"'`]+/g, "/home/~")
    .replace(/([A-Za-z]:[\\/]+Users[\\/]+)[^\\/\s"'`]+/gi, "$1~");
}

function scrubExtra(extra: Record<string, unknown>): Record<string, unknown> {
  const scrubbed: Record<string, unknown> = {};
  for (const [key, value] of Object.entries(extra)) {
    scrubbed[key] = typeof value === "string" ? scrubHomePaths(value) : value;
  }
  return scrubbed;
}

/** The last stop before an event leaves: no account names in any text it carries. */
export function scrubEvent(event: SentryEvent): SentryEvent {
  if (event.message) event.message = scrubHomePaths(event.message);
  for (const exception of event.exception?.values ?? []) {
    if (exception.value) exception.value = scrubHomePaths(exception.value);
  }
  for (const crumb of event.breadcrumbs ?? []) {
    if (crumb.message) crumb.message = scrubHomePaths(crumb.message);
  }
  if (event.extra) event.extra = scrubExtra(event.extra);
  return event;
}

function describe(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  try {
    return JSON.stringify(error) ?? String(error);
  } catch {
    return String(error);
  }
}

/**
 * The message with everything that varies per player taken out, so the same
 * failure on two machines is one issue. Paths go first: they are what differs
 * most, and a game's folder name is not part of what went wrong.
 */
function fingerprintOf(message: string): string {
  return message
    .replace(/\S*[\\/]\S*/g, "<path>")
    .replace(/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/gi, "<id>")
    .replace(/\d+/g, "#")
    .slice(0, 160);
}

function send(error: unknown, scope?: CaptureScope): void {
  if (!starting) return;
  if (!sdk) {
    hold(error, scope);
    return;
  }
  sdk.captureException(error, scope);
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
        beforeSend: scrubEvent,
        // Traces are on at a low rate — enough to see a slow library load, not
        // enough to make a hobby project's quota a problem.
        tracesSampleRate: 0.1,
        // Session Replay is what turns a rage click into something Sentry can
        // flag: it watches the clicks, and the replay shows what was under them.
        // One session in ten is recorded, and every session that hits an error
        // is kept from the minute before it.
        replaysSessionSampleRate: 0.1,
        replaysOnErrorSampleRate: 1.0,
        integrations: [
          loaded.replayIntegration({
            // The recording is a layout, not a screen: text, form fields and
            // media are replaced by blocks before they leave the machine. Game
            // titles, folder paths and account names stay on the player's side;
            // the clicks, the scrolling and the rage are all that is left.
            maskAllText: true,
            maskAllInputs: true,
            blockAllMedia: true,
            // The compression worker is a blob: script, which the app's CSP
            // (`script-src 'self'`) refuses. Uncompressed replays are larger on
            // the wire but only one session in ten sends one.
            useCompression: false,
          }),
          loaded.feedbackIntegration({
            // Orivo places the trigger itself, next to the profile picture. Sentry's
            // own floating button would fight the layout and sit over the rail.
            autoInject: false,
            colorScheme: "dark",
            // The widget lives in a Shadow DOM that styles.css cannot reach, so
            // its palette is set here. The values mirror the glass panels beside
            // the profile picture: the panel's near-black, the --text colour,
            // the violet of --violet running into the blue of the Play ring.
            themeDark: {
              background: "rgba(22, 20, 23, 0.96)",
              foreground: "#f4f2f7",
              accentBackground: "linear-gradient(105deg, #7d54f4 0%, #4d63ff 100%)",
              accentForeground: "#ffffff",
              successColor: "#67da94",
              errorColor: "#ef7074",
              boxShadow: "0 18px 44px rgba(0, 0, 0, 0.36), inset 0 1px rgba(255, 255, 255, 0.055)",
              outline: "1.5px solid rgba(185, 168, 255, 0.6)",
            },
            showBranding: false,
            formTitle: t("misc.feedback.title"),
            buttonLabel: t("misc.feedback.button"),
            submitButtonLabel: t("misc.feedback.submit"),
            cancelButtonLabel: t("misc.feedback.cancel"),
            successMessageText: t("misc.feedback.success"),
            nameLabel: t("misc.feedback.nameLabel"),
            emailLabel: t("misc.feedback.emailLabel"),
            messageLabel: t("misc.feedback.messageLabel"),
            isRequiredLabel: t("misc.feedback.required"),
            addScreenshotButtonLabel: t("misc.feedback.addScreenshot"),
            removeScreenshotButtonLabel: t("misc.feedback.removeScreenshot"),
            messagePlaceholder: t("misc.feedback.message"),
            namePlaceholder: t("misc.feedback.name"),
            emailPlaceholder: t("misc.feedback.email"),
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
        loaded.captureException(entry.error, entry.scope);
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
  send(error, context ? { extra: context } : undefined);
}

/**
 * Report something that failed and was handled — shown as a toast, retried,
 * swallowed — so that a failure only the player saw is one Sentry sees too.
 *
 * `source` is what failed: a host command's name, or a step such as
 * `updater:check`. It becomes a tag to filter by and, with the message, the
 * fingerprint, so a thousand players hitting one refusal make one issue. Each
 * fingerprint is sent once per session.
 */
export function reportFailure(source: string, error: unknown): void {
  if (!starting) return;

  const message = describe(error);
  const fingerprint = [source, fingerprintOf(message)];
  const key = fingerprint.join("\n");
  if (reportedFailures.has(key) || reportedFailures.size >= MAX_FAILURE_KINDS) return;
  reportedFailures.add(key);

  // A fresh error rather than the caller's: a host rejection is a bare string,
  // and the stack of a wrapper that only re-throws would point at the wrong
  // place. The source and the message are the whole story.
  const failure = new Error(`${source}: ${message}`);
  failure.name = "HandledFailure";
  failure.stack = undefined;
  send(failure, { tags: { failure: source }, fingerprint });
}
