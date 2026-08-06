import type { PageRestoreState } from "./contracts";
import type { LibraryGame } from "./mock-library";
import { fallbackLibrary } from "./mock-library";
import { computeCognitiveProfile, type CognitiveProfile } from "./me-model";
import type { AppPage, PageActivation } from "./page-lifecycle";

export interface MePageOptions {
  /**
   * Jeux analysés par le scan cognitif. Par défaut la bibliothèque de
   * démonstration ; le shell peut injecter la bibliothèque réelle.
   */
  games?: LibraryGame[];
}

const SVG_NS = "http://www.w3.org/2000/svg";

/** Durée de la montée des compteurs à l'arrivée sur la page. */
const INTRO_MS = 1_150;

/**
 * Une valeur animée à l'arrivée : reçoit une progression 0→1 et peint l'état
 * correspondant. Chaque indicateur en enregistre une pendant le rendu, ce qui
 * garde la construction du DOM et l'animation dans le même endroit.
 */
type Tween = (progress: number) => void;

function element<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  className: string,
  text?: string,
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

function svg<K extends keyof SVGElementTagNameMap>(
  tag: K,
  attrs: Record<string, string> = {},
): SVGElementTagNameMap[K] {
  const node = document.createElementNS(SVG_NS, tag);
  for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, value);
  return node;
}

/** Décélération franche : rapide au départ, posée à l'arrivée. */
const easeOut = (t: number): number => 1 - Math.pow(1 - t, 3);

function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/** Longueur d'une polyligne, pour un tracé progressif sans mesurer le DOM. */
function polylineLength(points: ReadonlyArray<readonly [number, number]>): number {
  let total = 0;
  for (let index = 1; index < points.length; index += 1) {
    total += Math.hypot(points[index][0] - points[index - 1][0], points[index][1] - points[index - 1][1]);
  }
  return total;
}

/** Petits pictogrammes locaux : la page est autonome, rien dans icons.ts. */
const GLYPHS = {
  session:
    '<rect x="3" y="7" width="18" height="10" rx="5"/><circle cx="8.6" cy="12" r="1.4" fill="currentColor" stroke="none"/><circle cx="15.4" cy="12" r="1.4" fill="currentColor" stroke="none"/>',
  clock: '<circle cx="12" cy="12" r="8.4"/><path d="M12 7.6V12l3 2.2"/>',
  screen: '<rect x="3.6" y="5" width="16.8" height="11.4" rx="2"/><path d="M9 20h6"/>',
  pause: '<circle cx="12" cy="12" r="8.4"/><path d="M10 9v6M14 9v6"/>',
  info: '<circle cx="12" cy="12" r="8.4"/><path d="M12 11v5"/><circle cx="12" cy="8" r="1" fill="currentColor" stroke="none"/>',
  warning: '<path d="M12 4 21 19H3z" stroke-linejoin="round"/><path d="M12 10v4"/><circle cx="12" cy="16.6" r=".9" fill="currentColor" stroke="none"/>',
  chevronRight: '<path d="m10 7 5 5-5 5"/>',
  chevronDown: '<path d="m7 10 5 5 5-5"/>',
  timer: '<circle cx="12" cy="13" r="7.4"/><path d="M12 13V9.4M9.6 3.4h4.8"/>',
  runner:
    '<circle cx="14.6" cy="5" r="1.8"/><path d="m8.4 20 2.6-4.6 3-1.6-1.2-3.6-3.4 1.4-1.8 2.8M13 10.2l2.4 2 3 .8M12.8 13.8l1.6 3 3 2.4"/>',
  lotus:
    '<path d="M12 5.4c1.4 1.8 2 3.4 2 5 0 2.4-.9 4-2 4.8-1.1-.8-2-2.4-2-4.8 0-1.6.6-3.2 2-5Z"/><path d="M4.6 11.2c2.3.2 4 .9 5.2 2 1.4 1.3 1.9 2.9 1.8 4.4-2.3-.2-4.2-.9-5.4-2.2-1-1-1.5-2.5-1.6-4.2ZM19.4 11.2c-2.3.2-4 .9-5.2 2-1.4 1.3-1.9 2.9-1.8 4.4 2.3-.2 4.2-.9 5.4-2.2 1-1 1.5-2.5 1.6-4.2Z"/>',
  target: '<circle cx="12" cy="12" r="8.4"/><circle cx="12" cy="12" r="4.6"/><circle cx="12" cy="12" r="1.2" fill="currentColor" stroke="none"/>',
  brain:
    '<path d="M9.4 4.8a2.6 2.6 0 0 0-2.7 2.5 2.7 2.7 0 0 0-1.9 4.4 2.7 2.7 0 0 0 .8 4.6 2.7 2.7 0 0 0 4.4 2c.5.5 1.2.8 2 .8V4.9a2.6 2.6 0 0 0-2.6-.1ZM14.6 4.8a2.6 2.6 0 0 1 2.7 2.5 2.7 2.7 0 0 1 1.9 4.4 2.7 2.7 0 0 1-.8 4.6 2.7 2.7 0 0 1-4.4 2c-.5.5-1.2.8-2 .8V4.9a2.6 2.6 0 0 1 2.6-.1Z"/>',
} as const;

function glyph(name: keyof typeof GLYPHS, className = ""): HTMLElement {
  const wrap = element("span", `me-glyph ${className}`.trim());
  wrap.setAttribute("aria-hidden", "true");
  wrap.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round">${GLYPHS[name]}</svg>`;
  return wrap;
}

/* ------------------------------------------------------------------ modèle */

/** Un axe du schéma cognitif, dérivé d'une métrique du scan (score /10). */
interface CognitiveAxis {
  id: string;
  label: string;
  value: number;
  description: string;
}

/** Couleurs partagées entre le radar, l'évolution et la légende. */
const AXIS_COLORS: Record<string, string> = {
  focus: "#8a5cff",
  reflexes: "#4aa8ff",
  reflexion: "#ffb43f",
  memoire: "#ef6bcb",
  creativite: "#43d98c",
};

/**
 * Les taches du fond, relevées sur le fond isolé de la maquette. Le cluster
 * cyan du centre-haut domine ; les teintes froides et le violet des bords en
 * découlent. Ordre = ordre d'empilement (la composition alpha n'est pas
 * commutative).
 */
const AURORA_BLOBS: ReadonlyArray<{ x: number; y: number; s: number; c: string; a: number }> = [
  { x: 46.9, y: 23.4, s: 33.6, c: "62, 157, 254", a: 0.356 },
  { x: 45.8, y: 0.8, s: 30.4, c: "120, 172, 255", a: 0.288 },
  { x: 53.6, y: 39.8, s: 32.8, c: "62, 146, 255", a: 0.335 },
  { x: 66.7, y: 28.9, s: 27.3, c: "77, 129, 255", a: 0.225 },
  { x: 64.6, y: 48.4, s: 50.9, c: "86, 143, 255", a: 0.186 },
  { x: 26.6, y: 32, s: 9.8, c: "172, 113, 255", a: 0.144 },
  { x: 12.5, y: 96.1, s: 30.4, c: "36, 117, 255", a: 0.165 },
  { x: 0, y: 32, s: 14.8, c: "104, 81, 255", a: 0.153 },
  { x: 24, y: 0, s: 12.3, c: "103, 139, 254", a: 0.249 },
  { x: 0.5, y: 43.8, s: 13.1, c: "20, 42, 255", a: 0.208 },
  { x: 78.1, y: 84.4, s: 45.9, c: "72, 127, 255", a: 0.136 },
  { x: 35.9, y: 47.7, s: 27.3, c: "90, 135, 255", a: 0.119 },
  { x: 66.7, y: 77.3, s: 27.3, c: "98, 135, 255", a: 0.119 },
  { x: 1, y: 4.7, s: 11.5, c: "159, 186, 255", a: 0.085 },
  { x: 0.5, y: 84.4, s: 41, c: "242, 95, 255", a: 0.076 },
  { x: 79.2, y: 40.6, s: 27.3, c: "107, 110, 254", a: 0.089 },
  { x: 33.3, y: 60.2, s: 27.3, c: "155, 133, 255", a: 0.081 },
  { x: 0, y: 0, s: 17.2, c: "153, 200, 255", a: 0.068 },
  { x: 50, y: 78.9, s: 27.3, c: "118, 76, 254", a: 0.089 },
];

/** Les deux traînées obliques qui barrent le bas de la maquette. */
const AURORA_STREAKS: ReadonlyArray<{
  x: number; y: number; w: number; angle: number; c: string; a: number;
}> = [
  { x: 12, y: 83, w: 34, angle: -9, c: "226, 96, 236", a: 0.5 },
  { x: 88, y: 82, w: 30, angle: 13, c: "196, 108, 240", a: 0.44 },
];

/**
 * Le scan calcule cinq métriques (0-100) depuis la bibliothèque ; le design
 * les présente comme cinq capacités notées /10. Le mapping est fixe pour que
 * chaque axe garde une source réelle et déterministe.
 */
function buildAxes(profile: CognitiveProfile): CognitiveAxis[] {
  const score = (id: string): number => {
    const ids: Record<string, string> = {
      focus: "engagement",
      reflexes: "intensite",
      reflexion: "regularite",
      creativite: "diversite",
      memoire: "equilibre",
    };
    const metric = profile.metrics.find((entry) => entry.id === ids[id]);
    return Math.round((metric?.score ?? 0) / 10 * 10) / 10;
  };
  return [
    { id: "focus", label: "Focus", value: score("focus"), description: "Capacité à maintenir l'attention" },
    { id: "reflexion", label: "Réflexion", value: score("reflexion"), description: "Analyse et résolution de problèmes" },
    { id: "memoire", label: "Mémoire", value: score("memoire"), description: "Rétention et rappel d'informations" },
    { id: "creativite", label: "Créativité", value: score("creativite"), description: "Pensée originale et adaptabilité" },
    { id: "reflexes", label: "Réflexes", value: score("reflexes"), description: "Vitesse et précision d'exécution" },
  ];
}

function globalScore(axes: CognitiveAxis[]): number {
  if (axes.length === 0) return 0;
  const mean = axes.reduce((sum, axis) => sum + axis.value, 0) / axes.length;
  return Math.round(mean * 10) / 10;
}

function scoreLabel(value: number): string {
  if (value >= 8.5) return "Excellent";
  if (value >= 7) return "Très Bon";
  if (value >= 5) return "Bon";
  if (value >= 2.5) return "Modéré";
  return "Faible";
}

function formatPlayFr(totalSeconds: number): string {
  const minutes = Math.max(0, Math.round(totalSeconds / 60));
  const hours = Math.floor(minutes / 60);
  return `${hours}h ${String(minutes % 60).padStart(2, "0")}m`;
}

/** Valeur /10 → décimale française d'affichage ("7.6"). */
const tenth = (value: number): string => value.toFixed(1);

/* --------------------------------------------------------------- fabriques */

function arcPath(cx: number, cy: number, r: number, a0: number, a1: number): string {
  const rad = (deg: number): [number, number] => [
    cx + r * Math.cos((deg * Math.PI) / 180),
    cy + r * Math.sin((deg * Math.PI) / 180),
  ];
  const [x0, y0] = rad(a0);
  const [x1, y1] = rad(a1);
  const large = a1 - a0 > 180 ? 1 : 0;
  return `M ${x0.toFixed(2)} ${y0.toFixed(2)} A ${r} ${r} 0 ${large} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}

/** Jauge en fer à cheval (Charge Cognitive, Risque d'Addiction). */
function buildArcGauge(
  id: string,
  percent: number,
  stops: [string, string],
  tweens: Tween[],
): HTMLElement {
  const wrap = element("div", "me-arc");
  wrap.setAttribute("role", "meter");
  wrap.setAttribute("aria-valuemin", "0");
  wrap.setAttribute("aria-valuemax", "100");
  wrap.setAttribute("aria-valuenow", String(Math.round(percent)));
  const chart = svg("svg", { viewBox: "0 0 120 112", class: "me-arc__svg" });
  const defs = svg("defs");
  const grad = svg("linearGradient", { id: `me-grad-${id}`, x1: "0", y1: "1", x2: "1", y2: "0" });
  grad.append(
    svg("stop", { offset: "0%", "stop-color": stops[0] }),
    svg("stop", { offset: "100%", "stop-color": stops[1] }),
  );
  defs.append(grad);
  const start = 140;
  const span = 260;
  const track = svg("path", {
    d: arcPath(60, 58, 46, start, start + span),
    class: "me-arc__track",
  });
  const value = svg("path", { class: "me-arc__value", stroke: `url(#me-grad-${id})` });
  chart.append(defs, track, value);
  const figure = element("span", "me-arc__figure");
  const percentNode = element("strong", "me-arc__percent");
  figure.append(percentNode, element("span", "me-arc__unit", "%"));
  const bounds = element("span", "me-arc__bounds");
  bounds.append(element("span", "", "0%"), element("span", "", "100%"));
  wrap.append(chart, figure, bounds);

  tweens.push((progress) => {
    const current = percent * progress;
    const clamped = Math.max(0.5, Math.min(100, current));
    value.setAttribute("d", arcPath(60, 58, 46, start, start + (span * clamped) / 100));
    percentNode.textContent = String(Math.round(current));
  });
  return wrap;
}

/** Anneau plein (Score Cognitif Global, Usure Cognitive). */
function buildRing(
  id: string,
  fraction: number,
  stops: string[],
  centre: HTMLElement,
  tweens: Tween[],
): HTMLElement {
  const wrap = element("div", "me-ring");
  const chart = svg("svg", { viewBox: "0 0 120 120", class: "me-ring__svg" });
  const defs = svg("defs");
  const grad = svg("linearGradient", { id: `me-grad-${id}`, x1: "0", y1: "0", x2: "1", y2: "1" });
  stops.forEach((color, index) => {
    grad.append(
      svg("stop", {
        offset: `${Math.round((index / Math.max(stops.length - 1, 1)) * 100)}%`,
        "stop-color": color,
      }),
    );
  });
  defs.append(grad);
  const radius = 50;
  const circumference = 2 * Math.PI * radius;
  const value = svg("circle", {
    cx: "60",
    cy: "60",
    r: String(radius),
    class: "me-ring__value",
    stroke: `url(#me-grad-${id})`,
    transform: "rotate(-90 60 60)",
  });
  chart.append(
    defs,
    svg("circle", { cx: "60", cy: "60", r: String(radius), class: "me-ring__track" }),
    value,
  );
  centre.classList.add("me-ring__centre");
  wrap.append(chart, centre);

  tweens.push((progress) => {
    const drawn = circumference * Math.max(0, Math.min(1, fraction * progress));
    value.setAttribute("stroke-dasharray", `${drawn.toFixed(1)} ${circumference.toFixed(1)}`);
  });
  return wrap;
}

/** Sparkline de tendance (Score Cognitif Global), tracée à l'arrivée. */
function buildSparkline(tweens: Tween[]): SVGSVGElement {
  const chart = svg("svg", { viewBox: "0 0 120 34", class: "me-spark" });
  const points: [number, number][] = [];
  for (let index = 0; index <= 12; index += 1) {
    const x = index * 10;
    points.push([x, 26 - index * 1.15 - Math.sin(index * 1.9) * 4.2]);
  }
  const line = svg("polyline", {
    points: points.map(([x, y]) => `${x},${y.toFixed(1)}`).join(" "),
    class: "me-spark__line",
  });
  chart.append(line);
  const length = polylineLength(points);
  line.setAttribute("stroke-dasharray", String(length.toFixed(1)));
  tweens.push((progress) => {
    line.setAttribute("stroke-dashoffset", (length * (1 - progress)).toFixed(1));
  });
  return chart;
}

/** Courbe d'énergie mentale : une onde douce, déterministe. */
function buildWave(tweens: Tween[]): SVGSVGElement {
  const chart = svg("svg", { viewBox: "0 0 220 64", class: "me-wave", preserveAspectRatio: "none" });
  const defs = svg("defs");
  const stroke = svg("linearGradient", { id: "me-grad-wave", x1: "0", y1: "0", x2: "1", y2: "0" });
  stroke.append(
    svg("stop", { offset: "0%", "stop-color": "#8a5cff" }),
    svg("stop", { offset: "100%", "stop-color": "#4aa8ff" }),
  );
  const fill = svg("linearGradient", { id: "me-grad-wave-fill", x1: "0", y1: "0", x2: "0", y2: "1" });
  fill.append(
    svg("stop", { offset: "0%", "stop-color": "rgba(124, 92, 255, 0.35)" }),
    svg("stop", { offset: "100%", "stop-color": "rgba(124, 92, 255, 0)" }),
  );
  defs.append(stroke, fill);
  const points: [number, number][] = [[0, 40]];
  for (let x = 4; x <= 220; x += 4) {
    points.push([x, 36 - 11 * Math.sin(x / 12) - 7 * Math.sin(x / 31 + 1.6)]);
  }
  const line = points.map(([x, y], index) => `${index === 0 ? "M" : "L"} ${x} ${y.toFixed(1)}`).join(" ");
  const area = svg("path", {
    d: `${line} L 220 64 L 0 64 Z`,
    class: "me-wave__area",
    fill: "url(#me-grad-wave-fill)",
  });
  const curve = svg("path", { d: line, class: "me-wave__line", stroke: "url(#me-grad-wave)" });
  chart.append(defs, area, curve);
  const length = polylineLength(points);
  curve.setAttribute("stroke-dasharray", String(length.toFixed(1)));
  tweens.push((progress) => {
    curve.setAttribute("stroke-dashoffset", (length * (1 - progress)).toFixed(1));
    area.setAttribute("opacity", progress.toFixed(3));
  });
  return chart;
}

/** Le pentagone du Schéma Cognitif, avec ses étiquettes positionnées autour. */
function buildRadar(axes: CognitiveAxis[], tweens: Tween[]): HTMLElement {
  const stage = element("div", "me-radar__stage");
  const size = 460;
  const cx = size / 2;
  const cy = 218;
  const radius = 150;
  const angleFor = (index: number): number => -90 + index * 72;
  const point = (index: number, r: number): [number, number] => {
    const angle = (angleFor(index) * Math.PI) / 180;
    return [cx + r * Math.cos(angle), cy + r * Math.sin(angle)];
  };

  const chart = svg("svg", { viewBox: `0 0 ${size} 436`, class: "me-radar__svg" });
  const defs = svg("defs");
  const fillGrad = svg("radialGradient", { id: "me-grad-radar", cx: "0.5", cy: "0.4", r: "0.75" });
  fillGrad.append(
    svg("stop", { offset: "0%", "stop-color": "rgba(148, 108, 255, 0.42)" }),
    svg("stop", { offset: "100%", "stop-color": "rgba(96, 78, 255, 0.12)" }),
  );
  defs.append(fillGrad);
  chart.append(defs);

  // Grille : pentagones imbriqués + rayons.
  for (const level of [0.25, 0.5, 0.75, 1]) {
    const ring = axes
      .map((_, index) => point(index, radius * level).map((v) => v.toFixed(1)).join(","))
      .join(" ");
    chart.append(svg("polygon", { points: ring, class: "me-radar__grid" }));
  }
  for (let index = 0; index < axes.length; index += 1) {
    const [x, y] = point(index, radius);
    chart.append(
      svg("line", {
        x1: String(cx), y1: String(cy), x2: x.toFixed(1), y2: y.toFixed(1),
        class: "me-radar__spoke",
      }),
    );
  }

  // Polygone des valeurs + sommets lumineux : il se déplie depuis le centre.
  const shape = svg("polygon", { class: "me-radar__shape", fill: "url(#me-grad-radar)" });
  chart.append(shape);
  const halos: SVGCircleElement[] = [];
  const dots: SVGCircleElement[] = [];
  for (let index = 0; index < axes.length; index += 1) {
    const halo = svg("circle", { r: "6.5", class: "me-radar__dot-halo" });
    const dot = svg("circle", { r: "3.4", class: "me-radar__dot" });
    halos.push(halo);
    dots.push(dot);
    chart.append(halo, dot);
  }

  // Le cerveau au centre.
  chart.append(svg("circle", { cx: String(cx), cy: String(cy), r: "34", class: "me-radar__core" }));
  stage.append(chart);

  const core = glyph("brain", "me-radar__brain");
  core.style.left = `${(cx / size) * 100}%`;
  core.style.top = `${(cy / 436) * 100}%`;
  stage.append(core);

  // Étiquettes HTML autour du pentagone (nom, note, description).
  const scoreNodes: HTMLElement[] = [];
  axes.forEach((axis, index) => {
    const [x, y] = point(index, radius + 22);
    const label = element("div", "me-radar__label");
    label.dataset.axis = axis.id;
    label.style.left = `${(x / size) * 100}%`;
    label.style.top = `${(y / 436) * 100}%`;
    const score = element("span", "me-radar__label-score");
    scoreNodes.push(score);
    label.append(
      element("strong", "me-radar__label-name", axis.label),
      score,
      element("p", "me-radar__label-copy", axis.description),
    );
    stage.append(label);
  });

  tweens.push((progress) => {
    const values = axes.map((axis, index) =>
      point(index, radius * Math.max(0.06, (axis.value / 10) * progress)),
    );
    shape.setAttribute(
      "points",
      values.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" "),
    );
    values.forEach(([x, y], index) => {
      halos[index].setAttribute("cx", x.toFixed(1));
      halos[index].setAttribute("cy", y.toFixed(1));
      dots[index].setAttribute("cx", x.toFixed(1));
      dots[index].setAttribute("cy", y.toFixed(1));
    });
    axes.forEach((axis, index) => {
      scoreNodes[index].textContent = tenth(axis.value * progress);
    });
  });
  return stage;
}

/** Graphe multi-lignes « Évolution Cognitive » : 30 jours simulés par axe. */
function buildEvolution(axes: CognitiveAxis[], tweens: Tween[]): SVGSVGElement {
  const chart = svg("svg", { viewBox: "0 0 560 168", class: "me-evolution__svg", preserveAspectRatio: "none" });
  const plotY = (value: number): number => 152 - (Math.max(0, Math.min(10, value)) / 10) * 138;
  for (const [row, gridValue] of [[10, "10"], [5, "5"], [0, "0"]] as const) {
    const y = plotY(row);
    chart.append(
      svg("line", { x1: "0", y1: y.toFixed(1), x2: "532", y2: y.toFixed(1), class: "me-evolution__grid" }),
    );
    const label = svg("text", { x: "544", y: (y + 4).toFixed(1), class: "me-evolution__axis" });
    label.textContent = gridValue;
    chart.append(label);
  }
  axes.forEach((axis, series) => {
    const points: [number, number][] = [];
    for (let day = 0; day < 15; day += 1) {
      const wobble =
        Math.sin(day * 0.9 + series * 1.7) * 1.15 + Math.sin(day * 0.37 + series * 0.8) * 0.7;
      points.push([8 + day * 37.4, plotY(axis.value + wobble)]);
    }
    const line = svg("polyline", {
      points: points.map(([x, y]) => `${x.toFixed(1)},${y.toFixed(1)}`).join(" "),
      class: "me-evolution__line",
      stroke: AXIS_COLORS[axis.id],
    });
    chart.append(line);
    const marks: SVGCircleElement[] = [];
    for (const [x, y] of points) {
      const mark = svg("circle", {
        cx: x.toFixed(1), cy: y.toFixed(1), r: "2.4",
        class: "me-evolution__point",
        fill: AXIS_COLORS[axis.id],
      });
      marks.push(mark);
      chart.append(mark);
    }
    // Chaque courbe se trace de gauche à droite, légèrement décalée de la
    // précédente, ce qui donne au bloc son entrée en cascade.
    const length = polylineLength(points);
    line.setAttribute("stroke-dasharray", String(length.toFixed(1)));
    const delay = series * 0.08;
    tweens.push((progress) => {
      const local = Math.max(0, Math.min(1, (progress - delay) / Math.max(0.001, 1 - delay)));
      line.setAttribute("stroke-dashoffset", (length * (1 - local)).toFixed(1));
      for (const [index, mark] of marks.entries()) {
        mark.setAttribute("opacity", index / marks.length <= local ? "1" : "0");
      }
    });
  });
  return chart;
}

/* ---------------------------------------------------------------- la page */

/**
 * Crée la page « Moi » (dashboard Performance Cognitive). Même contrat que
 * `createStorePage` : le shell injecte la navigation et importe `me-page.css` ;
 * la page ne possède ni topbar ni styles globaux, et le host du shell
 * (`.app-page--scroll`) reste le conteneur de défilement.
 */
export function createMePage(options: MePageOptions = {}): AppPage {
  const games = options.games ?? fallbackLibrary;
  let container: HTMLElement | null = null;
  let pageRoot: HTMLElement | null = null;
  let activation: PageActivation | null = null;
  /** Les indicateurs animés du rendu courant, rejoués à chaque arrivée. */
  let tweens: Tween[] = [];
  let introFrame: number | null = null;

  const isActive = (context = activation): context is PageActivation =>
    Boolean(context && context.isCurrent() && !context.signal.aborted);

  const readScrollTop = (): number => Math.max(pageRoot?.scrollTop ?? 0, container?.scrollTop ?? 0);

  const writeScrollTop = (value: number): void => {
    if (pageRoot) pageRoot.scrollTop = value;
    if (container) container.scrollTop = value;
  };

  const paint = (progress: number): void => {
    for (const tween of tweens) tween(progress);
  };

  const stopIntro = (): void => {
    if (introFrame !== null) {
      cancelAnimationFrame(introFrame);
      introFrame = null;
    }
  };

  /**
   * Rejoue la montée des compteurs : tout part de zéro et rejoint sa valeur.
   * Un utilisateur qui a demandé moins d'animations voit directement le
   * résultat final.
   */
  const playIntro = (): void => {
    stopIntro();
    if (tweens.length === 0) return;
    if (prefersReducedMotion() || typeof requestAnimationFrame !== "function") {
      paint(1);
      return;
    }
    // Peint l'état zéro dans la même tâche que le rendu : aucun clignotement
    // entre le DOM construit à sa valeur finale et le début de l'animation.
    paint(0);
    const started = performance.now();
    const step = (now: number): void => {
      const elapsed = Math.max(0, now - started);
      const linear = Math.min(1, elapsed / INTRO_MS);
      paint(easeOut(linear));
      introFrame = linear < 1 ? requestAnimationFrame(step) : null;
    };
    introFrame = requestAnimationFrame(step);
  };

  /**
   * Chaque carte est une cible de navigation. Sans cela la page « Moi » ne
   * contient aucun élément focusable — le dashboard n'a ni bouton ni lien — et
   * devient un cul-de-sac au clavier comme à la manette. La clé de focus est
   * dérivée de la classe de la carte, qui est déjà unique par carte, ce qui
   * donne au restaurateur d'état un sélecteur stable d'une visite à l'autre.
   */
  const card = (className: string, title?: string): HTMLElement => {
    const article = element("article", `me-card ${className}`.trim());
    article.tabIndex = 0;
    article.dataset.focusKey = className || "card";
    article.setAttribute("role", "group");
    if (title) article.append(element("h2", "me-card__title", title));
    return article;
  };

  /**
   * Les halos flous du fond, relevés un à un sur le fond isolé de la maquette
   * (`assets/moc-images/`) : position, diamètre, teinte et opacité de chaque
   * tache viennent d'une détection de maxima locaux multi-échelle sur cette
   * image, pas d'une approximation à l'œil. D'où leur nombre — c'est une
   * nébuleuse de petites taches, pas quelques grands halos.
   *
   * `x`/`y` sont le centre en % de la page, `s` le diamètre en % de la largeur.
   */
  const renderAurora = (): HTMLElement => {
    const layer = element("div", "me-aurora");
    layer.setAttribute("aria-hidden", "true");
    for (const [index, blob] of AURORA_BLOBS.entries()) {
      const node = element("span", "me-aurora__orb");
      // Seule la *donnée* relevée sur la maquette passe ici, sous forme de
      // variables. L'apparence (taille finale, opacité, flou, dégradé) se règle
      // dans me-page.css : une valeur peinte en dur ici gagnerait contre la
      // feuille de style et rendrait tout réglage à la main sans effet.
      node.style.setProperty("--bx", `${blob.x}%`);
      node.style.setProperty("--by", `${blob.y}%`);
      node.style.setProperty("--bs", String(blob.s));
      node.style.setProperty("--bc", blob.c);
      node.style.setProperty("--ba", String(blob.a));
      node.style.setProperty("--bdur", `${38 + ((index * 7) % 29)}s`);
      node.style.setProperty("--bdelay", `-${(index * 11) % 37}s`);
      // Vecteur de dérive propre à chaque tache. L'angle d'or répartit les
      // directions sans jamais en aligner deux, et la portée varie d'une tache
      // à l'autre : le fond respire au lieu de glisser d'un bloc.
      const heading = ((index * 137.5) % 360) * (Math.PI / 180);
      const reach = 3.4 + ((index * 3) % 5);
      node.style.setProperty("--dx", (Math.cos(heading) * reach).toFixed(2));
      node.style.setProperty("--dy", (Math.sin(heading) * reach).toFixed(2));
      layer.append(node);
    }
    // Les deux traînées lumineuses qui barrent le bas de la maquette.
    for (const streak of AURORA_STREAKS) {
      const node = element("span", "me-aurora__streak");
      node.style.setProperty("--bx", `${streak.x}%`);
      node.style.setProperty("--by", `${streak.y}%`);
      node.style.setProperty("--bs", String(streak.w));
      node.style.setProperty("--bc", streak.c);
      node.style.setProperty("--ba", String(streak.a));
      node.style.setProperty("--bangle", `${streak.angle}deg`);
      layer.append(node);
    }
    return layer;
  };

  const renderHeader = (): HTMLElement => {
    const header = element("header", "me-head");
    header.append(
      element("h1", "me-head__title", "Performance Cognitive"),
      element("p", "me-head__sub", "Suivi de vos capacités et bien-être mental"),
    );
    const select = element("button", "me-head__select");
    select.type = "button";
    select.append(element("span", "", "Vue d'ensemble"), glyph("chevronDown"));
    header.append(select);
    return header;
  };

  const renderScoreCard = (axes: CognitiveAxis[]): HTMLElement => {
    const section = card("me-score", "Score Cognitif Global");
    const total = globalScore(axes);
    const centre = element("span", "");
    const value = element("strong", "me-score__value");
    centre.append(value, element("span", "me-score__scale", "/10"));
    const row = element("div", "me-score__row");
    row.append(buildRing("score", total / 10, ["#8a5cff", "#4aa8ff", "#ef6bcb"], centre, tweens));
    const copy = element("div", "me-score__copy");
    copy.append(
      element("strong", "me-score__band", scoreLabel(total)),
      element("span", "me-score__delta", "+0.6 ce mois-ci"),
      buildSparkline(tweens),
    );
    row.append(copy);
    section.append(row);
    tweens.push((progress) => {
      value.textContent = tenth(total * progress);
    });
    return section;
  };

  const renderDailyCard = (profile: CognitiveProfile): HTMLElement => {
    const section = card("me-daily", "Résumé Quotidien");
    const list = element("ul", "me-daily__list");
    const row = (
      name: keyof typeof GLYPHS,
      tint: string,
      label: string,
      format: (progress: number) => string,
    ): HTMLElement => {
      const item = element("li", "me-daily__row");
      const mark = glyph(name, "me-daily__glyph");
      mark.style.color = tint;
      const value = element("strong", "me-daily__value");
      item.append(mark, element("span", "me-daily__label", label), value);
      tweens.push((progress) => {
        value.textContent = format(progress);
      });
      return item;
    };
    list.append(
      row("session", "#8a5cff", "Sessions", (p) => String(Math.round(2 * p))),
      row("clock", "#4aa8ff", "Temps de jeu", (p) => formatPlayFr(profile.totalPlayTimeSeconds * p)),
      row("screen", "#43d98c", "Jeux joués", (p) => String(Math.round(profile.gameCount * p))),
      row("pause", "#ef6bcb", "Pause moyenne", (p) => `${Math.round(45 * p)}m`),
    );
    section.append(list);
    return section;
  };

  const renderEnergyCard = (): HTMLElement => {
    const section = card("me-energy");
    const head = element("div", "me-card__titlerow");
    head.append(element("h2", "me-card__title", "Énergie Mentale"), glyph("info", "me-card__hint"));
    section.append(head);
    const row = element("div", "me-energy__row");
    row.append(buildWave(tweens));
    const copy = element("div", "me-energy__copy");
    const figure = element("span", "me-energy__figure");
    const value = element("strong", "", "");
    figure.append(value, element("span", "", "%"));
    copy.append(figure, element("span", "me-energy__band", "Élevée"), element("span", "me-energy__delta", "+12% vs hier"));
    row.append(copy);
    section.append(row);
    tweens.push((progress) => {
      value.textContent = String(Math.round(72 * progress));
    });
    return section;
  };

  const renderRadarSection = (axes: CognitiveAxis[]): HTMLElement => {
    const section = element("section", "me-schema");
    section.setAttribute("aria-label", "Schéma cognitif");
    const head = element("div", "me-schema__head");
    head.append(
      element("h2", "me-schema__title", "Schéma Cognitif"),
      element("p", "me-schema__sub", "Vos capacités mentales clés"),
    );
    section.append(head, buildRadar(axes, tweens));
    const detail = element("button", "me-schema__detail");
    detail.type = "button";
    detail.append(element("span", "", "Voir le détail par capacité"), glyph("chevronDown"));
    section.append(detail);
    return section;
  };

  const renderChargeCard = (percent: number): HTMLElement => {
    const section = card("me-charge", "Charge Cognitive");
    const row = element("div", "me-charge__row");
    row.append(buildArcGauge("charge", percent, ["#43e0ff", "#3f7bff"], tweens));
    const copy = element("div", "me-charge__copy");
    copy.append(
      element("strong", "", percent >= 70 ? "Charge Élevée" : percent >= 35 ? "Charge Modérée" : "Charge Faible"),
      element("span", "", percent >= 70 ? "À surveiller" : "Bonne gestion"),
    );
    row.append(copy);
    section.append(row);
    return section;
  };

  const renderSaturationCard = (percent: number): HTMLElement => {
    const section = card("me-saturation", "Saturation Dopaminergique");
    const figure = element("div", "me-saturation__figure");
    const value = element("strong", "");
    const number = element("span", "me-saturation__number");
    value.append(number, element("span", "me-saturation__unit", "%"));
    figure.append(value, element("span", "me-saturation__band", percent >= 60 ? "Élevée" : "Modérée"));
    section.append(figure);
    const bar = element("div", "me-saturation__bar");
    bar.setAttribute("role", "meter");
    bar.setAttribute("aria-valuemin", "0");
    bar.setAttribute("aria-valuemax", "100");
    bar.setAttribute("aria-valuenow", String(Math.round(percent)));
    const marker = element("span", "me-saturation__marker");
    bar.append(marker);
    const bounds = element("div", "me-saturation__bounds");
    bounds.append(element("span", "", "0%"), element("span", "", "50%"), element("span", "", "100%"));
    section.append(bar, bounds);
    const notice = element("div", "me-saturation__notice");
    notice.append(
      glyph("warning", "me-saturation__warn"),
      (() => {
        const copy = element("div", "me-saturation__notice-copy");
        copy.append(
          element("strong", "", "Risque de surcharge dopaminergique"),
          element(
            "p",
            "",
            "Limitez les récompenses instantanées (jeux, réseaux sociaux, contenu court).",
          ),
        );
        return copy;
      })(),
    );
    section.append(notice);
    tweens.push((progress) => {
      const current = percent * progress;
      number.textContent = String(Math.round(current));
      marker.style.left = `${Math.max(2, Math.min(98, current))}%`;
    });
    return section;
  };

  const renderRiskCard = (percent: number): HTMLElement => {
    const section = card("me-risk", "Risque d'Addiction");
    const row = element("div", "me-risk__row");
    row.append(buildArcGauge("risk", percent, ["#ffd25e", "#ff8a3d"], tweens));
    const copy = element("div", "me-risk__copy");
    copy.append(
      element("strong", "", percent >= 60 ? "Risque Élevé" : percent >= 25 ? "Risque Modéré" : "Risque Faible"),
      element("span", "", percent >= 25 ? "Surveillance conseillée" : "Rien à signaler"),
    );
    row.append(copy);
    section.append(row);
    const signals = element("div", "me-risk__signals");
    signals.append(element("h3", "me-risk__signals-title", "Signes Détectés"));
    const list = element("ul", "me-risk__list");
    const item = (tint: string, label: string): HTMLElement => {
      const entry = element("li", "me-risk__item");
      const dot = element("span", "me-risk__dot");
      dot.style.background = tint;
      entry.append(dot, element("span", "", label));
      return entry;
    };
    list.append(
      item("#ffd25e", "Sessions prolongées fréquentes"),
      item("#4aa8ff", "Baisse de motivation hors-jeu"),
      item("#ef6bcb", "Recherche de dopamine rapide"),
    );
    signals.append(list);
    const more = element("button", "me-risk__more");
    more.type = "button";
    more.setAttribute("aria-label", "Voir tous les signes détectés");
    more.append(glyph("chevronRight"));
    signals.append(more);
    section.append(signals);
    return section;
  };

  const renderEvolutionCard = (axes: CognitiveAxis[]): HTMLElement => {
    const section = card("me-evolution");
    const head = element("div", "me-card__titlerow");
    head.append(
      element("h2", "me-card__title", "Évolution Cognitive"),
      element("span", "me-evolution__period", "(30 derniers jours)"),
    );
    section.append(head, buildEvolution(axes, tweens));
    const legend = element("ul", "me-evolution__legend");
    const order = ["focus", "reflexes", "reflexion", "memoire", "creativite"];
    for (const id of order) {
      const axis = axes.find((entry) => entry.id === id);
      if (!axis) continue;
      const item = element("li", "me-evolution__legend-item");
      const dot = element("span", "me-evolution__legend-dot");
      dot.style.background = AXIS_COLORS[id];
      item.append(dot, element("span", "", axis.label));
      legend.append(item);
    }
    section.append(legend);
    return section;
  };

  const renderWearCard = (percent: number): HTMLElement => {
    const section = card("me-wear", "Usure Cognitive");
    const row = element("div", "me-wear__row");
    const centre = element("span", "");
    const value = element("strong", "me-wear__value");
    centre.append(value, element("span", "me-wear__band", percent >= 55 ? "Usure Élevée" : "Usure Modérée"));
    row.append(buildRing("wear", percent / 100, ["#43d98c", "#ffd25e", "#ff8a3d"], centre, tweens));
    const causes = element("div", "me-wear__causes");
    causes.append(element("h3", "me-wear__causes-title", "Causes principales"));
    const list = element("ul", "me-wear__list");
    const item = (tint: string, label: string): HTMLElement => {
      const entry = element("li", "me-wear__item");
      const dot = element("span", "me-wear__dot");
      dot.style.background = tint;
      entry.append(dot, element("span", "", label));
      return entry;
    };
    list.append(
      item("#ffd25e", "Manque de sommeil"),
      item("#4aa8ff", "Sessions tardives"),
      item("#8a5cff", "Surcharge d'informations"),
    );
    causes.append(list);
    row.append(causes);
    section.append(row);
    tweens.push((progress) => {
      value.textContent = `${Math.round(percent * progress)}%`;
    });
    return section;
  };

  const renderRecommendationsCard = (): HTMLElement => {
    const section = card("me-reco", "Recommandations Personnalisées");
    const list = element("ul", "me-reco__list");
    const row = (name: keyof typeof GLYPHS, tint: string, label: string): HTMLElement => {
      const item = element("li", "me-reco__item");
      const button = element("button", "me-reco__button");
      button.type = "button";
      const mark = glyph(name, "me-reco__glyph");
      mark.style.color = tint;
      button.append(mark, element("span", "me-reco__label", label), glyph("chevronRight", "me-reco__chevron"));
      item.append(button);
      return item;
    };
    list.append(
      row("timer", "#4aa8ff", "Prendre une pause de 15 min"),
      row("runner", "#ef6bcb", "Activité physique recommandée"),
      row("lotus", "#43d98c", "Pratique de la méditation"),
    );
    section.append(list);
    return section;
  };

  const renderObjective = (): HTMLElement => {
    const footer = element("footer", "me-objective");
    const pill = element("div", "me-objective__pill");
    pill.append(
      glyph("target", "me-objective__glyph"),
      element("p", "me-objective__copy", "Prochain objectif : Améliorer la mémoire à 7.5+"),
    );
    const progressRow = element("div", "me-objective__progress");
    const track = element("span", "me-objective__track");
    const fill = element("span", "me-objective__fill");
    track.append(fill);
    const hint = element("span", "me-objective__hint");
    const percent = element("strong", "me-objective__percent");
    progressRow.append(hint, track, percent);
    pill.append(progressRow);
    footer.append(pill);
    tweens.push((progress) => {
      const current = Math.round(68 * progress);
      fill.style.width = `${68 * progress}%`;
      hint.textContent = `${current}%`;
      percent.textContent = `${current}%`;
    });
    return footer;
  };

  const render = (): void => {
    if (!pageRoot) return;
    stopIntro();
    tweens = [];
    const scrollTop = readScrollTop();
    const profile = computeCognitiveProfile(games);
    const axes = buildAxes(profile);
    // Indicateurs dérivés du même scan, chacun avec une lecture stable :
    // la charge suit l'intensité, la saturation la part de jeux « courts »,
    // le risque combine volume et déséquilibre, l'usure l'inverse de l'équilibre.
    const metric = (id: string): number =>
      profile.metrics.find((entry) => entry.id === id)?.score ?? 0;
    const charge = Math.round(metric("intensite"));
    const saturation = Math.round(
      Math.max(profile.shortPlayShare * 100, metric("engagement") * 0.6),
    );
    const risk = Math.round(
      Math.max(0, Math.min(100, metric("engagement") * 0.45 + (100 - metric("equilibre")) * 0.25)),
    );
    const wear = Math.round(Math.max(0, Math.min(100, (100 - metric("equilibre")) * 0.5)));

    const fragment = document.createDocumentFragment();
    fragment.append(renderAurora());
    const grid = element("div", "me-grid");

    const left = element("div", "me-col me-col--left");
    left.append(renderHeader(), renderScoreCard(axes), renderDailyCard(profile), renderEnergyCard());

    const centre = element("div", "me-col me-col--centre");
    centre.append(renderRadarSection(axes));

    const right = element("div", "me-col me-col--right");
    right.append(renderChargeCard(charge), renderSaturationCard(saturation), renderRiskCard(risk));

    grid.append(left, centre, right);

    const bottom = element("div", "me-bottom");
    bottom.append(renderEvolutionCard(axes), renderWearCard(wear), renderRecommendationsCard());

    fragment.append(grid, bottom, renderObjective());
    pageRoot.replaceChildren(fragment);
    // Le DOM sort du rendu à sa valeur finale ; `playIntro` le ramène à zéro
    // avant la première peinture quand la page devient visible.
    paint(1);
    if (scrollTop > 0) writeScrollTop(scrollTop);
  };

  /**
   * Clé de focus courante : uniquement si le focus est réellement dans la page
   * (sinon on renverrait la clé d'un élément d'une autre page).
   */
  const readFocusKey = (): string | null => {
    if (!pageRoot) return null;
    const active = document.activeElement;
    if (!(active instanceof HTMLElement) || !pageRoot.contains(active)) return null;
    return active.dataset.focusKey ?? null;
  };

  const restorePageState = (restoreState: PageRestoreState | null): void => {
    if (!pageRoot || !restoreState) return;
    writeScrollTop(Math.max(0, restoreState.scrollTop));
    const { focusKey } = restoreState;
    if (!focusKey) return;
    const target = pageRoot.querySelector<HTMLElement>(
      `[data-focus-key="${CSS.escape(focusKey)}"]`,
    );
    target?.focus({ preventScroll: true });
  };

  return {
    mount(host) {
      container = host;
      // Exactly one `main` per screen: the shell wrapper is a plain `div`, so
      // this page root is that landmark (same rule as the Store page).
      pageRoot = element("main", "me-page");
      pageRoot.tabIndex = -1;
      pageRoot.setAttribute("aria-label", "Moi");
      container.replaceChildren(pageRoot);
      render();
    },
    activate(context) {
      activation = context;
      if (context.route.page !== "me") return;
      render();
      playIntro();
      requestAnimationFrame(() => {
        if (isActive(context)) restorePageState(context.restoreState);
      });
    },
    deactivate() {
      stopIntro();
      const restoreState: PageRestoreState = {
        scrollTop: readScrollTop(),
        focusKey: readFocusKey(),
      };
      activation = null;
      return restoreState;
    },
  };
}
