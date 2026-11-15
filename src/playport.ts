/* ---------------------------------------------------------------------------
   Playport, on an iPhone or an iPad that has it.

   Playport runs Windows games on the device itself, installed from the same
   Steam, GOG and Epic Games accounts Orivo reads (`src-tauri/src/playport.rs`).
   On iOS a Steam game's Install already has an answer — its Steam page at
   least — so there Playport is one more row of the menu the host builds, and
   nothing here is needed. A GOG or Epic game has no other answer, so its
   button is live only while Playport is installed. The host is asked at start,
   and again whenever Orivo comes back to the front: that is when an app can
   have been installed or deleted.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";

const DESKTOP_PLATFORMS: readonly string[] = ["windows", "macos", "linux"];

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** Whether the host says Playport is installed. Anything but `true` is a no. */
export async function loadPlayportInstalled(): Promise<boolean> {
  if (!isTauriRuntime()) return false;
  try {
    return (await invoke<unknown>("is_playport_installed")) === true;
  } catch {
    // A host that predates the command has no Playport to offer.
    return false;
  }
}

/**
 * A GOG or Epic Games game Playport could take: one with a Windows build, or
 * whose store published no platforms at all. The host applies the same rule
 * before it offers the row (`playport::game_for`).
 */
export function playportTakes(game: {
  source?: string;
  supportedPlatforms?: readonly string[];
}): boolean {
  if (game.source !== "gog" && game.source !== "epic") return false;
  const platforms = (game.supportedPlatforms ?? []).filter((platform) =>
    DESKTOP_PLATFORMS.includes(platform),
  );
  return platforms.length === 0 || platforms.includes("windows");
}
