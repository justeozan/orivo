/**
 * The one door from the webview to the Rust host.
 *
 * Every screen asks the host for things by name, and when the host says no the
 * screen shows a toast and carries on. That is right for the player and it means
 * nobody who could fix the cause ever hears about it: nothing was thrown at the
 * window, so Sentry's own handlers see nothing.
 *
 * This is Tauri's `invoke`, unchanged, with one addition: a rejected call is
 * reported with the command's name before the caller gets it back. The caller
 * still receives the same rejection and handles it as before.
 *
 * It is a module and not a patch on `window.__TAURI_INTERNALS__.invoke` because
 * Tauri defines that property as read-only.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { reportFailure } from "./sentry";

export function invoke<T>(...args: Parameters<typeof tauriInvoke>): Promise<T> {
  // `Promise.resolve` lets the call be a plain value, which is what a test's
  // stand-in for the host often returns.
  return Promise.resolve(tauriInvoke<T>(...args)).catch((error: unknown) => {
    reportFailure(args[0], error);
    throw error;
  });
}
