import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppRoute } from "./contracts";
import type { AppPage } from "./page-lifecycle";
import { mountApp } from "./app";

/**
 * The About panel's update row is wired through the shell's delegated settings
 * click handler and two dynamically imported Tauri plugins. The state machine
 * itself is covered by `updater-model.test.ts`; these tests cover the wiring
 * that machine is useless without — that the button is bound at all, that the
 * plugins are only ever reached from inside a handler, and that a finished
 * download leads to `relaunch()`.
 */
const tauri = vi.hoisted(() => ({
  invoke: vi.fn<(command: string, args?: Record<string, unknown>) => Promise<unknown>>(),
}));

interface DownloadEvent {
  event: "Started" | "Progress" | "Finished";
  data?: { contentLength?: number; chunkLength?: number };
}

interface UpdateHandleStub {
  version: string;
  currentVersion: string;
  body: string | null;
  downloadAndInstall: (onEvent: (event: DownloadEvent) => void) => Promise<void>;
  close: () => Promise<void>;
}

const updater = vi.hoisted(() => ({
  check: vi.fn<() => Promise<unknown>>(),
}));
const processPlugin = vi.hoisted(() => ({
  relaunch: vi.fn<() => Promise<void>>(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: tauri.invoke,
  convertFileSrc: (path: string) => path,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));
vi.mock("@tauri-apps/api/app", () => ({
  getVersion: () => Promise.resolve("0.3.0"),
  getTauriVersion: () => Promise.resolve("2.11.5"),
}));
vi.mock("@tauri-apps/api/path", () => ({
  appCacheDir: () => Promise.resolve("/cache"),
  join: (...parts: string[]) => Promise.resolve(parts.join("/")),
}));
vi.mock("@tauri-apps/plugin-updater", () => ({ check: updater.check }));
vi.mock("@tauri-apps/plugin-process", () => ({ relaunch: processPlugin.relaunch }));

function stubPage(): AppPage {
  return {
    mount() {},
    activate(_activation: { route: AppRoute }) {},
    deactivate() {
      return null;
    },
  };
}

async function settle(): Promise<void> {
  for (let index = 0; index < 4; index += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
  }
}

/**
 * A download the test drives one event at a time, so every intermediate render
 * can be asserted instead of only the end state.
 */
function pendingDownload(overrides: Partial<UpdateHandleStub> = {}): {
  handle: UpdateHandleStub;
  emit: (event: DownloadEvent) => void;
  finish: () => void;
  fail: (reason: unknown) => void;
} {
  let emit: (event: DownloadEvent) => void = () => {};
  let finish: () => void = () => {};
  let fail: (reason: unknown) => void = () => {};
  const handle: UpdateHandleStub = {
    version: "0.4.0",
    currentVersion: "0.3.0",
    body: "Faster store browsing.",
    downloadAndInstall: (onEvent) =>
      new Promise<void>((resolve, reject) => {
        emit = onEvent;
        finish = resolve;
        fail = reject;
      }),
    close: vi.fn(() => Promise.resolve()),
    ...overrides,
  };
  return {
    handle,
    emit: (event) => emit(event),
    finish: () => finish(),
    fail: (reason) => fail(reason),
  };
}

describe("About panel updater wiring", () => {
  let root: HTMLElement;

  const button = (): HTMLButtonElement => {
    const node = root.querySelector<HTMLButtonElement>("#check-updates-button");
    if (!node) throw new Error("The update button is not in the About panel");
    return node;
  };
  const statusLabel = (): string =>
    root.querySelector<HTMLElement>("#update-status .update-status__label")?.textContent ?? "";
  const statusDetail = (): string =>
    root.querySelector<HTMLElement>("#update-status .update-status__detail")?.textContent ?? "";
  const progress = (): HTMLElement => {
    const node = root.querySelector<HTMLElement>("#update-progress");
    if (!node) throw new Error("The progress track is not in the About panel");
    return node;
  };

  const press = async (): Promise<void> => {
    button().click();
    await settle();
  };

  beforeEach(async () => {
    window.location.hash = "";
    window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
    // The updater is desktop-only, so every one of these tests runs as if the
    // page were inside the Tauri webview.
    (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
    tauri.invoke.mockReset();
    tauri.invoke.mockResolvedValue(undefined);
    updater.check.mockReset();
    processPlugin.relaunch.mockReset();
    processPlugin.relaunch.mockResolvedValue(undefined);

    document.body.replaceChildren();
    root = document.createElement("div");
    document.body.append(root);
    mountApp(root, { storePage: stubPage(), gameDetailPage: stubPage() });
    window.location.hash = "#/settings/about";
    await settle();
  });

  afterEach(() => {
    delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
    window.location.hash = "";
  });

  it("never downloads or restarts on its own", () => {
    // Orivo looks for a release by itself, once the shell has gone quiet — that
    // is the whole point of an app that updates itself. Looking is all it does:
    // nothing is fetched and nothing restarts until the button is pressed.
    expect(updater.check).not.toHaveBeenCalled();
    expect(processPlugin.relaunch).not.toHaveBeenCalled();
    expect(button().disabled).toBe(false);
    expect(button().textContent).toBe("Check for updates");
    expect(progress().hidden).toBe(true);
  });

  it("reports an up-to-date install and names the running version", async () => {
    updater.check.mockResolvedValue(null);

    await press();

    expect(updater.check).toHaveBeenCalledTimes(1);
    expect(statusLabel()).toBe("You're on the latest version.");
    expect(statusDetail()).toBe("Orivo 0.3.0 is the newest release.");
    expect(button().textContent).toBe("Check again");
    expect(button().disabled).toBe(false);
  });

  it("downloads, installs and then offers a restart", async () => {
    const download = pendingDownload();
    updater.check.mockResolvedValue(download.handle);

    await press();
    expect(statusLabel()).toBe("Version 0.4.0 is available");
    expect(statusDetail()).toBe("Faster store browsing.");
    expect(button().textContent).toBe("Download and install");

    await press();
    expect(button().disabled).toBe(true);

    download.emit({ event: "Started", data: { contentLength: 4_000_000 } });
    expect(progress().hidden).toBe(false);
    expect(progress().getAttribute("aria-valuenow")).toBe("0");

    download.emit({ event: "Progress", data: { chunkLength: 1_000_000 } });
    expect(statusLabel()).toBe("Downloading update — 25%");
    expect(progress().getAttribute("aria-valuenow")).toBe("25");

    download.emit({ event: "Progress", data: { chunkLength: 3_000_000 } });
    expect(statusLabel()).toBe("Downloading update — 100%");

    download.emit({ event: "Finished" });
    download.finish();
    await settle();

    expect(progress().hidden).toBe(true);
    expect(statusLabel()).toBe("Update ready to install");
    expect(button().textContent).toBe("Restart to update");
    expect(button().disabled).toBe(false);

    await press();
    expect(processPlugin.relaunch).toHaveBeenCalledTimes(1);
  });

  it("runs an indeterminate bar when the server sends no content length", async () => {
    const download = pendingDownload();
    updater.check.mockResolvedValue(download.handle);

    await press();
    await press();

    download.emit({ event: "Started", data: {} });
    download.emit({ event: "Progress", data: { chunkLength: 512_000 } });

    expect(statusLabel()).toBe("Downloading update…");
    expect(statusLabel()).not.toContain("NaN");
    expect(statusDetail()).toBe("500.0 KB downloaded");
    expect(progress().hidden).toBe(false);
    expect(progress().classList.contains("update-progress--indeterminate")).toBe(true);
    expect(progress().hasAttribute("aria-valuenow")).toBe(false);

    download.emit({ event: "Finished" });
    download.finish();
    await settle();
    expect(button().textContent).toBe("Restart to update");
  });

  it("surfaces a failed check and lets the user retry", async () => {
    updater.check.mockRejectedValueOnce(new Error("release feed unreachable"));

    await press();
    expect(statusLabel()).toBe("The update check failed");
    expect(statusDetail()).toBe("release feed unreachable");
    expect(button().textContent).toBe("Try again");
    expect(progress().hidden).toBe(true);

    updater.check.mockResolvedValueOnce(null);
    await press();
    expect(updater.check).toHaveBeenCalledTimes(2);
    expect(statusLabel()).toBe("You're on the latest version.");
  });

  it("surfaces a failed download without stranding the progress bar", async () => {
    const download = pendingDownload();
    updater.check.mockResolvedValue(download.handle);

    await press();
    await press();
    download.emit({ event: "Started", data: { contentLength: 4_000_000 } });
    download.fail(new Error("disk full"));
    await settle();

    expect(progress().hidden).toBe(true);
    // The row must name the step that actually failed, not the check that
    // succeeded a moment earlier.
    expect(statusLabel()).toBe("The update could not be installed");
    expect(statusDetail()).toBe("disk full");
    expect(button().disabled).toBe(false);
    expect(processPlugin.relaunch).not.toHaveBeenCalled();
  });

  it("releases the previous update handle when a new check starts", async () => {
    const download = pendingDownload();
    updater.check.mockResolvedValue(download.handle);

    await press();
    await press();
    download.fail(new Error("offline"));
    await settle();
    expect(download.handle.close).not.toHaveBeenCalled();

    // The handle is a reference to a resource on the Rust side; a re-check that
    // simply dropped it would leak one entry per press.
    updater.check.mockResolvedValue(null);
    await press();
    expect(download.handle.close).toHaveBeenCalledTimes(1);
  });

  it("keeps the update on offer when the restart itself fails", async () => {
    const download = pendingDownload();
    updater.check.mockResolvedValue(download.handle);
    processPlugin.relaunch.mockRejectedValue(new Error("process is pinned"));

    await press();
    await press();
    download.emit({ event: "Finished" });
    download.finish();
    await settle();

    await press();
    expect(processPlugin.relaunch).toHaveBeenCalledTimes(1);
    // Still ready, so pressing again retries the restart instead of
    // re-downloading a build that is already installed.
    expect(statusLabel()).toBe("Update ready to install");
    expect(button().textContent).toBe("Restart to update");

    await press();
    expect(processPlugin.relaunch).toHaveBeenCalledTimes(2);
    expect(updater.check).toHaveBeenCalledTimes(1);
  });
});

describe("About panel updater outside the desktop app", () => {
  it("disables the action instead of importing a plugin that cannot load", async () => {
    window.location.hash = "";
    window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
    updater.check.mockReset();
    document.body.replaceChildren();
    const root = document.createElement("div");
    document.body.append(root);
    mountApp(root, { storePage: stubPage(), gameDetailPage: stubPage() });
    window.location.hash = "#/settings/about";
    await settle();

    const button = root.querySelector<HTMLButtonElement>("#check-updates-button")!;
    expect(button.disabled).toBe(true);
    button.click();
    await settle();
    expect(updater.check).not.toHaveBeenCalled();
    window.location.hash = "";
  });
});

describe("About panel updater after the shell is torn down", () => {
  /**
   * The automatic check waits several seconds before it runs. A shell that is
   * replaced in the meantime must not still fire it: the panel it would write
   * into is detached, and in a test run every mounted shell would otherwise
   * keep a live timer pointed at whatever ran next. That is exactly how this
   * suite started failing on CI and passing locally — the wiring tests above
   * left timers behind, and only a slow enough machine let them land.
   */
  it("drops the deferred check when its shell has been replaced", async () => {
    vi.useFakeTimers();
    try {
      window.location.hash = "";
      window.matchMedia ??= (() => ({ matches: false })) as unknown as typeof window.matchMedia;
      (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {};
      tauri.invoke.mockReset();
      tauri.invoke.mockResolvedValue(undefined);
      updater.check.mockReset();

      document.body.replaceChildren();
      const root = document.createElement("div");
      document.body.append(root);
      mountApp(root, { storePage: stubPage(), gameDetailPage: stubPage() });

      // The shell goes away before the deferred check comes due.
      document.body.replaceChildren();
      await vi.advanceTimersByTimeAsync(30_000);

      expect(updater.check).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
      delete (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__;
      window.location.hash = "";
    }
  });
});
