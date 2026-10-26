import { t } from "./i18n";

/**
 * What this machine is called in user-facing copy — settings hints, toasts,
 * onboarding rows. Read from the user agent because this copy spans modules
 * the catalog's per-game `hostPlatform` never reaches, and because it is a
 * fact about the session, not about any one game.
 */
export function hostDeviceLabel(): string {
  const userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (userAgent.includes("Windows")) return t("misc.device.pc");
  if (userAgent.includes("Mac")) return t("misc.device.mac");
  if (userAgent.includes("Linux") || userAgent.includes("X11")) {
    return t("misc.device.machine");
  }
  return t("misc.device.device");
}

/**
 * How Orivo gets out of a started game's way here, in the words of the system
 * it runs on: a Mac hides an app and brings it back with ⌘-Tab or the Dock,
 * Windows and Linux minimise a window to the taskbar (`step_aside_for_game`).
 */
export function stepAsideCopy(): { action: string; hint: string } {
  const userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (userAgent.includes("Mac")) {
    return {
      action: t("misc.stepAside.hide.action"),
      hint: t("misc.stepAside.hide.hint"),
    };
  }
  return {
    action: t("misc.stepAside.minimise.action"),
    hint: t("misc.stepAside.minimise.hint"),
  };
}
