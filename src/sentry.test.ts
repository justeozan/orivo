import { afterEach, describe, expect, it, vi } from "vitest";

/**
 * The SDK is fetched on demand, so these tests are about the window between
 * "reporting is on" and "the SDK exists" — the only thing deferring it could
 * plausibly break. `vitest.config.ts` pins `VITE_SENTRY_DSN` empty, and the
 * DSN is inlined at build time anyway, so every test goes through the seam.
 */
function fakeSdk() {
  const feedback = { attachTo: vi.fn() };
  return {
    init: vi.fn(),
    setTag: vi.fn(),
    captureException: vi.fn(),
    feedbackIntegration: vi.fn(() => ({ name: "Feedback" })),
    getFeedback: vi.fn(() => feedback),
    feedback,
  };
}

type FakeSdk = ReturnType<typeof fakeSdk>;

/**
 * Throw at the window the way the browser does. Cancelling it keeps jsdom from
 * also reporting it as an uncaught exception and failing the run over an error
 * the test threw on purpose.
 */
function throwAtWindow(error: unknown): void {
  window.addEventListener("error", (event) => event.preventDefault(), { once: true });
  window.dispatchEvent(new ErrorEvent("error", { error, cancelable: true }));
}

/** A fresh module per test: whether Sentry started is module state, once. */
async function freshModule(): Promise<typeof import("./sentry")> {
  vi.resetModules();
  return import("./sentry");
}

function load(sdk: FakeSdk): () => Promise<never> {
  return () => Promise.resolve(sdk) as unknown as Promise<never>;
}

afterEach(() => {
  vi.resetModules();
});

describe("initErrorReporting", () => {
  it("never fetches the SDK without a DSN", async () => {
    const { initErrorReporting, attachFeedbackTo } = await freshModule();
    const loader = vi.fn();

    expect(initErrorReporting("browser", { dsn: "", load: loader as never })).toBe(false);
    expect(loader).not.toHaveBeenCalled();
    await expect(
      attachFeedbackTo(document.createElement("button"), () => ({})),
    ).resolves.toBe(false);
  });

  it("answers before the SDK has arrived", async () => {
    const { initErrorReporting } = await freshModule();
    const sdk = fakeSdk();

    // The shell needs this answer while it is still building the topbar, so it
    // cannot be a promise — and the SDK has provably not landed yet.
    expect(initErrorReporting("desktop", { dsn: "https://k@o.test/1", load: load(sdk) })).toBe(
      true,
    );
    expect(sdk.init).not.toHaveBeenCalled();

    await vi.waitFor(() => expect(sdk.init).toHaveBeenCalledTimes(1));
    expect(sdk.setTag).toHaveBeenCalledWith("runtime", "desktop");
  });

  it("starts only once", async () => {
    const { initErrorReporting } = await freshModule();
    const sdk = fakeSdk();
    const options = { dsn: "https://k@o.test/1", load: load(sdk) };

    expect(initErrorReporting("browser", options)).toBe(true);
    expect(initErrorReporting("browser", options)).toBe(false);

    await vi.waitFor(() => expect(sdk.init).toHaveBeenCalledTimes(1));
  });

  it("hands the SDK the errors thrown while it was in flight", async () => {
    const { initErrorReporting } = await freshModule();
    const sdk = fakeSdk();
    let arrive!: (value: FakeSdk) => void;
    initErrorReporting("browser", {
      dsn: "https://k@o.test/1",
      load: () => new Promise<FakeSdk>((resolve) => (arrive = resolve)) as never,
    });

    const early = new Error("thrown during the first render");
    throwAtWindow(early);
    window.dispatchEvent(
      new PromiseRejectionEvent("unhandledrejection", {
        promise: Promise.resolve(),
        reason: "a rejected library load",
      }),
    );

    arrive(sdk);
    await vi.waitFor(() => expect(sdk.captureException).toHaveBeenCalledTimes(2));
    expect(sdk.captureException).toHaveBeenNthCalledWith(1, early, undefined);
    expect(sdk.captureException).toHaveBeenNthCalledWith(2, "a rejected library load", undefined);
  });

  it("stops holding errors once Sentry watches for them itself", async () => {
    const { initErrorReporting } = await freshModule();
    const sdk = fakeSdk();
    initErrorReporting("browser", { dsn: "https://k@o.test/1", load: load(sdk) });
    await vi.waitFor(() => expect(sdk.init).toHaveBeenCalledTimes(1));

    // Sentry's own handlers are live now. A second set would report this twice.
    throwAtWindow(new Error("later"));
    await Promise.resolve();

    expect(sdk.captureException).not.toHaveBeenCalled();
  });

  it("keeps a handled report rather than dropping it into the gap", async () => {
    const { initErrorReporting, reportError } = await freshModule();
    const sdk = fakeSdk();
    let arrive!: (value: FakeSdk) => void;
    initErrorReporting("browser", {
      dsn: "https://k@o.test/1",
      load: () => new Promise<FakeSdk>((resolve) => (arrive = resolve)) as never,
    });

    const failure = new Error("the catalogue would not save");
    reportError(failure, { gameId: "local:alpha" });
    expect(sdk.captureException).not.toHaveBeenCalled();

    arrive(sdk);
    await vi.waitFor(() => expect(sdk.captureException).toHaveBeenCalledTimes(1));
    expect(sdk.captureException).toHaveBeenCalledWith(failure, {
      extra: { gameId: "local:alpha" },
    });
  });

  it("leaves the app running when the SDK chunk never arrives", async () => {
    const { initErrorReporting, attachFeedbackTo } = await freshModule();

    expect(
      initErrorReporting("browser", {
        dsn: "https://k@o.test/1",
        load: () => Promise.reject(new Error("chunk did not arrive")) as never,
      }),
    ).toBe(true);

    await expect(
      attachFeedbackTo(document.createElement("button"), () => ({})),
    ).resolves.toBe(false);
  });
});

describe("attachFeedbackTo", () => {
  it("waits for the SDK, then tags the report with what is on screen", async () => {
    const { initErrorReporting, attachFeedbackTo } = await freshModule();
    const sdk = fakeSdk();
    initErrorReporting("desktop", { dsn: "https://k@o.test/1", load: load(sdk) });

    const button = document.createElement("button");
    await expect(attachFeedbackTo(button, () => ({ page: "store", game: "" }))).resolves.toBe(true);
    expect(sdk.feedback.attachTo).toHaveBeenCalledWith(button);

    button.click();
    expect(sdk.setTag).toHaveBeenCalledWith("page", "store");
    // An empty value is an absence, not a tag reading "".
    expect(sdk.setTag).not.toHaveBeenCalledWith("game", "");
  });
});
