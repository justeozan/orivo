/* ---------------------------------------------------------------------------
   Store folders in the Mac App Store edition.

   That edition runs in the App Sandbox: the Steam and Epic libraries still
   come from the accounts, but which games are installed is written in the
   stores' own folders, which the sandbox reads only once the player has
   granted them. The host opens the folder picker already on the right folder
   (folder_access.rs); this module only says which stores are left to grant.
   Every other build reads the folders directly and shows none of this.
   --------------------------------------------------------------------------- */

import { invoke } from "@tauri-apps/api/core";
import { t } from "./i18n";

export type StoreFolder = "steam" | "epic";

export interface StoreFolderAccess {
  needsGrant: boolean;
  steam: boolean;
  epic: boolean;
}

const STORES: readonly StoreFolder[] = ["steam", "epic"];

/** A host that predates the command, or a malformed answer, asks for nothing. */
export function readStoreFolderAccess(value: unknown): StoreFolderAccess {
  const record = (value && typeof value === "object" ? value : {}) as Record<string, unknown>;
  return {
    needsGrant: record.needsGrant === true,
    steam: record.steam === true,
    epic: record.epic === true,
  };
}

export async function loadStoreFolderAccess(): Promise<StoreFolderAccess> {
  try {
    return readStoreFolderAccess(await invoke<unknown>("get_store_folder_access"));
  } catch {
    return readStoreFolderAccess(null);
  }
}

/** Opens the picker; resolves with the new access, or rejects with the host's reason. */
export async function grantStoreFolder(store: StoreFolder): Promise<StoreFolderAccess> {
  return readStoreFolderAccess(await invoke<unknown>("grant_store_folder_access", { store }));
}

const STORE_NAMES: Record<StoreFolder, string> = { steam: "Steam", epic: "Epic Games" };

/** One row per store: granted, or a button that opens the picker. */
export function renderStoreFolders(panel: HTMLElement, access: StoreFolderAccess): void {
  panel.hidden = !access.needsGrant;
  if (!access.needsGrant) return;
  const rows = STORES.map((store) => {
    const row = document.createElement("div");
    row.className = "settings-row";
    const copy = document.createElement("div");
    copy.className = "settings-row__copy";
    const name = document.createElement("strong");
    name.textContent = STORE_NAMES[store];
    const hint = document.createElement("small");
    hint.textContent = access[store]
      ? t("app.storeFolders.granted")
      : t("app.storeFolders.hint", { store: STORE_NAMES[store] });
    copy.append(name, hint);
    row.append(copy);
    if (!access[store]) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "settings-button";
      button.dataset.storeFolderGrant = store;
      button.textContent = t("app.storeFolders.allow");
      row.append(button);
    }
    return row;
  });
  const intro = document.createElement("p");
  intro.className = "settings-card__intro";
  intro.textContent = t("app.storeFolders.intro");
  panel.replaceChildren(intro, ...rows);
}
