/**
 * A segmented control: two or three words in one pill, the chosen one lit.
 *
 * Every choice is a button, so the keys and a controller reach it like any
 * other control: the group is a `data-nav-row`, left and right walk its words,
 * up and down leave it, and coming back lands on the lit one (`aria-pressed`).
 * Moving along it only moves focus. Choosing takes a press — a click, Space,
 * Enter or A — because some choices, the language among them, reload the page.
 *
 * Labels come from `t()` and are markup, not data: nothing from a store or a
 * plugin may be passed in here.
 */

export interface SegmentOption {
  value: string;
  label: string;
  /** The option's own language, for a language name written in it. */
  lang?: string;
  /** What the option does, when the word alone leaves it open. */
  title?: string;
}

export function segmentedControl(
  preference: string,
  labelledBy: string,
  options: readonly SegmentOption[],
): string {
  const buttons = options
    .map((option) => {
      const lang = option.lang ? ` lang="${option.lang}"` : "";
      const title = option.title ? ` title="${option.title}"` : "";
      return `<button type="button" class="settings-segmented__option" data-segment-value="${option.value}" aria-pressed="false" tabindex="-1"${lang}${title}>${option.label}</button>`;
    })
    .join("");
  return `<div class="settings-segmented" role="group" aria-labelledby="${labelledBy}" data-segmented="${preference}" data-nav-row>${buttons}</div>`;
}

/**
 * Lights `value` and makes it the group's one Tab stop. A value the group does
 * not offer lights nothing and leaves the first word as the Tab stop, so the
 * group is never unreachable.
 */
export function syncSegmented(group: HTMLElement | null, value: string): void {
  if (!group) return;
  const options = Array.from(
    group.querySelectorAll<HTMLButtonElement>(".settings-segmented__option"),
  );
  const chosen = options.find((option) => option.dataset.segmentValue === value) ?? null;
  for (const option of options) {
    const on = option === chosen;
    option.setAttribute("aria-pressed", String(on));
    option.tabIndex = on || (!chosen && option === options[0]) ? 0 : -1;
  }
}

/** The preference and value a press on a segment asks for, if it was one. */
export function segmentChoice(
  target: EventTarget | null,
): { preference: string; value: string } | null {
  if (!(target instanceof Element)) return null;
  const option = target.closest<HTMLButtonElement>(".settings-segmented__option");
  const group = option?.closest<HTMLElement>("[data-segmented]");
  const preference = group?.dataset.segmented;
  const value = option?.dataset.segmentValue;
  if (!preference || value === undefined) return null;
  return { preference, value };
}
