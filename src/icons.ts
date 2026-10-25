export type IconName =
  | "orivo"
  | "home"
  | "library"
  | "collections"
  | "store"
  | "settings"
  | "search"
  | "bell"
  | "link"
  | "sliders"
  | "feedback"
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
  | "cast"
  | "plus"
  | "wine"
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
  | "sun"
  | "cover"
  | "landscape"
  | "background"
  | "cog"
  | "database"
  | "info"
  | "sync"
  | "android";

const paths: Record<IconName, string> = {
  orivo:
    '<path fill="currentColor" stroke="none" d="M 2 28 C 4 18 7 8 13 3 C 15 1 18 1 20 3 C 26 8 29 18 30 28 L 23 28 C 22 21 20 15 17 12 C 14 14 11 20 10 28 Z M 12 21 C 14 19 18 19 20 21 L 21 25 L 11 25 Z" />',
  home: '<path d="M3 10.5 12 3l9 7.5M5.5 9V20h13V9" />',
  // Library, Store and Settings are Lucide's ("library-big", "store",
  // "settings"; ISC licence, https://lucide.dev): the navigation's three
  // marks are the most-seen glyphs in the app and deserve drawn ones.
  library:
    '<rect width="8" height="18" x="3" y="3" rx="1" /><path d="M7 3v18" /><path d="M20.4 18.9c.2.5-.1 1.1-.6 1.3l-1.9.7c-.5.2-1.1-.1-1.3-.6L11.1 5.1c-.2-.5.1-1.1.6-1.3l1.9-.7c.5-.2 1.1.1 1.3.6Z" />',
  collections: '<rect x="4" y="4" width="16" height="16" rx="1" /><path d="M4 9h16M9 4v16" />',
  store:
    '<path d="M15 21v-5a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v5" /><path d="M17.774 10.31a1.12 1.12 0 0 0-1.549 0 2.5 2.5 0 0 1-3.451 0 1.12 1.12 0 0 0-1.548 0 2.5 2.5 0 0 1-3.452 0 1.12 1.12 0 0 0-1.549 0 2.5 2.5 0 0 1-3.77-3.248l2.889-4.184A2 2 0 0 1 7 2h10a2 2 0 0 1 1.653.873l2.895 4.192a2.5 2.5 0 0 1-3.774 3.244" /><path d="M4 10.95V19a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8.05" />',
  settings:
    '<path d="M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915" /><circle cx="12" cy="12" r="3" />',
  search: '<circle cx="10.5" cy="10.5" r="6.5" /><path d="m16 16 5 5" />',
  bell: '<path d="M18 9a6 6 0 0 0-12 0c0 7-3 8-3 10h18c0-2-3-3-3-10M10 22h4" />',
  // Two half-links, open in the middle: the "connect a library" step reads as a
  // join rather than as a chain, which is what a store connection actually is.
  link:
    '<path d="M10.4 13.6a3.9 3.9 0 0 0 5.6 0l2.6-2.6a3.9 3.9 0 1 0-5.5-5.5l-1.3 1.3" /><path d="M13.6 10.4a3.9 3.9 0 0 0-5.6 0l-2.6 2.6a3.9 3.9 0 1 0 5.5 5.5l1.3-1.3" />',
  // Mixing-desk sliders for "personalise": two tracks, two handles at
  // different offsets, so the glyph reads as adjustment and not as settings.
  sliders:
    '<path d="M3.6 8.4h8.2M16.6 8.4h3.8M3.6 15.6h3.8M12.2 15.6h8.2" /><circle cx="14.2" cy="8.4" r="2.1" /><circle cx="9.8" cy="15.6" r="2.1" />',
  feedback: '<path d="M4 5.5h16a1 1 0 0 1 1 1v9a1 1 0 0 1-1 1h-8.6L7 21v-4.5H4a1 1 0 0 1-1-1v-9a1 1 0 0 1 1-1Z" />',
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
  // Lucide's "monitor", "cast" and "wine" (ISC licence): the Source row's
  // glyphs for Local, Moonlight streaming and Wine, which have no wordmark.
  monitor: '<rect width="20" height="14" x="2" y="3" rx="2" /><line x1="8" x2="16" y1="21" y2="21" /><line x1="12" x2="12" y1="17" y2="21" />',
  plus: '<path d="M5 12h14" /><path d="M12 5v14" />',
  cast: '<path d="M2 8V6a2 2 0 0 1 2-2h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2h-6" /><path d="M2 12a9 9 0 0 1 8 8" /><path d="M2 16a5 5 0 0 1 4 4" /><line x1="2" x2="2.01" y1="20" y2="20" />',
  wine: '<path d="M8 22h8" /><path d="M7 10h10" /><path d="M12 15v7" /><path d="M12 15a5 5 0 0 0 5-5c0-2-.5-4-2-8H9c-1.5 4-2 6-2 8a5 5 0 0 0 5 5Z" />',
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
  // A game installed on the phone as an app of its own: Android's head.
  android:
    '<path d="M4.5 17a7.5 7.5 0 0 1 15 0z" /><path d="M7.5 7.5 6 5M16.5 7.5 18 5" /><path d="M9.25 13.25h.01M14.75 13.25h.01" />',
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
  // The three wallpaper shapes, drawn as the frame each row actually renders:
  // a portrait plate, a wide plate, and a wide plate with scenery in it.
  cover: '<rect x="6.6" y="3" width="10.8" height="18" rx="2" />',
  landscape: '<rect x="3" y="5.4" width="18" height="13.2" rx="2" />',
  background:
    '<rect x="3" y="5.4" width="18" height="13.2" rx="2" /><circle cx="8.3" cy="9.9" r="1.4" /><path d="m3.4 16.9 4.3-4 2.9 2.7 3.3-3.4 4.7 4.7" />',
  // Settings › sidebar glyphs, taken from Lucide (ISC) rather than redrawn:
  // the cog the mock draws for General, the database cylinder for Data, the
  // circled "i" for About, and the two-arrow cycle a source row syncs with.
  cog:
    '<path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" /><circle cx="12" cy="12" r="3" />',
  database: '<ellipse cx="12" cy="5" rx="9" ry="3" /><path d="M3 5V19A9 3 0 0 0 21 19V5" /><path d="M3 12A9 3 0 0 0 21 12" />',
  info: '<circle cx="12" cy="12" r="10" /><path d="M12 16v-4" /><path d="M12 8h.01" />',
  sync:
    '<path d="M3 12a9 9 0 0 1 9-9 9.75 9.75 0 0 1 6.74 2.74L21 8" /><path d="M21 3v5h-5" /><path d="M21 12a9 9 0 0 1-9 9 9.75 9.75 0 0 1-6.74-2.74L3 16" /><path d="M8 16H3v5" />',
  // Connected-store marks. Epic, GOG and Ubisoft are their official marks
  // from simple-icons (CC0, https://simpleicons.org), filled, which is what
  // keeps a store logo legible at 20px.
  epic:
    '<path fill="currentColor" stroke="none" d="M3.537 0C2.165 0 1.66.506 1.66 1.879V18.44a4.262 4.262 0 00.02.433c.031.3.037.59.316.92.027.033.311.245.311.245.153.075.258.13.43.2l8.335 3.491c.433.199.614.276.928.27h.002c.314.006.495-.071.928-.27l8.335-3.492c.172-.07.277-.124.43-.2 0 0 .284-.211.311-.243.28-.33.285-.621.316-.92a4.261 4.261 0 00.02-.434V1.879c0-1.373-.506-1.88-1.878-1.88zm13.366 3.11h.68c1.138 0 1.688.553 1.688 1.696v1.88h-1.374v-1.8c0-.369-.17-.54-.523-.54h-.235c-.367 0-.537.17-.537.539v5.81c0 .369.17.54.537.54h.262c.353 0 .523-.171.523-.54V8.619h1.373v2.143c0 1.144-.562 1.71-1.7 1.71h-.694c-1.138 0-1.7-.566-1.7-1.71V4.82c0-1.144.562-1.709 1.7-1.709zm-12.186.08h3.114v1.274H6.117v2.603h1.648v1.275H6.117v2.774h1.74v1.275h-3.14zm3.816 0h2.198c1.138 0 1.7.564 1.7 1.708v2.445c0 1.144-.562 1.71-1.7 1.71h-.799v3.338h-1.4zm4.53 0h1.4v9.201h-1.4zm-3.13 1.235v3.392h.575c.354 0 .523-.171.523-.54V4.965c0-.368-.17-.54-.523-.54zm-3.74 10.147a1.708 1.708 0 01.591.108 1.745 1.745 0 01.49.299l-.452.546a1.247 1.247 0 00-.308-.195.91.91 0 00-.363-.068.658.658 0 00-.28.06.703.703 0 00-.224.163.783.783 0 00-.151.243.799.799 0 00-.056.299v.008a.852.852 0 00.056.31.7.7 0 00.157.245.736.736 0 00.238.16.774.774 0 00.303.058.79.79 0 00.445-.116v-.339h-.548v-.565H7.37v1.255a2.019 2.019 0 01-.524.307 1.789 1.789 0 01-.683.123 1.642 1.642 0 01-.602-.107 1.46 1.46 0 01-.478-.3 1.371 1.371 0 01-.318-.455 1.438 1.438 0 01-.115-.58v-.008a1.426 1.426 0 01.113-.57 1.449 1.449 0 01.312-.46 1.418 1.418 0 01.474-.309 1.58 1.58 0 01.598-.111 1.708 1.708 0 01.045 0zm11.963.008a2.006 2.006 0 01.612.094 1.61 1.61 0 01.507.277l-.386.546a1.562 1.562 0 00-.39-.205 1.178 1.178 0 00-.388-.07.347.347 0 00-.208.052.154.154 0 00-.07.127v.008a.158.158 0 00.022.084.198.198 0 00.076.066.831.831 0 00.147.06c.062.02.14.04.236.061a3.389 3.389 0 01.43.122 1.292 1.292 0 01.328.17.678.678 0 01.207.24.739.739 0 01.071.337v.008a.865.865 0 01-.081.382.82.82 0 01-.229.285 1.032 1.032 0 01-.353.18 1.606 1.606 0 01-.46.061 2.16 2.16 0 01-.71-.116 1.718 1.718 0 01-.593-.346l.43-.514c.277.223.578.335.9.335a.457.457 0 00.236-.05.157.157 0 00.082-.142v-.008a.15.15 0 00-.02-.077.204.204 0 00-.073-.066.753.753 0 00-.143-.062 2.45 2.45 0 00-.233-.062 5.036 5.036 0 01-.413-.113 1.26 1.26 0 01-.331-.16.72.72 0 01-.222-.243.73.73 0 01-.082-.36v-.008a.863.863 0 01.074-.359.794.794 0 01.214-.283 1.007 1.007 0 01.34-.185 1.423 1.423 0 01.448-.066 2.006 2.006 0 01.025 0zm-9.358.025h.742l1.183 2.81h-.825l-.203-.499H8.623l-.198.498h-.81zm2.197.02h.814l.663 1.08.663-1.08h.814v2.79h-.766v-1.602l-.711 1.091h-.016l-.707-1.083v1.593h-.754zm3.469 0h2.235v.658h-1.473v.422h1.334v.61h-1.334v.442h1.493v.658h-2.255zm-5.3.897l-.315.793h.624zm-1.145 5.19h8.014l-4.09 1.348z" />',
  gog:
    '<path fill="currentColor" stroke="none" d="M7.15 15.24H4.36a.4.4 0 0 0-.4.4v2c0 .21.18.4.4.4h2.8v1.32h-3.5c-.56 0-1.02-.46-1.02-1.03v-3.39c0-.56.46-1.02 1.03-1.02h3.48v1.32zM8.16 11.54c0 .58-.47 1.05-1.05 1.05H2.63v-1.35h3.78a.4.4 0 0 0 .4-.4V6.39a.4.4 0 0 0-.4-.4H4.39a.4.4 0 0 0-.41.4v2.02c0 .23.18.4.4.4H6v1.35H3.68c-.58 0-1.05-.46-1.05-1.04V5.68c0-.57.47-1.04 1.05-1.04H7.1c.58 0 1.05.47 1.05 1.04v5.86zM21.36 19.36h-1.32v-4.12h-.93a.4.4 0 0 0-.4.4v3.72h-1.33v-4.12h-.93a.4.4 0 0 0-.4.4v3.72h-1.33v-4.42c0-.56.46-1.02 1.03-1.02h5.61v5.44zM21.37 11.54c0 .58-.47 1.05-1.05 1.05h-4.48v-1.35h3.78a.4.4 0 0 0 .4-.4V6.39a.4.4 0 0 0-.4-.4h-2.03a.4.4 0 0 0-.4.4v2.02c0 .23.18.4.4.4h1.62v1.35H16.9c-.58 0-1.05-.46-1.05-1.04V5.68c0-.57.47-1.04 1.05-1.04h3.43c.58 0 1.05.47 1.05 1.04v5.86zM13.72 4.64h-3.44c-.58 0-1.04.47-1.04 1.04v3.44c0 .58.46 1.04 1.04 1.04h3.44c.57 0 1.04-.46 1.04-1.04V5.68c0-.57-.47-1.04-1.04-1.04m-.3 1.75v2.02a.4.4 0 0 1-.4.4h-2.03a.4.4 0 0 1-.4-.4V6.4c0-.22.17-.4.4-.4H13c.23 0 .4.18.4.4zM12.63 13.92H9.24c-.57 0-1.03.46-1.03 1.02v3.39c0 .57.46 1.03 1.03 1.03h3.39c.57 0 1.03-.46 1.03-1.03v-3.39c0-.56-.46-1.02-1.03-1.02m-.3 1.72v2a.4.4 0 0 1-.4.4v-.01H9.94a.4.4 0 0 1-.4-.4v-1.99c0-.22.18-.4.4-.4h2c.22 0 .4.18.4.4zM23.49 1.1a1.74 1.74 0 0 0-1.24-.52H1.75A1.74 1.74 0 0 0 0 2.33v19.34a1.74 1.74 0 0 0 1.75 1.75h20.5A1.74 1.74 0 0 0 24 21.67V2.33c0-.48-.2-.92-.51-1.24m0 20.58a1.23 1.23 0 0 1-1.24 1.24H1.75A1.23 1.23 0 0 1 .5 21.67V2.33a1.23 1.23 0 0 1 1.24-1.24h20.5a1.24 1.24 0 0 1 1.24 1.24v19.34z" />',
  ubisoft:
    '<path fill="currentColor" stroke="none" d="M23.561 11.988C23.301-.304 6.954-4.89.656 6.634c.282.206.661.477.943.672a11.747 11.747 0 00-.976 3.067 11.885 11.885 0 00-.184 2.071C.439 18.818 5.621 24 12.005 24c6.385 0 11.556-5.17 11.556-11.556v-.455zm-20.27 2.06c-.152 1.246-.054 1.636-.054 1.788l-.282.098c-.108-.206-.37-.932-.488-1.908C2.163 10.308 4.7 6.96 8.57 6.33c3.544-.52 6.937 1.68 7.728 4.758l-.282.098c-.087-.087-.228-.336-.77-.878-4.281-4.281-11.002-2.32-11.956 3.74zm11.002 2.081a3.145 3.145 0 01-2.59 1.355 3.15 3.15 0 01-3.155-3.155 3.159 3.159 0 012.927-3.144c1.018-.043 1.972.51 2.416 1.398a2.58 2.58 0 01-.455 2.95c.293.205.575.4.856.595zm6.58.12c-1.669 3.782-5.106 5.766-8.77 5.712-7.034-.347-9.083-8.466-4.38-11.393l.207.206c-.076.108-.358.325-.791 1.182-.51 1.041-.672 2.081-.607 2.732.369 5.67 8.314 6.83 11.045 1.214C21.057 8.217 11.822.401 3.626 6.374l-.184-.184C5.599 2.808 9.816 1.3 13.837 2.309c6.147 1.55 9.453 7.956 7.035 13.94z" />',
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
  // The official marks (simple-icons, CC0): Epic and Ubisoft in white, as they
  // present themselves on a dark surface; GOG in its own purple.
  epic: paths.epic.replace('fill="currentColor"', 'fill="#f4f3f7"'),
  gog: paths.gog.replace('fill="currentColor"', 'fill="#b25fd6"'),
  ubisoft: paths.ubisoft.replace('fill="currentColor"', 'fill="#f4f3f7"'),
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
