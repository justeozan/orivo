import type { PageRestoreState } from "./contracts";
import type { LibraryGame } from "./mock-library";
import { fallbackLibrary } from "./mock-library";
import {
  computeCognitiveProfile,
  describePlayerProfile,
  scoreBand,
  type CognitiveMetric,
  type CognitiveProfile,
} from "./me-model";
import type { AppPage, PageActivation } from "./page-lifecycle";

export interface MePageOptions {
  /**
   * Jeux analysés par le scan cognitif. Par défaut la bibliothèque de
   * démonstration ; le shell peut injecter la bibliothèque réelle.
   */
  games?: LibraryGame[];
}

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

function formatHoursFr(totalPlayTimeSeconds: number): string {
  const hours = Math.round(totalPlayTimeSeconds / 3_600);
  if (hours <= 0) return "Moins d'une heure";
  return `${hours} h`;
}

/**
 * Crée la page « Moi » (scan cognitif du joueur). Même contrat que
 * `createStorePage` : le shell injecte la navigation et importe `me-page.css` ;
 * la page ne possède ni topbar ni styles globaux, et le host du shell
 * (`.app-page--scroll`) reste le conteneur de défilement.
 */
export function createMePage(options: MePageOptions = {}): AppPage {
  const games = options.games ?? fallbackLibrary;
  let container: HTMLElement | null = null;
  let pageRoot: HTMLElement | null = null;
  let activation: PageActivation | null = null;

  const isActive = (context = activation): context is PageActivation =>
    Boolean(context && context.isCurrent() && !context.signal.aborted);

  /**
   * Comme sur la page Store, le host du shell est le vrai scroller ; les deux
   * nœuds sont lus et écrits pour rester correct dans un host qui ne défile pas.
   */
  const readScrollTop = (): number => Math.max(pageRoot?.scrollTop ?? 0, container?.scrollTop ?? 0);

  const writeScrollTop = (value: number): void => {
    if (pageRoot) pageRoot.scrollTop = value;
    if (container) container.scrollTop = value;
  };

  const renderHeader = (profile: CognitiveProfile): HTMLElement => {
    const header = element("header", "me-header");
    header.append(
      element("p", "me-header__eyebrow", "Scan cognitif"),
      element("h1", "me-header__title", "Moi"),
      element(
        "p",
        "me-header__lede",
        profile.gameCount === 0
          ? "Aucune donnée de jeu pour le moment. Le scan se remplira avec vos premières sessions."
          : "Une lecture quantitative de vos habitudes de jeu, calculée localement depuis votre bibliothèque.",
      ),
    );
    return header;
  };

  const renderMetricCard = (metric: CognitiveMetric): HTMLElement => {
    const card = element("article", "me-metric-card");
    card.dataset.metric = metric.id;
    card.dataset.band = scoreBand(metric.score);

    const head = element("div", "me-metric-card__head");
    const title = element("h3", "me-metric-card__label", metric.label);
    const score = element("p", "me-metric-card__score");
    score.append(
      element("strong", "me-metric-card__value", String(metric.score)),
      element("span", "me-metric-card__scale", "/100"),
    );
    head.append(title, score);

    const gauge = element("div", "me-gauge");
    gauge.setAttribute("role", "meter");
    gauge.setAttribute("aria-valuemin", "0");
    gauge.setAttribute("aria-valuemax", "100");
    gauge.setAttribute("aria-valuenow", String(metric.score));
    gauge.setAttribute("aria-label", `${metric.label} : ${metric.score} sur 100`);
    const fill = element("span", "me-gauge__fill");
    fill.style.width = `${metric.score}%`;
    // Étire le dégradé sur toute la piste : la couleur en bout de jauge
    // reflète ainsi le score (voir `background-size` dans me-page.css).
    fill.style.setProperty("--me-fill", String(Math.max(metric.score, 1) / 100));
    gauge.append(fill);

    card.append(head, gauge, element("p", "me-metric-card__description", metric.description));
    return card;
  };

  const renderMetrics = (profile: CognitiveProfile): HTMLElement => {
    const section = element("section", "me-metrics");
    section.setAttribute("aria-labelledby", "me-metrics-title");
    const title = element("h2", "me-section-title", "Métriques du scan");
    title.id = "me-metrics-title";
    const grid = element("div", "me-metrics__grid");
    for (const metric of profile.metrics) grid.append(renderMetricCard(metric));
    section.append(title, grid);
    return section;
  };

  const renderProfile = (profile: CognitiveProfile): HTMLElement => {
    const section = element("section", "me-profile");
    section.setAttribute("aria-labelledby", "me-profile-title");
    const title = element("h2", "me-section-title", "Profil de joueur");
    title.id = "me-profile-title";

    const stats = element("dl", "me-profile__stats");
    const stat = (label: string, value: string): HTMLElement => {
      const block = element("div", "me-profile__stat");
      block.append(
        element("dt", "me-profile__stat-label", label),
        element("dd", "me-profile__stat-value", value),
      );
      return block;
    };
    stats.append(
      stat("Jeux analysés", String(profile.gameCount)),
      stat("Temps de jeu", formatHoursFr(profile.totalPlayTimeSeconds)),
      stat("Genre dominant", profile.dominantGenre ?? "—"),
    );

    const summary = element("p", "me-profile__summary", describePlayerProfile(profile));
    section.append(title, stats, summary);
    return section;
  };

  const render = (): void => {
    if (!pageRoot) return;
    const scrollTop = readScrollTop();
    const profile = computeCognitiveProfile(games);
    const fragment = document.createDocumentFragment();
    fragment.append(renderHeader(profile), renderMetrics(profile), renderProfile(profile));
    pageRoot.replaceChildren(fragment);
    if (scrollTop > 0) writeScrollTop(scrollTop);
  };

  const restorePageState = (restoreState: PageRestoreState | null): void => {
    if (!pageRoot || !restoreState) return;
    writeScrollTop(Math.max(0, restoreState.scrollTop));
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
      requestAnimationFrame(() => {
        if (isActive(context)) restorePageState(context.restoreState);
      });
    },
    deactivate() {
      const restoreState: PageRestoreState = {
        scrollTop: readScrollTop(),
        focusKey: null,
      };
      activation = null;
      return restoreState;
    },
  };
}
