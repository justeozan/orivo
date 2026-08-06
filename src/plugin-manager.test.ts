import { describe, expect, it } from "vitest";
import {
  createDefaultPluginManagerClient,
  createPluginManagerController,
  emptyPluginCatalog,
  formatInstallLabel,
  formatPluginSize,
  formatPluginStatus,
  isPluginInstallBusy,
  pluginErrorMessage,
  pluginPercent,
  readPluginCatalog,
  type AvailablePluginView,
  type InstalledPluginView,
  type PluginCatalogView,
  type PluginInstallProgress,
  type PluginManagerClient,
} from "./plugin-manager";

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

function installed(overrides: Partial<InstalledPluginView> = {}): InstalledPluginView {
  return {
    id: "com.orivo.dolphin",
    name: "Dolphin",
    version: "5.0.1",
    extensions: [".iso", ".rvz"],
    state: "ready",
    message: "",
    trusted: true,
    ...overrides,
  };
}

function available(overrides: Partial<AvailablePluginView> = {}): AvailablePluginView {
  return {
    id: "com.orivo.dolphin",
    name: "Dolphin",
    version: "5.0.1",
    summary: "GameCube and Wii games.",
    sizeBytes: 48_234_496,
    installed: false,
    ...overrides,
  };
}

function catalog(overrides: Partial<PluginCatalogView> = {}): PluginCatalogView {
  return { installed: [installed()], available: [available()], ...overrides };
}

function progress(overrides: Partial<PluginInstallProgress> = {}): PluginInstallProgress {
  return {
    pluginId: "com.orivo.dolphin",
    phase: "downloading",
    percent: 42,
    message: "",
    ...overrides,
  };
}

interface RecordedPluginManager {
  client: PluginManagerClient;
  calls: string[];
  teardowns: string[];
  emit(next: PluginInstallProgress): void;
}

function createFakePluginManager(
  overrides: Partial<PluginManagerClient> = {},
): RecordedPluginManager {
  const calls: string[] = [];
  const teardowns: string[] = [];
  const listeners = new Set<(next: PluginInstallProgress) => void>();
  const client: PluginManagerClient = {
    async getCatalog(signal) {
      calls.push("getCatalog");
      return overrides.getCatalog ? overrides.getCatalog(signal) : catalog();
    },
    async installFromRegistry(pluginId, signal) {
      calls.push(`installFromRegistry:${pluginId}`);
      if (overrides.installFromRegistry) await overrides.installFromRegistry(pluginId, signal);
    },
    async installFromFile(signal) {
      calls.push("installFromFile");
      return overrides.installFromFile ? overrides.installFromFile(signal) : "com.orivo.mame";
    },
    async uninstall(pluginId, signal) {
      calls.push(`uninstall:${pluginId}`);
      if (overrides.uninstall) await overrides.uninstall(pluginId, signal);
    },
    subscribe(onProgress) {
      calls.push("subscribe");
      listeners.add(onProgress);
      return () => {
        teardowns.push("unsubscribe");
        listeners.delete(onProgress);
      };
    },
  };
  return {
    client,
    calls,
    teardowns,
    emit(next) {
      for (const listener of [...listeners]) listener(next);
    },
  };
}

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

const liveSignal = (): AbortSignal => new AbortController().signal;

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

describe("formatPluginStatus", () => {
  it("names every state a signed plugin can be in", () => {
    expect(formatPluginStatus(installed({ state: "ready" }))).toBe("Installed");
    expect(formatPluginStatus(installed({ state: "incompatible" }))).toBe("Incompatible");
    expect(formatPluginStatus(installed({ state: "invalid" }))).toBe("Invalid");
  });

  it("marks an unsigned package in every state without hiding the state", () => {
    expect(formatPluginStatus(installed({ state: "ready", trusted: false }))).toBe(
      "Installed · unsigned",
    );
    expect(formatPluginStatus(installed({ state: "incompatible", trusted: false }))).toBe(
      "Incompatible · unsigned",
    );
    expect(formatPluginStatus(installed({ state: "invalid", trusted: false }))).toBe(
      "Invalid · unsigned",
    );
  });
});

describe("formatInstallLabel", () => {
  it("offers the install when nothing is happening", () => {
    expect(formatInstallLabel(null, available())).toBe("Install");
    expect(formatInstallLabel(null, available({ installed: true }))).toBe("Installed");
  });

  it("reports each phase the host can announce", () => {
    expect(formatInstallLabel(progress({ phase: "downloading", percent: 42 }), available())).toBe(
      "Downloading 42%",
    );
    expect(formatInstallLabel(progress({ phase: "verifying" }), available())).toBe("Verifying…");
    expect(formatInstallLabel(progress({ phase: "installing" }), available())).toBe(
      "Installing…",
    );
    expect(formatInstallLabel(progress({ phase: "installed", percent: 100 }), available())).toBe(
      "Installed",
    );
    expect(formatInstallLabel(progress({ phase: "failed" }), available())).toBe("Retry");
  });

  it("rounds and clamps the percentage the host sent", () => {
    expect(formatInstallLabel(progress({ percent: 41.6 }), available())).toBe(
      "Downloading 42%",
    );
    expect(formatInstallLabel(progress({ percent: -12 }), available())).toBe("Downloading 0%");
    expect(formatInstallLabel(progress({ percent: 240 }), available())).toBe(
      "Downloading 100%",
    );
    expect(pluginPercent(null)).toBe(0);
    expect(pluginPercent(progress({ percent: Number.NaN }))).toBe(0);
  });

  it("still reads Réessayer for a failure on an already installed plugin", () => {
    // A failed upgrade must not read "Installed" just because an older build is
    // still on disk: the button has to offer the retry.
    expect(
      formatInstallLabel(progress({ phase: "failed" }), available({ installed: true })),
    ).toBe("Retry");
  });
});

describe("isPluginInstallBusy", () => {
  it("is busy only while the host is still working", () => {
    expect(isPluginInstallBusy(null)).toBe(false);
    expect(isPluginInstallBusy(progress({ phase: "downloading" }))).toBe(true);
    expect(isPluginInstallBusy(progress({ phase: "verifying" }))).toBe(true);
    expect(isPluginInstallBusy(progress({ phase: "installing" }))).toBe(true);
    expect(isPluginInstallBusy(progress({ phase: "installed" }))).toBe(false);
    expect(isPluginInstallBusy(progress({ phase: "failed" }))).toBe(false);
  });
});

describe("formatPluginSize", () => {
  it("uses the units a French desktop shows and drops a meaningless zero", () => {
    expect(formatPluginSize(48_234_496)).toBe("48,2 Mo");
    expect(formatPluginSize(2_400_000_000)).toBe("2,4 Go");
    expect(formatPluginSize(84_000)).toBe("84 ko");
    expect(formatPluginSize(0)).toBe("");
    expect(formatPluginSize(Number.NaN)).toBe("");
  });
});

describe("pluginErrorMessage", () => {
  it("prefers the host's own words and falls back to a sentence", () => {
    expect(pluginErrorMessage("Signature invalide.")).toBe("Signature invalide.");
    expect(pluginErrorMessage(new Error("Le disque est plein."))).toBe("Le disque est plein.");
    expect(pluginErrorMessage(null)).toBe("L'opération n'a pas abouti.");
    expect(pluginErrorMessage("   ")).toBe("L'opération n'a pas abouti.");
  });
});

// ---------------------------------------------------------------------------
// Reading the host's answer
// ---------------------------------------------------------------------------

describe("readPluginCatalog", () => {
  it("collapses anything that is not a catalogue to an empty one", () => {
    expect(readPluginCatalog(null)).toEqual(emptyPluginCatalog());
    expect(readPluginCatalog("nope")).toEqual(emptyPluginCatalog());
    expect(readPluginCatalog({})).toEqual(emptyPluginCatalog());
  });

  it("drops rows that cannot name the plugin they act on", () => {
    const view = readPluginCatalog({
      installed: [{ name: "No id" }, installed()],
      available: [{ summary: "No id either" }, available()],
    });
    expect(view.installed.map((plugin) => plugin.id)).toEqual(["com.orivo.dolphin"]);
    expect(view.available.map((plugin) => plugin.id)).toEqual(["com.orivo.dolphin"]);
  });

  it("reads an unknown state as invalid and an unstated signature as untrusted", () => {
    const view = readPluginCatalog({
      installed: [{ id: "com.orivo.mystery", state: "brand-new" }],
      available: [],
    });
    expect(view.installed[0]).toEqual({
      id: "com.orivo.mystery",
      name: "com.orivo.mystery",
      version: "",
      extensions: [],
      state: "invalid",
      message: "",
      trusted: false,
    });
    expect(formatPluginStatus(view.installed[0])).toBe("Invalid · unsigned");
  });
});

// ---------------------------------------------------------------------------
// The controller
// ---------------------------------------------------------------------------

describe("createPluginManagerController", () => {
  it("keeps the progress map in step with the host and notifies on every change", async () => {
    const fake = createFakePluginManager();
    const controller = createPluginManagerController(fake.client);
    let changes = 0;
    controller.onChange(() => {
      changes += 1;
    });

    await controller.load(liveSignal());
    expect(controller.catalog().available[0].name).toBe("Dolphin");
    expect(controller.progressFor("com.orivo.dolphin")).toBeNull();
    expect(fake.calls).toEqual(["getCatalog", "subscribe"]);

    fake.emit(progress({ phase: "downloading", percent: 12 }));
    expect(controller.progressFor("com.orivo.dolphin")?.phase).toBe("downloading");

    fake.emit(progress({ phase: "verifying", percent: 90, message: "Signature Ed25519" }));
    expect(controller.progressFor("com.orivo.dolphin")).toEqual(
      progress({ phase: "verifying", percent: 90, message: "Signature Ed25519" }),
    );

    fake.emit(progress({ pluginId: "com.orivo.mame", phase: "installing", percent: 30 }));
    expect(controller.progressFor("com.orivo.dolphin")?.percent).toBe(90);
    expect(controller.progressFor("com.orivo.mame")?.percent).toBe(30);
    expect(controller.progressFor("com.orivo.unknown")).toBeNull();

    // One notification per load and per emitted progress, and no more.
    expect(changes).toBe(4);
  });

  it("ignores an event that names no plugin or no phase it knows", async () => {
    const fake = createFakePluginManager();
    const controller = createPluginManagerController(fake.client);
    let changes = 0;
    controller.onChange(() => {
      changes += 1;
    });
    await controller.load(liveSignal());

    fake.emit({ phase: "downloading", percent: 10, message: "" } as PluginInstallProgress);
    fake.emit(progress({ phase: "teleporting" as PluginInstallProgress["phase"] }));

    expect(controller.progressFor("com.orivo.dolphin")).toBeNull();
    expect(changes).toBe(1);
  });

  it("opens exactly one event channel however many times it is loaded", async () => {
    const fake = createFakePluginManager();
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());
    await controller.load(liveSignal());

    expect(fake.calls).toEqual(["getCatalog", "subscribe", "getCatalog"]);
  });

  it("starts at downloading on click and reloads the catalogue once installed", async () => {
    const fake = createFakePluginManager({
      getCatalog: async () =>
        fake.calls.filter((call) => call === "getCatalog").length > 1
          ? catalog({ available: [available({ installed: true })] })
          : catalog(),
    });
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());

    const started = controller.installFromRegistry("com.orivo.dolphin");
    // The phase flips before the command resolves: the user clicked now.
    expect(controller.progressFor("com.orivo.dolphin")?.phase).toBe("downloading");
    expect(formatInstallLabel(controller.progressFor("com.orivo.dolphin"), available())).toBe(
      "Downloading 0%",
    );
    await started;

    expect(fake.calls).toEqual([
      "getCatalog",
      "subscribe",
      "installFromRegistry:com.orivo.dolphin",
      "getCatalog",
    ]);
    // Even with no event bus at all, the row lands on Installé.
    expect(controller.progressFor("com.orivo.dolphin")?.phase).toBe("installed");
    expect(controller.catalog().available[0].installed).toBe(true);
  });

  it("turns a refused install into a failed phase carrying the host's message", async () => {
    const fake = createFakePluginManager({
      installFromRegistry: async () => {
        throw "Signature invalide.";
      },
    });
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());
    await controller.installFromRegistry("com.orivo.dolphin");

    const failed = controller.progressFor("com.orivo.dolphin");
    expect(failed?.phase).toBe("failed");
    expect(failed?.message).toBe("Signature invalide.");
    expect(formatInstallLabel(failed, available())).toBe("Retry");
    // A refused install changed nothing on disk, so nothing is re-read.
    expect(fake.calls).toEqual([
      "getCatalog",
      "subscribe",
      "installFromRegistry:com.orivo.dolphin",
    ]);
  });

  it("reloads the catalogue after a file install and returns the installed id", async () => {
    const fake = createFakePluginManager();
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());

    await expect(controller.installFromFile()).resolves.toBe("com.orivo.mame");
    expect(fake.calls).toEqual(["getCatalog", "subscribe", "installFromFile", "getCatalog"]);
  });

  it("treats a cancelled picker as a no-op", async () => {
    const fake = createFakePluginManager({ installFromFile: async () => null });
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());

    await expect(controller.installFromFile()).resolves.toBeNull();
    // Nothing changed on disk, so the panel is not made to flicker.
    expect(fake.calls).toEqual(["getCatalog", "subscribe", "installFromFile"]);
  });

  it("hands a refused file install back to the caller for a toast", async () => {
    const fake = createFakePluginManager({
      installFromFile: async () => {
        throw "Ce paquet n'est pas un plugin Orivo.";
      },
    });
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());

    await expect(controller.installFromFile()).rejects.toBe("Ce paquet n'est pas un plugin Orivo.");
    expect(fake.calls).toEqual(["getCatalog", "subscribe", "installFromFile"]);
  });

  it("reloads the catalogue after an uninstall and forgets the old progress", async () => {
    const fake = createFakePluginManager({
      getCatalog: async () =>
        fake.calls.filter((call) => call === "getCatalog").length > 1
          ? emptyPluginCatalog()
          : catalog(),
    });
    const controller = createPluginManagerController(fake.client);
    let changes = 0;
    controller.onChange(() => {
      changes += 1;
    });
    await controller.load(liveSignal());
    fake.emit(progress({ phase: "installed", percent: 100 }));
    expect(controller.catalog().installed).toHaveLength(1);

    await controller.uninstall("com.orivo.dolphin");

    expect(fake.calls).toEqual([
      "getCatalog",
      "subscribe",
      "uninstall:com.orivo.dolphin",
      "getCatalog",
    ]);
    expect(controller.catalog().installed).toEqual([]);
    // A reinstall has to start from "Install", not from the old bar.
    expect(controller.progressFor("com.orivo.dolphin")).toBeNull();
    expect(changes).toBe(3);
  });

  it("hands a refused uninstall back to the caller and keeps the row", async () => {
    const fake = createFakePluginManager({
      uninstall: async () => {
        throw new Error("Le plugin est en cours d'utilisation.");
      },
    });
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());

    await expect(controller.uninstall("com.orivo.dolphin")).rejects.toThrow(
      "Le plugin est en cours d'utilisation.",
    );
    expect(controller.catalog().installed).toHaveLength(1);
    expect(fake.calls).toEqual(["getCatalog", "subscribe", "uninstall:com.orivo.dolphin"]);
  });

  it("drops a catalogue that resolves after the activation was cancelled", async () => {
    const aborted = new AbortController();
    const fake = createFakePluginManager({
      getCatalog: async () => {
        aborted.abort();
        return catalog();
      },
    });
    const controller = createPluginManagerController(fake.client);
    await controller.load(aborted.signal);

    expect(controller.catalog()).toEqual(emptyPluginCatalog());
    expect(fake.calls).toEqual(["getCatalog"]);
  });

  it("drops its subscription and stops notifying once disposed", async () => {
    const fake = createFakePluginManager();
    const controller = createPluginManagerController(fake.client);
    let changes = 0;
    controller.onChange(() => {
      changes += 1;
    });
    await controller.load(liveSignal());
    fake.emit(progress());
    expect(changes).toBe(2);

    controller.dispose();
    expect(fake.teardowns).toEqual(["unsubscribe"]);
    fake.emit(progress({ percent: 90 }));

    expect(changes).toBe(2);
    expect(controller.progressFor("com.orivo.dolphin")).toBeNull();
    expect(controller.catalog()).toEqual(emptyPluginCatalog());

    // A disposed controller is inert rather than throwing: a click that lands
    // during teardown must not take the panel down with it.
    await controller.installFromRegistry("com.orivo.dolphin");
    await expect(controller.installFromFile()).resolves.toBeNull();
    await controller.uninstall("com.orivo.dolphin");
    await flush();
    expect(fake.calls).toEqual(["getCatalog", "subscribe"]);
  });

  it("stops listening as soon as onChange is unsubscribed", async () => {
    const fake = createFakePluginManager();
    const controller = createPluginManagerController(fake.client);
    let changes = 0;
    const off = controller.onChange(() => {
      changes += 1;
    });
    await controller.load(liveSignal());
    off();
    fake.emit(progress());
    expect(changes).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// A host that cannot answer
// ---------------------------------------------------------------------------

describe("a host without the plugin commands", () => {
  it("yields an empty catalogue when getCatalog throws", async () => {
    const fake = createFakePluginManager({
      getCatalog: async () => {
        throw new Error("unknown command: get_plugin_catalog");
      },
    });
    const controller = createPluginManagerController(fake.client);
    await controller.load(liveSignal());

    expect(controller.catalog()).toEqual({ installed: [], available: [] });
    expect(controller.catalog().installed).toEqual([]);
    expect(controller.catalog().available).toEqual([]);
    // The panel still paints, so the event channel is still worth holding: an
    // install can only ever be started from a row, and there are none.
    expect(fake.calls).toEqual(["getCatalog", "subscribe"]);
  });

  it("never lets the default client's catalogue throw outside the desktop shell", async () => {
    const client = createDefaultPluginManagerClient();
    await expect(client.getCatalog(liveSignal())).resolves.toEqual({
      installed: [],
      available: [],
    });
    // No native picker in a browser tab, which reads as a cancelled pick.
    await expect(client.installFromFile(liveSignal())).resolves.toBeNull();
    await expect(client.uninstall("com.orivo.dolphin", liveSignal())).resolves.toBeUndefined();
    expect(() => client.subscribe(() => {})()).not.toThrow();
  });

  it("says why a registry install cannot run outside the desktop shell", async () => {
    const client = createDefaultPluginManagerClient();
    // The row prints this message, so it has to be a sentence and not a stack.
    await expect(client.installFromRegistry("com.orivo.dolphin", liveSignal())).rejects.toThrow(
      "L'installation de plugins est réservée à l'application Orivo.",
    );

    const controller = createPluginManagerController(client);
    await controller.load(liveSignal());
    await controller.installFromRegistry("com.orivo.dolphin");
    expect(controller.progressFor("com.orivo.dolphin")?.message).toBe(
      "L'installation de plugins est réservée à l'application Orivo.",
    );
    expect(formatInstallLabel(controller.progressFor("com.orivo.dolphin"), available())).toBe(
      "Retry",
    );
  });
});
