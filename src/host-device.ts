/**
 * What this machine is called in user-facing copy — settings hints, toasts,
 * onboarding rows. Read from the user agent because this copy spans modules
 * the catalog's per-game `hostPlatform` never reaches, and because it is a
 * fact about the session, not about any one game.
 */
export function hostDeviceLabel(): string {
  const userAgent = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (userAgent.includes("Windows")) return "this PC";
  if (userAgent.includes("Mac")) return "this Mac";
  if (userAgent.includes("Linux") || userAgent.includes("X11")) {
    return "this machine";
  }
  return "this device";
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
      action: "Hide Orivo",
      hint: "Orivo hides once the game is on screen. ⌘-Tab or the Dock brings it back.",
    };
  }
  return {
    action: "Minimise Orivo",
    hint: "Orivo minimises once the game is on screen. The taskbar brings it back.",
  };
}
