/**
 * Settings › Plugins, the pure half: what each card says about how its plugin
 * stands, and which plugin's detail is open when the person picked none.
 * No DOM and no host, so app.ts only gathers the facts and draws the answer.
 */
import type { CardStatus } from "./gamestream-setup-model";
import { t } from "./i18n";

export type { CardStatus };

/**
 * The detail open beside the list. The person's own pick wins; with none, the
 * first card that opens a detail in place — the panel is never an empty
 * "pick a plugin". A card that leads elsewhere (Winlator, set up under
 * Libraries) is skipped, since opening it by default would leave the page.
 */
export function defaultPluginView<View extends string>(
  picked: View | "list",
  cards: readonly View[],
  leavesPage: (view: View) => boolean = () => false,
): View | "list" {
  if (picked !== "list") return picked;
  return cards.find((view) => !leavesPage(view)) ?? "list";
}

/** A registry plugin: what is wrong with it if anything is, else Ready. */
export function managedPluginStatus(input: {
  /** What is wrong, in a few words, or "" (`pluginTrouble` in app.ts). */
  trouble: string;
  updating: boolean;
}): CardStatus {
  if (input.trouble) return { text: input.trouble, tone: "warn" };
  if (input.updating) return { text: t("app.plugin.updatingEllipsis"), tone: "muted" };
  return { text: t("app.plugins.ready"), tone: "ok" };
}

/** Wine, from the runner check: ready, being checked, or what stops it. */
export function wineCardStatus(
  runner: { state: string; message: string } | null,
  loading: boolean,
): CardStatus | null {
  if (!runner) return loading ? { text: t("app.plugins.checking"), tone: "muted" } : null;
  switch (runner.state) {
    case "ready":
      return { text: t("app.plugins.ready"), tone: "ok" };
    case "checking":
      return { text: t("app.plugins.checking"), tone: "muted" };
    case "unavailable":
      return { text: t("app.plugins.missing"), tone: "warn" };
    default:
      return { text: t("app.plugins.needsAttention"), tone: "warn" };
  }
}

export function profileCount(count: number): string {
  return count === 1 ? t("app.plugins.profileOne") : t("app.plugins.profileMany", { count });
}

/**
 * Native Switch, from its checklist: what is still to install comes first
 * (nothing builds without it), then the keys, which only a game needs.
 */
export function nxnativeCardStatus(
  status: { supported: boolean; checks: ReadonlyArray<{ state: string }>; keysChosen: boolean } | null,
): CardStatus | null {
  if (!status) return null;
  if (!status.supported) return { text: t("app.plugins.unsupported"), tone: "warn" };
  const missing = status.checks.filter((check) => check.state !== "ok").length;
  if (missing > 0) return { text: t("app.plugins.toInstall", { count: missing }), tone: "warn" };
  if (!status.keysChosen) return { text: t("app.plugins.keysMissing"), tone: "warn" };
  return { text: t("app.plugins.ready"), tone: "ok" };
}

/** The grey line at the bottom of a plugin's ⋯ menu: version · source · updates. */
export function pluginMetaLine(input: { version: string; trusted: boolean; automatic: boolean }): string {
  return [
    input.version ? `v${input.version}` : "",
    input.trusted ? t("app.plugins.official") : t("app.plugins.community"),
    input.automatic ? t("app.plugins.autoUpdatesOn") : t("app.plugins.autoUpdatesOff"),
  ]
    .filter(Boolean)
    .join(" · ");
}
