import { describe, expect, it, vi } from "vitest";
import type { AppPage, PageActivation } from "./page-lifecycle";
import { PageLifecycleHost } from "./page-lifecycle";

const libraryRoute = { page: "library" } as const;

describe("PageLifecycleHost", () => {
  it("mounts once and makes inactive pages hidden and inert", async () => {
    const container = document.createElement("section");
    const mount = vi.fn();
    const activate = vi.fn();
    const deactivate = vi.fn(() => ({ scrollTop: 48, focusKey: "game:1" }));
    const page: AppPage = { mount, activate, deactivate };
    const host = new PageLifecycleHost(container, page);

    expect(container.hidden).toBe(true);
    expect(container.inert).toBe(true);
    await host.activate(libraryRoute);
    await host.activate(libraryRoute);

    expect(mount).toHaveBeenCalledTimes(1);
    expect(activate).toHaveBeenCalledTimes(2);
    expect(container.hidden).toBe(false);
    expect(container.inert).toBe(false);
    expect(host.deactivate()).toEqual({ scrollTop: 48, focusKey: "game:1" });
    expect(container.hidden).toBe(true);
    expect(container.inert).toBe(true);
  });

  it("captures restore state before the container loses its layout box", async () => {
    const container = document.createElement("section");
    // `hidden` is `boolean | "until-found"` in the DOM typings, so widen it
    // rather than coercing — the assertion below wants the exact value.
    const observed: Array<{ hidden: boolean | string; inert: boolean }> = [];
    const page: AppPage = {
      mount: () => undefined,
      activate: () => undefined,
      deactivate: () => {
        // A real page reads scrollTop and document.activeElement here. Both are
        // destroyed by `hidden` (display: none) and `inert` (focus fixup), so
        // the host must not have applied either yet.
        observed.push({ hidden: container.hidden, inert: container.inert });
        return { scrollTop: 120, focusKey: "card:7" };
      },
    };
    const host = new PageLifecycleHost(container, page);

    await host.activate(libraryRoute);
    expect(host.deactivate()).toEqual({ scrollTop: 120, focusKey: "card:7" });
    expect(observed).toEqual([{ hidden: false, inert: false }]);
    expect(container.hidden).toBe(true);
    expect(container.inert).toBe(true);
  });

  it("invalidates late asynchronous activations", async () => {
    const container = document.createElement("section");
    const activations: PageActivation[] = [];
    const page: AppPage = {
      mount: () => undefined,
      activate: (activation) => {
        activations.push(activation);
      },
      deactivate: () => null,
    };
    const host = new PageLifecycleHost(container, page);

    await host.activate(libraryRoute);
    const first = activations[0];
    expect(first.isCurrent()).toBe(true);
    await host.activate(libraryRoute);

    expect(first.signal.aborted).toBe(true);
    expect(first.isCurrent()).toBe(false);
    expect(activations[1].isCurrent()).toBe(true);
  });
});
