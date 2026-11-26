import { describe, expect, it } from "vitest";
import { segmentChoice, segmentedControl, syncSegmented } from "./settings-segmented";

const mount = (): HTMLElement => {
  document.body.innerHTML = `<span id="motion-label">Animations</span>${segmentedControl(
    "motion",
    "motion-label",
    [
      { value: "full", label: "All" },
      { value: "system", label: "System" },
      { value: "reduced", label: "Reduced" },
    ],
  )}`;
  return document.querySelector<HTMLElement>("[data-segmented='motion']")!;
};

const options = (group: HTMLElement): HTMLButtonElement[] =>
  Array.from(group.querySelectorAll<HTMLButtonElement>("button"));

describe("segmented control", () => {
  it("is a band of buttons the keys and a controller can walk", () => {
    const group = mount();
    expect(group.getAttribute("role")).toBe("group");
    expect(group.getAttribute("aria-labelledby")).toBe("motion-label");
    expect(group.hasAttribute("data-nav-row")).toBe(true);
    expect(options(group).map((option) => option.type)).toEqual(["button", "button", "button"]);
  });

  it("lights the chosen value and makes it the one Tab stop", () => {
    const group = mount();
    syncSegmented(group, "system");
    expect(options(group).map((option) => option.getAttribute("aria-pressed"))).toEqual([
      "false",
      "true",
      "false",
    ]);
    expect(options(group).map((option) => option.tabIndex)).toEqual([-1, 0, -1]);

    syncSegmented(group, "full");
    expect(options(group).map((option) => option.getAttribute("aria-pressed"))).toEqual([
      "true",
      "false",
      "false",
    ]);
    expect(options(group).map((option) => option.tabIndex)).toEqual([0, -1, -1]);
  });

  it("stays reachable when the value is not one it offers", () => {
    const group = mount();
    syncSegmented(group, "fast");
    expect(options(group).every((option) => option.getAttribute("aria-pressed") === "false")).toBe(true);
    expect(options(group).map((option) => option.tabIndex)).toEqual([0, -1, -1]);
  });

  it("reads which preference a press asks for", () => {
    const group = mount();
    expect(segmentChoice(options(group)[2])).toEqual({ preference: "motion", value: "reduced" });
    expect(segmentChoice(group)).toBeNull();
    expect(segmentChoice(document.body)).toBeNull();
    expect(segmentChoice(null)).toBeNull();
  });
});
