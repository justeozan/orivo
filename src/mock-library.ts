export interface LibraryGame {
  id: string;
  title: string;
  /** Presentation source returned by the desktop catalog; fixture entries can omit it. */
  source?: "steam" | "local" | "showcase";
  description: string;
  metadata: string;
  genre: string;
  heroUrl: string;
  coverUrl: string;
  landscapeUrl: string;
  lastPlayedAt: string;
  playTimeSeconds: number;
  launchable: boolean;
  /** Native platform support declared by Steam Store, not compatibility-layer support. */
  hostPlatform?: "windows" | "macos" | "linux" | "other";
  supportedPlatforms?: Array<"windows" | "macos" | "linux">;
  compatibleWithHost?: boolean;
}

const hero = (name: string) => `/media/igdb/heroes/${name}`;
const cover = (name: string) => `/media/igdb/covers/${name}`;
const landscape = (name: string) => `/media/igdb/landscapes/${name}`;

export const fallbackLibrary: LibraryGame[] = [
  {
    id: "showcase-elden-ring",
    title: "Elden Ring",
    description: "A vast world full of mystery and peril. What will you discover?",
    metadata: "Achievements 67/82",
    genre: "RPG",
    heroUrl: hero("elden-ring-wallpaper.png"),
    coverUrl: cover("elden-ring.jpg"),
    landscapeUrl: landscape("elden-ring.jpg"),
    lastPlayedAt: "2 days ago",
    playTimeSeconds: 128 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-cyberpunk-2077",
    title: "Cyberpunk 2077",
    description: "Night City is yours for the taking. Choose your legend.",
    metadata: "Achievements 41/57",
    genre: "Action RPG",
    heroUrl: hero("cyberpunk-2077.webp"),
    coverUrl: cover("cyberpunk-2077.jpg"),
    landscapeUrl: landscape("cyberpunk-2077.webp"),
    lastPlayedAt: "5 days ago",
    playTimeSeconds: 85 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-baldurs-gate-3",
    title: "Baldur's Gate 3",
    description: "Gather your party and return to the Forgotten Realms.",
    metadata: "Achievements 39/54",
    genre: "RPG",
    heroUrl: hero("baldurs-gate-3.jpg"),
    coverUrl: cover("baldurs-gate-3.jpg"),
    landscapeUrl: landscape("baldurs-gate-3.jpg"),
    lastPlayedAt: "1 week ago",
    playTimeSeconds: 97 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-hades-2",
    title: "Hades II",
    description: "Defy the Titan of Time beneath a moonlit underworld.",
    metadata: "Achievements 26/50",
    genre: "Roguelike",
    heroUrl: hero("hades-2.jpg"),
    coverUrl: cover("hades-2.jpg"),
    landscapeUrl: landscape("hades-2.jpg"),
    lastPlayedAt: "1 week ago",
    playTimeSeconds: 51 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-red-dead-redemption-2",
    title: "Red Dead Redemption 2",
    description: "Outlaws for life in a fading American frontier.",
    metadata: "Achievements 35/51",
    genre: "Adventure",
    heroUrl: hero("red-dead-redemption-2.jpg"),
    coverUrl: cover("red-dead-redemption-2.jpg"),
    landscapeUrl: landscape("red-dead-redemption-2.jpg"),
    lastPlayedAt: "2 weeks ago",
    playTimeSeconds: 110 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-the-witcher-3",
    title: "The Witcher 3",
    description: "Track monsters and chase the Wild Hunt across the Continent.",
    metadata: "Achievements 56/78",
    genre: "RPG",
    heroUrl: hero("the-witcher-3-wild-hunt.jpg"),
    coverUrl: cover("the-witcher-3-wild-hunt.jpg"),
    landscapeUrl: landscape("the-witcher-3-wild-hunt.jpg"),
    lastPlayedAt: "3 weeks ago",
    playTimeSeconds: 200 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-horizon-forbidden-west",
    title: "Horizon Forbidden West",
    description: "Explore a vibrant frontier ruled by colossal machines.",
    metadata: "Achievements 33/59",
    genre: "Adventure",
    heroUrl: hero("horizon-forbidden-west.jpg"),
    coverUrl: cover("horizon-forbidden-west.jpg"),
    landscapeUrl: landscape("horizon-forbidden-west.jpg"),
    lastPlayedAt: "1 month ago",
    playTimeSeconds: 68 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-god-of-war",
    title: "God of War",
    description: "A deeply personal journey through the Norse realms.",
    metadata: "Achievements 28/37",
    genre: "Action",
    heroUrl: hero("god-of-war.jpg"),
    coverUrl: cover("god-of-war.jpg"),
    landscapeUrl: landscape("god-of-war.jpg"),
    lastPlayedAt: "1 month ago",
    playTimeSeconds: 120 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-unrailed",
    title: "Unrailed!",
    description: "Build a railway together before the runaway train reaches the end.",
    metadata: "Local co-op • Railway 18",
    genre: "Co-op",
    heroUrl: hero("unrailed.jpg"),
    coverUrl: cover("unrailed.jpg"),
    landscapeUrl: landscape("unrailed.jpg"),
    lastPlayedAt: "2 months ago",
    playTimeSeconds: 24 * 3_600,
    launchable: false,
  },
  {
    id: "showcase-astro-duel-2",
    title: "Astro Duel 2",
    description: "A high-speed space battle between ship and body.",
    metadata: "Versus • Campaign",
    genre: "Action",
    heroUrl: hero("astro-duel-2.jpg"),
    coverUrl: cover("astro-duel-2.jpg"),
    landscapeUrl: landscape("astro-duel-2.jpg"),
    lastPlayedAt: "2 months ago",
    playTimeSeconds: 12 * 3_600,
    launchable: false,
  },
];

export function formatPlayTime(seconds: number): string {
  const hours = Math.floor(Math.max(0, seconds) / 3_600);
  if (hours > 0) {
    return `${hours}h played`;
  }

  const minutes = Math.floor(Math.max(0, seconds) / 60);
  return minutes > 0 ? `${minutes}m played` : "Ready to play";
}
