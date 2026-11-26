/* ---------------------------------------------------------------------------
   Game streaming, the pure half: which step the person is on, which one
   question the panel asks about it, and what the plugin's card says.

   No DOM and no host here, so the order of the flow is stated once and tested
   without a screen. The controller and the view (`gamestream-setup.ts`) read
   these and nothing else decides.
   --------------------------------------------------------------------------- */

import { t } from "./i18n";

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

export function gameCount(count: number): string {
  return count === 1 ? t("runners.setup.gameOne") : t("runners.setup.gameMany", { count });
}

/* ---------------------------------------------------------------------------
   The one question on screen
   --------------------------------------------------------------------------- */

/**
 * What the panel shows. One screen at a time, each a single question with one
 * main button:
 *
 * - `missing`  the streaming app is not on this computer: Install.
 * - `app`      which streaming app (reopened from the ⋯ menu).
 * - `pick`     which PC: the one Orivo knows, or another.
 * - `address`  another PC: its name or address.
 * - `pair`     the PC answered but does not know this computer yet.
 * - `pin`      the PIN to type on the PC, while Orivo waits for it.
 * - `host`     the PC, ready (or getting its games): See my games.
 */
export type FlowScreen = "loading" | "missing" | "app" | "pick" | "address" | "pair" | "pin" | "host";

export interface FlowInput {
  loading: boolean;
  stage: SetupStage;
  /** A step reopened from the ⋯ menu, or null. */
  editing: SetupStepId | null;
  pairingPhase: string;
  /** The PC Orivo knows, or "" when none was ever saved. */
  savedHost: string;
  /** "Other" was picked: the address field is open. */
  addingHost: boolean;
}

export function flowScreen(input: FlowInput): FlowScreen {
  if (input.loading) return "loading";
  // A change asked for from the menu wins over the stage: it is what the
  // person just pressed.
  if (input.editing === "client") return "app";
  if (input.stage === "client") return "missing";
  if (input.editing === "host" || input.stage === "host") {
    // With no PC known there is nothing to pick between: straight to the field.
    return input.savedHost && !input.addingHost ? "pick" : "address";
  }
  if (input.stage === "pair") return input.pairingPhase === "waiting" ? "pin" : "pair";
  return "host";
}

export type PillTone = "ok" | "warn" | "muted";
export interface StatePill {
  text: string;
  tone: PillTone;
}

/** The pills beside the PC's name: whether it answers, then how many games. */
export function hostPills(input: {
  checking: boolean;
  reachable: boolean;
  paired: boolean;
  gameCount: number;
}): StatePill[] {
  const pills: StatePill[] = [];
  if (input.checking) pills.push({ text: t("runners.flow.checking"), tone: "muted" });
  else if (input.reachable || input.paired) pills.push({ text: t("runners.flow.online"), tone: "ok" });
  else pills.push({ text: t("runners.flow.offline"), tone: "warn" });
  if (input.gameCount > 0) pills.push({ text: gameCount(input.gameCount), tone: "muted" });
  return pills;
}

/** "Moonlight.app" reads as "Moonlight". */
export function appName(label: string, fallback = "Moonlight"): string {
  return label.replace(/\.app$/i, "").trim() || fallback;
}

/* ---------------------------------------------------------------------------
   The plugin's card in the list
   --------------------------------------------------------------------------- */

export interface CardStatus {
  text: string;
  tone: PillTone;
}

export interface StreamingCardInput {
  /** The plugin cannot run at all, with the reason. */
  issue: string | null;
  profile: { applicationLabel: string; applicationAvailable: boolean; gameCount: number } | null;
  savedHost: string;
}

/**
 * The two lines under Game streaming's name: how it stands, then the app and
 * the PC it streams from. Ready means games came from the PC; anything short
 * of that is a setup still to finish, and wears the warning.
 */
export function streamingCardStatus(input: StreamingCardInput): { status: CardStatus; detail: string } {
  const app = input.profile ? appName(input.profile.applicationLabel) : "";
  const detail = [app, input.savedHost].filter(Boolean).join(" · ");
  if (input.issue) return { status: { text: input.issue, tone: "warn" }, detail };
  if (input.profile && !input.profile.applicationAvailable) {
    return { status: { text: t("runners.flow.appMissingShort", { app }), tone: "warn" }, detail };
  }
  if (input.profile && input.profile.gameCount > 0) {
    return {
      status: {
        text: t("runners.flow.readyWithGames", { games: gameCount(input.profile.gameCount) }),
        tone: "ok",
      },
      detail,
    };
  }
  return { status: { text: t("runners.flow.toSetUp"), tone: "warn" }, detail };
}
