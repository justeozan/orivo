/* ---------------------------------------------------------------------------
   Settings › Plugins › Orivo Streaming: the setup of the client built into
   Orivo (`builtin_stream.rs`).

   Same walk as the Moonlight setup (`gamestream-setup.ts`), one step shorter:
   there is no app to find, because the client is Orivo itself. So three steps,

     1. the gaming PC     (one field, checked before it is kept)
     2. pairing           (Orivo draws the PIN and waits for it)
     3. the games         (listed and added to the Library)

   and the same split: `builtinSetupStage` decides the step from the facts, the
   controller owns the flow, the view draws what the controller says. The
   pairing itself (certificates, keys) never leaves Rust; the WebView sees an
   address, a PIN and a yes or a no.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";
import { SETUP_COPY, thisComputerName, tidyHostInput } from "./gamestream-setup";
import { t } from "./i18n";

/** Its row in Settings › Plugins, and the detail view that row opens. */
export const BUILTIN_STREAM_VIEW = "builtin-streaming";

/* ---------------------------------------------------------------------------
   Which step is the current one
   --------------------------------------------------------------------------- */

export type BuiltinStepId = "host" | "pair" | "games";
export type BuiltinStage = BuiltinStepId | "done";
export type BuiltinStepStatus = "done" | "active" | "todo";

export const BUILTIN_STEPS: readonly BuiltinStepId[] = ["host", "pair", "games"];

export interface BuiltinFacts {
  /** An address for the gaming PC is saved. */
  hostSaved: boolean;
  /** The PC answered when it was last asked. */
  hostReachable: boolean;
  /** The PC accepts this client. */
  paired: boolean;
  /** The PC's games were brought into the Library during this visit. */
  gamesImported: boolean;
}

/**
 * The step the person is on. A step counts only once every step before it
 * does, and being paired proves the PC answered, so a paired PC is never sent
 * back to "Connect".
 */
export function builtinSetupStage(facts: BuiltinFacts): BuiltinStage {
  if (!facts.hostSaved || !(facts.hostReachable || facts.paired)) return "host";
  if (!facts.paired) return "pair";
  if (!facts.gamesImported) return "games";
  return "done";
}

export function builtinStepStatuses(
  facts: BuiltinFacts,
): Record<BuiltinStepId, BuiltinStepStatus> {
  const stage = builtinSetupStage(facts);
  const current = stage === "done" ? BUILTIN_STEPS.length : BUILTIN_STEPS.indexOf(stage);
  const statuses = {} as Record<BuiltinStepId, BuiltinStepStatus>;
  BUILTIN_STEPS.forEach((step, index) => {
    statuses[step] = index < current ? "done" : index === current ? "active" : "todo";
  });
  return statuses;
}

/* ---------------------------------------------------------------------------
   The host's side
   --------------------------------------------------------------------------- */

export type BuiltinHostState = "paired" | "notPaired" | "unreachable";

export interface BuiltinHostStatus {
  state: BuiltinHostState;
  host: string;
  games: number;
}

export type BuiltinPairingStatus =
  | { phase: "idle" }
  | { phase: "waiting"; pin: string }
  | { phase: "paired" }
  | { phase: "failed"; message: string };

export interface BuiltinStreamClient {
  settings(): Promise<{ host: string; paired: boolean }>;
  /** An empty host asks the saved one. A PC that answered is kept. */
  probe(host: string): Promise<BuiltinHostStatus>;
  beginPairing(deviceName: string): Promise<{ pin: string; host: string }>;
  pairingStatus(): Promise<BuiltinPairingStatus>;
  cancelPairing(): Promise<void>;
  forget(): Promise<void>;
  syncGames(): Promise<{ games: number; changed: boolean }>;
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function readHostStatus(value: unknown, typed: string): BuiltinHostStatus {
  const raw = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  const state: BuiltinHostState =
    raw.state === "paired" || raw.state === "notPaired" ? raw.state : "unreachable";
  return {
    state,
    host: typeof raw.host === "string" ? raw.host : typed,
    games: typeof raw.games === "number" ? raw.games : 0,
  };
}

export function readPairingStatus(value: unknown): BuiltinPairingStatus {
  const raw = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  switch (raw.phase) {
    case "waiting":
      return { phase: "waiting", pin: typeof raw.pin === "string" ? raw.pin : "" };
    case "paired":
      return { phase: "paired" };
    case "failed":
      return {
        phase: "failed",
        message: typeof raw.message === "string" && raw.message ? raw.message : SETUP_COPY.pairingFailed,
      };
    default:
      return { phase: "idle" };
  }
}

export function builtinStreamErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return t("runners.error.generic");
}

export function createBuiltinStreamClient(): BuiltinStreamClient {
  const needsApp = (): void => {
    if (!isTauriRuntime()) throw new Error(t("runners.builtin.appOnly"));
  };
  return {
    async settings() {
      if (!isTauriRuntime()) return { host: "", paired: false };
      try {
        const raw = (await invoke<unknown>("get_builtin_stream_settings")) as Record<string, unknown>;
        return {
          host: typeof raw?.host === "string" ? raw.host : "",
          paired: raw?.paired === true,
        };
      } catch {
        return { host: "", paired: false };
      }
    },
    async probe(host) {
      needsApp();
      return readHostStatus(await invoke("probe_builtin_stream_host", { host }), host);
    },
    async beginPairing(deviceName) {
      needsApp();
      const raw = (await invoke<unknown>("begin_builtin_stream_pairing", { deviceName })) as Record<
        string,
        unknown
      >;
      return {
        pin: typeof raw?.pin === "string" ? raw.pin : "",
        host: typeof raw?.host === "string" ? raw.host : "",
      };
    },
    async pairingStatus() {
      needsApp();
      return readPairingStatus(await invoke("builtin_stream_pairing_status"));
    },
    async cancelPairing() {
      if (!isTauriRuntime()) return;
      await invoke("cancel_builtin_stream_pairing");
    },
    async forget() {
      needsApp();
      await invoke("forget_builtin_stream");
    },
    async syncGames() {
      needsApp();
      const raw = (await invoke<unknown>("sync_stream_games")) as Record<string, unknown>;
      return {
        games: typeof raw?.games === "number" ? raw.games : 0,
        changed: raw?.changed === true,
      };
    },
  };
}

/**
 * "this iPhone", "this Mac": what the person is holding. Asked before the
 * desktop names, because an iPhone's user agent says "like Mac OS X".
 */
export function builtinComputerName(userAgent: string): string {
  if (/iPhone/i.test(userAgent)) return t("runners.builtin.thisIphone");
  if (/iPad/i.test(userAgent)) return t("runners.builtin.thisIpad");
  if (/Android/i.test(userAgent)) return t("runners.builtin.thisPhone");
  return thisComputerName(userAgent);
}

/** The name Sunshine lists this client under: "Orivo on this Mac" reads oddly there. */
export function deviceNameFor(userAgent: string): string {
  if (/iPhone/i.test(userAgent)) return "Orivo (iPhone)";
  if (/iPad/i.test(userAgent)) return "Orivo (iPad)";
  if (/Android/i.test(userAgent)) return "Orivo (Android)";
  if (/Mac/i.test(userAgent)) return "Orivo (Mac)";
  if (/Windows/i.test(userAgent)) return "Orivo (PC)";
  return "Orivo";
}

/* ---------------------------------------------------------------------------
   The controller
   --------------------------------------------------------------------------- */

export type BuiltinPairingPhase = "idle" | "starting" | "waiting" | "failed" | "paired";
export type BuiltinGamesPhase = "idle" | "importing" | "ready" | "empty" | "failed";

export interface BuiltinSetupSnapshot {
  opened: boolean;
  loading: boolean;
  computerName: string;
  facts: BuiltinFacts;
  stage: BuiltinStage;
  statuses: Record<BuiltinStepId, BuiltinStepStatus>;
  editing: BuiltinStepId | null;
  host: {
    input: string;
    saved: string;
    checking: boolean;
    busy: boolean;
    error: string | null;
  };
  pairing: {
    phase: BuiltinPairingPhase;
    pin: string;
    message: string | null;
  };
  games: {
    phase: BuiltinGamesPhase;
    count: number;
    message: string | null;
  };
}

export interface BuiltinSetupDeps {
  client: BuiltinStreamClient;
  /** The Library changed: read it again. */
  refreshLibrary(): Promise<void> | void;
  openLibrary(): void;
  computerName?: string;
  deviceName?: string;
  setTimer?: (callback: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
  now?: () => number;
  pairingPollMs?: number;
  /** The host holds the pairing open for three minutes (`PAIRING_WAIT`); a little more here. */
  pairingTimeoutMs?: number;
}

export interface BuiltinStreamSetup {
  snapshot(): BuiltinSetupSnapshot;
  onChange(callback: () => void): () => void;
  open(): Promise<void>;
  close(): void;
  setHostInput(value: string): void;
  connect(): Promise<void>;
  startPairing(): Promise<void>;
  getGames(): Promise<void>;
  recheck(): Promise<void>;
  forget(): Promise<void>;
  openLibrary(): void;
  edit(step: BuiltinStepId): void;
  stopEditing(): void;
}

const DEFAULT_POLL_MS = 1_500;
const DEFAULT_PAIRING_TIMEOUT_MS = 3 * 60_000 + 15_000;

export function createBuiltinStreamSetup(deps: BuiltinSetupDeps): BuiltinStreamSetup {
  const setTimer = deps.setTimer ?? ((callback, ms) => setTimeout(callback, ms));
  const clearTimer =
    deps.clearTimer ?? ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>));
  const now = deps.now ?? (() => Date.now());
  const pollMs = deps.pairingPollMs ?? DEFAULT_POLL_MS;
  const pairingTimeoutMs = deps.pairingTimeoutMs ?? DEFAULT_PAIRING_TIMEOUT_MS;
  const userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent;
  const computerName = deps.computerName ?? builtinComputerName(userAgent);
  const deviceName = deps.deviceName ?? deviceNameFor(userAgent);

  const listeners = new Set<() => void>();
  let isOpen = false;
  let loading = false;
  let savedHost = "";
  let hostInput = "";
  let hostChecking = false;
  let hostBusy = false;
  let hostError: string | null = null;
  let hostReachable = false;
  let paired = false;
  let editing: BuiltinStepId | null = null;
  let pairingPhase: BuiltinPairingPhase = "idle";
  let pairingPin = "";
  let pairingMessage: string | null = null;
  let pairingToken = 0;
  let pairingTimer: unknown = null;
  let gamesPhase: BuiltinGamesPhase = "idle";
  let gamesCount = 0;
  let gamesMessage: string | null = null;

  const notify = (): void => {
    for (const listener of [...listeners]) listener();
  };

  const facts = (): BuiltinFacts => ({
    hostSaved: savedHost !== "",
    hostReachable,
    paired,
    gamesImported: gamesPhase === "ready" || gamesPhase === "empty",
  });

  const snapshot = (): BuiltinSetupSnapshot => {
    const current = facts();
    return {
      opened: isOpen,
      loading,
      computerName,
      facts: current,
      stage: builtinSetupStage(current),
      statuses: builtinStepStatuses(current),
      editing,
      host: { input: hostInput, saved: savedHost, checking: hostChecking, busy: hostBusy, error: hostError },
      pairing: { phase: pairingPhase, pin: pairingPin, message: pairingMessage },
      games: { phase: gamesPhase, count: gamesCount, message: gamesMessage },
    };
  };

  const stopPolling = (): void => {
    pairingToken += 1;
    if (pairingTimer !== null) clearTimer(pairingTimer);
    pairingTimer = null;
  };

  /* -- games ---------------------------------------------------------------- */

  const getGames = async (): Promise<void> => {
    if (gamesPhase === "importing") return;
    gamesPhase = "importing";
    gamesMessage = null;
    notify();
    try {
      const result = await deps.client.syncGames();
      gamesCount = result.games;
      gamesPhase = result.games > 0 ? "ready" : "empty";
      if (result.changed) await deps.refreshLibrary();
    } catch (error) {
      gamesPhase = "failed";
      gamesMessage = builtinStreamErrorMessage(error);
    }
    notify();
  };

  const advanceToGames = async (): Promise<void> => {
    editing = null;
    await getGames();
  };

  /* -- pairing -------------------------------------------------------------- */

  const poll = async (token: number, deadline: number): Promise<void> => {
    if (token !== pairingToken) return;
    let status: BuiltinPairingStatus;
    try {
      status = await deps.client.pairingStatus();
    } catch {
      // One failed question is not the end of the attempt; the deadline is.
      status = { phase: "waiting", pin: pairingPin };
    }
    if (token !== pairingToken) return;
    if (status.phase === "paired") {
      stopPolling();
      paired = true;
      hostReachable = true;
      pairingPhase = "paired";
      notify();
      await advanceToGames();
      return;
    }
    if (status.phase === "failed" || status.phase === "idle") {
      stopPolling();
      pairingPhase = "failed";
      pairingMessage = status.phase === "failed" ? status.message : SETUP_COPY.pairingFailed;
      notify();
      return;
    }
    if (now() >= deadline) {
      stopPolling();
      await deps.client.cancelPairing().catch(() => undefined);
      pairingPhase = "failed";
      pairingMessage = SETUP_COPY.pairingFailed;
      notify();
      return;
    }
    pairingTimer = setTimer(() => void poll(token, deadline), pollMs);
  };

  const startPairing = async (): Promise<void> => {
    stopPolling();
    const token = pairingToken;
    pairingPhase = "starting";
    pairingMessage = null;
    notify();
    let started: { pin: string; host: string };
    try {
      started = await deps.client.beginPairing(deviceName);
    } catch (error) {
      if (token !== pairingToken) return;
      pairingPhase = "failed";
      pairingMessage = builtinStreamErrorMessage(error);
      notify();
      return;
    }
    if (token !== pairingToken) return;
    pairingPin = started.pin;
    pairingPhase = "waiting";
    notify();
    pairingTimer = setTimer(() => void poll(token, now() + pairingTimeoutMs), pollMs);
  };

  /* -- the PC --------------------------------------------------------------- */

  const fold = (status: BuiltinHostStatus): void => {
    hostReachable = status.state !== "unreachable";
    paired = status.state === "paired";
  };

  const connect = async (): Promise<void> => {
    if (hostBusy) return;
    const typed = tidyHostInput(hostInput);
    hostError = null;
    if (!typed) {
      hostError = SETUP_COPY.needHost;
      notify();
      return;
    }
    hostInput = typed;
    hostBusy = true;
    stopPolling();
    notify();
    try {
      const status = await deps.client.probe(typed);
      fold(status);
      if (status.state === "unreachable") {
        hostError = SETUP_COPY.unreachable;
      } else {
        // The host keeps an address only once the PC behind it has answered.
        savedHost = status.host || typed;
        hostInput = savedHost;
        editing = null;
        if (status.state === "paired") {
          pairingPhase = "paired";
          hostBusy = false;
          await advanceToGames();
        } else {
          pairingPhase = "idle";
          hostBusy = false;
          // Straight to the PIN: the next thing they need is the number.
          await startPairing();
        }
      }
    } catch (error) {
      hostError = builtinStreamErrorMessage(error);
    } finally {
      hostBusy = false;
      notify();
    }
  };

  const recheck = async (): Promise<void> => {
    if (!savedHost) return;
    hostChecking = true;
    hostError = null;
    gamesMessage = null;
    notify();
    try {
      const status = await deps.client.probe("");
      fold(status);
      if (status.state === "unreachable") hostError = SETUP_COPY.unreachable;
      if (status.state === "notPaired") pairingPhase = "idle";
      if (status.state === "paired" && gamesPhase === "failed") gamesPhase = "idle";
    } catch (error) {
      hostError = builtinStreamErrorMessage(error);
    } finally {
      hostChecking = false;
      notify();
    }
  };

  return {
    snapshot,

    onChange(callback) {
      listeners.add(callback);
      return () => {
        listeners.delete(callback);
      };
    },

    async open() {
      isOpen = true;
      loading = true;
      hostError = null;
      editing = null;
      pairingPhase = "idle";
      pairingMessage = null;
      gamesPhase = "idle";
      gamesMessage = null;
      notify();
      const settings = await deps.client.settings();
      if (!isOpen) return;
      savedHost = settings.host.trim();
      hostInput = savedHost;
      hostReachable = false;
      paired = false;
      loading = false;
      notify();
      // Shown how things are now, not how they were left: the PC may be off,
      // or may have forgotten this client in Sunshine since.
      if (savedHost) {
        hostChecking = true;
        notify();
        try {
          fold(await deps.client.probe(""));
        } catch {
          // The PC not answering is a state, not a failure of opening the view.
        }
        hostChecking = false;
        notify();
        // A paired PC is asked for its games straight away: the list is one
        // request, and it is what the person came back to see.
        if (paired && isOpen) {
          pairingPhase = "paired";
          await getGames();
        }
      }
    },

    close() {
      isOpen = false;
      const waiting = pairingPhase === "waiting" || pairingPhase === "starting";
      stopPolling();
      if (waiting) void deps.client.cancelPairing().catch(() => undefined);
      if (pairingPhase !== "paired") pairingPhase = "idle";
    },

    setHostInput(value) {
      // No notify: the view owns the box while it is typed in.
      hostInput = value;
    },

    connect,
    startPairing,
    getGames,
    recheck,

    async forget() {
      stopPolling();
      try {
        await deps.client.forget();
        savedHost = "";
        hostInput = "";
        hostReachable = false;
        paired = false;
        pairingPhase = "idle";
        gamesPhase = "idle";
        gamesCount = 0;
        editing = null;
      } catch (error) {
        hostError = builtinStreamErrorMessage(error);
      }
      notify();
    },

    openLibrary() {
      deps.openLibrary();
    },

    edit(step) {
      editing = step;
      notify();
    },

    stopEditing() {
      editing = null;
      notify();
    },
  };
}

/* ---------------------------------------------------------------------------
   The view: the Moonlight setup's stepper (`gs-*` styles), three steps long
   --------------------------------------------------------------------------- */

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
  button.dataset.bsAction = name;
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

export function mountBuiltinStreamSetup(
  root: HTMLElement,
  setup: BuiltinStreamSetup,
): { render: () => void } {
  const STEP_TITLES: Record<BuiltinStepId, () => string> = {
    host: () => t("runners.setup.stepHost"),
    pair: () => t("runners.setup.stepPair"),
    games: () => t("runners.setup.stepGames"),
  };

  function statusLine(step: BuiltinStepId, view: BuiltinSetupSnapshot): string {
    const status = view.statuses[step];
    switch (step) {
      case "host":
        if (view.host.checking) return t("runners.setup.checkingPc");
        return status === "done" ? view.host.saved : t("runners.setup.whereIsIt");
      case "pair":
        return status === "done" ? t("runners.setup.paired") : t("runners.setup.pairHint");
      case "games":
        return view.games.phase === "ready" || view.games.phase === "empty"
          ? gameCount(view.games.count)
          : t("runners.setup.addedToLibraryHint");
    }
  }

  function hostBody(view: BuiltinSetupSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const field = element("div", "credentials-form__field");
    const label = element("label", "", t("runners.setup.hostLabel"));
    label.htmlFor = "bs-host";
    const input = element("input", "credentials-form__input");
    input.id = "bs-host";
    input.type = "text";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.autocapitalize = "off";
    input.placeholder = t("runners.setup.hostPlaceholder");
    input.value = view.host.input;
    input.disabled = view.host.busy;
    input.addEventListener("input", () => setup.setHostInput(input.value));
    input.addEventListener("blur", () => {
      const tidy = tidyHostInput(input.value);
      if (tidy !== input.value) {
        input.value = tidy;
        setup.setHostInput(tidy);
      }
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        void setup.connect();
      }
    });
    field.append(label, input, element("small", "", t("runners.setup.hostHint")));
    body.append(field);

    const actions = element("div", "gs-step__actions");
    const connect = action(
      view.host.busy ? t("runners.setup.connecting") : t("runners.setup.connect"),
      "connect",
      "primary",
    );
    connect.disabled = view.host.busy;
    connect.addEventListener("click", () => void setup.connect());
    actions.append(connect);
    if (view.editing === "host" && view.facts.hostSaved) {
      const cancel = action(t("runners.setup.cancel"), "done-editing");
      cancel.addEventListener("click", () => setup.stopEditing());
      actions.append(cancel);
    }
    body.append(actions);
    if (view.host.error) {
      const error = element("p", "gs-step__error", view.host.error);
      error.setAttribute("role", "alert");
      body.append(error);
    }
    return body;
  }

  function pairBody(view: BuiltinSetupSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const { pairing } = view;
    if (pairing.phase === "waiting") {
      body.append(element("div", "gs-pin", pairing.pin));
      body.append(
        element("p", "gs-step__text", t("runners.setup.pinInstructions", { pin: pairing.pin })),
      );
      const waiting = element("p", "gs-step__status", t("runners.setup.waitingPc"));
      waiting.setAttribute("role", "status");
      body.append(waiting);
      return body;
    }
    if (pairing.phase === "starting") {
      const status = element("p", "gs-step__status", t("runners.setup.gettingPin"));
      status.setAttribute("role", "status");
      body.append(status);
      return body;
    }
    if (view.host.error) {
      const error = element("p", "gs-step__error", view.host.error);
      error.setAttribute("role", "alert");
      body.append(error);
    }
    body.append(
      element(
        "p",
        "gs-step__text",
        pairing.phase === "failed"
          ? (pairing.message ?? SETUP_COPY.pairingFailed)
          : t("runners.setup.pairIntro", { computer: view.computerName }),
      ),
    );
    const actions = element("div", "gs-step__actions");
    const start = action(
      pairing.phase === "failed" ? t("runners.setup.tryAgain") : t("runners.setup.pairWithPc"),
      "start-pairing",
      "primary",
    );
    start.addEventListener("click", () => void setup.startPairing());
    actions.append(start);
    body.append(actions);
    return body;
  }

  function gamesBody(view: BuiltinSetupSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const { games } = view;
    const actions = element("div", "gs-step__actions");
    const refresh = (label: string, kind: "primary" | "quiet" = "quiet"): HTMLButtonElement => {
      const button = action(label, "refresh-games", kind);
      button.addEventListener("click", () => void setup.getGames());
      return button;
    };
    switch (games.phase) {
      case "importing": {
        const status = element("p", "gs-step__status", t("runners.setup.lookingForGames"));
        status.setAttribute("role", "status");
        body.append(status);
        break;
      }
      case "ready": {
        body.append(
          element("p", "gs-step__text", t("runners.setup.addedToLibrary", { games: gameCount(games.count) })),
        );
        const library = action(t("runners.setup.openLibrary"), "open-library", "primary");
        library.addEventListener("click", () => setup.openLibrary());
        actions.append(library, refresh(t("runners.setup.refreshGames")));
        break;
      }
      case "empty":
        body.append(element("p", "gs-step__text", SETUP_COPY.noGames));
        actions.append(refresh(t("runners.setup.refreshGames")));
        break;
      case "failed": {
        const error = element("p", "gs-step__error", games.message ?? t("runners.setup.gamesFailed"));
        error.setAttribute("role", "alert");
        body.append(error);
        const check = action(t("runners.setup.checkMyPc"), "recheck");
        check.addEventListener("click", () => void setup.recheck());
        actions.append(refresh(t("runners.setup.tryAgain"), "primary"), check);
        break;
      }
      case "idle":
        body.append(element("p", "gs-step__text", t("runners.setup.gamesIntro")));
        actions.append(refresh(t("runners.setup.getGames"), "primary"));
        break;
    }
    body.append(actions);
    return body;
  }

  function renderStep(step: BuiltinStepId, index: number, view: BuiltinSetupSnapshot): HTMLElement {
    const status = view.statuses[step];
    const expanded =
      status === "active" || view.editing === step || (step === "games" && view.stage === "done");
    const item = element("li", `gs-step is-${status}`);
    item.dataset.bsStep = step;
    if (status === "active") item.setAttribute("aria-current", "step");

    const head = element("div", "gs-step__head");
    const mark = element("span", "gs-step__mark", status === "done" ? "✓" : String(index + 1));
    mark.setAttribute("aria-hidden", "true");
    const copy = element("div", "gs-step__copy");
    copy.append(element("strong", "", STEP_TITLES[step]()), element("small", "", statusLine(step, view)));
    head.append(mark, copy);
    if (status === "done" && step === "host" && view.editing !== "host") {
      const change = action(t("runners.setup.change"), "edit-host");
      change.addEventListener("click", () => setup.edit("host"));
      head.append(change);
    }
    item.append(head);

    if (expanded) {
      const body =
        step === "host" ? hostBody(view) : step === "pair" ? pairBody(view) : gamesBody(view);
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
    const active = document.activeElement;
    const focusId = active instanceof HTMLInputElement && root.contains(active) ? active.id : "";
    const selection =
      active instanceof HTMLInputElement && focusId
        ? { start: active.selectionStart, end: active.selectionEnd }
        : null;

    if (view.loading) {
      const loading = element("p", "settings-hint", t("runners.setup.loading"));
      loading.setAttribute("role", "status");
      root.replaceChildren(loading);
      return;
    }
    const steps = element("ol", "gs-steps");
    BUILTIN_STEPS.forEach((step, index) => steps.append(renderStep(step, index, view)));
    const children: HTMLElement[] = [steps];
    if (view.facts.hostSaved) {
      // Forgetting is rare and final, so it sits under the steps, quiet.
      const footer = element("div", "gs-step__actions bs-forget");
      const forget = action(t("runners.builtin.forget"), "forget");
      forget.addEventListener("click", () => void setup.forget());
      footer.append(forget, element("small", "settings-hint", t("runners.builtin.forgetHint")));
      children.push(footer);
    }
    root.replaceChildren(...children);

    if (focusId) {
      const restored = root.querySelector<HTMLInputElement>(`#${focusId}`);
      if (restored && !restored.disabled) {
        restored.focus();
        if (selection?.start != null && selection.end != null) {
          restored.setSelectionRange(selection.start, selection.end);
        }
      }
    }
  }

  setup.onChange(render);
  render();
  return { render };
}
