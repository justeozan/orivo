import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { LibraryGame } from "./mock-library";
import { createMePage } from "./me-page";
import { PageLifecycleHost } from "./page-lifecycle";

const flush = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 0));

/**
 * Une bibliothèque volontairement chargée : chaque indicateur du dashboard a
 * ainsi une valeur finale non nulle, ce qui rend la montée depuis zéro
 * observable.
 */
const game = (overrides: Partial<LibraryGame> & { id: string }): LibraryGame => ({
  title: "Alpha",
  source: "local",
  description: "",
  metadata: "",
  genre: "Action",
  heroUrl: "/hero.jpg",
  coverUrl: "/cover.jpg",
  landscapeUrl: "/landscape.jpg",
  lastPlayedAt: "2 days ago",
  playTimeSeconds: 432_000,
  launchable: true,
  ...overrides,
});

const games: LibraryGame[] = [
  game({ id: "local:1" }),
  game({ id: "local:2", title: "Beta", genre: "Puzzle", playTimeSeconds: 144_000, lastPlayedAt: "5 days ago" }),
];

describe("me page", () => {
  let container: HTMLElement;
  let host: PageLifecycleHost | null = null;

  beforeEach(() => {
    document.body.replaceChildren();
    container = document.createElement("div");
    document.body.append(container);
    // jsdom n'expose pas `matchMedia` ; la page l'interroge pour le respect de
    // « moins d'animations ».
    window.matchMedia ??= ((query: string) => ({
      matches: false,
      media: query,
      addEventListener() {},
      removeEventListener() {},
    })) as unknown as typeof window.matchMedia;
  });

  afterEach(() => {
    host?.deactivate();
    host = null;
    vi.restoreAllMocks();
  });

  const mount = async (): Promise<void> => {
    const page = createMePage({ games });
    host = new PageLifecycleHost(container, page);
    await host.activate({ page: "me" });
    await flush();
  };

  it("paints the dashboard sections from the cognitive scan", async () => {
    await mount();
    expect(container.querySelector(".me-head__title")?.textContent).toBe("Performance Cognitive");
    // Les trois colonnes et la rangée basse sont toutes présentes.
    expect(container.querySelector(".me-score")).not.toBeNull();
    expect(container.querySelector(".me-daily")).not.toBeNull();
    expect(container.querySelector(".me-energy")).not.toBeNull();
    expect(container.querySelector(".me-radar__stage")).not.toBeNull();
    expect(container.querySelector(".me-charge")).not.toBeNull();
    expect(container.querySelector(".me-saturation")).not.toBeNull();
    expect(container.querySelector(".me-risk")).not.toBeNull();
    expect(container.querySelector(".me-evolution")).not.toBeNull();
    expect(container.querySelector(".me-wear")).not.toBeNull();
    expect(container.querySelector(".me-reco")).not.toBeNull();
    expect(container.querySelector(".me-objective")).not.toBeNull();
    // Un axe par capacité, chacun avec son étiquette.
    expect(container.querySelectorAll(".me-radar__label")).toHaveLength(5);
  });

  it("lays the blurred aurora behind the content without joining the grid", async () => {
    await mount();
    const aurora = container.querySelector<HTMLElement>(".me-aurora");
    expect(aurora).not.toBeNull();
    // Purement décoratif : jamais annoncé, jamais cliquable.
    expect(aurora?.getAttribute("aria-hidden")).toBe("true");
    // Une nébuleuse de petites taches relevées sur le fond isolé de la
    // maquette, plus les deux traînées obliques du bas.
    expect(container.querySelectorAll(".me-aurora__orb").length).toBeGreaterThan(12);
    expect(container.querySelectorAll(".me-aurora__streak")).toHaveLength(2);
    // Le script ne fournit que la donnée relevée de chaque tache, sous forme de
    // variables — et surtout aucune valeur d'apparence : une propriété peinte
    // en inline l'emporterait sur la feuille de style, et tout réglage dans
    // me-page.css resterait sans effet.
    const orb = container.querySelector<HTMLElement>(".me-aurora__orb")!;
    for (const variable of ["--bx", "--by", "--bs", "--bc", "--ba", "--dx", "--dy"]) {
      expect(orb.style.getPropertyValue(variable)).not.toBe("");
    }
    // Chaque tache dérive dans sa propre direction : deux vecteurs identiques
    // feraient glisser le fond d'un bloc au lieu de le faire respirer.
    const headings = [...container.querySelectorAll<HTMLElement>(".me-aurora__orb")].map(
      (node) => `${node.style.getPropertyValue("--dx")},${node.style.getPropertyValue("--dy")}`,
    );
    expect(new Set(headings).size).toBe(headings.length);
    for (const painted of ["left", "top", "width", "background", "filter", "transform"]) {
      expect(orb.style.getPropertyValue(painted)).toBe("");
    }
    // Le calque est un frère de la grille, pas un de ses enfants : il ne peut
    // donc pas décaler les colonnes.
    expect(aurora?.parentElement?.classList.contains("me-page")).toBe(true);
    expect(aurora?.querySelector(".me-grid")).toBeNull();
  });

  it("runs every counter up from zero to its scanned value", async () => {
    // Les frames sont pilotées à la main pour observer le début, le milieu et
    // la fin de la montée.
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    const now = vi.spyOn(performance, "now");
    now.mockReturnValue(0);

    await mount();

    const score = container.querySelector<HTMLElement>(".me-score__value");
    const chargePercent = container.querySelector<HTMLElement>(".me-charge .me-arc__percent");
    const games = container.querySelectorAll<HTMLElement>(".me-daily__value")[2];
    const objective = container.querySelector<HTMLElement>(".me-objective__percent");

    // Au premier instant, tout est à zéro.
    expect(score?.textContent).toBe("0.0");
    expect(chargePercent?.textContent).toBe("0");
    expect(games?.textContent).toBe("0");
    expect(objective?.textContent).toBe("0%");

    // Une frame précoce : les valeurs ont décollé sans être arrivées.
    now.mockReturnValue(100);
    frames.shift()?.(100);
    const midScore = Number(score?.textContent);
    expect(midScore).toBeGreaterThan(0);

    // Passée la durée, chaque compteur tient sa valeur finale. La file est
    // drainée : l'activation y met aussi la frame de restauration du scroll.
    now.mockReturnValue(5_000);
    while (frames.length > 0) frames.shift()?.(5_000);
    expect(Number(score?.textContent)).toBeGreaterThan(midScore);
    expect(Number(chargePercent?.textContent)).toBeGreaterThan(0);
    // Les deux jeux de la bibliothèque sont comptés.
    expect(games?.textContent).toBe("2");
    expect(objective?.textContent).toBe("68%");
  });

  it("shows final values immediately when reduced motion is requested", async () => {
    window.matchMedia = ((query: string) => ({
      matches: query.includes("reduced-motion"),
      media: query,
      addEventListener() {},
      removeEventListener() {},
    })) as unknown as typeof window.matchMedia;
    const raf = vi.fn();
    vi.stubGlobal("requestAnimationFrame", raf);

    await mount();

    // Aucune montée : la valeur finale est peinte d'emblée.
    expect(container.querySelector(".me-objective__percent")?.textContent).toBe("68%");
    expect(container.querySelectorAll(".me-daily__value")[2]?.textContent).toBe("2");
  });
});
