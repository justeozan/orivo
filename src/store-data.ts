// Static content for the Store / "Découvrir" page. Mirrors the approved
// design mock (assets/moc-images/orivo-store-clean.png): a mindful game
// storefront with calm, curated recommendations.

export type StoreIconName =
  | "brain"
  | "leaf"
  | "clock"
  | "book"
  | "heart"
  | "barchart"
  | "arrow-right"
  | "chevron-down"
  | "windows"
  | "playstation"
  | "xbox"
  | "switch"
  | "emulator";

export interface StoreInfoRow {
  icon: StoreIconName;
  title: string;
  description: string;
}

export interface StoreRating {
  label: string;
  value: number; // 0..5 filled dots
}

export interface StoreGame {
  id: string;
  title: string;
  /** When true the game's wordmark is baked into the artwork, so no UI title is drawn. */
  titleInArt: boolean;
  art: string;
  genre: string;
  duration: string;
  mode: string;
  ratings: StoreRating[];
  description: string;
  favorite?: boolean;
  selected?: boolean;
}

export interface StorePlatform {
  id: string;
  icon: StoreIconName;
  label: string;
  active?: boolean;
  /** Renders the icon after the label (used by the trailing "Plus" control). */
  trailing?: boolean;
}

const art = (name: string): string => `/media/store/${name}`;

export const STORE_INFO_ROWS: StoreInfoRow[] = [
  {
    icon: "brain",
    title: "Bon pour le cerveau",
    description: "Stimule la réflexion, la créativité et la mémoire.",
  },
  {
    icon: "clock",
    title: "Peu de temps",
    description: "Parfait pour des sessions courtes et satisfaisantes.",
  },
  {
    icon: "book",
    title: "Bonne histoire",
    description: "Des récits marquants qui restent avec toi.",
  },
];

export const STORE_GAMES: StoreGame[] = [
  {
    id: "planet-of-lana",
    title: "Planet of Lana",
    titleInArt: false,
    art: art("planet-of-lana.jpg"),
    genre: "Aventure, Réflexion",
    duration: "3-4h",
    mode: "Solo",
    ratings: [
      { label: "Réflexion", value: 5 },
      { label: "Créativité", value: 4 },
      { label: "Relaxation", value: 4 },
    ],
    description: "Histoire émouvante sans violence.",
    favorite: true,
    selected: true,
  },
  {
    id: "firewatch",
    title: "Firewatch",
    titleInArt: true,
    art: art("firewatch.jpg"),
    genre: "Aventure, Exploration",
    duration: "4-5h",
    mode: "Solo",
    ratings: [
      { label: "Réflexion", value: 3 },
      { label: "Immersion", value: 4 },
      { label: "Déconnexion", value: 4 },
    ],
    description: "Une histoire humaine et contemplative.",
  },
  {
    id: "inscryption",
    title: "Inscryption",
    titleInArt: true,
    art: art("inscryption.jpg"),
    genre: "Stratégie, Cartes",
    duration: "6-8h",
    mode: "Solo",
    ratings: [
      { label: "Réflexion", value: 4 },
      { label: "Mémoire", value: 4 },
      { label: "Stratégie", value: 4 },
    ],
    description: "Un jeu intelligent qui challenge ton esprit.",
  },
  {
    id: "dorf-romantik",
    title: "Dorf Romantik",
    titleInArt: true,
    art: art("dorf-romantik.jpg"),
    genre: "Stratégie, Créatif",
    duration: "2-3h",
    mode: "Solo",
    ratings: [
      { label: "Créativité", value: 4 },
      { label: "Relaxation", value: 4 },
      { label: "Focus", value: 3 },
    ],
    description: "Construis, détends-toi, recommence.",
  },
  {
    id: "a-short-hike",
    title: "A Short Hike",
    titleInArt: true,
    art: art("a-short-hike.jpg"),
    genre: "Aventure, Exploration",
    duration: "1-2h",
    mode: "Solo",
    ratings: [
      { label: "Bien-être", value: 3 },
      { label: "Exploration", value: 4 },
      { label: "Sérénité", value: 3 },
    ],
    description: "Petit jeu, grand bol d'air.",
  },
];

export const STORE_TABS: string[] = [
  "Pour toi",
  "Bon pour le cerveau",
  "Courte durée",
  "Récits forts",
  "Relaxant",
  "Tous les jeux",
];

export const STORE_PLATFORMS: StorePlatform[] = [
  { id: "pc", icon: "windows", label: "PC", active: true },
  { id: "playstation", icon: "playstation", label: "PlayStation" },
  { id: "xbox", icon: "xbox", label: "Xbox" },
  { id: "switch", icon: "switch", label: "Switch" },
  { id: "emulateurs", icon: "emulator", label: "Emulateurs" },
  { id: "plus", icon: "chevron-down", label: "Plus", trailing: true },
];
