/* ---------------------------------------------------------------------------
   Settings › Plugins › Game streaming: one question at a time.

   Playing a game from another machine takes four things, in this order:

     1. Moonlight on this computer      (found on its own, or installed)
     2. the gaming PC                   (picked, or typed and checked)
     3. pairing                         (Orivo shows the PIN and watches for it)
     4. the games                       (listed and added to the Library)

   The detail panel asks only the question in front of the person, large, with
   one main button; once everything works it shows the PC and "See my games",
   and the rest (another PC, resync, the app, removal) waits in a ⋯ menu.
   Which question that is gets decided in `gamestream-setup-model.ts` — pure,
   so the order is stated once and tested without a screen. The controller
   below owns the flow (probing, polling, importing) and the view renders
   whatever it says; neither knows a path, a credential or a process
   (`runner_commands.rs`, `gamestream.rs`).
   --------------------------------------------------------------------------- */

import {
  appName,
  flowScreen,
  type FlowScreen,
  gameCount,
  hostPills,
  setupStage,
  setupStepStatuses,
  tidyHostInput,
  type SetupFacts,
  type SetupStage,
  type SetupStepId,
  type StepStatus,
} from "./gamestream-setup-model";
import { gameStreamErrorMessage, type GameStreamClient } from "./gamestream-settings";
import { t } from "./i18n";
import { icon, type IconName } from "./icons";
import { pluginMoreMenu, type MoreMenuEntry } from "./plugin-cards";
import {
  GAMESTREAM_PLUGIN_ID,
  runnerErrorMessage,
  type DetectedClientView,
  type RunnerManagerController,
  type RunnerProfileView,
} from "./runner-manager";

/** The plugin the setup is for. Its row in Settings › Plugins opens this view. */
export { GAMESTREAM_PLUGIN_ID };

export {
  SETUP_STEPS,
  flowScreen,
  setupStage,
  setupStepStatuses,
  tidyHostInput,
  type FlowScreen,
  type SetupFacts,
  type SetupStage,
  type SetupStepId,
  type StepStatus,
} from "./gamestream-setup-model";

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

/**
 * The program on the PC the PIN is typed into. Orivo cannot ask which one runs
 * there (Apollo is a fork that keeps the same PIN page), and the page it opens
 * for the PIN is Sunshine's (`open_gamestream_link`), so that is the name.
 */
const HOST_APP = "Sunshine";

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
  /** The one question the panel asks now (`flowScreen`). */
  screen: FlowScreen;
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
    /** "Other" was picked over the PC Orivo knows: the address field is open. */
    adding: boolean;
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
  /** Connect to a PC by name: the one Orivo knows, picked from its tile. */
  connectTo(host: string): Promise<void>;
  /** Open the address field for a PC Orivo does not know yet. */
  addHost(): void;
  /** Close it again, back to the PC Orivo knows. */
  cancelAddHost(): void;
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
  let addingHost = false;
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
      return appName(current.applicationLabel);
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
    const stage = setupStage(current);
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
      stage,
      statuses: setupStepStatuses(current),
      screen: flowScreen({
        loading,
        stage,
        editing,
        pairingPhase,
        savedHost,
        addingHost,
      }),
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
        adding: addingHost,
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
          addingHost = false;
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
      addingHost = false;
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
      addingHost = false;
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

    async connectTo(host) {
      hostInput = host;
      addingHost = false;
      await connect();
    },

    addHost() {
      addingHost = true;
      hostInput = "";
      hostError = null;
      hostNotice = null;
      canSaveAnyway = false;
      notify();
    },

    cancelAddHost() {
      addingHost = false;
      hostInput = savedHost;
      hostError = null;
      canSaveAnyway = false;
      notify();
    },

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
      addingHost = false;
      hostError = null;
      hostNotice = null;
      canSaveAnyway = false;
      if (step === "host") hostInput = savedHost;
      notify();
    },

    stopEditing() {
      editing = null;
      addingHost = false;
      hostError = null;
      canSaveAnyway = false;
      hostInput = savedHost;
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

/** The one main button of a screen (lavender pill), or a quiet word beside it. */
function action(
  label: string,
  name: string,
  kind: "primary" | "quiet" = "quiet",
  glyph: IconName | null = null,
): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.dataset.gsAction = name;
  if (kind === "primary") {
    button.className = "gs-cta";
    if (glyph) button.insertAdjacentHTML("beforeend", icon(glyph));
    button.append(element("span", "", label));
  } else {
    button.className = "settings-button settings-button--quiet";
    button.textContent = label;
  }
  return button;
}

/** A big choice: a PC or an app, as a card the size of a thumb. */
function tile(
  name: string,
  glyph: IconName,
  actionName: string,
  options: { selected?: boolean; ghost?: boolean; pill?: HTMLElement | null; disabled?: boolean } = {},
): HTMLButtonElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "gs-tile";
  button.dataset.gsAction = actionName;
  button.classList.toggle("is-selected", Boolean(options.selected));
  button.classList.toggle("gs-tile--ghost", Boolean(options.ghost));
  if (options.selected) button.setAttribute("aria-pressed", "true");
  button.disabled = Boolean(options.disabled);
  button.insertAdjacentHTML("beforeend", icon(glyph));
  button.append(element("strong", "gs-tile__name", name));
  if (options.pill) button.append(options.pill);
  return button;
}

function pill(text: string, tone: string): HTMLElement {
  const node = element("span", "plugin-pill gs-pill", text);
  node.dataset.tone = tone;
  return node;
}

function waiting(text: string): HTMLElement {
  const line = element("p", "gs-wait");
  line.setAttribute("role", "status");
  const dot = element("span", "gs-wait__dot");
  dot.setAttribute("aria-hidden", "true");
  line.append(dot, element("span", "", text));
  return line;
}

function alertLine(text: string): HTMLElement {
  const line = element("p", "gs-flow__error", text);
  line.setAttribute("role", "alert");
  return line;
}

export interface GameStreamViewOptions {
  /**
   * The plugin's own entries for the ⋯ menu — update, automatic updates,
   * removal, the version line — which only the plugin list knows about.
   */
  pluginMenu?: () => MoreMenuEntry[];
}

/**
 * Render the setup as one question at a time and keep it in step with the
 * controller.
 *
 * Re-rendering replaces the subtree, so what must survive it is put back: the
 * address box (focus and cursor) and an open ⋯ menu.
 */
export function mountGameStreamSetup(
  root: HTMLElement,
  setup: GameStreamSetup,
  options: GameStreamViewOptions = {},
): { render: () => void } {
  let menuOpen = false;

  function question(text: string): HTMLElement {
    return element("h3", "gs-flow__q", text);
  }

  function row(...children: HTMLElement[]): HTMLElement {
    const node = element("div", "gs-flow__actions");
    node.append(...children);
    return node;
  }

  /** The streaming app is not on this computer: one Install. */
  function missingScreen(view: SetupSnapshot): HTMLElement[] {
    const mark = element("span", "plugin-icon gs-flow__mark");
    mark.dataset.category = "streaming";
    mark.setAttribute("aria-hidden", "true");
    mark.innerHTML = icon("cast");
    const install = action(t("runners.flow.install"), "install-moonlight", "primary", "download");
    install.addEventListener("click", () => void setup.installMoonlight());
    const again = action(t("runners.setup.checkAgain"), "check-again");
    again.disabled = view.client.busy;
    again.addEventListener("click", () => void setup.checkAgain());
    const choose = action(t("runners.setup.chooseApp"), "choose-client");
    choose.disabled = view.client.busy;
    choose.addEventListener("click", () => void setup.chooseClientByHand());
    const nodes: HTMLElement[] = [
      mark,
      question(t("runners.flow.missingApp", { app: view.client.label })),
      install,
      row(again, choose),
    ];
    if (view.client.stillMissing) nodes.push(element("p", "gs-flow__note", SETUP_COPY.stillNotFound));
    return nodes;
  }

  /** Which streaming app: the ones Orivo found, or another picked by hand. */
  function appScreen(view: SetupSnapshot): HTMLElement[] {
    const tiles = element("div", "gs-tiles");
    for (const client of view.client.clients) {
      const choice = tile(client.label, "cast", "select-client", {
        selected: client.id === view.client.selectedId,
      });
      choice.dataset.gsClient = client.id;
      choice.addEventListener("click", () => setup.selectClient(client.id));
      tiles.append(choice);
    }
    const other = tile(t("runners.flow.other"), "plus", "choose-client", {
      ghost: true,
      disabled: view.client.busy,
    });
    other.addEventListener("click", () => void setup.chooseClientByHand());
    tiles.append(other);
    const done = action(t("runners.setup.done"), "done-editing", "primary");
    done.addEventListener("click", () => setup.stopEditing());
    return [question(t("runners.flow.whichApp")), tiles, done];
  }

  /** Which PC: the one Orivo knows, or another. */
  function pickScreen(view: SetupSnapshot): HTMLElement[] {
    const tiles = element("div", "gs-tiles");
    const [state] = hostPills({
      checking: view.host.checking || view.host.busy,
      reachable: view.facts.hostReachable,
      paired: view.facts.paired,
      gameCount: 0,
    });
    const known = tile(view.host.saved, "monitor", "connect-saved", {
      selected: true,
      pill: state ? pill(state.text, state.tone) : null,
      disabled: view.host.busy,
    });
    known.addEventListener("click", () => void setup.connectTo(view.host.saved));
    const other = tile(t("runners.flow.other"), "plus", "add-host", { ghost: true, disabled: view.host.busy });
    other.addEventListener("click", () => setup.addHost());
    tiles.append(known, other);
    const nodes: HTMLElement[] = [question(t("runners.flow.whichPc")), tiles];
    nodes.push(...hostMessages(view));
    if (view.editing === "host" && view.stage !== "host") {
      const cancel = action(t("runners.setup.cancel"), "done-editing");
      cancel.addEventListener("click", () => setup.stopEditing());
      nodes.push(row(cancel));
    }
    return nodes;
  }

  function hostMessages(view: SetupSnapshot): HTMLElement[] {
    const nodes: HTMLElement[] = [];
    if (view.host.error) nodes.push(alertLine(view.host.error));
    if (view.host.canSaveAnyway) {
      const anyway = action(t("runners.setup.saveAnyway"), "save-anyway");
      anyway.disabled = view.host.busy;
      anyway.addEventListener("click", () => void setup.saveAnyway());
      nodes.push(row(anyway));
    }
    if (view.host.notice) {
      const notice = element("p", "gs-flow__note", view.host.notice);
      notice.setAttribute("role", "status");
      nodes.push(notice);
    }
    return nodes;
  }

  /** Another PC: its name or address, checked before it is kept. */
  function addressScreen(view: SetupSnapshot): HTMLElement[] {
    const input = element("input", "gs-address");
    input.id = "gs-host";
    input.type = "text";
    input.autocomplete = "off";
    input.spellcheck = false;
    input.placeholder = t("runners.setup.hostPlaceholder");
    input.title = t("runners.setup.hostHint");
    input.setAttribute("aria-label", t("runners.setup.hostLabel"));
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
    const connect = action(
      view.host.busy ? t("runners.setup.connecting") : t("runners.setup.connect"),
      "connect",
      "primary",
    );
    connect.disabled = view.host.busy;
    connect.addEventListener("click", () => void setup.connect());
    const nodes: HTMLElement[] = [question(t("runners.flow.whichPc")), input, connect];
    nodes.push(...hostMessages(view));
    // Back to the PC Orivo knows, or out of a change asked for from the menu.
    if (view.host.saved) {
      const cancel = action(t("runners.setup.cancel"), "cancel-add-host");
      cancel.addEventListener("click", () =>
        view.host.adding ? setup.cancelAddHost() : setup.stopEditing(),
      );
      nodes.push(row(cancel));
    }
    return nodes;
  }

  /** The PC answered and does not know this computer yet. */
  function pairScreen(view: SetupSnapshot): HTMLElement[] {
    const { pairing } = view;
    const nodes: HTMLElement[] = [
      element("p", "gs-flow__host", pairing.host || view.host.saved),
      question(t("runners.flow.pairQuestion")),
    ];
    if (pairing.phase === "starting") {
      nodes.push(waiting(t("runners.setup.gettingPin")));
      return nodes;
    }
    if (pairing.phase !== "idle" && pairing.phase !== "paired") {
      nodes.push(alertLine(pairing.message ?? SETUP_COPY.pairingFailed));
    }
    const start = action(
      pairing.phase === "idle" || pairing.phase === "paired"
        ? t("runners.flow.pair")
        : t("runners.setup.tryAgain"),
      "start-pairing",
      "primary",
    );
    start.addEventListener("click", () => void setup.startPairing());
    nodes.push(start);
    if (pairing.phase === "clientMissing") {
      const choose = action(t("runners.setup.chooseApp"), "choose-client");
      choose.addEventListener("click", () => void setup.chooseClientByHand());
      nodes.push(row(choose));
    }
    return nodes;
  }

  /** The PIN, large enough to read from the other machine's keyboard. */
  function pinScreen(view: SetupSnapshot): HTMLElement[] {
    const { pairing } = view;
    const pin = element("div", "gs-pin", pairing.pin);
    pin.setAttribute("aria-label", t("runners.flow.pinAria", { pin: pairing.pin }));
    const open = action(t("runners.flow.openPinPage"), "open-pin-page");
    // The browser warns about Sunshine's own certificate: said where the click
    // that leads to it is, not as a paragraph under it.
    open.title = SETUP_COPY.certificate;
    open.addEventListener("click", () => void setup.openPinPage());
    const nodes: HTMLElement[] = [
      element("p", "gs-flow__host", pairing.host || view.host.saved),
      pin,
      element("p", "gs-flow__hint", t("runners.flow.typePin", { app: HOST_APP })),
      waiting(t("runners.flow.waiting")),
    ];
    if (pairing.lostContact) nodes.push(alertLine(SETUP_COPY.pairingLostContact));
    nodes.push(row(open));
    return nodes;
  }

  /** The ⋯ menu beside the main button once the PC is known. */
  function hostMenu(view: SetupSnapshot): HTMLElement {
    const busy = view.games.phase === "preparing" || view.games.phase === "importing";
    const entries: MoreMenuEntry[] = [
      {
        kind: "item",
        label: t("runners.flow.changePc"),
        icon: "monitor",
        data: { gsAction: "edit-host" },
        run: () => setup.edit("host"),
      },
      {
        kind: "item",
        label: t("runners.flow.resync"),
        icon: "sync",
        disabled: busy,
        data: { gsAction: "refresh-games" },
        run: () => void setup.getGames(),
      },
      {
        kind: "item",
        label: t("runners.flow.streamingApp"),
        icon: "cast",
        data: { gsAction: "edit-client" },
        run: () => setup.edit("client"),
      },
      ...(options.pluginMenu?.() ?? []),
    ];
    return pluginMoreMenu({
      label: t("runners.flow.more"),
      entries,
      open: menuOpen,
      onToggle: (open) => {
        menuOpen = open;
      },
    });
  }

  /** The PC, once it is paired: its state, then "See my games". */
  function hostScreen(view: SetupSnapshot): HTMLElement[] {
    const { games } = view;
    const card = element("div", "gs-host");
    const mark = element("span", "plugin-icon gs-host__mark");
    mark.dataset.category = "streaming";
    mark.setAttribute("aria-hidden", "true");
    mark.innerHTML = icon("monitor");
    const copy = element("div", "gs-host__copy");
    const pills = element("div", "gs-host__pills");
    for (const entry of hostPills({
      checking: view.host.checking,
      reachable: view.facts.hostReachable,
      paired: view.facts.paired,
      gameCount: games.phase === "importing" ? 0 : games.count,
    })) {
      pills.append(pill(entry.text, entry.tone));
    }
    copy.append(element("strong", "gs-host__name", view.host.saved), pills);
    card.append(mark, copy);

    const nodes: HTMLElement[] = [card];
    const main = element("div", "gs-flow__actions gs-flow__actions--main");
    switch (games.phase) {
      case "preparing":
      case "importing": {
        nodes.push(
          waiting(
            games.phase === "importing" && games.found > 0
              ? t("runners.setup.foundGames", { games: gameCount(games.found) })
              : t("runners.setup.lookingForGames"),
          ),
        );
        break;
      }
      case "ready": {
        const library = action(t("runners.flow.seeGames"), "open-library", "primary");
        library.addEventListener("click", () => setup.openLibrary());
        main.append(library);
        break;
      }
      case "empty": {
        nodes.push(element("p", "gs-flow__note", SETUP_COPY.noGames));
        const again = action(t("runners.setup.refreshGames"), "refresh-games", "primary");
        again.addEventListener("click", () => void setup.getGames());
        main.append(again);
        break;
      }
      case "failed": {
        nodes.push(alertLine(games.message ?? ""));
        const retry = action(t("runners.setup.tryAgain"), "refresh-games", "primary");
        retry.addEventListener("click", () => void setup.getGames());
        const check = action(t("runners.setup.checkMyPc"), "recheck");
        check.addEventListener("click", () => void setup.recheck());
        main.append(retry, check);
        break;
      }
      case "idle": {
        const get = action(t("runners.setup.getGames"), "refresh-games", "primary");
        get.addEventListener("click", () => void setup.getGames());
        main.append(get);
        break;
      }
    }
    main.append(hostMenu(view));
    nodes.push(main);
    if (games.findingArtwork) nodes.push(waiting(t("runners.setup.findingArtwork")));
    return nodes;
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

    const flow = element("div", "gs-flow");
    flow.dataset.gsScreen = view.screen;
    if (view.pluginIssue) flow.append(alertLine(view.pluginIssue));
    if (view.screen !== "host") menuOpen = false;
    switch (view.screen) {
      case "loading":
        flow.append(waiting(t("runners.setup.loading")));
        break;
      case "missing":
        flow.append(...missingScreen(view));
        break;
      case "app":
        flow.append(...appScreen(view));
        break;
      case "pick":
        flow.append(...pickScreen(view));
        break;
      case "address":
        flow.append(...addressScreen(view));
        break;
      case "pair":
        flow.append(...pairScreen(view));
        break;
      case "pin":
        flow.append(...pinScreen(view));
        break;
      case "host":
        flow.append(...hostScreen(view));
        break;
    }
    root.replaceChildren(flow);

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
