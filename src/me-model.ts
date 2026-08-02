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
  /** Libellé court en français, affiché sur la carte. */
  label: string;
  /** Score entier entre 0 et 100 inclus. */
  score: number;
  /** Description courte en français, dépendante du score. */
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
    games.map((game) => game.genre.trim().toLowerCase()).filter((genre) => genre.length > 0),
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
    const key = game.genre.trim().toLowerCase();
    if (!key) continue;
    const entry = totals.get(key) ?? { label: game.genre.trim(), seconds: 0 };
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
      return "Un volume de jeu léger, encore ponctuel.";
    case "modere":
      return "Une pratique installée, sans excès.";
    case "soutenu":
      return "Un investissement conséquent dans le jeu.";
    default:
      return "Un temps de jeu très élevé sur la durée.";
  }
}

function regularityDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return "Des sessions rares ou anciennes.";
    case "modere":
      return "Un rythme irrégulier, par vagues.";
    case "soutenu":
      return "Un rythme de jeu assez régulier.";
    default:
      return "Une pratique quasi quotidienne.";
  }
}

function diversityDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return "Fidèle à un seul univers de jeu.";
    case "modere":
      return "Quelques genres de prédilection.";
    case "soutenu":
      return "Une belle variété de genres explorés.";
    default:
      return "Un profil très éclectique.";
  }
}

function intensityDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return "Des expériences surtout calmes et contemplatives.";
    case "modere":
      return "Un penchant pour les jeux posés.";
    case "soutenu":
      return "Un mélange dosé d'action et de calme.";
    default:
      return "Une nette préférence pour l'action nerveuse.";
  }
}

function balanceDescription(score: number): string {
  switch (scoreBand(score)) {
    case "faible":
      return "Un format de session très marqué domine.";
    case "modere":
      return "Un format de session nettement favori.";
    case "soutenu":
      return "Un bon dosage entre formats de jeu.";
    default:
      return "Sessions courtes et longues bien équilibrées.";
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
        label: "Temps de jeu",
        score: engagement,
        description: engagementDescription(engagement),
      },
      {
        id: "regularite",
        label: "Régularité",
        score: regularite,
        description: regularityDescription(regularite),
      },
      {
        id: "diversite",
        label: "Diversité des genres",
        score: diversite,
        description: diversityDescription(diversite),
      },
      {
        id: "intensite",
        label: "Intensité",
        score: intensite,
        description: intensityDescription(intensite),
      },
      {
        id: "equilibre",
        label: "Équilibre des sessions",
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
  if (profile.gameCount === 0) {
    return "Aucune donnée de jeu disponible pour établir un profil. Lancez quelques parties et revenez faire le scan.";
  }
  const hours = Math.round(profile.totalPlayTimeSeconds / 3_600);
  const engagement = metricScore(profile, "engagement");
  const regularite = metricScore(profile, "regularite");
  const diversite = metricScore(profile, "diversite");
  const intensite = metricScore(profile, "intensite");
  const equilibre = metricScore(profile, "equilibre");

  const engagementWord =
    scoreBand(engagement) === "faible"
      ? "occasionnelle"
      : scoreBand(engagement) === "modere"
        ? "régulière"
        : scoreBand(engagement) === "soutenu"
          ? "soutenue"
          : "intensive";

  const rhythm =
    scoreBand(regularite) === "faible" || scoreBand(regularite) === "modere"
      ? "Vous jouez par vagues plutôt qu'à rythme fixe."
      : "Vous gardez un rythme de jeu régulier.";

  const taste = profile.dominantGenre
    ? scoreBand(diversite) === "faible" || scoreBand(diversite) === "modere"
      ? `Votre cœur va au ${profile.dominantGenre}, avec peu d'écarts.`
      : `Le ${profile.dominantGenre} domine, mais vous explorez volontiers d'autres genres.`
    : "Vos genres de prédilection restent à découvrir.";

  const pace =
    intensite >= 65
      ? "Vous recherchez l'action et la tension."
      : intensite <= 35
        ? "Vous privilégiez les expériences calmes."
        : "Vous alternez entre action et moments plus posés.";

  const sessions =
    scoreBand(equilibre) === "eleve" || scoreBand(equilibre) === "soutenu"
      ? "Vos sessions alternent bien formats courts et longues traversées."
      : profile.shortPlayShare >= 0.5
        ? "Vos sessions penchent nettement vers les jeux courts."
        : "Vos sessions penchent nettement vers les longues traversées.";

  return `Sur ${profile.gameCount} jeux et environ ${hours} heures, votre pratique est ${engagementWord}. ${rhythm} ${taste} ${pace} ${sessions}`;
}
