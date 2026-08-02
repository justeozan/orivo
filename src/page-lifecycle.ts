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

export class PageLifecycleHost {
  readonly #container: HTMLElement;
  readonly #page: AppPage;
  #mounted = false;
  #generation = 0;
  #controller: AbortController | null = null;

  constructor(container: HTMLElement, page: AppPage) {
    this.#container = container;
    this.#page = page;
    this.#container.hidden = true;
    this.#container.inert = true;
  }

  async activate(route: AppRoute, restoreState: PageRestoreState | null = null): Promise<void> {
    if (!this.#mounted) {
      await this.#page.mount(this.#container);
      this.#mounted = true;
    }
    this.#controller?.abort();
    const controller = new AbortController();
    const generation = ++this.#generation;
    this.#controller = controller;
    this.#container.hidden = false;
    this.#container.inert = false;
    await this.#page.activate({
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
    const restoreState = this.#mounted ? this.#page.deactivate() : null;
    this.#container.hidden = true;
    this.#container.inert = true;
    return restoreState;
  }
}
