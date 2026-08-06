import { convertFileSrc } from "@tauri-apps/api/core";
import { appCacheDir, join } from "@tauri-apps/api/path";

const CACHE_PREFIX = "cache:";
let mediaDirectoryPromise: Promise<string> | null = null;
/**
 * The resolved media directory, kept once it is known so a cached artwork token
 * can be turned into a URL without awaiting anything.
 *
 * This is what lets the first paint of the library show the real covers. Before
 * it, every card whose art lives in the cache rendered a placeholder until an
 * async hydration pass caught up — and only the first handful ever did.
 */
let mediaDirectory: string | null = null;

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
  if (!isTauriRuntime() || !isOpaqueFilename(opaqueFilename)) {
    return "";
  }

  try {
    mediaDirectoryPromise ??= appCacheDir().then((cacheDirectory) => join(cacheDirectory, "media"));
    mediaDirectory = await mediaDirectoryPromise;
    return assetUrl(mediaDirectory, opaqueFilename);
  } catch {
    mediaDirectoryPromise = null;
    mediaDirectory = null;
    return "";
  }
}

/**
 * The same resolution without awaiting. Returns `""` until the media directory
 * has been learned once, which `primeMediaDirectory` does at startup.
 */
export function resolveMediaUrlSync(value: string | null | undefined): string {
  if (!value) {
    return "";
  }
  if (!value.startsWith(CACHE_PREFIX)) {
    return value;
  }
  const opaqueFilename = value.slice(CACHE_PREFIX.length);
  if (!mediaDirectory || !isTauriRuntime() || !isOpaqueFilename(opaqueFilename)) {
    return "";
  }
  return assetUrl(mediaDirectory, opaqueFilename);
}

/** Learn the media directory once, so the first library paint is already right. */
export async function primeMediaDirectory(): Promise<void> {
  if (!isTauriRuntime() || mediaDirectory) {
    return;
  }
  await resolveMediaUrl(`${CACHE_PREFIX}prime`);
}

function assetUrl(directory: string, opaqueFilename: string): string {
  const separator = directory.includes("\\") ? "\\" : "/";
  const suffix = directory.endsWith("/") || directory.endsWith("\\") ? "" : separator;
  return convertFileSrc(directory + suffix + opaqueFilename);
}

function isOpaqueFilename(value: string): boolean {
  return /^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(value);
}
