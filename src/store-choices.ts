/* ---------------------------------------------------------------------------
   Where a game can be got on a device that cannot run it.

   On an iPhone, an iPad or an Android device "Install" cannot hand a download
   to Steam, so it asks the host what this device can do instead: open the
   game's own iOS version on the App Store when there is one and the device is
   an iPhone or an iPad, and its Steam page otherwise
   (`src-tauri/src/store_choices.rs`). The WebView only ever sends back a game
   id and the id of the row it was offered; the destination stays in Rust.
   --------------------------------------------------------------------------- */

import { invoke } from "./host";
import { t } from "./i18n";

/** The row's name in the player's language; an id this build does not know keeps the host's. */
const CHOICE_LABELS: Readonly<Record<string, () => string>> = {
  "app-store": () => t("app.store.choice.appStore"),
  playport: () => t("app.store.choice.playport"),
  "steam-page": () => t("app.store.choice.steamPage"),
  "steam-link": () => t("app.store.choice.steamLink"),
  // Epic's marketplace app is how this device gets the game, as the App Store is for others.
  "epic-store": () => t("app.store.choice.appStore"),
};

export interface StoreChoice {
  id: string;
  label: string;
  detail: string;
}

export interface StoreChoicesClient {
  list(gameId: string): Promise<StoreChoice[]>;
  /** Whether the game itself opened: its iPhone version was already on the device. */
  open(gameId: string, choiceId: string): Promise<boolean>;
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
      return (await invoke<unknown>("open_store_choice", { gameId, choiceId })) === true;
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
      label.textContent = CHOICE_LABELS[choice.id]?.() ?? choice.label;
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
