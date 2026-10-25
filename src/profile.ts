/**
 * The player's own name and picture, as the top bar's profile menu edits them.
 *
 * Kept in this WebView's storage rather than the host: it is presentation,
 * one small record, and it has to be there for the very first paint.
 */

export interface PlayerProfile {
  name: string;
  /** A data URL of the square picture, or "" for the default one. */
  avatar: string;
}

export const DEFAULT_AVATAR = "/media/steam-avatar.png";
const STORAGE_KEY = "orivo.profile";
const MAX_NAME_LENGTH = 32;
/** Drawn at 36–64px; twice that keeps it sharp on a retina screen. */
const AVATAR_SIZE = 160;

export function cleanName(value: string): string {
  return value.replace(/\s+/g, " ").trim().slice(0, MAX_NAME_LENGTH);
}

export function loadProfile(storage: Pick<Storage, "getItem"> | null = safeStorage()): PlayerProfile {
  try {
    const raw = storage?.getItem(STORAGE_KEY);
    const record = raw ? (JSON.parse(raw) as Partial<PlayerProfile>) : {};
    return {
      name: typeof record.name === "string" ? cleanName(record.name) : "",
      avatar: typeof record.avatar === "string" && record.avatar.startsWith("data:image/") ? record.avatar : "",
    };
  } catch {
    return { name: "", avatar: "" };
  }
}

export function saveProfile(
  profile: PlayerProfile,
  storage: Pick<Storage, "setItem"> | null = safeStorage(),
): void {
  try {
    storage?.setItem(STORAGE_KEY, JSON.stringify({ name: cleanName(profile.name), avatar: profile.avatar }));
  } catch {
    // A full or blocked storage keeps the profile for this session only.
  }
}

/** The picture to show: the player's own, or the default. */
export function avatarSource(profile: PlayerProfile): string {
  return profile.avatar || DEFAULT_AVATAR;
}

/**
 * A chosen picture, centre-cropped to a square and scaled down, so a 12 MB
 * photo becomes a few kilobytes of storage rather than the whole quota.
 */
export function avatarFromFile(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    if (!file.type.startsWith("image/")) {
      reject(new Error("That file is not a picture."));
      return;
    }
    const url = URL.createObjectURL(file);
    const image = new Image();
    image.onload = () => {
      URL.revokeObjectURL(url);
      const side = Math.min(image.naturalWidth, image.naturalHeight);
      if (side === 0) {
        reject(new Error("That picture could not be read."));
        return;
      }
      const canvas = document.createElement("canvas");
      canvas.width = AVATAR_SIZE;
      canvas.height = AVATAR_SIZE;
      const context = canvas.getContext("2d");
      if (!context) {
        reject(new Error("That picture could not be read."));
        return;
      }
      context.drawImage(
        image,
        (image.naturalWidth - side) / 2,
        (image.naturalHeight - side) / 2,
        side,
        side,
        0,
        0,
        AVATAR_SIZE,
        AVATAR_SIZE,
      );
      resolve(canvas.toDataURL("image/jpeg", 0.86));
    };
    image.onerror = () => {
      URL.revokeObjectURL(url);
      reject(new Error("That picture could not be read."));
    };
    image.src = url;
  });
}

function safeStorage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}
