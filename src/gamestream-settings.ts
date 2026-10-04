/* ---------------------------------------------------------------------------
   The game-streaming host card inside Settings › Plugins.

   One remote machine, and one field: where it is. Orivo asks that machine which
   games it can stream by running the profile's own streaming client
   (`moonlight list <host>`), and writes one stream description per game into
   the profile's folder (`gamestream.rs`).

   There is no credential on this card, and that is the point rather than an
   omission. The client is authorised by the certificate it established when it
   paired with the machine — the same authorisation that lets it stream — so
   nothing has to be asked for, stored, or kept out of the WebView. A user who
   can stream can list.

   Same contract as the rest of Settings: the panel talks to an interface, so a
   browser with no Tauri behind it renders an empty field instead of an error,
   and a test hands in a recorder.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";

export interface GameStreamSettingsView {
  /** `astra.local`, `astra.local:47990`, `192.168.1.40`, `[fd00::1]:47990`. */
  host: string;
}

export interface GameStreamSettingsUpdate {
  host?: string;
}

export interface GameStreamClient {
  load(signal: AbortSignal): Promise<GameStreamSettingsView>;
  save(update: GameStreamSettingsUpdate, signal: AbortSignal): Promise<GameStreamSettingsView>;
}

export interface GameStreamPanelHandlers {
  showToast(message: string): void;
}

export const EMPTY_GAMESTREAM_SETTINGS: GameStreamSettingsView = { host: "" };

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function assertActive(signal: AbortSignal): void {
  if (signal.aborted) throw new DOMException("La requête a été annulée.", "AbortError");
}

export function gameStreamErrorMessage(error: unknown): string {
  if (typeof error === "string" && error.trim()) return error.trim();
  if (error instanceof Error && error.message.trim()) return error.message.trim();
  return "Orivo could not save that. Try again.";
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
        throw new Error("Game streaming is only available in the Orivo desktop app.");
      }
      assertActive(signal);
      return readGameStreamSettings(await invoke("update_gamestream_settings", { update }));
    },
  };
}

function field(
  id: string,
  label: string,
  type: "text" | "password",
  placeholder: string,
  hint: string,
): { wrapper: HTMLElement; input: HTMLInputElement } {
  const wrapper = document.createElement("div");
  wrapper.className = "credentials-form__field";
  const labelElement = document.createElement("label");
  labelElement.htmlFor = id;
  labelElement.textContent = label;
  const input = document.createElement("input");
  input.id = id;
  input.className = "credentials-form__input";
  input.type = type;
  input.autocomplete = "off";
  input.spellcheck = false;
  input.placeholder = placeholder;
  wrapper.append(labelElement, input);
  if (hint) {
    const small = document.createElement("small");
    small.textContent = hint;
    wrapper.append(small);
  }
  return { wrapper, input };
}

/**
 * Render the card's body and keep it in step with the host.
 *
 * `load` is called on every visit to Settings › Plugins rather than once: the
 * address is host-private configuration another window could have changed, and
 * re-reading it costs one command.
 */
export function mountGameStreamPanel(
  root: HTMLElement,
  client: GameStreamClient,
  handlers: GameStreamPanelHandlers,
): { load: (signal: AbortSignal) => Promise<void> } {
  let saving = false;

  const intro = document.createElement("div");
  intro.className = "settings-row";
  const introCopy = document.createElement("div");
  introCopy.className = "settings-row__copy";
  const introTitle = document.createElement("strong");
  introTitle.textContent = "How it works";
  const introText = document.createElement("small");
  introText.textContent =
    "Orivo asks this machine which games it can stream and writes one card per game into your streaming profile's folder. Importing that profile is what refreshes the list. No password is needed: pairing your client with the machine is what authorises the question. Leave the address empty to maintain that folder yourself.";
  introCopy.append(introTitle, introText);
  intro.append(introCopy);

  const form = document.createElement("div");
  form.className = "credentials-form";
  const host = field(
    "gamestream-host",
    "Host address",
    "text",
    "astra.local",
    "The machine running Sunshine or Apollo. A port is accepted and then ignored — pairing and streaming use the machine's own ports, not its web interface's.",
  );

  const actions = document.createElement("div");
  actions.className = "credentials-form__actions";
  const save = document.createElement("button");
  save.type = "button";
  save.className = "settings-button";
  save.id = "gamestream-save";
  save.textContent = "Save host";
  const status = document.createElement("small");
  status.id = "gamestream-status";
  actions.append(save, status);

  form.append(host.wrapper, actions);
  root.replaceChildren(intro, form);

  const fill = (settings: GameStreamSettingsView): void => {
    host.input.value = settings.host;
    status.textContent = settings.host
      ? "Saved. A streaming profile's next import will use it."
      : "No host yet.";
  };
  fill({ ...EMPTY_GAMESTREAM_SETTINGS });

  save.addEventListener("click", () => {
    if (saving) return;
    saving = true;
    save.disabled = true;
    status.textContent = "Saving…";
    const lifetime = new AbortController();
    void client
      .save({ host: host.input.value }, lifetime.signal)
      .then((saved) => {
        fill(saved);
        handlers.showToast(
          saved.host ? "Game streaming host saved." : "Game streaming host cleared.",
        );
      })
      .catch((error: unknown) => {
        // The host validates the address where the user typed it, so its
        // refusal is the message worth showing — beside the box, not only in
        // a toast that is gone by the time they look back at the field.
        const message = gameStreamErrorMessage(error);
        status.textContent = message;
        handlers.showToast(message);
      })
      .finally(() => {
        saving = false;
        save.disabled = false;
      });
  });

  return {
    async load(signal) {
      const settings = await client.load(signal);
      if (signal.aborted || saving) return;
      fill(settings);
    },
  };
}
