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
  | "check"
  | "leaf"
  | "heart"
  | "heart-filled"
  | "brain"
  | "book"
  | "compass"
  | "sparkle"
  | "puzzle"
  | "moon"
  | "palette"
  | "arrow-left"
  | "arrow-right"
  | "chevron-up"
  | "playstation"
  | "xbox"
  | "switch"
  | "emulator"
  | "chart"
  | "epic"
  | "gog"
  | "ubisoft"
  | "microsoft"
  | "instant-gaming"
  | "sun";

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
  leaf:
    '<path d="M4.5 19.5s-.4-7.2 5-11.2C13 5.6 19.5 4.5 19.5 4.5s-.4 6.9-4.2 10.6c-4.3 4.2-10.8 4.4-10.8 4.4Z" /><path d="M4.7 19.3C8.5 14 12 11 16.5 8.8" />',
  check: '<path d="M20 6.5 9.2 17.3 4 12.1" />',
  heart:
    '<path d="M12 20.3 4.6 13a4.7 4.7 0 0 1 6.6-6.7l.8.8.8-.8A4.7 4.7 0 0 1 19.4 13Z" />',
  // Same geometry as `heart`, filled instead of stroked, so the wishlist
  // toggle does not jump in size when it flips on.
  "heart-filled":
    '<path fill="currentColor" stroke="none" d="M12 20.3 4.6 13a4.7 4.7 0 0 1 6.6-6.7l.8.8.8-.8A4.7 4.7 0 0 1 19.4 13Z" />',
  // The split-brain glyph from the design's "Bon pour le cerveau" row: two
  // lobes with a visible seam, not a literal anatomical drawing.
  brain:
    '<path d="M12 3.8v14.8" /><path d="M10.2 5.4a3.7 4 0 0 0-4.6 2.6 3.2 3.5 0 0 0-1 5.4 3.3 3.6 0 0 0 5.1 4.4 1.8 2 0 0 0 .5-1.9" /><path d="M13.8 5.4a3.7 4 0 0 1 4.6 2.6 3.2 3.5 0 0 1 1 5.4 3.3 3.6 0 0 1-5.1 4.4 1.8 2 0 0 1-.5-1.9" />',
  book: '<path d="M4 5.2h5.1c1.6 0 2.9 1 2.9 2.3v11c0-1-1.1-1.8-2.5-1.8H4Z" /><path d="M20 5.2h-5.1c-1.6 0-2.9 1-2.9 2.3v11c0-1 1.1-1.8 2.5-1.8H20Z" />',
  compass: '<circle cx="12" cy="12" r="8.5" /><path d="m15.2 8.8-1.9 4.5-4.5 1.9 1.9-4.5z" />',
  sparkle:
    '<path d="M12 3.5c0 6 2.5 8.5 8.5 8.5-6 0-8.5 2.5-8.5 8.5 0-6-2.5-8.5-8.5-8.5 6 0 8.5-2.5 8.5-8.5Z" />',
  puzzle:
    '<path d="M9.7 5.3a1.9 1.9 0 0 1 3.8 0h3.6a1.6 1.6 0 0 1 1.6 1.6v3.5a1.9 1.9 0 0 0 0 3.8v3.5a1.6 1.6 0 0 1-1.6 1.6H6.5a1.6 1.6 0 0 1-1.6-1.6V6.9a1.6 1.6 0 0 1 1.6-1.6z" />',
  moon: '<path d="M20 14.4A8.4 8.4 0 0 1 9.6 4 8.4 8.4 0 1 0 20 14.4Z" />',
  palette:
    '<path d="M12 20.5a8.5 8.5 0 1 1 8.5-8.5c0 2-1.6 3-3.3 3h-1.6a2 2 0 0 0-1.4 3.4c.5.6.2 2.1-2.2 2.1Z" /><circle cx="8.2" cy="12.4" r="1" fill="currentColor" stroke="none" /><circle cx="9.8" cy="8.6" r="1" fill="currentColor" stroke="none" /><circle cx="14" cy="8" r="1" fill="currentColor" stroke="none" />',
  "arrow-left": '<path d="M19 12H5m0 0 6-6m-6 6 6 6" />',
  "arrow-right": '<path d="M5 12h14m0 0-6-6m6 6-6 6" />',
  "chevron-up": '<path d="m6 15 6-6 6 6" />',
  // Simplified platform marks: legible at 15px, no vendor artwork copied.
  playstation:
    '<path d="M8.8 3.6v16.8" /><path d="M8.8 3.6c4 1 6 2.7 6 5.1 0 2.1-1.5 3.2-3.6 3.2H8.8M8.8 16.2c-1.2 1-2.4 1.4-3.4 1.2M15.4 16.5h4" />',
  xbox:
    '<circle cx="12" cy="12" r="8.5" /><path d="M6.6 5.9c2.6 1.1 4.4 3 5.4 5 1-2 2.8-3.9 5.4-5" /><path d="M6.6 18.1c1.5-3.6 3.3-6.2 5.4-7.8 2.1 1.6 3.9 4.2 5.4 7.8" />',
  switch:
    '<rect x="3.6" y="3.6" width="7" height="16.8" rx="3" /><rect x="13.4" y="3.6" width="7" height="16.8" rx="3" /><circle cx="7.1" cy="8" r="1.1" fill="currentColor" stroke="none" /><circle cx="16.9" cy="16" r="1.1" fill="currentColor" stroke="none" />',
  emulator:
    '<rect x="2.8" y="6.4" width="18.4" height="11.2" rx="2.4" /><path d="M6.6 12h2.8M8 10.6v2.8" /><circle cx="16" cy="11.4" r=".9" fill="currentColor" stroke="none" /><circle cx="18" cy="13.4" r=".9" fill="currentColor" stroke="none" />',
  chart:
    '<rect x="4.8" y="12" width="2.9" height="6.6" rx=".5" /><rect x="10.2" y="5.9" width="4.1" height="12.7" rx=".6" /><rect x="16.3" y="13.3" width="2.9" height="5.3" rx=".5" /><path d="M3.2 18.6h17.6" />',
  sun: '<circle cx="12" cy="12" r="3.4" /><path d="M12 3.4v2.2M12 18.4v2.2M3.4 12h2.2M18.4 12h2.2M6 6l1.6 1.6M16.4 16.4 18 18M18 6l-1.6 1.6M7.6 16.4 6 18" />',
  // Connected-store marks, drawn as the recognisable brand shapes so a card
  // reads at a glance. These are solid, filled marks rather than the outline
  // style above, because that is what makes a store logo legible at 20px.
  // Epic: the launcher's shield, drawn as an outline with a solid "E".
  // A filled shield turns into a white blob at 20px, which is exactly what it
  // looked like on the library badge; the outline keeps the silhouette while
  // letting the letter carry the mark.
  epic:
    '<path d="M5.9 3.2h12.2c.8 0 1.4.6 1.4 1.4v9.7c0 1.3-.5 1.8-1.4 2.3l-5.8 3c-.2.1-.4.1-.6 0l-5.8-3c-.9-.5-1.4-1-1.4-2.3V4.6c0-.8.6-1.4 1.4-1.4Z" stroke-width="1.6" /><path fill="currentColor" stroke="none" d="M9.2 6.9h5.6v1.6h-3.8v1.7h3v1.6h-3v1.8h3.9v1.6H9.2z" />',
  // GOG: the round mark carrying its "G". The stepped inner counters of the
  // full wordmark collapse into an unreadable "20" at this size, so the circle
  // is drawn as an outline and the letter carries the mark.
  gog:
    '<circle cx="12" cy="12" r="8.7" stroke-width="1.6" /><path fill="currentColor" stroke="none" d="M12.2 7.3c1.4 0 2.6.5 3.5 1.4l-1.3 1.3a3.1 3.1 0 1 0-2.2 5.3 3.1 3.1 0 0 0 2.8-1.7h-2.8v-1.8h4.8v1a4.9 4.9 0 1 1-4.8-5.5Z" />',
  // Ubisoft: the open spiral of the Ubisoft mark.
  ubisoft:
    '<path fill="currentColor" stroke="none" d="M12 2.6c-4 0-7.4 2.6-8.7 6.2l1.7.6C6.1 6.5 8.8 4.4 12 4.4a7.6 7.6 0 0 1 0 15.2c-2.7 0-5-1.4-6.3-3.6l-1.6.9A9.4 9.4 0 1 0 12 2.6Zm0 4.5a4.9 4.9 0 0 0-4.7 3.5l1.8.5A3 3 0 1 1 12 15a3 3 0 0 1-2.3-1.1l-1.4 1.2A4.9 4.9 0 1 0 12 7.1Z" />',
  // Microsoft: the four-square mark, shared by Xbox's PC library.
  microsoft:
    '<path fill="currentColor" stroke="none" d="M3 3h8.4v8.4H3zM12.6 3H21v8.4h-8.4zM3 12.6h8.4V21H3zM12.6 12.6H21V21h-8.4z" />',
  // Instant Gaming sells keys, so its mark is a key rather than a storefront.
  "instant-gaming":
    '<path fill="currentColor" stroke="none" fill-rule="evenodd" d="M8.6 6.4a5.6 5.6 0 1 0 0 11.2 5.6 5.6 0 0 0 5.3-3.8h1.6v2.4h2.3v-2.4h1.1v2.4h2.3v-4.6h-7.3A5.6 5.6 0 0 0 8.6 6.4Zm0 3.6a2 2 0 1 1 0 4 2 2 0 0 1 0-4Z" />',
};

/**
 * Full-colour brand marks, used where a logo is presented as itself rather than
 * as a small status glyph — the Settings list of connectable stores.
 *
 * The library, the hero badge and the detail page keep the monochrome marks
 * above, which inherit `currentColor` and therefore read white on the artwork.
 */
const brandPaths: Partial<Record<IconName, string>> = {
  steam: '<circle cx="12" cy="12" r="11.2" fill="#1b2838" />' + paths.steam.replace('fill="currentColor"', 'fill="#ffffff"'),
  // Epic's mark as it is presented on a dark surface: a light shield carrying
  // the dark "E", rather than the outline the white library badge uses.
  epic:
    '<path fill="#f4f3f7" stroke="none" d="M5.9 3.2h12.2c.8 0 1.4.6 1.4 1.4v9.7c0 1.3-.5 1.8-1.4 2.3l-5.8 3c-.2.1-.4.1-.6 0l-5.8-3c-.9-.5-1.4-1-1.4-2.3V4.6c0-.8.6-1.4 1.4-1.4Z" />' +
    '<path fill="#2a2a2a" stroke="none" d="M9.2 6.9h5.6v1.6h-3.8v1.7h3v1.6h-3v1.8h3.9v1.6H9.2z" />',
  gog: paths.gog.replace('fill="currentColor"', 'fill="#a04ff5"'),
  ubisoft: paths.ubisoft.replace('fill="currentColor"', 'fill="#0b7fda"'),
  xbox: '<circle cx="12" cy="12" r="9.4" fill="#107c10" stroke="none" />' +
    '<path d="M6.6 5.9c2.6 1.1 4.4 3 5.4 5 1-2 2.8-3.9 5.4-5" stroke="#ffffff" /><path d="M6.6 18.1c1.5-3.6 3.3-6.2 5.4-7.8 2.1 1.6 3.9 4.2 5.4 7.8" stroke="#ffffff" />',
  // The Microsoft mark is the one case where the colours carry the identity.
  microsoft:
    '<path fill="#f25022" stroke="none" d="M3 3h8.4v8.4H3z" />' +
    '<path fill="#7fba00" stroke="none" d="M12.6 3H21v8.4h-8.4z" />' +
    '<path fill="#00a4ef" stroke="none" d="M3 12.6h8.4V21H3z" />' +
    '<path fill="#ffb900" stroke="none" d="M12.6 12.6H21V21h-8.4z" />',
  "instant-gaming": paths["instant-gaming"].replace('fill="currentColor"', 'fill="#f76b15"'),
};

export function icon(name: IconName, className = "", label?: string): string {
  return svg(name, paths[name], className, label);
}

/**
 * The brand's own colours where one exists, and the monochrome mark otherwise.
 * A store without a colour variant is not a gap: Xbox, Microsoft and the rest
 * that do have one simply present as themselves, and the others still read.
 */
export function brandIcon(name: IconName, className = "", label?: string): string {
  return svg(name, brandPaths[name] ?? paths[name], `${className} icon--brand`.trim(), label);
}

export function hasBrandIcon(name: IconName): boolean {
  return brandPaths[name] !== undefined;
}

function svg(name: IconName, body: string, className: string, label?: string): string {
  const title = label ? `<title>${label}</title>` : "";
  const viewBox = name === "orivo" ? "0 0 32 30" : "0 0 24 24";
  return `<svg class="icon ${className}" viewBox="${viewBox}" fill="none" stroke="currentColor" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round" aria-hidden="${label ? "false" : "true"}"${label ? ' role="img"' : ""}>${title}${body}</svg>`;
}
