/**
 * The Library's empty state: what Orivo shows someone whose catalogue holds
 * nothing yet.
 *
 * It replaces the bundled showcase games that used to stand in for a real
 * library. Those made an empty install look full, which is the one thing an
 * empty install must not do — a first-run screen that lies about owning ten
 * games has nothing left to ask for.
 *
 * This module owns the copy and the navigation of that screen and nothing
 * else: no DOM, no backend calls. `app.ts` renders it and runs the connectors.
 */
import type { ConnectedSource, SourceConnectStyle } from "./contracts";
import type { IconName } from "./icons";
import { CONNECTED_SOURCES } from "./source-model";

/**
 * A store the welcome panel can start. Steam is in the list but is not a
 * `ConnectedSource`: it has its own web login and its own installed-games
 * import, so the shell routes it through a different pair of commands.
 */
export type OnboardingSource = "steam" | ConnectedSource;

export interface OnboardingStep {
  icon: IconName;
  title: string;
  detail: string;
}

/**
 * What the next few minutes look like. Three, because a fourth is a checklist
 * and a checklist is a chore — and because only the first one is asked for on
 * this screen; the other two are a promise about where it leads.
 */
export const ONBOARDING_STEPS: readonly OnboardingStep[] = [
  {
    icon: "link",
    title: "Connect your libraries",
    detail: "Steam, Epic, GOG…",
  },
  {
    icon: "sliders",
    title: "Personalise Orivo",
    detail: "Themes, layouts, wallpapers…",
  },
  {
    icon: "play",
    title: "Play your games",
    detail: "Launch, organise, enjoy.",
  },
] as const;

export interface OnboardingSourceDescriptor {
  provider: OnboardingSource;
  label: string;
  icon: IconName;
  /**
   * What connecting this store actually puts in the library, in one line that
   * fits a row of the panel. It is written to fit: a store list where every
   * second entry ends in an ellipsis says nothing about any of them.
   */
  detail: string;
  /**
   * How this store proves who you are, when the backend has not said. Steam is
   * the only one with a fixed answer here; every other store's real sign-in
   * style arrives on its `SourceAccountStatus`, and that answer wins.
   */
  signIn?: string;
}

/**
 * Per-store copy. Kept as its own map so the ordered list below can take the
 * label and the icon from the source registry rather than restating them: a
 * store renamed in one place can never read differently in the other.
 */
const SOURCE_COPY: Record<OnboardingSource, { detail: string; signIn?: string }> = {
  steam: {
    detail: "Owned games, plus installs on this Mac",
    signIn: "Signs in through Steam's own web login.",
  },
  epic: { detail: "Your Epic library and its installs" },
  gog: { detail: "Your GOG library and installers" },
  ubisoft: { detail: "Your Ubisoft Connect library" },
  xbox: { detail: "Xbox and PC Game Pass" },
  "microsoft-store": { detail: "Your Microsoft Store games" },
  "instant-gaming": { detail: "The keys you bought there" },
};

/**
 * Every store the welcome panel offers, in the order it offers them. Steam
 * leads because it is the one store most libraries start from; the rest keep
 * the registry order Settings already uses.
 */
export const ONBOARDING_SOURCES: readonly OnboardingSourceDescriptor[] = [
  { provider: "steam", label: "Steam", icon: "steam", ...SOURCE_COPY.steam },
  ...CONNECTED_SOURCES.map((source) => ({
    provider: source.provider,
    label: source.label,
    icon: source.icon,
    ...SOURCE_COPY[source.provider],
  })),
];

export function onboardingSourceDescriptor(
  provider: OnboardingSource,
): OnboardingSourceDescriptor {
  return (
    ONBOARDING_SOURCES.find((source) => source.provider === provider) ??
    ONBOARDING_SOURCES[0]
  );
}

export function isOnboardingSource(value: string): value is OnboardingSource {
  return ONBOARDING_SOURCES.some((source) => source.provider === value);
}

/**
 * One sentence about what signing in will feel like, because the two styles
 * feel nothing alike: a token store asks once and syncs quietly afterwards, a
 * session store keeps its own window and does the work inside it.
 */
export function onboardingSignInLine(style: SourceConnectStyle): string {
  return style === "session"
    ? "No account API: a sign-in window opens and stays signed in, and every sync runs inside it."
    : "Signs in once. The credential is kept in your keychain and syncs run in the background.";
}

/**
 * The welcome panel's navigation. Three views deep at most, and every one of
 * them is one press from the one before it — a first-run screen is not a place
 * to get lost in.
 */
export type OnboardingView =
  | { step: "choice" }
  | { step: "sources" }
  | { step: "connect"; provider: OnboardingSource };

export const INITIAL_ONBOARDING_VIEW: OnboardingView = { step: "choice" };

/** Where Back goes. `choice` is the root, so it stays put. */
export function onboardingBack(view: OnboardingView): OnboardingView {
  return view.step === "connect" ? { step: "sources" } : { step: "choice" };
}

/** The label Back wears, which names the destination rather than the gesture. */
export function onboardingBackLabel(view: OnboardingView): string {
  return view.step === "connect" ? "Libraries" : "Back";
}

/** Two views are the same view when they show the same thing. */
export function isSameOnboardingView(a: OnboardingView, b: OnboardingView): boolean {
  if (a.step !== b.step) return false;
  return a.step !== "connect" || b.step !== "connect" || a.provider === b.provider;
}
