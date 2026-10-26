/* ---------------------------------------------------------------------------
   Settings › Plugins › Game streaming: one guided setup.

   Playing a game from another machine takes four things, in this order, and
   the setup walks them one at a time instead of spreading them over three
   cards and a "Launches" menu:

     1. Moonlight on this computer      (found on its own, or installed)
     2. the gaming PC                   (one field, checked before it is kept)
     3. pairing                         (Orivo shows the PIN and watches for it)
     4. the games                       (listed and added to the Library)

   Every step is decided by five facts, and `setupStage` is the one place that
   decides — pure, so the order is stated once and tested without a screen.
   The controller below owns the flow (probing, polling, importing) and the
   view renders whatever it says; neither knows a path, a credential or a
   process (`runner_commands.rs`, `gamestream.rs`).
   --------------------------------------------------------------------------- */

import { gameStreamErrorMessage, type GameStreamClient } from "./gamestream-settings";
import { t } from "./i18n";
import {
  GAMESTREAM_PLUGIN_ID,
  runnerErrorMessage,
  type DetectedClientView,
  type RunnerManagerController,
  type RunnerProfileView,
} from "./runner-manager";

/** The plugin the setup is for. Its row in Settings › Plugins opens this view. */
export { GAMESTREAM_PLUGIN_ID };

/* ---------------------------------------------------------------------------
   Which step is the current one
   --------------------------------------------------------------------------- */

export type SetupStepId = "client" | "host" | "pair" | "games";
export type SetupStage = SetupStepId | "done";
export type StepStatus = "done" | "active" | "todo";

export const SETUP_STEPS: readonly SetupStepId[] = ["client", "host", "pair", "games"];

/** Everything the order of the steps depends on. */
export interface SetupFacts {
  /** A streaming client Orivo can start: detected, or chosen by hand. */
  clientFound: boolean;
  /** An address for the gaming PC is saved. */
  hostSaved: boolean;
  /** The PC answered when it was last asked. */
  hostReachable: boolean;
  /** The PC accepts this client. */
  paired: boolean;
  /** The Library holds games from the PC. */
  gamesImported: boolean;
}

/**
 * The step the person is on.
 *
 * A step is only done when every step before it is, so a fact that is true out
 * of order — a saved address while Moonlight has been uninstalled — sends them
 * back to the earlier step instead of leaving a gap. Being paired proves the PC
 * answered, so it counts for the "reachable" half of step 2 too: nobody is
 * asked to reconnect to a PC that is already working.
 */
export function setupStage(facts: SetupFacts): SetupStage {
  if (!facts.clientFound) return "client";
  if (!facts.hostSaved || !(facts.hostReachable || facts.paired)) return "host";
  if (!facts.paired) return "pair";
  if (!facts.gamesImported) return "games";
  return "done";
}

/** Each step's state: done before the current one, active on it, todo after. */
export function setupStepStatuses(facts: SetupFacts): Record<SetupStepId, StepStatus> {
  const stage = setupStage(facts);
  const current = stage === "done" ? SETUP_STEPS.length : SETUP_STEPS.indexOf(stage);
  const statuses = {} as Record<SetupStepId, StepStatus>;
  SETUP_STEPS.forEach((step, index) => {
    statuses[step] = index < current ? "done" : index === current ? "active" : "todo";
  });
  return statuses;
}

/* ---------------------------------------------------------------------------
   What was typed
   --------------------------------------------------------------------------- */

/**
 * Reduce a pasted address to the machine: no scheme, no path, no credentials,
 * no port. People copy Sunshine's web address out of their browser's bar
 * (`https://my-pc.local:47990/pin`); asking them to take it apart would be
 * asking them to do this function's work. The host applies the same rules
 * (`gamestream.rs`), so what is shown here is what will be saved.
 */
export function tidyHostInput(input: string): string {
  let text = input.trim();
  text = text.replace(/^https?:\/\//i, "");
  const cut = text.search(/[/?#]/);
  if (cut >= 0) text = text.slice(0, cut);
  const at = text.lastIndexOf("@");
  if (at >= 0) text = text.slice(at + 1);
  // A port after a bracketed IPv6 literal, or after a name with exactly one
  // colon. A bare IPv6 address has several and is left for the host to refuse.
  const bracketed = /^(\[[^\]]*\]):\d+$/.exec(text);
  if (bracketed) return bracketed[1] ?? text;
  if ((text.match(/:/g) ?? []).length === 1) text = text.slice(0, text.indexOf(":"));
  return text.trim();
}

function gameCount(count: number): string {
  return count === 1 ? t("runners.setup.gameOne") : t("runners.setup.gameMany", { count });
}

/** "this Mac", "this PC": what the person is sitting at. */
export function thisComputerName(userAgent: string): string {
  if (/Mac/i.test(userAgent)) return t("runners.setup.thisMac");
  if (/Windows/i.test(userAgent)) return t("runners.setup.thisPc");
  return t("runners.setup.thisComputer");
}

/* ---------------------------------------------------------------------------
   Words
   --------------------------------------------------------------------------- */

export const SETUP_COPY = {
  needHost: t("runners.setup.needHost"),
  unreachable: t("runners.setup.unreachable"),
  clientGone: t("runners.setup.clientGone"),
  savedAnyway: t("runners.setup.savedAnyway"),
  pairingFailed: t("runners.setup.pairingFailed"),
  pairingLostContact: t("runners.setup.pairingLostContact"),
  certificate: t("runners.setup.certificate"),
  noGames: t("runners.setup.noGames"),
  stillNotFound: t("runners.setup.stillNotFound"),
} as const;

/** The picker's refusal when the user simply closed it. Not worth a message. */
const PICKER_CANCELLED = "No app was chosen.";

/* ---------------------------------------------------------------------------
   The controller
   --------------------------------------------------------------------------- */

export type PairingPhase =
  | "idle"
  | "starting"
  | "waiting"
  | "failed"
  | "timedOut"
  | "clientMissing"
  | "paired";

export type GamesPhase = "idle" | "preparing" | "importing" | "ready" | "empty" | "failed";

export interface SetupSnapshot {
  /** The view is open. Closed, it draws nothing and asks the host nothing. */
  opened: boolean;
  loading: boolean;
  /** Set when the plugin cannot be used at all, with the reason. */
  pluginIssue: string | null;
  profile: RunnerProfileView | null;
  computerName: string;
  facts: SetupFacts;
  stage: SetupStage;
  statuses: Record<SetupStepId, StepStatus>;
  /** A step the person reopened to change it, or null. */
  editing: SetupStepId | null;
  client: {
    clients: DetectedClientView[];
    selectedId: string | null;
    label: string;
    busy: boolean;
    /** "Check again" found nothing. */
    stillMissing: boolean;
  };
  host: {
    input: string;
    saved: string;
    checking: boolean;
    busy: boolean;
    error: string | null;
    notice: string | null;
    canSaveAnyway: boolean;
  };
  pairing: {
    phase: PairingPhase;
    pin: string;
    host: string;
    message: string | null;
    /** The PC went quiet while the PIN was on screen. */
    lostContact: boolean;
  };
  games: {
    phase: GamesPhase;
    found: number;
    count: number;
    message: string | null;
    findingArtwork: boolean;
  };
}

export interface SetupDeps {
  runners: RunnerManagerController;
  stream: GameStreamClient;
  showToast(message: string): void;
  /** Leave Settings for the Library. */
  openLibrary(): void;
  computerName?: string;
  /** Injected so a test can drive the poll without waiting. */
  setTimer?: (callback: () => void, ms: number) => unknown;
  clearTimer?: (handle: unknown) => void;
  now?: () => number;
  /** Between two asks while the PIN is on screen. */
  pairingPollMs?: number;
  /** How long the PIN stays worth waiting for. */
  pairingTimeoutMs?: number;
}

export interface GameStreamSetup {
  snapshot(): SetupSnapshot;
  onChange(callback: () => void): () => void;
  /** Read the current state of everything and, if a PC is saved, ask it. */
  open(): Promise<void>;
  /** Stop polling and drop any pairing in flight. The view is going away. */
  close(): void;
  selectClient(clientId: string): void;
  checkAgain(): Promise<void>;
  chooseClientByHand(): Promise<void>;
  installMoonlight(): Promise<void>;
  setHostInput(value: string): void;
  connect(): Promise<void>;
  saveAnyway(): Promise<void>;
  startPairing(): Promise<void>;
  openPinPage(): Promise<void>;
  /** List the PC's games into the Library. */
  getGames(): Promise<void>;
  /** Ask the saved PC again and move to whichever step that puts the person on. */
  recheck(): Promise<void>;
  openLibrary(): void;
  edit(step: SetupStepId): void;
  stopEditing(): void;
}

const DEFAULT_POLL_MS = 2_000;
const DEFAULT_PAIRING_TIMEOUT_MS = 3 * 60_000;

export function createGameStreamSetup(deps: SetupDeps): GameStreamSetup {
  const setTimer = deps.setTimer ?? ((callback, ms) => setTimeout(callback, ms));
  const clearTimer =
    deps.clearTimer ?? ((handle) => clearTimeout(handle as ReturnType<typeof setTimeout>));
  const now = deps.now ?? (() => Date.now());
  const pollMs = deps.pairingPollMs ?? DEFAULT_POLL_MS;
  const pairingTimeoutMs = deps.pairingTimeoutMs ?? DEFAULT_PAIRING_TIMEOUT_MS;
  const computerName =
    deps.computerName ??
    thisComputerName(typeof navigator === "undefined" ? "" : navigator.userAgent);

  const listeners = new Set<() => void>();
  let unsubscribeRunners: (() => void) | null = null;
  let lifetime = new AbortController();
  let isOpen = false;

  // What the person has done and the host has said. Everything the view shows
  // is derived from these and from the runner controller's profile list.
  let loading = false;
  let clients: DetectedClientView[] = [];
  let selectedClientId: string | null = null;
  /** The person picked a different client than the profile holds. */
  let clientDirty = false;
  /** The host said the client is gone, whatever the last detection found. */
  let clientLost = false;
  let clientBusy = false;
  let stillMissing = false;
  let savedHost = "";
  let hostInput = "";
  let hostChecking = false;
  let hostBusy = false;
  let hostError: string | null = null;
  let hostNotice: string | null = null;
  let canSaveAnyway = false;
  let hostReachable = false;
  let paired = false;
  let editing: SetupStepId | null = null;
  let pairingPhase: PairingPhase = "idle";
  let pairingPin = "";
  let pairingHost = "";
  let pairingMessage: string | null = null;
  let lostContact = false;
  let pairingToken = 0;
  let pairingTimer: unknown = null;
  let gamesRequested = false;
  let gamesBusy: "preparing" | null = null;
  let gamesError: string | null = null;

  const notify = (): void => {
    for (const listener of [...listeners]) listener();
  };

  const runner = () => deps.runners.runners().find((entry) => entry.id === GAMESTREAM_PLUGIN_ID);
  const profile = (): RunnerProfileView | null => runner()?.profiles[0] ?? null;

  const selectedClient = (): DetectedClientView | null =>
    clients.find((entry) => entry.id === selectedClientId) ?? clients[0] ?? null;

  const clientLabel = (): string => {
    const current = profile();
    if (current?.applicationAvailable && !clientDirty) {
      return current.applicationLabel.replace(/\.app$/i, "") || "Moonlight";
    }
    return selectedClient()?.label ?? "Moonlight";
  };

  const facts = (): SetupFacts => {
    const current = profile();
    return {
      clientFound: !clientLost && (clients.length > 0 || current?.applicationAvailable === true),
      hostSaved: savedHost !== "",
      hostReachable,
      paired,
      gamesImported: (current?.gameCount ?? 0) > 0,
    };
  };

  /** The state of the games step, read from the import job the runner controller keeps. */
  const gamesView = (): SetupSnapshot["games"] => {
    const current = profile();
    const job = current ? deps.runners.importFor(current.id) : null;
    const count = current?.gameCount ?? 0;
    const findingArtwork = current ? deps.runners.isFindingArtwork(current.id) : false;
    const found = job ? job.imported + job.refreshed : 0;
    let phase: GamesPhase = "idle";
    let message: string | null = null;
    if (gamesError) {
      phase = "failed";
      message = gamesError;
    } else if (gamesBusy) {
      phase = "preparing";
    } else if (gamesRequested && job) {
      if (job.phase === "running") phase = "importing";
      else if (job.phase === "failed") {
        phase = "failed";
        message = job.message || t("runners.setup.gamesFailed");
      } else if (job.phase === "ready") phase = count > 0 ? "ready" : "empty";
      else phase = "idle";
    } else if (count > 0) {
      phase = "ready";
    }
    return { phase, found, count, message, findingArtwork };
  };

  const snapshot = (): SetupSnapshot => {
    const current = facts();
    const entry = runner();
    return {
      opened: isOpen,
      loading,
      pluginIssue:
        entry && entry.state !== "ready"
          ? entry.message || t("runners.setup.pluginCantRun")
          : null,
      profile: profile(),
      computerName,
      facts: current,
      stage: setupStage(current),
      statuses: setupStepStatuses(current),
      editing,
      client: {
        clients,
        selectedId: selectedClient()?.id ?? null,
        label: clientLabel(),
        busy: clientBusy,
        stillMissing,
      },
      host: {
        input: hostInput,
        saved: savedHost,
        checking: hostChecking,
        busy: hostBusy,
        error: hostError,
        notice: hostNotice,
        canSaveAnyway,
      },
      pairing: {
        phase: pairingPhase,
        pin: pairingPin,
        host: pairingHost,
        message: pairingMessage,
        lostContact,
      },
      games: gamesView(),
    };
  };

  /* -- pairing -------------------------------------------------------------- */

  const stopPolling = (): void => {
    pairingToken += 1;
    if (pairingTimer !== null) clearTimer(pairingTimer);
    pairingTimer = null;
  };

  const clientIsGone = async (): Promise<void> => {
    clientLost = true;
    stopPolling();
    await deps.runners.refreshStreamClients();
    // A client that is still detected is one the person can pick again; one
    // that is not leaves the step asking for it. Either way the profile's own
    // application is stale and the next Connect points it at the choice.
    if (clients.length > 0) clientLost = false;
    clientDirty = true;
  };

  /* -- games ---------------------------------------------------------------- */

  const getGames = async (): Promise<void> => {
    const current = profile();
    if (!current || gamesBusy) return;
    gamesRequested = true;
    gamesError = null;
    gamesBusy = "preparing";
    notify();
    try {
      // The folder is Orivo's own, made and allowed here: nobody is asked to
      // pick one. Done before every import so a folder that went missing is
      // simply made again.
      await deps.runners.prepareLibrary(current.id);
      await deps.runners.startImport(current.id);
    } catch (error) {
      gamesError = runnerErrorMessage(error);
    }
    // Held until the import has a job to show, so an earlier run's result
    // never flashes between the click and the first progress.
    gamesBusy = null;
    notify();
  };

  /** Called whenever the facts say the PC is paired: straight on to the games. */
  const advanceToGames = async (): Promise<void> => {
    editing = null;
    if (!gamesRequested) await getGames();
  };

  /* -- the PC --------------------------------------------------------------- */

  const saveHost = async (host: string): Promise<boolean> => {
    try {
      const saved = await deps.stream.save({ host }, lifetime.signal);
      savedHost = saved.host;
      return true;
    } catch (error) {
      hostError = gameStreamErrorMessage(error);
      return false;
    }
  };

  /**
   * Ask the saved or typed PC and fold the answer into the facts. Returns the
   * state so the caller decides what comes next.
   */
  const askHost = async (typed: string): Promise<"paired" | "notPaired" | "unreachable" | "stop"> => {
    const current = profile();
    if (!current) return "stop";
    const status = await deps.runners.probeHost(current.id, typed);
    if (status.host) hostInput = status.host;
    if (status.state === "clientMissing") {
      hostReachable = false;
      paired = false;
      await clientIsGone();
      return "stop";
    }
    hostReachable = status.state !== "unreachable";
    paired = status.state === "paired";
    return status.state;
  };

  const schedulePoll = (token: number, deadline: number): void => {
    pairingTimer = setTimer(() => {
      void poll(token, deadline);
    }, pollMs);
  };

  const poll = async (token: number, deadline: number): Promise<void> => {
    if (token !== pairingToken) return;
    const current = profile();
    if (!current) return;
    let state: Awaited<ReturnType<RunnerManagerController["pairingStatus"]>>;
    try {
      state = await deps.runners.pairingStatus(current.id);
    } catch {
      // One failed question is not the end of the attempt; the deadline is.
      state = "waiting";
    }
    if (token !== pairingToken) return;
    if (state === "paired") {
      stopPolling();
      paired = true;
      hostReachable = true;
      pairingPhase = "paired";
      lostContact = false;
      notify();
      await advanceToGames();
      return;
    }
    if (state === "failed") {
      stopPolling();
      pairingPhase = "failed";
      pairingMessage = SETUP_COPY.pairingFailed;
      notify();
      return;
    }
    if (state === "clientMissing") {
      pairingPhase = "clientMissing";
      pairingMessage = SETUP_COPY.clientGone;
      await clientIsGone();
      notify();
      return;
    }
    lostContact = state === "unreachable";
    if (now() >= deadline) {
      stopPolling();
      await deps.runners.cancelPairing(current.id);
      pairingPhase = "timedOut";
      pairingMessage = SETUP_COPY.pairingFailed;
      notify();
      return;
    }
    notify();
    schedulePoll(token, deadline);
  };

  const startPairing = async (): Promise<void> => {
    const current = profile();
    if (!current) return;
    stopPolling();
    const token = pairingToken;
    pairingPhase = "starting";
    pairingMessage = null;
    lostContact = false;
    notify();
    let started: Awaited<ReturnType<RunnerManagerController["beginPairing"]>>;
    try {
      // A retry gets a new PIN, and the host ends the attempt it replaces.
      started = await deps.runners.beginPairing(current.id);
    } catch (error) {
      if (token !== pairingToken) return;
      pairingPhase = "failed";
      pairingMessage = runnerErrorMessage(error);
      notify();
      return;
    }
    if (token !== pairingToken) return;
    if (started.state === "alreadyPaired") {
      paired = true;
      hostReachable = true;
      pairingPhase = "paired";
      notify();
      await advanceToGames();
      return;
    }
    pairingPin = started.pin;
    pairingHost = started.host;
    pairingPhase = "waiting";
    notify();
    schedulePoll(token, now() + pairingTimeoutMs);
  };

  const connect = async (): Promise<void> => {
    if (hostBusy) return;
    const typed = tidyHostInput(hostInput);
    hostError = null;
    hostNotice = null;
    canSaveAnyway = false;
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
      const current = profile();
      // The profile is made when the person commits to a PC, not before: a
      // setup abandoned at the first step leaves nothing behind. A client they
      // chose differently since — or one that moved — is pointed at here too.
      if (!current || clientDirty || !current.applicationAvailable) {
        await deps.runners.createStreamProfile(selectedClient()?.id);
        clientDirty = false;
      }
      const answer = await askHost(typed);
      if (answer === "stop") {
        hostError = SETUP_COPY.clientGone;
      } else if (answer === "unreachable") {
        hostError = SETUP_COPY.unreachable;
        canSaveAnyway = true;
      } else {
        // Kept only now that the PC has answered for it.
        if (await saveHost(hostInput)) {
          editing = null;
          if (answer === "paired") {
            pairingPhase = "paired";
            await advanceToGames();
          } else {
            pairingPhase = "idle";
            // Straight to the PIN: the next thing they need is the number.
            hostBusy = false;
            await startPairing();
          }
        }
      }
    } catch (error) {
      hostError = runnerErrorMessage(error);
    } finally {
      hostBusy = false;
      notify();
    }
  };

  const saveAnyway = async (): Promise<void> => {
    const typed = tidyHostInput(hostInput);
    if (!typed) {
      hostError = SETUP_COPY.needHost;
      notify();
      return;
    }
    hostBusy = true;
    hostError = null;
    notify();
    hostInput = typed;
    const saved = await saveHost(typed);
    hostBusy = false;
    if (saved) {
      hostReachable = false;
      paired = false;
      canSaveAnyway = false;
      hostNotice = SETUP_COPY.savedAnyway;
    }
    notify();
  };

  const recheck = async (): Promise<void> => {
    const current = profile();
    if (!current || !savedHost) return;
    hostChecking = true;
    gamesError = null;
    notify();
    try {
      const answer = await askHost(savedHost);
      if (answer === "notPaired") pairingPhase = "idle";
      if (answer === "unreachable") hostError = SETUP_COPY.unreachable;
    } catch (error) {
      hostError = runnerErrorMessage(error);
    } finally {
      hostChecking = false;
      notify();
    }
  };

  /* -- the client ----------------------------------------------------------- */

  const checkAgain = async (): Promise<void> => {
    clientBusy = true;
    stillMissing = false;
    notify();
    clients = await deps.runners.refreshStreamClients();
    clientBusy = false;
    if (clients.length > 0) {
      clientLost = false;
      selectedClientId ??= clients[0]?.id ?? null;
      clientDirty = true;
    } else {
      stillMissing = true;
    }
    notify();
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
      lifetime = new AbortController();
      loading = true;
      hostError = null;
      hostNotice = null;
      canSaveAnyway = false;
      editing = null;
      gamesRequested = false;
      gamesError = null;
      pairingPhase = "idle";
      clientLost = false;
      clientDirty = false;
      stillMissing = false;
      unsubscribeRunners?.();
      unsubscribeRunners = deps.runners.onChange(notify);
      notify();
      // Runners (and with them the detected clients) and the saved address are
      // independent reads.
      const [, settings] = await Promise.all([
        deps.runners.load(lifetime.signal),
        deps.stream.load(lifetime.signal).catch(() => ({ host: "" })),
      ]);
      if (!isOpen) return;
      clients = deps.runners.streamClients();
      selectedClientId = clients[0]?.id ?? null;
      savedHost = settings.host.trim();
      hostInput = savedHost;
      hostReachable = false;
      paired = false;
      loading = false;
      notify();
      // Someone coming back is shown how things are now, not how they were
      // left: the PC may have been switched off or unpaired since.
      if (profile() && savedHost && snapshot().facts.clientFound) {
        hostChecking = true;
        notify();
        try {
          await askHost(savedHost);
        } catch {
          // The PC not answering is a state, not a failure of opening the view.
        }
        hostChecking = false;
        notify();
      }
    },

    close() {
      isOpen = false;
      const current = profile();
      const waiting = pairingPhase === "waiting" || pairingPhase === "starting";
      stopPolling();
      if (waiting && current) void deps.runners.cancelPairing(current.id);
      pairingPhase = pairingPhase === "paired" ? "paired" : "idle";
      lifetime.abort();
      unsubscribeRunners?.();
      unsubscribeRunners = null;
    },

    selectClient(clientId) {
      if (!clients.some((entry) => entry.id === clientId)) return;
      selectedClientId = clientId;
      clientDirty = true;
      notify();
    },

    checkAgain,

    async chooseClientByHand() {
      clientBusy = true;
      notify();
      try {
        // The operating system's own picker; Orivo never sees the path.
        await deps.runners.createStreamProfile();
        clientLost = false;
        clientDirty = false;
        stillMissing = false;
        editing = null;
      } catch (error) {
        const message = runnerErrorMessage(error);
        if (message !== PICKER_CANCELLED) deps.showToast(message);
      } finally {
        clientBusy = false;
        notify();
      }
    },

    async installMoonlight() {
      try {
        await deps.stream.openLink("moonlight", lifetime.signal);
      } catch (error) {
        deps.showToast(gameStreamErrorMessage(error));
      }
    },

    setHostInput(value) {
      // No notify: the view owns the box while it is being typed in, and a
      // repaint here would take the cursor with it.
      hostInput = value;
    },

    connect,
    saveAnyway,
    startPairing,

    async openPinPage() {
      try {
        await deps.stream.openLink("sunshine-pin", lifetime.signal);
      } catch (error) {
        deps.showToast(gameStreamErrorMessage(error));
      }
    },

    getGames,
    recheck,

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
   The view
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

/** A word-sized button; `primary` wears the Settings account-button look. */
function action(label: string, name: string, kind: "primary" | "quiet" = "quiet"): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.dataset.gsAction = name;
  if (kind === "primary") {
    button.className = "source-account-row__account gs-primary";
    const word = element("span", "source-account-row__account-name", label);
    button.append(word);
  } else {
    button.className = "settings-button settings-button--quiet";
    button.textContent = label;
  }
  return button;
}

/**
 * Render the setup as a stepper and keep it in step with the controller.
 *
 * Re-rendering replaces the subtree, so the one thing that must survive it is
 * the address box: its focus and cursor are put back, otherwise a repaint in
 * the middle of typing would drop the cursor.
 */
export function mountGameStreamSetup(
  root: HTMLElement,
  setup: GameStreamSetup,
): { render: () => void } {
  const STEP_TITLES: Record<SetupStepId, (computer: string) => string> = {
    client: (computer) => t("runners.setup.stepClient", { computer }),
    host: () => t("runners.setup.stepHost"),
    pair: () => t("runners.setup.stepPair"),
    games: () => t("runners.setup.stepGames"),
  };

  function statusLine(step: SetupStepId, view: SetupSnapshot): string {
    const status = view.statuses[step];
    switch (step) {
      case "client":
        return status === "done"
          ? t("runners.setup.clientFoundStatus", { label: view.client.label })
          : t("runners.setup.clientMissingStatus");
      case "host":
        if (view.host.checking) return t("runners.setup.checkingPc");
        return status === "done" ? view.host.saved : t("runners.setup.whereIsIt");
      case "pair":
        return status === "done" ? t("runners.setup.paired") : t("runners.setup.pairHint");
      case "games":
        if (view.games.phase === "ready") return gameCount(view.games.count);
        return status === "done" ? gameCount(view.games.count) : t("runners.setup.addedToLibraryHint");
    }
  }

  function clientBody(view: SetupSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const found = view.client.clients.length > 0 || view.facts.clientFound;
    if (found) {
      body.append(element("p", "gs-step__text", t("runners.setup.clientFound", { label: view.client.label })));
      if (view.client.clients.length > 1) {
        const select = element("select", "settings-select");
        select.id = "gs-client-select";
        select.setAttribute("aria-label", t("runners.setup.streamingApp"));
        for (const client of view.client.clients) {
          const option = element("option", "", client.label);
          option.value = client.id;
          select.append(option);
        }
        select.value = view.client.selectedId ?? "";
        select.addEventListener("change", () => setup.selectClient(select.value));
        body.append(select);
      }
    } else {
      body.append(
        element(
          "p",
          "gs-step__text",
          t("runners.setup.clientIntro", { computer: view.computerName }),
        ),
      );
      if (view.client.stillMissing) {
        body.append(element("p", "gs-step__note", SETUP_COPY.stillNotFound));
      }
    }
    const actions = element("div", "gs-step__actions");
    if (!found) {
      const install = action(t("runners.setup.installMoonlight"), "install-moonlight", "primary");
      install.addEventListener("click", () => void setup.installMoonlight());
      actions.append(install);
      const again = action(t("runners.setup.checkAgain"), "check-again");
      again.disabled = view.client.busy;
      again.addEventListener("click", () => void setup.checkAgain());
      actions.append(again);
    }
    const choose = action(t("runners.setup.chooseApp"), "choose-client");
    choose.disabled = view.client.busy;
    choose.addEventListener("click", () => void setup.chooseClientByHand());
    actions.append(choose);
    if (found && view.editing === "client") {
      const done = action(t("runners.setup.done"), "done-editing");
      done.addEventListener("click", () => setup.stopEditing());
      actions.append(done);
    }
    body.append(actions);
    return body;
  }

  function hostBody(view: SetupSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const field = element("div", "credentials-form__field");
    const label = element("label", "", t("runners.setup.hostLabel"));
    label.htmlFor = "gs-host";
    const input = element("input", "credentials-form__input");
    input.id = "gs-host";
    input.type = "text";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.placeholder = t("runners.setup.hostPlaceholder");
    input.value = view.host.input;
    input.disabled = view.host.busy;
    input.addEventListener("input", () => setup.setHostInput(input.value));
    // A pasted web address is shown as the name it contains, so what will be
    // saved is what the person sees.
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
    const hint = element(
      "small",
      "",
      t("runners.setup.hostHint"),
    );
    field.append(label, input, hint);
    body.append(field);

    const actions = element("div", "gs-step__actions");
    const connect = action(view.host.busy ? t("runners.setup.connecting") : t("runners.setup.connect"), "connect", "primary");
    connect.disabled = view.host.busy;
    connect.addEventListener("click", () => void setup.connect());
    actions.append(connect);
    if (view.host.canSaveAnyway) {
      const anyway = action(t("runners.setup.saveAnyway"), "save-anyway");
      anyway.disabled = view.host.busy;
      anyway.addEventListener("click", () => void setup.saveAnyway());
      actions.append(anyway);
    }
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
    if (view.host.notice) {
      const notice = element("p", "gs-step__note", view.host.notice);
      notice.setAttribute("role", "status");
      body.append(notice);
    }
    return body;
  }

  function pairBody(view: SetupSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const { pairing } = view;
    if (pairing.phase === "waiting") {
      body.append(element("div", "gs-pin", pairing.pin));
      body.append(
        element(
          "p",
          "gs-step__text",
          t("runners.setup.pinInstructions", { pin: pairing.pin }),
        ),
      );
      const actions = element("div", "gs-step__actions");
      const open = action(t("runners.setup.openPinPage"), "open-pin-page", "primary");
      open.addEventListener("click", () => void setup.openPinPage());
      actions.append(open);
      body.append(actions, element("p", "gs-step__note", SETUP_COPY.certificate));
      const waiting = element("p", "gs-step__status", t("runners.setup.waitingPc"));
      waiting.setAttribute("role", "status");
      body.append(waiting);
      if (pairing.lostContact) {
        body.append(element("p", "gs-step__error", SETUP_COPY.pairingLostContact));
      }
      return body;
    }
    if (pairing.phase === "starting") {
      const status = element("p", "gs-step__status", t("runners.setup.gettingPin"));
      status.setAttribute("role", "status");
      body.append(status);
      return body;
    }
    body.append(
      element(
        "p",
        "gs-step__text",
        pairing.phase === "idle"
          ? t("runners.setup.pairIntro", { computer: view.computerName })
          : (pairing.message ?? SETUP_COPY.pairingFailed),
      ),
    );
    const actions = element("div", "gs-step__actions");
    const start = action(
      pairing.phase === "idle" ? t("runners.setup.pairWithPc") : t("runners.setup.tryAgain"),
      "start-pairing",
      "primary",
    );
    start.addEventListener("click", () => void setup.startPairing());
    actions.append(start);
    if (pairing.phase === "clientMissing") {
      const choose = action(t("runners.setup.chooseApp"), "choose-client");
      choose.addEventListener("click", () => void setup.chooseClientByHand());
      actions.append(choose);
    }
    body.append(actions);
    return body;
  }

  function gamesBody(view: SetupSnapshot): HTMLElement {
    const body = element("div", "gs-step__content");
    const { games } = view;
    const actions = element("div", "gs-step__actions");
    const refresh = (label: string): HTMLButtonElement => {
      const button = action(label, "refresh-games");
      button.addEventListener("click", () => void setup.getGames());
      return button;
    };
    switch (games.phase) {
      case "preparing":
      case "importing": {
        const text =
          games.phase === "importing" && games.found > 0
            ? t("runners.setup.foundGames", { games: gameCount(games.found) })
            : t("runners.setup.lookingForGames");
        const status = element("p", "gs-step__status", text);
        status.setAttribute("role", "status");
        body.append(status);
        break;
      }
      case "ready": {
        body.append(element("p", "gs-step__text", t("runners.setup.addedToLibrary", { games: gameCount(games.count) })));
        if (games.findingArtwork) {
          body.append(element("p", "gs-step__note", t("runners.setup.findingArtwork")));
        }
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
        const error = element("p", "gs-step__error", games.message ?? "");
        error.setAttribute("role", "alert");
        body.append(error);
        const retry = action(t("runners.setup.tryAgain"), "refresh-games", "primary");
        retry.addEventListener("click", () => void setup.getGames());
        const check = action(t("runners.setup.checkMyPc"), "recheck");
        check.addEventListener("click", () => void setup.recheck());
        actions.append(retry, check);
        break;
      }
      case "idle": {
        body.append(
          element(
            "p",
            "gs-step__text",
            t("runners.setup.gamesIntro"),
          ),
        );
        const get = action(t("runners.setup.getGames"), "refresh-games", "primary");
        get.addEventListener("click", () => void setup.getGames());
        actions.append(get);
        break;
      }
    }
    body.append(actions);
    return body;
  }

  function renderStep(step: SetupStepId, index: number, view: SetupSnapshot): HTMLElement {
    const status = view.statuses[step];
    // The active step is open; a finished one folds to a line the person can
    // reopen; the games step stays open once everything is done, because that
    // is where "Open Library" lives.
    const expanded =
      status === "active" ||
      view.editing === step ||
      (step === "games" && view.stage === "done");
    const item = element("li", `gs-step is-${status}`);
    item.dataset.gsStep = step;
    if (status === "active") item.setAttribute("aria-current", "step");

    const head = element("div", "gs-step__head");
    const mark = element("span", "gs-step__mark", status === "done" ? "✓" : String(index + 1));
    mark.setAttribute("aria-hidden", "true");
    const copy = element("div", "gs-step__copy");
    copy.append(
      element("strong", "", STEP_TITLES[step](view.computerName)),
      element("small", "", statusLine(step, view)),
    );
    head.append(mark, copy);
    if (status === "done" && view.editing !== step && step !== "games" && step !== "pair") {
      const change = action(t("runners.setup.change"), `edit-${step}`);
      change.addEventListener("click", () => setup.edit(step));
      head.append(change);
    }
    item.append(head);

    if (expanded) {
      const body =
        step === "client"
          ? clientBody(view)
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
    // Put the address box back exactly as it was: focus and cursor.
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
    const children: HTMLElement[] = [];
    if (view.pluginIssue) {
      const issue = element("p", "gs-step__error", view.pluginIssue);
      issue.setAttribute("role", "alert");
      children.push(issue);
    }
    const steps = element("ol", "gs-steps");
    SETUP_STEPS.forEach((step, index) => steps.append(renderStep(step, index, view)));
    children.push(steps);
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
