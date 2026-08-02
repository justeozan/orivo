import { describe, expect, it } from "vitest";
import type { LibraryGame } from "./mock-library";
import { fallbackLibrary } from "./mock-library";
import {
  computeBalanceScore,
  computeCognitiveProfile,
  computeDiversityScore,
  computeDominantGenre,
  computeEngagementScore,
  computeIntensityScore,
  computeRegularityScore,
  computeShortPlayShare,
  computeTotalPlayTimeSeconds,
  describePlayerProfile,
  parseLastPlayedDays,
  REFERENCE_HOURS,
  scoreBand,
} from "./me-model";

const HOUR = 3_600;

let nextId = 0;
function game(overrides: Partial<LibraryGame> = {}): LibraryGame {
  nextId += 1;
  return {
    id: `test-${nextId}`,
    title: `Jeu ${nextId}`,
    description: "Un jeu de test.",
    metadata: "Test",
    genre: "RPG",
    heroUrl: "",
    coverUrl: "",
    landscapeUrl: "",
    lastPlayedAt: "2 days ago",
    playTimeSeconds: 10 * HOUR,
    launchable: false,
    ...overrides,
  };
}

describe("parseLastPlayedDays", () => {
  it("interprète les libellés relatifs usuels", () => {
    expect(parseLastPlayedDays("2 days ago")).toBe(2);
    expect(parseLastPlayedDays("1 week ago")).toBe(7);
    expect(parseLastPlayedDays("3 weeks ago")).toBe(21);
    expect(parseLastPlayedDays("1 month ago")).toBe(30);
    expect(parseLastPlayedDays("2 months ago")).toBe(60);
    expect(parseLastPlayedDays("1 year ago")).toBe(365);
    expect(parseLastPlayedDays("12 hours ago")).toBe(0.5);
  });

  it("gère aujourd'hui, hier et l'instant présent", () => {
    expect(parseLastPlayedDays("today")).toBe(0);
    expect(parseLastPlayedDays("Just now")).toBe(0);
    expect(parseLastPlayedDays("yesterday")).toBe(1);
  });

  it("retourne null pour un libellé inconnu", () => {
    expect(parseLastPlayedDays("n'importe quoi")).toBeNull();
    expect(parseLastPlayedDays("")).toBeNull();
  });
});

describe("computeEngagementScore", () => {
  it("sature à 100 au-delà du volume de référence", () => {
    const games = [game({ playTimeSeconds: (REFERENCE_HOURS + 50) * HOUR })];
    expect(computeEngagementScore(games)).toBe(100);
  });

  it("est proportionnel au temps total en dessous de la référence", () => {
    const games = [game({ playTimeSeconds: 40 * HOUR })];
    expect(computeEngagementScore(games)).toBe(10);
  });

  it("vaut 0 sans temps de jeu", () => {
    expect(computeEngagementScore([])).toBe(0);
    expect(computeEngagementScore([game({ playTimeSeconds: 0 })])).toBe(0);
  });
});

describe("computeRegularityScore", () => {
  it("vaut 100 quand tout a été joué aujourd'hui", () => {
    const games = [game({ lastPlayedAt: "today" }), game({ lastPlayedAt: "today" })];
    expect(computeRegularityScore(games)).toBe(100);
  });

  it("vaut 0 pour des sessions au-delà de la fenêtre de récence", () => {
    const games = [game({ lastPlayedAt: "1 year ago" }), game({ lastPlayedAt: "3 months ago" })];
    expect(computeRegularityScore(games)).toBe(0);
  });

  it("ignore les libellés non interprétables et vaut 0 sans donnée exploitable", () => {
    expect(computeRegularityScore([game({ lastPlayedAt: "???" })])).toBe(0);
    const mixed = [game({ lastPlayedAt: "???" }), game({ lastPlayedAt: "today" })];
    expect(computeRegularityScore(mixed)).toBe(100);
  });
});

describe("computeDiversityScore", () => {
  it("est faible pour une bibliothèque mono-genre", () => {
    const games = [
      game({ genre: "RPG" }),
      game({ genre: "RPG" }),
      game({ genre: "rpg" }),
      game({ genre: "RPG " }),
    ];
    expect(computeDiversityScore(games)).toBe(25);
  });

  it("vaut 100 quand chaque jeu apporte un genre distinct", () => {
    const games = [
      game({ genre: "RPG" }),
      game({ genre: "Action" }),
      game({ genre: "Puzzle" }),
      game({ genre: "Aventure" }),
    ];
    expect(computeDiversityScore(games)).toBe(100);
  });

  it("vaut 0 pour une bibliothèque vide", () => {
    expect(computeDiversityScore([])).toBe(0);
  });
});

describe("computeIntensityScore", () => {
  it("vaut 100 pour une bibliothèque uniquement nerveuse", () => {
    const games = [game({ genre: "Action" }), game({ genre: "Roguelike" })];
    expect(computeIntensityScore(games)).toBe(100);
  });

  it("vaut 0 pour une bibliothèque uniquement calme", () => {
    const games = [game({ genre: "Puzzle" }), game({ genre: "Adventure" })];
    expect(computeIntensityScore(games)).toBe(0);
  });

  it("traite les genres neutres à mi-chemin", () => {
    expect(computeIntensityScore([game({ genre: "RPG" })])).toBe(50);
  });

  it("pondère par le temps de jeu", () => {
    const games = [
      game({ genre: "Action", playTimeSeconds: 3 * HOUR }),
      game({ genre: "Puzzle", playTimeSeconds: 1 * HOUR }),
    ];
    expect(computeIntensityScore(games)).toBe(75);
  });
});

describe("computeBalanceScore", () => {
  it("vaut 100 quand le temps se partage à égalité entre jeux courts et longs", () => {
    const games = [
      game({ playTimeSeconds: 30 * HOUR }),
      game({ playTimeSeconds: 30 * HOUR }),
      game({ playTimeSeconds: 60 * HOUR }),
    ];
    expect(computeShortPlayShare(games)).toBeCloseTo(0.5);
    expect(computeBalanceScore(games)).toBe(100);
  });

  it("baisse quand un format domine", () => {
    const games = [
      game({ playTimeSeconds: 20 * HOUR }),
      game({ playTimeSeconds: 60 * HOUR }),
    ];
    expect(computeBalanceScore(games)).toBe(50);
  });

  it("vaut 0 sans temps de jeu", () => {
    expect(computeBalanceScore([])).toBe(0);
    expect(computeBalanceScore([game({ playTimeSeconds: 0 })])).toBe(0);
  });
});

describe("computeDominantGenre", () => {
  it("retourne le genre cumulant le plus de temps", () => {
    const games = [
      game({ genre: "Action", playTimeSeconds: 10 * HOUR }),
      game({ genre: "RPG", playTimeSeconds: 8 * HOUR }),
      game({ genre: "RPG", playTimeSeconds: 8 * HOUR }),
    ];
    expect(computeDominantGenre(games)).toBe("RPG");
  });

  it("retourne null pour une bibliothèque vide", () => {
    expect(computeDominantGenre([])).toBeNull();
  });
});

describe("computeCognitiveProfile", () => {
  it("retourne toujours les cinq métriques dans un ordre stable", () => {
    const profile = computeCognitiveProfile(fallbackLibrary);
    expect(profile.metrics.map((metric) => metric.id)).toEqual([
      "engagement",
      "regularite",
      "diversite",
      "intensite",
      "equilibre",
    ]);
  });

  it("borne chaque score entre 0 et 100 avec un libellé et une description", () => {
    const profile = computeCognitiveProfile(fallbackLibrary);
    for (const metric of profile.metrics) {
      expect(Number.isInteger(metric.score)).toBe(true);
      expect(metric.score).toBeGreaterThanOrEqual(0);
      expect(metric.score).toBeLessThanOrEqual(100);
      expect(metric.label.length).toBeGreaterThan(0);
      expect(metric.description.length).toBeGreaterThan(0);
    }
  });

  it("agrège le temps total et le genre dominant", () => {
    const profile = computeCognitiveProfile(fallbackLibrary);
    expect(profile.totalPlayTimeSeconds).toBe(computeTotalPlayTimeSeconds(fallbackLibrary));
    expect(profile.gameCount).toBe(fallbackLibrary.length);
    expect(profile.dominantGenre).toBe("RPG");
  });

  it("gère une bibliothèque vide sans erreur", () => {
    const profile = computeCognitiveProfile([]);
    expect(profile.gameCount).toBe(0);
    expect(profile.totalPlayTimeSeconds).toBe(0);
    expect(profile.dominantGenre).toBeNull();
    expect(profile.shortPlayShare).toBe(0);
    for (const metric of profile.metrics) expect(metric.score).toBe(0);
  });

  it("ne modifie pas la liste reçue", () => {
    const games = [game(), game({ genre: "Action" })];
    const snapshot = JSON.stringify(games);
    computeCognitiveProfile(games);
    expect(JSON.stringify(games)).toBe(snapshot);
  });
});

describe("describePlayerProfile", () => {
  it("génère un résumé non vide mentionnant le volume de jeu", () => {
    const summary = describePlayerProfile(computeCognitiveProfile(fallbackLibrary));
    expect(summary.length).toBeGreaterThan(0);
    expect(summary).toContain("heures");
    expect(summary).toContain(String(fallbackLibrary.length));
  });

  it("est déterministe pour un même profil", () => {
    const profile = computeCognitiveProfile(fallbackLibrary);
    expect(describePlayerProfile(profile)).toBe(describePlayerProfile(profile));
  });

  it("explique l'absence de données pour une bibliothèque vide", () => {
    const summary = describePlayerProfile(computeCognitiveProfile([]));
    expect(summary).toContain("Aucune donnée");
  });
});

describe("scoreBand", () => {
  it("découpe l'échelle en quatre bandes", () => {
    expect(scoreBand(0)).toBe("faible");
    expect(scoreBand(24)).toBe("faible");
    expect(scoreBand(25)).toBe("modere");
    expect(scoreBand(49)).toBe("modere");
    expect(scoreBand(50)).toBe("soutenu");
    expect(scoreBand(74)).toBe("soutenu");
    expect(scoreBand(75)).toBe("eleve");
    expect(scoreBand(100)).toBe("eleve");
  });
});
