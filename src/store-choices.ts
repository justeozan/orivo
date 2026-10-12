/* ---------------------------------------------------------------------------
   Where a game can be got on a device that cannot run it.

   On an iPhone or an iPad "Install" cannot hand a download to Steam, so it
   asks the host what this device can do instead: open the game's own iOS
   version on the App Store when there is one, and its Steam page otherwise
   (`src-tauri/src/store_choices.rs`). The WebView only ever sends back a game
   id and the id of the row it was offered; the destination stays in Rust.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";

export interface StoreChoice {
  id: string;
  label: string;
  detail: string;
}

export interface StoreChoicesClient {
  list(gameId: string): Promise<StoreChoice[]>;
  open(gameId: string, choiceId: string): Promise<void>;
}

/** Rows the host sent with an id and a label; anything else is dropped. */
export function readStoreChoices(value: unknown): StoreChoice[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    if (!entry || typeof entry !== "object") return [];
    const { id, label, detail } = entry as Partial<Record<keyof StoreChoice, unknown>>;
    if (typeof id !== "string" || !id || typeof label !== "string" || !label) return [];
    return [{ id, label, detail: typeof detail === "string" ? detail : "" }];
  });
}

export function createStoreChoicesClient(): StoreChoicesClient {
  return {
    async list(gameId) {
      return readStoreChoices(await invoke<unknown>("get_store_choices", { gameId }));
    },
    async open(gameId, choiceId) {
      await invoke("open_store_choice", { gameId, choiceId });
    },
  };
}

/**
 * Fill a `.play-launch-menu` with one row per choice. The menu is the same one
 * Play opens when a game can be started two ways, so it reads, focuses and
 * closes the same way; only its rows carry a second line.
 */
export function fillStoreChoiceMenu(
  menu: HTMLElement,
  choices: readonly StoreChoice[],
  pick: (choice: StoreChoice) => void,
): void {
  menu.replaceChildren(
    ...choices.map((choice) => {
      const option = document.createElement("button");
      option.type = "button";
      option.className = "play-launch-option play-launch-option--store";
      option.setAttribute("role", "menuitem");
      option.dataset.storeChoice = choice.id;
      const label = document.createElement("span");
      label.className = "play-launch-option__label";
      label.textContent = choice.label;
      option.append(label);
      if (choice.detail) {
        const detail = document.createElement("small");
        detail.className = "play-launch-option__detail";
        detail.textContent = choice.detail;
        option.append(detail);
      }
      option.addEventListener("click", () => pick(choice));
      return option;
    }),
  );
}
