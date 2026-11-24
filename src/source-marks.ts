/**
 * The mark each segment of the Library's source row wears, beside its name.
 *
 * Every segment is the same two things — a mark and a word, set in the
 * interface's own type — so a row of stores, consoles and folders reads as
 * one row. The marks are vectors drawn in `currentColor`: they are as sharp
 * at 13px on a phone as at 18px on a 5K display, and they dim and light with
 * the word beside them.
 *
 * A brand is shown by its own mark. Orivo's own groupings — All Games, Local,
 * Stream, a plugin — and a console no mark can be shipped for take a glyph
 * from `icons.ts`. Where each mark comes from is in `assets/brands/SOURCES.md`.
 */
import { icon, type IconName } from "./icons";
import type { BrowseSegment } from "./library-browse";

/** A brand mark drawn from the brand's own artwork, filled. */
function mark(viewBox: string, body: string): string {
  return `<svg class="icon source-mark source-mark--brand" viewBox="${viewBox}" fill="currentColor" aria-hidden="true">${body}</svg>`;
}

const BRAND_MARKS: Readonly<Record<string, string>> = {
  // Simple Icons 16.35.0, `android.svg` (CC0 1.0).
  android: mark("0 0 24 24", '<path d="M18.4395 5.5586c-.675 1.1664-1.352 2.3318-2.0274 3.498-.0366-.0155-.0742-.0286-.1113-.043-1.8249-.6957-3.484-.8-4.42-.787-1.8551.0185-3.3544.4643-4.2597.8203-.084-.1494-1.7526-3.021-2.0215-3.4864a1.1451 1.1451 0 0 0-.1406-.1914c-.3312-.364-.9054-.4859-1.379-.203-.475.282-.7136.9361-.3886 1.5019 1.9466 3.3696-.0966-.2158 1.9473 3.3593.0172.031-.4946.2642-1.3926 1.0177C2.8987 12.176.452 14.772 0 18.9902h24c-.119-1.1108-.3686-2.099-.7461-3.0683-.7438-1.9118-1.8435-3.2928-2.7402-4.1836a12.1048 12.1048 0 0 0-2.1309-1.6875c.6594-1.122 1.312-2.2559 1.9649-3.3848.2077-.3615.1886-.7956-.0079-1.1191a1.1001 1.1001 0 0 0-.8515-.5332c-.5225-.0536-.9392.3128-1.0488.5449zm-.0391 8.461c.3944.5926.324 1.3306-.1563 1.6503-.4799.3197-1.188.0985-1.582-.4941-.3944-.5927-.324-1.3307.1563-1.6504.4727-.315 1.1812-.1086 1.582.4941zM7.207 13.5273c.4803.3197.5506 1.0577.1563 1.6504-.394.5926-1.1038.8138-1.584.4941-.48-.3197-.5503-1.0577-.1563-1.6504.4008-.6021 1.1087-.8106 1.584-.4941z" />'),
  // Simple Icons 16.35.0, `playstation.svg` (CC0 1.0): the PSP is a PlayStation.
  psp: mark("0 0 24 24", '<path d="M8.984 2.596v17.547l3.915 1.261V6.688c0-.69.304-1.151.794-.991.636.18.76.814.76 1.505v5.875c2.441 1.193 4.362-.002 4.362-3.152 0-3.237-1.126-4.675-4.438-5.827-1.307-.448-3.728-1.186-5.39-1.502zm4.656 16.241l6.296-2.275c.715-.258.826-.625.246-.818-.586-.192-1.637-.139-2.357.123l-4.205 1.5V14.98l.24-.085s1.201-.42 2.913-.615c1.696-.18 3.785.03 5.437.661 1.848.601 2.04 1.472 1.576 2.072-.465.6-1.622 1.036-1.622 1.036l-8.544 3.107V18.86zM1.807 18.6c-1.9-.545-2.214-1.668-1.352-2.32.801-.586 2.16-1.052 2.16-1.052l5.615-2.013v2.313L4.205 17c-.705.271-.825.632-.239.826.586.195 1.637.15 2.343-.12L8.247 17v2.074c-.12.03-.256.044-.39.073-1.939.331-3.996.196-6.038-.479z" />'),
  // The two Joy-Con of Wikimedia's public-domain "Nintendo Switch Logo.svg"
  // (`assets/brands/source/nintendo-switch.svg`), without the name under them.
  switch: mark(
    "201 75 483 483",
    '<g transform="translate(0 900) scale(.1 -.1)"><path d="M2965 8314 c-481 -86 -868 -442 -990 -910 -44 -169 -47 -268 -42 -1579 3 -1204 4 -1232 24 -1325 111 -501 467 -858 973 -976 66 -15 150 -18 691 -21 560 -4 618 -3 633 12 15 15 16 208 16 2396 0 1622 -3 2386 -10 2400 -10 18 -27 19 -613 18 -476 -1 -619 -4 -682 -15z m905 -2400 l0 -2026 -407 5 c-375 4 -415 6 -490 25 -322 83 -561 331 -628 654 -22 101 -22 2589 -1 2688 60 281 255 514 518 619 132 53 193 59 621 60 l387 1 0 -2026z M3051 7329 c-63 -12 -159 -60 -210 -105 -105 -91 -157 -220 -149 -372 4 -79 9 -100 41 -164 47 -97 118 -168 215 -216 67 -33 84 -37 171 -40 79 -3 107 0 160 18 217 73 348 284 311 500 -43 257 -287 429 -539 379z M4757 8323 c-4 -3 -7 -1087 -7 -2409 0 -2181 1 -2402 16 -2408 27 -10 803 -6 899 4 406 46 764 293 959 660 25 47 58 126 75 175 63 188 61 138 61 1575 0 1147 -2 1318 -16 1391 -99 521 -496 914 -1018 1004 -70 12 -178 15 -526 15 -240 0 -440 -3 -443 -7z m1068 -2178 c156 -41 284 -160 336 -312 33 -94 32 -232 -1 -318 -61 -158 -181 -269 -335 -310 -250 -65 -516 86 -589 334 -22 76 -21 204 4 282 75 245 335 389 585 324z" /></g>',
  ),
};

/** Marks `icons.ts` already carries, as the stores' own (Simple Icons) or as Orivo glyphs. */
const ICON_MARKS: Readonly<Record<string, IconName>> = {
  all: "grid",
  steam: "steam",
  epic: "epic",
  gog: "gog",
  ubisoft: "ubisoft",
  xbox: "xbox",
  "microsoft-store": "microsoft-store",
  "instant-gaming": "instant-gaming",
  local: "folder",
  stream: "cast",
};

/** The SVG markup for a segment's mark. Every segment has one. */
export function sourceMark(segment: Pick<BrowseSegment, "id" | "kind">): string {
  const brand = BRAND_MARKS[segment.id];
  if (brand) return brand;
  const name: IconName =
    ICON_MARKS[segment.id] ?? (segment.kind === "console" ? "gamepad" : segment.kind === "plugin" ? "puzzle" : "folder");
  return icon(name, "source-mark");
}
