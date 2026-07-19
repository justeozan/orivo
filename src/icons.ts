export type IconName =
  | "orivo"
  | "home"
  | "library"
  | "collections"
  | "store"
  | "settings"
  | "search"
  | "bell"
  | "clock"
  | "trophy"
  | "play"
  | "bookmark"
  | "chevron-left"
  | "chevron-right"
  | "chevron-down"
  | "grid"
  | "layout"
  | "navigate"
  | "more";

const paths: Record<IconName, string> = {
  orivo:
    '<path fill="currentColor" stroke="none" d="M 2 28 C 4 18 7 8 13 3 C 15 1 18 1 20 3 C 26 8 29 18 30 28 L 23 28 C 22 21 20 15 17 12 C 14 14 11 20 10 28 Z M 12 21 C 14 19 18 19 20 21 L 21 25 L 11 25 Z" />',
  home: '<path d="M3 10.5 12 3l9 7.5M5.5 9V20h13V9" />',
  library:
    '<path d="M4 5h6v14H4zM14 5h6v14h-6zM7 8h.01M17 8h.01" />',
  collections: '<rect x="4" y="4" width="16" height="16" rx="1" /><path d="M4 9h16M9 4v16" />',
  store:
    '<path d="M4 8h16l-1.5 12h-13zM8 8V6a4 4 0 0 1 8 0v2" />',
  settings:
    '<circle cx="12" cy="12" r="3.5" /><path d="M4 12h2M18 12h2M12 4v2M12 18v2M6.35 6.35l1.4 1.4M16.25 16.25l1.4 1.4M17.65 6.35l-1.4 1.4M7.75 16.25l-1.4 1.4" />',
  search: '<circle cx="10.5" cy="10.5" r="6.5" /><path d="m16 16 5 5" />',
  bell: '<path d="M18 9a6 6 0 0 0-12 0c0 7-3 8-3 10h18c0-2-3-3-3-10M10 22h4" />',
  clock: '<circle cx="12" cy="12" r="8" /><path d="M12 7v5l3 2" />',
  trophy:
    '<path d="M8 4h8v4c0 2.5-1.5 4.5-4 5-2.5-.5-4-2.5-4-5zM12 13v5M8 21h8M6 6H3v2c0 2 1.3 3.6 3.2 4M18 6h3v2c0 2-1.3 3.6-3.2 4" />',
  play: '<path fill="currentColor" stroke="none" d="m6 4 13 8-13 8z" />',
  bookmark: '<path d="M6 3h12v18l-6-4-6 4z" />',
  "chevron-left": '<path d="m15 4-7 8 7 8" />',
  "chevron-right": '<path d="m9 4 7 8-7 8" />',
  "chevron-down": '<path d="m6 9 6 6 6-6" />',
  grid: '<rect x="3" y="3" width="6" height="6" rx=".5" /><rect x="15" y="3" width="6" height="6" rx=".5" /><rect x="3" y="15" width="6" height="6" rx=".5" /><rect x="15" y="15" width="6" height="6" rx=".5" />',
  layout: '<rect x="3" y="3" width="6" height="6" rx=".5" /><rect x="15" y="3" width="6" height="6" rx=".5" /><rect x="3" y="15" width="6" height="6" rx=".5" /><rect x="15" y="15" width="6" height="6" rx=".5" />',
  navigate: '<path d="m12 3 7 9-7 9-7-9zM9 12h6" />',
  more: '<circle cx="5" cy="12" r="1.3" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1.3" fill="currentColor" stroke="none" /><circle cx="19" cy="12" r="1.3" fill="currentColor" stroke="none" />',
};

export function icon(name: IconName, className = "", label?: string): string {
  const title = label ? `<title>${label}</title>` : "";
  const viewBox = name === "orivo" ? "0 0 32 30" : "0 0 24 24";
  return `<svg class="icon ${className}" viewBox="${viewBox}" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="${label ? "false" : "true"}"${label ? ' role="img"' : ""}>${title}${paths[name]}</svg>`;
}
