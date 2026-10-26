/* ---------------------------------------------------------------------------
   The gaming PC's address, and the two pages Orivo can open for the setup.

   One remote machine, and one field: where it is. Orivo asks that machine which
   games it can stream by running the profile's own streaming client
   (`moonlight list <host>`), and writes one stream description per game into
   the profile's folder (`gamestream.rs`). The guided setup
   (`gamestream-setup.ts`) is the only screen that edits it.

   There is no credential here, and that is the point rather than an omission.
   The client is authorised by the certificate it established when it paired
   with the machine — the same authorisation that lets it stream — so nothing
   has to be asked for, stored, or kept out of the WebView. A user who can
   stream can list.

   Same contract as the rest of Settings: the setup talks to an interface, so a
   browser with no Tauri behind it reads an empty address instead of an error,
   and a test hands in a recorder.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

export interface GameStreamSettingsView {
  /** `astra.local`, `astra.local:47990`, `192.168.1.40`, `[fd00::1]:47990`. */
  host: string;
}

export interface GameStreamSettingsUpdate {
  host?: string;
}

/**
 * The two pages the setup can open in the browser. Names, not addresses: the
 * host builds the URL, so nothing the WebView sends becomes somewhere to go.
 */
export type GameStreamLink = "moonlight" | "sunshine-pin";

export interface GameStreamClient {
  load(signal: AbortSignal): Promise<GameStreamSettingsView>;
  save(update: GameStreamSettingsUpdate, signal: AbortSignal): Promise<GameStreamSettingsView>;
  openLink(link: GameStreamLink, signal: AbortSignal): Promise<void>;
}

export const EMPTY_GAMESTREAM_SETTINGS: GameStreamSettingsView = { host: "" };

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("The request was cancelled.", "AbortError");
}

export function gameStreamErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return t("runners.gsSettings.saveError");
}

/** Whatever the host answered, as the three strings this card renders. */
export function readGameStreamSettings(value: unknown): GameStreamSettingsView {
  if (!value || typeof value !== "object") return { ...EMPTY_GAMESTREAM_SETTINGS };
  const raw = value as Partial<GameStreamSettingsView>;
  return { host: typeof raw.host === "string" ? raw.host : "" };
}

export function createGameStreamClient(): GameStreamClient {
  return {
    async load(signal) {
      try {
        if (!isTauriRuntime()) return { ...EMPTY_GAMESTREAM_SETTINGS };
        assertActive(signal);
        const view = await invoke<unknown>("get_gamestream_settings");
        assertActive(signal);
        return readGameStreamSettings(view);
      } catch {
        // An unreadable settings file is already degraded to empty on the host
        // side; this is the same answer for a host that has no such command.
        return { ...EMPTY_GAMESTREAM_SETTINGS };
      }
    },

    async save(update, signal) {
      if (!isTauriRuntime()) {
        throw new Error(t("runners.gsSettings.desktopOnly"));
      }
      assertActive(signal);
      return readGameStreamSettings(await invoke("update_gamestream_settings", { update }));
    },

    async openLink(link, signal) {
      if (!isTauriRuntime()) {
        throw new Error(t("runners.gsSettings.desktopOnly"));
      }
      assertActive(signal);
      await invoke("open_gamestream_link", { target: link });
    },
  };
}
