import { convertFileSrc } from "@tauri-apps/api/core";
import { appCacheDir, join } from "@tauri-apps/api/path";

const CACHE_PREFIX = "cache:";

export function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/**
 * The backend intentionally returns only a cache token for imported artwork.
 * Bundled media remains an ordinary Vite public URL, while cached files are
 * mapped to Tauri's scoped asset protocol only inside the desktop app.
 */
export async function resolveMediaUrl(value: string | null | undefined): Promise<string> {
  if (!value) {
    return "";
  }

  if (!value.startsWith(CACHE_PREFIX)) {
    return value;
  }

  const opaqueFilename = value.slice(CACHE_PREFIX.length);
  if (!isTauriRuntime() || !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(opaqueFilename)) {
    return "";
  }

  try {
    const cacheDirectory = await appCacheDir();
    const mediaPath = await join(cacheDirectory, "media", opaqueFilename);
    return convertFileSrc(mediaPath);
  } catch {
    return "";
  }
}
