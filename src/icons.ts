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
  | "steam"
  | "download"
  | "close"
  | "refresh"
  | "folder"
  | "monitor"
  | "alert"
  | "star"
  | "thumbs-up"
  | "user"
  | "users"
  | "gamepad"
  | "cloud"
  | "windows"
  | "local"
  | "more"
  | "check";

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
  // Steam's official glyph, kept inline so source badges do not need a
  // network request or a raster asset at small sizes.
  steam:
    '<path fill="currentColor" stroke="none" d="M11.979 0C5.678 0 .511 4.86.022 11.037l6.432 2.658c.545-.371 1.203-.59 1.912-.59.063 0 .125.004.188.006l2.861-4.142V8.91c0-2.495 2.028-4.524 4.524-4.524 2.494 0 4.524 2.031 4.524 4.527s-2.03 4.525-4.524 4.525h-.105l-4.076 2.911c0 .052.004.105.004.159 0 1.875-1.515 3.396-3.39 3.396-1.635 0-3.016-1.173-3.331-2.727L.436 15.27C1.862 20.307 6.486 24 11.979 24c6.627 0 11.999-5.373 11.999-12S18.605 0 11.979 0zM7.54 18.21l-1.473-.61c.262.543.714.999 1.314 1.25 1.297.539 2.793-.076 3.332-1.375.263-.63.264-1.319.005-1.949s-.75-1.121-1.377-1.383c-.624-.26-1.29-.249-1.878-.03l1.523.63c.956.4 1.409 1.5 1.009 2.455-.397.957-1.497 1.41-2.454 1.012H7.54zm11.415-9.303c0-1.662-1.353-3.015-3.015-3.015-1.665 0-3.015 1.353-3.015 3.015 0 1.665 1.35 3.015 3.015 3.015 1.663 0 3.015-1.35 3.015-3.015zm-5.273-.005c0-1.252 1.013-2.266 2.265-2.266 1.249 0 2.266 1.014 2.266 2.266 0 1.251-1.017 2.265-2.266 2.265-1.253 0-2.265-1.014-2.265-2.265z" />',
  download: '<path d="M12 3v11M8 10l4 4 4-4M5 18v2h14v-2" />',
  close: '<path d="m6 6 12 12M18 6 6 18" />',
  refresh: '<path d="M20 11a8 8 0 1 0 1.2 4.2M20 5v6h-6" />',
  folder: '<path d="M3.5 6.5h6l2 2h9v9.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z" />',
  monitor: '<rect x="3" y="4" width="18" height="13" rx="1" /><path d="M8 21h8M12 17v4" />',
  alert: '<circle cx="12" cy="12" r="8.5" /><path d="M12 7.5v5M12 16h.01" />',
  star:
    '<path d="m12 3.6 2.55 5.16 5.7.83-4.12 4.02.97 5.67L12 16.9l-5.07 2.68.97-5.67-4.12-4.02 5.7-.83z" />',
  "thumbs-up":
    '<path fill="currentColor" stroke="none" d="M2 10.5h3.2V20H2zM7 20V9.6l4.2-6.4a1.6 1.6 0 0 1 2.94 1.05L13.4 8.4h5.3a2 2 0 0 1 1.96 2.42l-1.5 7a2.2 2.2 0 0 1-2.15 1.68z" />',
  user: '<circle cx="12" cy="8" r="3.6" /><path d="M5.5 20a6.5 6.5 0 0 1 13 0" />',
  users:
    '<circle cx="9" cy="8" r="3.2" /><path d="M3.4 19.5a5.6 5.6 0 0 1 11.2 0" /><path d="M15.5 5.1a3.2 3.2 0 0 1 0 6.1M17.4 19.5a5.6 5.6 0 0 0-2.6-4.7" />',
  gamepad:
    '<rect x="2.5" y="7.5" width="19" height="10" rx="5" /><path d="M7.5 11v3M6 12.5h3M15.5 11.5h.01M18 13.5h.01" />',
  cloud:
    '<path d="M7.2 18.5h9.6a4.3 4.3 0 0 0 .5-8.57 5.8 5.8 0 0 0-11.2-1.2A4 4 0 0 0 7.2 18.5z" />',
  windows:
    '<path fill="currentColor" stroke="none" d="M3 5.6 10.6 4.5v6.9H3zM11.7 4.34 21 3v8.4h-9.3zM3 12.6h7.6v6.9L3 18.4zM11.7 12.6H21V21l-9.3-1.35z" />',
  // A local install: a desktop computer, used as the source glyph for games
  // that live on this machine rather than in a connected store library.
  local:
    '<rect x="3" y="4.5" width="18" height="12" rx="1.5" /><path d="M8.5 20.5h7M12 16.5v4M6.5 13.5h5" />',
  more:
    '<circle cx="5" cy="12" r="1.7" fill="currentColor" stroke="none" /><circle cx="12" cy="12" r="1.7" fill="currentColor" stroke="none" /><circle cx="19" cy="12" r="1.7" fill="currentColor" stroke="none" />',
  check: '<path d="M20 6.5 9.2 17.3 4 12.1" />',
};

export function icon(name: IconName, className = "", label?: string): string {
  const title = label ? `<title>${label}</title>` : "";
  const viewBox = name === "orivo" ? "0 0 32 30" : "0 0 24 24";
  return `<svg class="icon ${className}" viewBox="${viewBox}" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="${label ? "false" : "true"}"${label ? ' role="img"' : ""}>${title}${paths[name]}</svg>`;
}
