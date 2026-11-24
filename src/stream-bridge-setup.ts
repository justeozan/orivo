/* ---------------------------------------------------------------------------
   Game streaming in the Mac App Store edition.

   The App Sandbox will not let Orivo start Moonlight with arguments, so the
   host goes through a script the player allows once (`moonlight_bridge.rs`).
   The steps are the desktop setup's, with "Moonlight on this Mac" asking for
   that one permission first: allow, PC, pairing, games. Moonlight keeps its own
   pairing, so a player who already streams with it goes straight to the games.
   --------------------------------------------------------------------------- */

import { t } from "./i18n";

export type BridgeStepId = "moonlight" | "host" | "pair" | "games";
export type BridgeStepStatus = "done" | "active" | "todo";
export const BRIDGE_STEPS: BridgeStepId[] = ["moonlight", "host", "pair", "games"];

export interface MoonlightBridgeView {
  available: boolean;
  installed: boolean;
  moonlight: boolean | null;
}

export type HostState = "paired" | "notPaired" | "unreachable" | "clientMissing";

export interface HostStatus {
  state: HostState;
  host: string;
  games: number;
}

export interface StreamBridgeDeps {
  /** The Tauri command bridge; a test passes a fake. */
  call<T>(command: string, args?: Record<string, unknown>): Promise<T>;
  openLink(target: "moonlight" | "sunshine-pin"): Promise<void>;
  showToast(message: string): void;
  /** The library changed: reload it. */
  onImported(): void;
  openLibrary(): void;
  setTimer?: (callback: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
  /** Between two asks while the PIN is on screen. */
  pollMs?: number;
  /** How long a PIN stays worth waiting for: the script gives up after 3 min. */
  pairingMs?: number;
}

export interface StreamBridgeSnapshot {
  opened: boolean;
  loading: boolean;
  bridge: MoonlightBridgeView;
  busy: boolean;
  hostInput: string;
  savedHost: string;
  host: HostStatus | null;
  error: string;
  pin: string | null;
  games: { phase: "idle" | "importing" | "ready"; count: number };
  editingHost: boolean;
  statuses: Record<BridgeStepId, BridgeStepStatus>;
}

export interface StreamBridgeSetup {
  open(): Promise<void>;
  close(): void;
  snapshot(): StreamBridgeSnapshot;
  onChange(listener: () => void): void;
  allow(): Promise<void>;
  installMoonlight(): Promise<void>;
  checkMoonlight(): Promise<void>;
  setHostInput(value: string): void;
  connect(): Promise<void>;
  editHost(): void;
  startPairing(): Promise<void>;
  openPinPage(): Promise<void>;
  getGames(): Promise<void>;
  openLibrary(): void;
}

const NO_BRIDGE: MoonlightBridgeView = { available: false, installed: false, moonlight: null };

export function readBridgeView(value: unknown): MoonlightBridgeView {
  if (!value || typeof value !== "object") return { ...NO_BRIDGE };
  const raw = value as Record<string, unknown>;
  return {
    available: raw.available === true,
    installed: raw.installed === true,
    moonlight: typeof raw.moonlight === "boolean" ? raw.moonlight : null,
  };
}

export function readHostStatus(value: unknown): HostStatus | null {
  if (!value || typeof value !== "object") return null;
  const raw = value as Record<string, unknown>;
  const states: HostState[] = ["paired", "notPaired", "unreachable", "clientMissing"];
  if (!states.includes(raw.state as HostState)) return null;
  return {
    state: raw.state as HostState,
    host: typeof raw.host === "string" ? raw.host : "",
    games: typeof raw.games === "number" && raw.games >= 0 ? Math.floor(raw.games) : 0,
  };
}

function message(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return t("runners.error.generic");
}

/** Which step is open, from what is known. Pure, so each case is tested. */
export function bridgeStatuses(
  bridge: MoonlightBridgeView,
  host: HostStatus | null,
  savedHost: string,
  gamesImported: boolean,
): Record<BridgeStepId, BridgeStepStatus> {
  const moonlight = bridge.installed && bridge.moonlight === true;
  const reached = Boolean(savedHost) && host !== null && (host.state === "paired" || host.state === "notPaired");
  const paired = host?.state === "paired";
  const order: [BridgeStepId, boolean][] = [
    ["moonlight", moonlight],
    ["host", reached],
    ["pair", paired],
    ["games", gamesImported],
  ];
  const statuses = {} as Record<BridgeStepId, BridgeStepStatus>;
  let activeGiven = false;
  for (const [step, done] of order) {
    if (done && !activeGiven) {
      statuses[step] = "done";
    } else if (!activeGiven) {
      statuses[step] = "active";
      activeGiven = true;
    } else {
      statuses[step] = "todo";
    }
  }
  return statuses;
}

export function createStreamBridgeSetup(deps: StreamBridgeDeps): StreamBridgeSetup {
  const setTimer = deps.setTimer ?? ((callback, ms) => setTimeout(callback, ms));
  const clearTimer = deps.clearTimer ?? ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>));
  const pollMs = deps.pollMs ?? 3_000;
  const pairingMs = deps.pairingMs ?? 180_000;
  const listeners: (() => void)[] = [];

  let opened = false;
  let loading = false;
  let bridge: MoonlightBridgeView = { ...NO_BRIDGE };
  let busy = false;
  let hostInput = "";
  let savedHost = "";
  let host: HostStatus | null = null;
  let error = "";
  let pin: string | null = null;
  let pollHandle: unknown = null;
  let pairingDeadline = 0;
  let games: StreamBridgeSnapshot["games"] = { phase: "idle", count: 0 };
  let editingHost = false;

  const notify = (): void => {
    for (const listener of listeners) listener();
  };

  const stopPolling = (): void => {
    if (pollHandle !== null) clearTimer(pollHandle);
    pollHandle = null;
  };

  const loadBridge = async (): Promise<void> => {
    bridge = readBridgeView(await deps.call("get_moonlight_bridge"));
  };

  const check = async (address: string): Promise<HostStatus | null> => {
    const status = readHostStatus(await deps.call("check_streaming_pc", { host: address }));
    if (status) {
      host = status;
      savedHost = status.host || address;
      hostInput = savedHost;
      if (status.state === "clientMissing") bridge = { ...bridge, moonlight: false };
    }
    return status;
  };

  const poll = (): void => {
    pollHandle = setTimer(async () => {
      pollHandle = null;
      if (!opened || pin === null) return;
      try {
        const status = await check(savedHost);
        if (status?.state === "paired") {
          pin = null;
          notify();
          return;
        }
      } catch {
        // A missed answer is not the end of the wait.
      }
      if (Date.now() > pairingDeadline) {
        pin = null;
        error = t("runners.bridge.pinExpired");
        notify();
        return;
      }
      poll();
    }, pollMs);
  };

  const guarded = async (run: () => Promise<void>): Promise<void> => {
    if (busy) return;
    busy = true;
    error = "";
    notify();
    try {
      await run();
    } catch (failure) {
      error = message(failure);
    } finally {
      busy = false;
      notify();
    }
  };

  const setup: StreamBridgeSetup = {
    async open() {
      if (opened) return;
      opened = true;
      loading = true;
      notify();
      try {
        await loadBridge();
        const settings = (await deps.call<{ host?: unknown }>("get_gamestream_settings")) ?? {};
        savedHost = typeof settings.host === "string" ? settings.host : "";
        hostInput = savedHost;
        if (savedHost && bridge.installed && bridge.moonlight) {
          await check(savedHost).catch(() => null);
        }
      } catch (failure) {
        error = message(failure);
      } finally {
        loading = false;
        notify();
      }
    },

    close() {
      opened = false;
      stopPolling();
      pin = null;
      notify();
    },

    snapshot() {
      return {
        opened,
        loading,
        bridge,
        busy,
        hostInput,
        savedHost,
        host,
        error,
        pin,
        games,
        editingHost,
        statuses: bridgeStatuses(bridge, editingHost ? null : host, savedHost, games.phase === "ready"),
      };
    },

    onChange(listener) {
      listeners.push(listener);
    },

    allow: () =>
      guarded(async () => {
        try {
          await deps.call("install_moonlight_bridge");
        } finally {
          await loadBridge();
        }
        if (bridge.installed && bridge.moonlight && savedHost) await check(savedHost);
      }),

    async installMoonlight() {
      try {
        await deps.openLink("moonlight");
      } catch (failure) {
        deps.showToast(message(failure));
      }
    },

    checkMoonlight: () =>
      guarded(async () => {
        await loadBridge();
        if (bridge.installed && bridge.moonlight && savedHost) await check(savedHost);
      }),

    setHostInput(value) {
      hostInput = value;
    },

    connect: () =>
      guarded(async () => {
        const address = hostInput.trim();
        if (!address) {
          error = t("runners.setup.needHost");
          return;
        }
        const status = await check(address);
        editingHost = false;
        if (status?.state === "unreachable") error = t("runners.setup.unreachable");
      }),

    editHost() {
      editingHost = true;
      notify();
    },

    startPairing: () =>
      guarded(async () => {
        const answer = await deps.call<{ host?: string; pin?: string | null }>("pair_streaming_pc");
        if (!answer?.pin) {
          if (savedHost) await check(savedHost);
          return;
        }
        pin = answer.pin;
        pairingDeadline = Date.now() + pairingMs;
        stopPolling();
        poll();
      }),

    async openPinPage() {
      try {
        await deps.openLink("sunshine-pin");
      } catch (failure) {
        deps.showToast(message(failure));
      }
    },

    getGames: () =>
      guarded(async () => {
        games = { phase: "importing", count: games.count };
        notify();
        try {
          const count = await deps.call<number>("import_streaming_games");
          games = { phase: "ready", count: typeof count === "number" ? count : 0 };
          deps.onImported();
        } catch (failure) {
          games = { phase: "idle", count: games.count };
          throw failure;
        }
      }),

    openLibrary() {
      deps.openLibrary();
    },
  };
  return setup;
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className = "",
  text = "",
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text) node.textContent = text;
  return node;
}

function action(label: string, name: string, kind: "primary" | "quiet" = "quiet"): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.dataset.gsAction = name;
  if (kind === "primary") {
    button.className = "source-account-row__account gs-primary";
    button.append(element("span", "source-account-row__account-name", label));
  } else {
    button.className = "settings-button settings-button--quiet";
    button.textContent = label;
  }
  return button;
}

function gameCount(count: number): string {
  return count === 1 ? t("runners.setup.gameOne") : t("runners.setup.gameMany", { count });
}

export function mountStreamBridgeSetup(root: HTMLElement, setup: StreamBridgeSetup): { render: () => void } {
  const titles: Record<BridgeStepId, string> = {
    moonlight: t("runners.setup.stepClient", { computer: t("runners.setup.thisMac") }),
    host: t("runners.setup.stepHost"),
    pair: t("runners.setup.stepPair"),
    games: t("runners.setup.stepGames"),
  };

  function statusLine(step: BridgeStepId, view: StreamBridgeSnapshot): string {
    const done = view.statuses[step] === "done";
    switch (step) {
      case "moonlight":
        if (done) return t("runners.bridge.ready");
        return view.bridge.installed ? t("runners.setup.clientMissingStatus") : t("runners.bridge.needsOk");
      case "host":
        if (view.busy && view.statuses.host === "active") return t("runners.setup.checkingPc");
        return done ? view.savedHost : t("runners.setup.whereIsIt");
      case "pair":
        return done ? t("runners.setup.paired") : t("runners.setup.pairHint");
      case "games":
        return view.games.phase === "ready" ? gameCount(view.games.count) : t("runners.setup.addedToLibraryHint");
    }
  }

  function moonlightBody(view: StreamBridgeSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const actions = element("div", "gs-step__actions");
    if (!view.bridge.installed) {
      body.append(element("p", "gs-step__text", t("runners.bridge.allowIntro")));
      const allow = action(t("runners.bridge.allow"), "allow-bridge", "primary");
      allow.disabled = view.busy;
      allow.addEventListener("click", () => void setup.allow());
      actions.append(allow);
    } else {
      body.append(element("p", "gs-step__text", t("runners.setup.clientIntro")));
      const install = action(t("runners.setup.installMoonlight"), "install-moonlight", "primary");
      install.addEventListener("click", () => void setup.installMoonlight());
      const again = action(t("runners.setup.checkAgain"), "check-again");
      again.disabled = view.busy;
      again.addEventListener("click", () => void setup.checkMoonlight());
      actions.append(install, again);
    }
    body.append(actions);
    return body;
  }

  function hostBody(view: StreamBridgeSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const field = element("div", "credentials-form__field");
    const label = element("label", "", t("runners.setup.hostLabel"));
    label.htmlFor = "bridge-host";
    const input = element("input", "credentials-form__input");
    input.id = "bridge-host";
    input.type = "text";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.placeholder = t("runners.setup.hostPlaceholder");
    input.value = view.hostInput;
    input.disabled = view.busy;
    input.addEventListener("input", () => setup.setHostInput(input.value));
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        void setup.connect();
      }
    });
    field.append(label, input, element("small", "", t("runners.setup.hostHint")));
    const actions = element("div", "gs-step__actions");
    const connect = action(view.busy ? t("runners.setup.connecting") : t("runners.setup.connect"), "connect", "primary");
    connect.disabled = view.busy;
    connect.addEventListener("click", () => void setup.connect());
    actions.append(connect);
    body.append(field, actions);
    return body;
  }

  function pairBody(view: StreamBridgeSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const actions = element("div", "gs-step__actions");
    if (view.pin) {
      body.append(
        element("div", "gs-pin", view.pin),
        element("p", "gs-step__text", t("runners.setup.pinInstructions", { pin: view.pin })),
      );
      const open = action(t("runners.setup.openPinPage"), "open-pin-page", "primary");
      open.addEventListener("click", () => void setup.openPinPage());
      actions.append(open);
      const waiting = element("p", "gs-step__status", t("runners.setup.waitingPc"));
      waiting.setAttribute("role", "status");
      body.append(actions, element("p", "gs-step__note", t("runners.setup.certificate")), waiting);
      return body;
    }
    body.append(element("p", "gs-step__text", t("runners.setup.pairIntro")));
    const start = action(t("runners.setup.pairWithPc"), "start-pairing", "primary");
    start.disabled = view.busy;
    start.addEventListener("click", () => void setup.startPairing());
    actions.append(start);
    body.append(actions);
    return body;
  }

  function gamesBody(view: StreamBridgeSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const actions = element("div", "gs-step__actions");
    if (view.games.phase === "importing") {
      const status = element("p", "gs-step__status", t("runners.setup.lookingForGames"));
      status.setAttribute("role", "status");
      body.append(status);
      return body;
    }
    if (view.games.phase === "ready") {
      body.append(
        element(
          "p",
          "gs-step__text",
          view.games.count > 0
            ? t("runners.setup.addedToLibrary", { games: gameCount(view.games.count) })
            : t("runners.setup.noGames"),
        ),
      );
      const library = action(t("runners.setup.openLibrary"), "open-library", "primary");
      library.addEventListener("click", () => setup.openLibrary());
      const refresh = action(t("runners.setup.refreshGames"), "refresh-games");
      refresh.disabled = view.busy;
      refresh.addEventListener("click", () => void setup.getGames());
      actions.append(library, refresh);
    } else {
      body.append(element("p", "gs-step__text", t("runners.setup.gamesIntro")));
      const get = action(t("runners.setup.getGames"), "refresh-games", "primary");
      get.disabled = view.busy;
      get.addEventListener("click", () => void setup.getGames());
      actions.append(get);
    }
    body.append(actions);
    return body;
  }

  function renderStep(step: BridgeStepId, index: number, view: StreamBridgeSnapshot): HTMLElement {
    const status = view.statuses[step];
    const expanded = status === "active" || (step === "games" && view.statuses.pair === "done");
    const item = element("li", `gs-step is-${status}`);
    item.dataset.gsStep = step;
    if (status === "active") item.setAttribute("aria-current", "step");
    const head = element("div", "gs-step__head");
    const mark = element("span", "gs-step__mark", status === "done" ? "✓" : String(index + 1));
    mark.setAttribute("aria-hidden", "true");
    const copy = element("div", "gs-step__copy");
    copy.append(element("strong", "", titles[step]), element("small", "", statusLine(step, view)));
    head.append(mark, copy);
    if (step === "host" && status === "done") {
      const change = action(t("runners.setup.change"), "edit-host");
      change.addEventListener("click", () => setup.editHost());
      head.append(change);
    }
    item.append(head);
    if (expanded) {
      const body =
        step === "moonlight"
          ? moonlightBody(view)
          : step === "host"
            ? hostBody(view)
            : step === "pair"
              ? pairBody(view)
              : gamesBody(view);
      body.classList.add("gs-step__body");
      item.append(body);
    }
    return item;
  }

  function render(): void {
    const view = setup.snapshot();
    if (!view.opened) {
      root.replaceChildren();
      return;
    }
    if (view.loading) {
      const loading = element("p", "settings-hint", t("runners.setup.loading"));
      loading.setAttribute("role", "status");
      root.replaceChildren(loading);
      return;
    }
    const focused = document.activeElement instanceof HTMLInputElement && root.contains(document.activeElement);
    const children: HTMLElement[] = [];
    if (view.error) {
      const error = element("p", "gs-step__error", view.error);
      error.setAttribute("role", "alert");
      children.push(error);
    }
    const steps = element("ol", "gs-steps");
    BRIDGE_STEPS.forEach((step, index) => steps.append(renderStep(step, index, view)));
    children.push(steps);
    root.replaceChildren(...children);
    if (focused) root.querySelector<HTMLInputElement>("#bridge-host")?.focus();
  }

  setup.onChange(render);
  render();
  return { render };
}
