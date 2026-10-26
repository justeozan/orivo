import { knownGenre } from "./library-browse";
import { t } from "./i18n";
import type { LibraryGame } from "./mock-library";

/**
 * Modèle du « scan cognitif » de la page Moi.
 *
 * Tout est calculé localement, de façon pure et déterministe, à partir de la
 * bibliothèque (`LibraryGame[]`). Aucune donnée ne quitte l'application : les
 * métriques ne sont que des lectures quantitatives des habitudes de jeu.
 */

export type CognitiveMetricId =
  | "engagement"
  | "regularite"
  | "diversite"
  | "intensite"
  | "equilibre";

export interface CognitiveMetric {
  id: CognitiveMetricId;
  /** Libellé court, traduit, affiché sur la carte. */
  label: string;
  /** Score entier entre 0 et 100 inclus. */
  score: number;
  /** Description courte, traduite, dépendante du score. */
  description: string;
}

export interface CognitiveProfile {
  /** Toujours les cinq métriques, dans un ordre stable. */
  metrics: CognitiveMetric[];
  totalPlayTimeSeconds: number;
  gameCount: number;
  /** Genre cumulant le plus de temps de jeu, `null` si la bibliothèque est vide. */
  dominantGenre: string | null;
  /**
   * Part (0-1) du temps passé sur des jeux « courts » (≤ SHORT_GAME_HOURS h au
   * total). Conserve la direction du déséquilibre que le score seul ne dit pas.
   */
  shortPlayShare: number;
}

export type ScoreBand = "faible" | "modere" | "soutenu" | "eleve";

/** Volume horaire de référence : à partir de 400 h, l'engagement sature à 100. */
export const REFERENCE_HOURS = 400;
/** Fenêtre de récence : une session vieille de 60 jours ne compte plus. */
export const RECENCY_WINDOW_DAYS = 60;
/** Un jeu totalisant au plus 40 h est considéré comme un jeu « court ». */
export const SHORT_GAME_HOURS = 40;

const INTENSE_KEYWORDS = [
  "action",
  "fps",
  "shooter",
  "fighting",
  "combat",
  "roguelike",
  "roguelite",
  "versus",
  "arcade",
  "hack",
];

const CALM_KEYWORDS = [
  "puzzle",
  "relax",
  "casual",
  "simulation",
  "strategy",
  "stratégie",
  "co-op",
  "coop",
  "adventure",
  "aventure",
  "narrative",
  "story",
];

const clampScore = (value: number): number =>
  Math.max(0, Math.min(100, Math.round(value)));

export function scoreBand(score: number): ScoreBand {
  if (score < 25) return "faible";
  if (score < 50) return "modere";
  if (score < 75) return "soutenu";
  return "eleve";
}

/**
 * Convertit un libellé relatif (« 2 days ago », « 1 week ago », « today »…) en
 * nombre de jours. Retourne `null` quand le libellé n'est pas interprétable :
 * l'appelant décide alors d'ignorer la donnée plutôt que de l'inventer.
 */
export function parseLastPlayedDays(label: string): number | null {
  const normalized = label.trim().toLowerCase();
  if (normalized === "today" || normalized === "just now" || normalized === "now") return 0;
  if (normalized === "yesterday") return 1;
  const match = normalized.match(/^(\d+)\s+(hour|day|week|month|year)s?\s+ago$/);
  if (!match) return null;
  const amount = Number.parseInt(match[1], 10);
  switch (match[2]) {
    case "hour":
      return amount / 24;
    case "day":
      return amount;
    case "week":
      return amount * 7;
    case "month":
      return amount * 30;
    case "year":
      return amount * 365;
    default:
      return null;
  }
}

export function computeTotalPlayTimeSeconds(games: LibraryGame[]): number {
  return games.reduce((total, game) => total + Math.max(0, game.playTimeSeconds), 0);
}

/** Temps de jeu total rapporté à `REFERENCE_HOURS` heures. */
export function computeEngagementScore(games: LibraryGame[]): number {
  const hours = computeTotalPlayTimeSeconds(games) / 3_600;
  return clampScore((hours / REFERENCE_HOURS) * 100);
}

/**
 * Régularité : moyenne des poids de récence (1 pour une session du jour, 0 au
 * delà de `RECENCY_WINDOW_DAYS` jours). Les libellés non interprétables sont
 * ignorés ; sans aucune donnée exploitable, le score est 0.
 */
export function computeRegularityScore(games: LibraryGame[]): number {
  const weights: number[] = [];
  for (const game of games) {
    const days = parseLastPlayedDays(game.lastPlayedAt);
    if (days === null) continue;
    weights.push(Math.max(0, 1 - days / RECENCY_WINDOW_DAYS));
  }
  if (weights.length === 0) return 0;
  const mean = weights.reduce((sum, weight) => sum + weight, 0) / weights.length;
  return clampScore(mean * 100);
}

/**
 * Diversité : nombre de genres distincts rapporté à la taille de la
 * bibliothèque, plafonnée à 8 — au-delà, plus de jeux n'exige pas plus de
 * genres pour être éclectique.
 */
export function computeDiversityScore(games: LibraryGame[]): number {
  if (games.length === 0) return 0;
  const distinct = new Set(
    // `knownGenre` drops the backend's "Library" filler: an Epic or GOG library
    // has no genres at all, and counting the placeholder as one made a
    // single-genre collection look eclectic.
    games.map((game) => knownGenre(game.genre).toLowerCase()).filter((genre) => genre.length > 0),
  ).size;
  const expected = Math.max(1, Math.min(games.length, 8));
  return clampScore((distinct / expected) * 100);
}

function intensityWeight(genre: string): number {
  const normalized = genre.toLowerCase();
  if (INTENSE_KEYWORDS.some((keyword) => normalized.includes(keyword))) return 1;
  if (CALM_KEYWORDS.some((keyword) => normalized.includes(keyword))) return 0;
  return 0.5;
}

/**
 * Intensité : part du temps de jeu passée sur des genres nerveux (action,
 * roguelike…) contre des genres calmes (aventure, puzzle…). Les genres neutres
 * (RPG…) pèsent 0,5. Pondérée par le temps de jeu ; à défaut, moyenne simple.
 */
export function computeIntensityScore(games: LibraryGame[]): number {
  if (games.length === 0) return 0;
  const total = computeTotalPlayTimeSeconds(games);
  if (total === 0) {
    const mean = games.reduce((sum, game) => sum + intensityWeight(game.genre), 0) / games.length;
    return clampScore(mean * 100);
  }
  const weighted = games.reduce(
    (sum, game) => sum + intensityWeight(game.genre) * Math.max(0, game.playTimeSeconds),
    0,
  );
  return clampScore((weighted / total) * 100);
}

/** Part (0-1) du temps total passé sur des jeux « courts ». */
export function computeShortPlayShare(games: LibraryGame[]): number {
  const total = computeTotalPlayTimeSeconds(games);
  if (total === 0) return 0;
  const short = games.reduce(
    (sum, game) =>
      game.playTimeSeconds <= SHORT_GAME_HOURS * 3_600
        ? sum + Math.max(0, game.playTimeSeconds)
        : sum,
    0,
  );
  return short / total;
}

/**
 * Équilibre : 100 quand le temps se répartit à parts égales entre jeux courts
 * et longues traversées, 0 quand tout le temps part d'un seul côté.
 */
export function computeBalanceScore(games: LibraryGame[]): number {
  if (computeTotalPlayTimeSeconds(games) === 0) return 0;
  const shortShare = computeShortPlayShare(games);
  return clampScore((1 - Math.abs(shortShare - 0.5) * 2) * 100);
}

export function computeDominantGenre(games: LibraryGame[]): string | null {
  const totals = new Map<string, { label: string; seconds: number }>();
  for (const game of games) {
    const genre = knownGenre(game.genre);
    const key = genre.toLowerCase();
    if (!key) continue;
    const entry = totals.get(key) ?? { label: genre, seconds: 0 };
    entry.seconds += Math.max(0, game.playTimeSeconds);
    totals.set(key, entry);
  }
  let best: { label: string; seconds: number } | null = null;
  for (const entry of totals.values()) {
    if (!best || entry.seconds > best.seconds) best = entry;
  }
  return best?.label ?? null;
}

function engagementDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return t("me.desc.engagement.low");
    case "modere":
      return t("me.desc.engagement.moderate");
    case "soutenu":
      return t("me.desc.engagement.strong");
    default:
      return t("me.desc.engagement.high");
  }
}

function regularityDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return t("me.desc.regularity.low");
    case "modere":
      return t("me.desc.regularity.moderate");
    case "soutenu":
      return t("me.desc.regularity.strong");
    default:
      return t("me.desc.regularity.high");
  }
}

function diversityDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return t("me.desc.diversity.low");
    case "modere":
      return t("me.desc.diversity.moderate");
    case "soutenu":
      return t("me.desc.diversity.strong");
    default:
      return t("me.desc.diversity.high");
  }
}

function intensityDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return t("me.desc.intensity.low");
    case "modere":
      return t("me.desc.intensity.moderate");
    case "soutenu":
      return t("me.desc.intensity.strong");
    default:
      return t("me.desc.intensity.high");
  }
}

function balanceDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return t("me.desc.balance.low");
    case "modere":
      return t("me.desc.balance.moderate");
    case "soutenu":
      return t("me.desc.balance.strong");
    default:
      return t("me.desc.balance.high");
  }
}

/**
 * Calcule le profil cognitif complet. Fonction pure : ne modifie jamais la
 * liste reçue et retourne toujours les cinq métriques dans le même ordre.
 */
export function computeCognitiveProfile(games: LibraryGame[]): CognitiveProfile {
  const engagement = computeEngagementScore(games);
  const regularite = computeRegularityScore(games);
  const diversite = computeDiversityScore(games);
  const intensite = computeIntensityScore(games);
  const equilibre = computeBalanceScore(games);
  return {
    metrics: [
      {
        id: "engagement",
        label: t("me.metric.engagement.label"),
        score: engagement,
        description: engagementDescription(engagement),
      },
      {
        id: "regularite",
        label: t("me.metric.regularite.label"),
        score: regularite,
        description: regularityDescription(regularite),
      },
      {
        id: "diversite",
        label: t("me.metric.diversite.label"),
        score: diversite,
        description: diversityDescription(diversite),
      },
      {
        id: "intensite",
        label: t("me.metric.intensite.label"),
        score: intensite,
        description: intensityDescription(intensite),
      },
      {
        id: "equilibre",
        label: t("me.metric.equilibre.label"),
        score: equilibre,
        description: balanceDescription(equilibre),
      },
    ],
    totalPlayTimeSeconds: computeTotalPlayTimeSeconds(games),
    gameCount: games.length,
    dominantGenre: computeDominantGenre(games),
    shortPlayShare: computeShortPlayShare(games),
  };
}

function metricScore(profile: CognitiveProfile, id: CognitiveMetricId): number {
  return profile.metrics.find((metric) => metric.id === id)?.score ?? 0;
}

/**
 * Résumé textuel du profil, généré uniquement depuis les scores calculés.
 * Déterministe : mêmes scores, même phrase.
 */
export function describePlayerProfile(profile: CognitiveProfile): string {
  if (profile.gameCount === 0) return t("me.summary.empty");
  const hours = Math.round(profile.totalPlayTimeSeconds / 3_600);
  const engagement = metricScore(profile, "engagement");
  const regularite = metricScore(profile, "regularite");
  const diversite = metricScore(profile, "diversite");
  const intensite = metricScore(profile, "intensite");
  const equilibre = metricScore(profile, "equilibre");

  const engagementWord =
    scoreBand(engagement) === "faible"
      ? t("me.summary.engagement.low")
      : scoreBand(engagement) === "modere"
        ? t("me.summary.engagement.moderate")
        : scoreBand(engagement) === "soutenu"
          ? t("me.summary.engagement.strong")
          : t("me.summary.engagement.high");

  const rhythm =
    scoreBand(regularite) === "faible" || scoreBand(regularite) === "modere"
      ? t("me.summary.rhythm.waves")
      : t("me.summary.rhythm.steady");

  const taste = profile.dominantGenre
    ? scoreBand(diversite) === "faible" || scoreBand(diversite) === "modere"
      ? t("me.summary.taste.focused", { genre: profile.dominantGenre })
      : t("me.summary.taste.varied", { genre: profile.dominantGenre })
    : t("me.summary.taste.none");

  const pace =
    intensite >= 65
      ? t("me.summary.pace.action")
      : intensite <= 35
        ? t("me.summary.pace.calm")
        : t("me.summary.pace.mixed");

  const sessions =
    scoreBand(equilibre) === "eleve" || scoreBand(equilibre) === "soutenu"
      ? t("me.summary.sessions.balanced")
      : profile.shortPlayShare >= 0.5
        ? t("me.summary.sessions.short")
        : t("me.summary.sessions.long");

  const count = profile.gameCount;
  return t("me.summary.full", {
    games: count === 1 ? t("me.summary.games.one") : t("me.summary.games.other", { count }),
    hours: hours === 1 ? t("me.summary.hours.one") : t("me.summary.hours.other", { count: hours }),
    engagement: engagementWord,
    rhythm,
    taste,
    pace,
    sessions,
  });
}
