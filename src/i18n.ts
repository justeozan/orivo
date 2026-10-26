/**
 * Orivo's interface language.
 *
 * English is the source catalogue: every key exists there, and its shape is the
 * type every other language is checked against. Another language may leave a
 * key out — that string then falls back to English instead of showing a key.
 *
 * The shell builds most of its markup once, so the language is settled before
 * anything renders: the last choice is remembered locally (like the motion
 * preference) and read synchronously here, at import. Changing it reloads the
 * page — simpler and safer than re-rendering 8 000 lines of templates live.
 *
 * Messages live in `src/locales/<area>.ts`, one `en` and one `fr` export each,
 * merged below. Adding a language: add its code to `LANGUAGES`, its own name to
 * `LANGUAGE_NAMES`, a catalogue per area and to `CATALOGUES`, and the variant to the Rust
 * `Language` enum in `src-tauri/src/preferences.rs`. See docs/i18n.md.
 */

import * as core from "./locales/core";
import * as app from "./locales/app";
import * as detail from "./locales/detail";
import * as runners from "./locales/runners";
import * as store from "./locales/store";
import * as me from "./locales/me";
import * as misc from "./locales/misc";

const en = { ...core.en, ...app.en, ...detail.en, ...runners.en, ...store.en, ...me.en, ...misc.en };

export type MessageKey = keyof typeof en;
type Catalogue = Partial<Record<MessageKey, string>>;

const fr: Catalogue = { ...core.fr, ...app.fr, ...detail.fr, ...runners.fr, ...store.fr, ...me.fr, ...misc.fr };

export const LANGUAGES = ["en", "fr"] as const;
export type Language = (typeof LANGUAGES)[number];
/** What the user picks: a language, or whatever the system speaks. */
export const LANGUAGE_PREFERENCES = ["system", ...LANGUAGES] as const;
export type LanguagePreference = (typeof LANGUAGE_PREFERENCES)[number];

/** Each language named in itself, so anyone can find their own in the list. */
export const LANGUAGE_NAMES: Record<Language, string> = {
  en: "English",
  fr: "Français",
};

const CATALOGUES: Record<Language, Catalogue> = { en, fr };
const FALLBACK: Language = "en";
const MEMORY_KEY = "orivo.language";

type LanguageMemory = Pick<Storage, "getItem" | "setItem">;

function localMemory(): LanguageMemory | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function systemLanguages(): readonly string[] {
  if (typeof navigator === "undefined") return [];
  return navigator.languages?.length ? navigator.languages : [navigator.language];
}

/** "fr-CA" → "fr"; the first system language Orivo speaks wins. */
export function resolveLanguage(
  preference: string | null | undefined,
  system: readonly string[] = systemLanguages(),
): Language {
  if (isLanguage(preference)) return preference;
  for (const tag of system) {
    const base = tag?.toLowerCase().split("-")[0];
    if (isLanguage(base)) return base;
  }
  return FALLBACK;
}

export function isLanguage(value: unknown): value is Language {
  return typeof value === "string" && (LANGUAGES as readonly string[]).includes(value);
}

function readMemory(memory: LanguageMemory | null): string | null {
  try {
    return memory?.getItem(MEMORY_KEY) ?? null;
  } catch {
    return null;
  }
}

let active: Language = resolveLanguage(readMemory(localMemory()));

export function currentLanguage(): Language {
  return active;
}

/** Test seam and boot hook; the shell itself changes language by reloading. */
export function setLanguage(language: Language): void {
  active = language;
  if (typeof document !== "undefined") document.documentElement.lang = language;
}

/**
 * Remember the preference for the next start. Returns whether the language it
 * resolves to differs from the one on screen — the caller then reloads.
 */
export function rememberLanguagePreference(
  preference: LanguagePreference,
  memory: LanguageMemory | null = localMemory(),
): boolean {
  try {
    memory?.setItem(MEMORY_KEY, preference);
  } catch {
    // A refused store only costs the next start its language.
  }
  return resolveLanguage(preference) !== active;
}

/** `t("key", { name: "Hades" })` replaces `{name}` in the message. */
export function t(key: MessageKey, params?: Record<string, string | number>): string {
  const message = CATALOGUES[active][key] ?? en[key];
  if (!params) return message;
  return message.replace(/\{(\w+)\}/g, (match, name: string) =>
    name in params ? String(params[name]) : match,
  );
}

/** Keys a language still lacks (it shows English there). Kept at zero by a test. */
export function missingTranslations(language: Language): MessageKey[] {
  return (Object.keys(en) as MessageKey[]).filter((key) => CATALOGUES[language][key] === undefined);
}
