import type { StoreBrand, StoreTabId } from "./contracts";
import { t } from "./i18n";
import type { IconName } from "./icons";

/**
 * What each built-in store looks like: its logo, its name, and the machines it
 * sells for. Colours are not here — they are tokens in `store-page.css`, keyed
 * on `data-store`, so the page has one place that paints a brand.
 *
 * Logos and machines are the makers' own: product renders and logos taken from
 * each brand's site or press kit, stored under `public/media/store-brands/`
 * (sources in `docs/store-brand-assets.md`). Epic sells no hardware, so its
 * store shows its logo alone.
 */

export interface StoreMachine {
  src: string;
  /** Where it stands: the centre one is in front and largest. */
  place: "left" | "centre" | "right";
}

export interface StoreTabLook {
  id: "all" | StoreBrand;
  /** A fallback mark from the icon set, for a logo that fails to load. */
  icon: IconName;
  /** The store's name, for the chevrons. Read at render, in the interface's language. */
  label(): string;
  /** The store's own logo with its wordmark: the masthead shows it alone. */
  lockupUrl?: string;
  /** Otherwise the brand's symbol, set beside its name in text. */
  markUrl?: string;
  /** The masthead's name, in two weights: the brand, then the shop. */
  name: string;
  shop(): string;
  machines: readonly StoreMachine[];
}

const MEDIA = "/media/store-brands";

export const STORE_TAB_LOOKS: readonly StoreTabLook[] = [
  {
    id: "all",
    icon: "store",
    label: () => t("store.tab.all"),
    markUrl: "/media/orivo-ring-icon.png",
    name: "Orivo",
    shop: () => t("store.mast.shop"),
    machines: [
      { src: `${MEDIA}/xbox/series-x.webp`, place: "left" },
      { src: `${MEDIA}/playstation/ps5-pro.webp`, place: "centre" },
      { src: `${MEDIA}/nintendo/switch-2.webp`, place: "right" },
    ],
  },
  {
    id: "playstation",
    icon: "playstation",
    label: () => "PlayStation",
    markUrl: `${MEDIA}/playstation/mark.svg`,
    name: "PlayStation",
    shop: () => t("store.mast.shop"),
    machines: [
      { src: `${MEDIA}/playstation/ps5-digital.webp`, place: "left" },
      { src: `${MEDIA}/playstation/ps5-pro.webp`, place: "centre" },
      { src: `${MEDIA}/playstation/portal.webp`, place: "right" },
    ],
  },
  {
    id: "xbox",
    icon: "xbox",
    label: () => "Xbox",
    lockupUrl: `${MEDIA}/xbox/logo.png`,
    name: "Xbox",
    shop: () => t("store.mast.shop"),
    machines: [
      { src: `${MEDIA}/xbox/series-s.webp`, place: "left" },
      { src: `${MEDIA}/xbox/series-x.webp`, place: "centre" },
      { src: `${MEDIA}/xbox/controller.webp`, place: "right" },
    ],
  },
  {
    id: "nintendo",
    icon: "switch",
    label: () => "Nintendo",
    lockupUrl: `${MEDIA}/nintendo/logo.svg`,
    name: "Nintendo",
    shop: () => "eShop",
    machines: [
      { src: `${MEDIA}/nintendo/switch-oled.webp`, place: "left" },
      { src: `${MEDIA}/nintendo/switch-2.webp`, place: "centre" },
      { src: `${MEDIA}/nintendo/switch-lite.webp`, place: "right" },
    ],
  },
  {
    id: "steam",
    icon: "steam",
    label: () => t("store.tab.steam"),
    lockupUrl: `${MEDIA}/steam/logo.svg`,
    name: "Steam",
    shop: () => t("store.mast.shop"),
    machines: [
      { src: `${MEDIA}/steam/deck-angle.webp`, place: "left" },
      { src: `${MEDIA}/steam/deck-oled.webp`, place: "centre" },
    ],
  },
  {
    id: "epic",
    icon: "epic",
    label: () => t("store.tab.epic"),
    lockupUrl: `${MEDIA}/epic/logo.svg`,
    name: "Epic Games",
    shop: () => t("store.mast.shop"),
    machines: [],
  },
];

export function storeTabLook(id: StoreTabId): StoreTabLook | null {
  return STORE_TAB_LOOKS.find((look) => look.id === id) ?? null;
}

/** "Exclusivité PlayStation": the pill a game exclusive to a brand wears. */
export function exclusiveLabel(brand: StoreBrand): string {
  const look = storeTabLook(brand);
  return t("store.detail.exclusive", { brand: look?.name ?? brand });
}
