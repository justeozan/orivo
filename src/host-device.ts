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
