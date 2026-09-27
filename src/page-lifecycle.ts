import type { AppRoute, PageRestoreState } from "./contracts";

export interface PageActivation {
  route: AppRoute;
  signal: AbortSignal;
  restoreState: PageRestoreState | null;
  isCurrent(): boolean;
}

export interface AppPage {
  mount(container: HTMLElement): void | Promise<void>;
  activate(activation: PageActivation): void | Promise<void>;
  deactivate(): PageRestoreState | null;
}

/**
 * A page, or the promise of one. The lazy form exists so a page that the first
 * screen never shows can live in its own chunk: the shell hands over a loader
 * instead of the page itself, and nothing downloads until someone navigates.
 */
export type AppPageSource = AppPage | (() => Promise<AppPage>);

export class PageLifecycleHost {
  readonly #container: HTMLElement;
  readonly #source: AppPageSource;
  #page: AppPage | null;
  #loading: Promise<AppPage> | null = null;
  #mounted = false;
  #generation = 0;
  #controller: AbortController | null = null;

  constructor(container: HTMLElement, source: AppPageSource) {
    this.#container = container;
    this.#source = source;
    this.#page = typeof source === "function" ? null : source;
    this.#container.hidden = true;
    this.#container.inert = true;
  }

  /**
   * Download a lazy page without showing it. Two calls share one download, so
   * an idle warm-up and a navigation that beats it never fetch twice.
   */
  load(): Promise<AppPage> {
    if (this.#page) return Promise.resolve(this.#page);
    this.#loading ??= (this.#source as () => Promise<AppPage>)().then(
      (page) => {
        this.#page = page;
        return page;
      },
      (error: unknown) => {
        // Never remember a failure. A chunk that did not arrive — the network
        // dropped, the shell was updated under a stale tab — has to be asked
        // for again, or one bad moment closes the page for the whole session.
        this.#loading = null;
        throw error;
      },
    );
    return this.#loading;
  }

  async activate(route: AppRoute, restoreState: PageRestoreState | null = null): Promise<void> {
    this.#controller?.abort();
    const controller = new AbortController();
    const generation = ++this.#generation;
    this.#controller = controller;
    // Resolving before mount() keeps the container hidden until the chunk and
    // its stylesheet have both landed, so a lazy page never paints unstyled.
    const page = this.#page ?? (await this.load());
    if (!this.#mounted) {
      await page.mount(this.#container);
      this.#mounted = true;
    }
    // A deactivate() that lands while mount() is pending — the beta gate walking
    // a deep link to #/me back to the Library, say — has to win: unhiding now
    // would stack a stale page over the one that replaced it.
    if (generation !== this.#generation) return;
    this.#container.hidden = false;
    this.#container.inert = false;
    await page.activate({
      route,
      signal: controller.signal,
      restoreState,
      isCurrent: () => !controller.signal.aborted && generation === this.#generation,
    });
  }

  deactivate(): PageRestoreState | null {
    this.#controller?.abort();
    this.#controller = null;
    this.#generation += 1;
    // Capture restore state while the page still has a layout box. `hidden`
    // collapses the container to `display: none`, so every scroll offset reads
    // 0, and `inert` blurs the active element, so every focus key reads null.
    const restoreState = this.#mounted && this.#page ? this.#page.deactivate() : null;
    this.#container.hidden = true;
    this.#container.inert = true;
    return restoreState;
  }
}
