/* ---------------------------------------------------------------------------
   What this build can do on the device it runs on.

   Asked of the host rather than read from the user agent: iPadOS presents
   itself as a Mac by default, so "is this iOS?" is a fact only the binary
   knows. A browser with no Tauri behind it, and a host binary that predates
   the command, both get the desktop answer — which is what they always were.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";

export interface PlatformFeatures {
  /** Plugins fetched from the registry or picked from a file. Off on iOS. */
  downloadablePlugins: boolean;
  /** Wine, Winlator, emulators and streaming: anything that starts a game here. */
  localRunners: boolean;
  /** "Install" asks the host where the game can be got on this device. */
  storeChoices: boolean;
}

export const DESKTOP_FEATURES: PlatformFeatures = {
  downloadablePlugins: true,
  localRunners: true,
  storeChoices: false,
};

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Each flag is taken only when the host actually sent a boolean for it. */
export function readPlatformFeatures(value: unknown): PlatformFeatures {
  if (!value || typeof value !== "object") return { ...DESKTOP_FEATURES };
  const raw = value as Partial<Record<keyof PlatformFeatures, unknown>>;
  const flag = (key: keyof PlatformFeatures): boolean =>
    typeof raw[key] === "boolean" ? (raw[key] as boolean) : DESKTOP_FEATURES[key];
  return {
    downloadablePlugins: flag("downloadablePlugins"),
    localRunners: flag("localRunners"),
    storeChoices: flag("storeChoices"),
  };
}

export async function loadPlatformFeatures(): Promise<PlatformFeatures> {
  if (!isTauriRuntime()) return { ...DESKTOP_FEATURES };
  try {
    return readPlatformFeatures(await invoke<unknown>("get_platform_features"));
  } catch {
    return { ...DESKTOP_FEATURES };
  }
}
