import { afterEach, describe, expect, it } from "vitest";
import { currentLanguage, LANGUAGES, missingTranslations, resolveLanguage, setLanguage, t } from "./i18n";

describe("i18n", () => {
  const initial = currentLanguage();
  afterEach(() => setLanguage(initial));

  it("resolves a choice, then the system languages, then English", () => {
    expect(resolveLanguage("fr", ["en-US"])).toBe("fr");
    expect(resolveLanguage("system", ["de-DE", "fr-CA"])).toBe("fr");
    expect(resolveLanguage("system", ["ja-JP"])).toBe("en");
    expect(resolveLanguage(null, [])).toBe("en");
  });

  it("translates and falls back to English for a missing key", () => {
    setLanguage("fr");
    expect(t("settings.data.label")).toBe("Données");
    expect(document.documentElement.lang).toBe("fr");
    setLanguage("en");
    expect(t("settings.data.label")).toBe("Data");
  });

  it("translates every message into every language", () => {
    for (const language of LANGUAGES) expect(missingTranslations(language)).toEqual([]);
  });
});
