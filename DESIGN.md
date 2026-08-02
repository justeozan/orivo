# Design System: Orivo

Référence visuelle principale du Selector: `assets/moc-images/orivo-full-screen.png`.
Référence secondaire pour le futur Home / Game Hub: `assets/moc-images/orivo_example_image.png`.

Ce document décrit le système visuel partagé d'Orivo et ses modes d'interface. La direction est celle d'un launcher gaming premium: sombre, cinématographique, translucide, dense mais respirant, avec une interface macOS-like et un accent violet froid.

Le **Selector fullscreen** est la cible prioritaire du premier prototype. Le **Home / Game Hub** est une surface plus dense, dérivée de la référence secondaire, qui viendra ensuite. Les tokens, la typographie, le verre, les jaquettes et la motion sont partagés, mais la structure et la densité ne le sont pas.

The shipped application is neither of those two references in isolation: it is a routed multi-page shell built on the same tokens, described in **§4.3 Application pages mode**. That section, plus §14 and §15, is the specification a developer follows when adding a new surface.

L'implémentation de référence est le frontend TypeScript/CSS de Tauri v2. Les règles CSS de ce document sont donc la spécification directe du WebView : le verre utilise le vrai `backdrop-filter` du navigateur, pas une image du hero floutée et recopiée dans chaque contrôle.

## 1. Theme Visuel & Atmosphere

- **Ambiance generale**: cockpit gaming nocturne, verre depoli, lumiere chaude issue des jaquettes et des paysages, controles froids et precis.
- **Densite**: 7/10. Beaucoup d'informations visibles sans sensation de tableau de bord enterprise.
- **Variance**: 6/10. Structure claire en colonnes, mais profondeur creee par le hero image, les cartes flottantes, les rails horizontaux et la sidebar.
- **Motion**: 5/10. Interactions calmes, rapides, tactiles. Pas d'animation decorative.
- **Signature visuelle**: grande image de jeu en fond, overlay sombre progressif, panneaux glassmorphism, textes blancs doux, accent violet pour selection et action primaire.
- **Surface Selector**: scene fullscreen, navigation horizontale, hero dominant, rail de jeux et HUD de contrôleur.
- **Surface Home / Game Hub**: application desktop dans une grande fenêtre arrondie, fond noir externe, chrome macOS visible, navigation latérale fixe.

## 2. Palette Couleur & Roles

### Neutres

- **Void Window** (`#0B0B0D`) - fond exterieur et zones les plus profondes. Ne jamais utiliser `#000000`.
- **Abyss Sidebar** (`#151618`) - fond de la sidebar et des zones fixes.
- **Obsidian Surface** (`#1D1C1D`) - surface principale sombre, panneaux bas, arriere-plan sous l'image.
- **Smoked Glass** (`rgba(255,255,255,0.08)`) - remplissage des cartes, boutons secondaires et search bar.
- **Frosted Rail** (`rgba(255,255,255,0.13)`) - etat selectionne dans la sidebar, hover appuye, blocs de navigation.
- **Hairline Glass Border** (`rgba(255,255,255,0.14)`) - bordures de cartes, separation sidebar, contours des boutons.
- **Soft Divider** (`rgba(255,255,255,0.08)`) - lignes internes tres discretes.

### Texte

- **Moon White** (`#F4F2F7`) - titres principaux et labels actifs.
- **Mist Text** (`#C8C5CF`) - texte secondaire lisible.
- **Ash Metadata** (`#9B97A3`) - metadonnees, sous-titres, timestamps, labels systeme.
- **Dim Hint** (`#6F6A76`) - placeholders, labels des barres, elements inactifs.

### Accent unique

- **Orivo Violet** (`#7D54F4`) - action primaire, logo, selection active, focus ring, barres de progression principales.
- **Violet Frost** (`#DAD8FF`) - surface du bouton primaire quand il doit paraitre lumineux.
- **Violet Deep** (`#4B2BB8`) - etat pressed, gradient interne subtil, ombres teintees.

Utiliser le violet comme seul accent de marque. Les autres couleurs vives sont reservees aux statuts fonctionnels.

### Couleurs fonctionnelles

- **Online Green** (`#43D66B`) - statut ami en ligne, FPS sain, indicateur positif.
- **Away Gold** (`#F2C94C`) - statut menu/idle, avertissement leger.
- **System Blue** (`#5C8DFF`) - jauge CPU ou information systeme froide.
- **Warm Art Glow** (`#D0BAA2`) - lumiere venant des images de jeu uniquement, jamais comme couleur UI de bouton.

## 3. Typographie

- **Police principale**: `SF Pro Display`, `SF Pro Text`, `Geist`, `Segoe UI`, sans-serif.
- **Police mono**: `SF Mono`, `Geist Mono`, `JetBrains Mono`, monospace pour raccourcis clavier, chiffres systeme et mesures.
- **Display title**: 44-52px desktop, weight 650-700, line-height 1.0, letter-spacing `0`.
- **Section title**: 16-18px, weight 600, couleur Moon White.
- **Navigation label**: 13-14px, weight 500, couleur Mist Text; actif en Moon White.
- **Body / metadata**: 12-14px, line-height 1.35-1.5, couleur Ash Metadata.
- **Numerique**: chiffres alignes visuellement, mono ou tabular nums pour durees, pourcentages et stats systeme.
- **Interdit**: pas d'Inter, pas de serif generique, pas de texte uppercase systematique, pas de letter-spacing negatif.

## 4. Modes & Structures d'Ecran

### 4.1 Selector fullscreen — cible principale

Le Selector est une scène de sélection, pas un dashboard. Un seul jeu possède la priorité visuelle à la fois. L'écran doit rester compréhensible en trois secondes, depuis un clavier comme depuis une manette.

#### Canvas et zones

- Référence: 1536 x 1024, viewport plein écran sans sidebar ni colonne de widgets.
- Le hero occupe toute la surface et reste visible jusque derrière le rail inférieur.
- La barre supérieure est une couche flottante horizontale, sans cadre de fenêtre visible.
- Le rail `Recently Played` occupe la bande basse du hero; il ne doit jamais recouvrir le titre ou les actions.
- Le HUD de navigation reste ancré au bord inférieur, avec catégorie à gauche, progression au centre et commandes à droite.
- Les zones de sécurité sont de 32px minimum sur les bords et d'environ 10% de la largeur pour le bloc hero.

#### Barre supérieure Selector

- Logo Orivo à gauche, 36-42px.
- Navigation horizontale: Home, Library, Collections, Store, Settings.
- Etat actif dans une capsule Frosted Rail; aucune sidebar permanente.
- Search bar centrée, 390-430px x 40px, rayon 22px.
- Notifications et avatar à droite.
- La barre peut se réduire visuellement pendant une transition de sélection, mais ne doit pas provoquer de reflow du hero.

#### Bloc hero sélectionné

- Artwork ou vidéo en plein fond; le sujet principal reste centre-droit.
- Bloc texte à gauche, largeur maximale 560-620px, ancré dans le tiers inférieur du hero.
- Eyebrow de genre en capsule discrète.
- Titre Selector: 64-76px, poids 650-700, ligne unique si possible, maximum deux lignes.
- Description courte: 16-18px, maximum deux lignes.
- Métadonnées en ligne: temps joué, dernière session, progression ou statut local.
- Actions: Play primaire 132 x 48px, Bookmark et More en boutons icon-only.
- Flèches précédent/suivant: cercles 48px, centrés verticalement sur la scène, à 24-32px des bords.

#### Rail de jeux

- Label `Recently Played` aligné à gauche, au-dessus du rail.
- Cartes de 220-240px de large, ratio image 16:9, hauteur totale environ 180-196px avec métadonnée.
- La carte sélectionnée possède une bordure Orivo Violet de 1.5-2px et une élévation très légère.
- Les cartes adjacentes restent visibles et légèrement moins lumineuses; elles ne sont jamais floutées au point de perdre leur identité.
- Le rail peut dépasser le bord droit du canvas pour suggérer la suite, sans overflow global de la page.

#### HUD contrôleur

- Gauche: catégorie ou collection active, par exemple `Popular`.
- Centre: segments de pagination; le segment actif est violet.
- Droite: raccourcis contextualisés, par exemple Navigate, Select, Back.
- Les indications manette sont masquées ou remplacées par des raccourcis clavier selon le dernier périphérique utilisé.

#### États de sélection

- `Selected`: hero complet, titre lisible, action Play disponible.
- `Adjacent`: carte visible, luminosité réduite, metadata secondaire.
- `Loading media`: conserver la géométrie finale avec placeholder sombre, jamais de déplacement de layout.
- `Missing media`: fallback artwork ou surface Obsidian avec message court et action pour corriger le chemin.
- `Reduced motion`: fondu simple de 160-220ms; pas de parallax, scale ou particules.

#### Règles média et performance

- Une seule vidéo hero active est décodée à la fois.
- Le jeu sélectionné reçoit le média haute résolution; les voisins utilisent des dérivés légers préchargés.
- Le changement de jeu anime uniquement `opacity`, `transform` et les propriétés GPU dédiées.
- Aucun décodage, redimensionnement lourd ou chargement disque ne doit bloquer la navigation.
- Le blur est limité aux petites surfaces glass : il ne s'applique ni au hero ni au viewport complet. En l'absence de `backdrop-filter`, les contrôles utilisent une surface Obsidian plus opaque sans modifier la géométrie.
- Le Selector doit rester utilisable avec une image statique si le backend vidéo, le fichier ou le décodage matériel est indisponible.

### 4.2 Home / Game Hub — référence secondaire (surface non livrée)

> **Statut : non implémenté.** Cette section décrit une surface Home / Game Hub qui n'existe pas dans l'application livrée. La navigation réelle est la topbar globale décrite en **§4.3**, et la seule sidebar de l'application est interne à Settings. Ne pas lire cette section comme une description du shell actuel : elle reste ici comme référence visuelle pour un éventuel Home futur.

Le Home / Game Hub reprend les mêmes fondations visuelles, mais ajoute une navigation persistante, des widgets et plusieurs sources d'information. La référence `orivo_example_image.png` sert pour cette surface, pas pour le Selector initial.

### Fenetre du Home / Game Hub

- Canvas 1536 x 1024 en reference desktop.
- Fenetre principale avec rayon 14-18px, bordure `1px rgba(255,255,255,0.12)`, fond sombre.
- Chrome macOS visible: trois pastilles 12px en haut gauche, espace top interne de 28-36px.
- Aucune page blanche. Tout part d'une scene sombre.

### Sidebar gauche du Home / Game Hub

- Largeur desktop: 240px.
- Position fixe, hauteur totale, fond Abyss Sidebar avec leger verre depoli.
- Separation droite: `1px rgba(255,255,255,0.08)`.
- Logo violet en haut, taille 36-42px.
- Items navigation: hauteur 40-44px, rayon 8px, icone 16-18px, gap 12px.
- Etat actif: Frosted Rail, texte Moon White, icone blanche.
- Quick launch: petites jaquettes 32px, rayon 6px, labels 13px.
- Lecteur audio en bas: pochette 48px, titre 13px, progress bar violet 2-3px.

### Barre superieure du Home / Game Hub

- Search bar centree, largeur 390-430px, hauteur 40px, rayon 22px.
- Fond Smoked Glass, bordure Hairline Glass Border, blur 18-24px.
- Icone search 16px, placeholder Ash Metadata.
- Raccourci clavier affiche dans des mini-kbd translucides 24-28px.
- Avatar utilisateur en haut droite: 32-36px, cercle ou rayon 50%, avec bordure fine.

### Zone principale du Home / Game Hub

- Marge gauche apres sidebar: 50px environ.
- Colonne contenu centrale + colonne widgets droite.
- Grille desktop: contenu fluide, colonne droite 266-300px, gap 28-36px.
- Max-width global: 1280-1360px apres sidebar.
- Espacement vertical entre sections: 22-30px.

### 4.3 Application pages mode — shipped shell

Orivo runs as a routed multi-page application. Library, Store, game detail, Settings and not-found are **pages**, not overlays stacked on a single screen. One shell hosts all of them and survives every navigation.

#### Global topbar

- The topbar (`header.topbar`) is rendered exactly once by the shell and is identical on every page. Only *what the search searches* changes; the bar itself is never rebuilt, moved or duplicated per page.
- Contents, left to right: brand mark (opens the library-sources menu), primary navigation (`Library`, `Store`, `Settings`), the contextual search, then notifications and avatar.
- Exactly one navigation link carries `aria-current="page"` (and the matching active capsule). A game detail page resolves to the entry it was opened from: `?from=store` marks Store, everything else marks Library.
- The search keeps its grid cell on every route. A page that cannot search hides it with `visibility: hidden` and disables the input, so the topbar never reflows between pages.
- `--topbar-height` (92px, declared on `.selector`) is the single source of truth for the vertical space the topbar occupies.

#### Settings sidebar — the only sidebar

- Settings owns the one vertical sidebar in the application (`nav.settings-sidebar`, `role="tablist"`, `aria-orientation="vertical"`). It is internal to the Settings page: it does not exist on Library, Store, game detail or not-found, and it is not a second level of global navigation.
- Sections behave like tabs: `aria-selected`, `aria-controls`, and roving Arrow / Home / End keyboard navigation scoped to the Settings host.
- On narrow widths the layout collapses to a single column and the sidebar becomes a static horizontal list instead of a sticky column.

#### Settings is a page, not a modal

This replaces the old modal/backdrop pattern. Settings now has:

- no backdrop and no dimmed page behind it,
- no close button — leaving is a navigation (another nav entry, or Back),
- no focus trap,
- no Escape-to-close.

#### Places vs tasks — the rule for the next surface

- A **place** is a destination: it can be linked to, returned to, and left with Back. Places are pages. A place gets a route, a page host, and a `PageRestoreState`.
- A **task** is a bounded flow the user starts and then finishes or abandons. It has no address and must not destroy the page underneath. Tasks stay overlays.
- The Wine wizard is the live example of a task: it floats above whichever page is active, never tears the shell down, and Escape closes it from anywhere.
- When adding a surface: if it deserves a URL and a Back button, build it as a page. If it is something the user *does* and then dismisses, build it as an overlay task. Do not build a place as an overlay to save routing work.

#### Route grammar

Routes live in the hash. The parser is total: every input resolves to an `AppRoute`, never to a thrown error.

- `#/library` — the default. `#`, `#/` and an empty hash all resolve here.
- `#/store` — optional `?category=<for-you|short-sessions|strong-stories|relaxing|all-games>`, repeated `?provider=<...>`, and `?q=<text>`. An unknown category falls back to `for-you`; unknown providers are dropped; duplicates collapse. Serialising omits defaults, so the canonical Store URL is bare `#/store`.
- `#/games/<gameId>?from=library|store` — `from` accepts only those two values, anything else becomes `null`. `gameId` is percent-encoded in the hash.
- `#/settings/<section>` — sections are `general`, `libraries`, `plugins`, `appearance`, `data`, `about`. `#/settings` alone means `general`. `?attachGame=<gameId>` targets the plugins section. An unknown section is **not** silently corrected to `general`: it is a not-found.
- Anything else — not-found, carrying the path it could not resolve. The page names that path and offers one way back to the Library.

Ids and trust across the boundary:

- Game ids crossing IPC are **opaque**: `steam:<appid>`, `local:<sha256>`. They are references the host already owns, never paths, URLs or commands. Never parse them for display; never construct one in the UI.
- A route segment that fails to decode, or that contains NUL, `/` or `\`, is rejected as not-found rather than repaired.
- No URL, filesystem path, executable or command line is ever accepted from the WebView. The route carries the opaque id and nothing else; the native host resolves everything else itself.

#### Page lifecycle

Each page implements `mount` / `activate` / `deactivate` and is driven by a lifecycle host wrapping its own `div.app-page`.

- `mount(container)` runs once, lazily, on first activation.
- `activate({ route, signal, restoreState, isCurrent })` runs on every entry. The host first aborts the previous activation's `AbortSignal` and bumps a generation counter.
- `deactivate()` returns a `PageRestoreState` (or `null`) and runs only when the route's page actually changes — a change of section, category or query inside the same page is a re-activation, not a remount.
- The inactive host is `hidden` **and** `inert`. Both are required: `hidden` removes it visually, `inert` takes it out of the tab order, the accessibility tree and hit-testing. A page that is only `hidden` is still reachable by keyboard.
- **Late async responses are discarded, not applied.** Work started by an activation checks `isCurrent()` (or its `AbortSignal`) before touching the DOM and returns silently otherwise. An abort is a normal outcome and never surfaces as an error toast.
- `PageRestoreState` is stored per page and replayed only when *returning* to that page — Back, or a repeat visit — never on a fresh navigation. It carries `scrollTop`, `focusKey`, and optionally `selectedGameId`, `query` and `filters`. There is a single scroll field: the Library rail is horizontal and stores its `scrollLeft` in it.
- Focus restoration happens after layout (`requestAnimationFrame`) and re-checks `isCurrent()` before focusing, so a fast Back-then-forward never steals focus into a page that is no longer visible.

#### CSS ownership

- `styles.css` owns the global layer: tokens, topbar, shell, the `.app-page` hosts, the Library scene and Settings.
- Every page added on top of the shell owns exactly one stylesheet with a class prefix reserved to it: `store-` in `store-page.css`, `gd-` in `game-detail-page.css`. A page never styles another page's classes and never restyles a shell class.
- **A page stylesheet must not fight the shell's top padding.** The shell gives scrolling pages their clearance with `.app-page--scroll { padding-top: var(--topbar-height) }`. No negative top margin, no `padding-top: 0` on the host, no repositioning of the host, no re-declaring `position` or `inset` on `.app-page`. A page that needs full-bleed artwork under the topbar draws it inside its own root element instead of cancelling the shell's padding.

## 5. Hero Cinématique partagé

- Le hero est une image plein fond de jeu, non une carte.
- Image visible sur toute la zone haute, avec overlay noir vertical et horizontal:
  - haut: `rgba(20,18,20,0.35)`
  - gauche: `rgba(12,13,15,0.42)`
  - bas: `rgba(11,11,13,0.88)`
- Le sujet principal de l'image doit rester visible vers centre-droit.
- Texte hero positionne gauche, jamais centre:
  - eyebrow 16px "Jump back in"
  - titre Home / Game Hub 44-52px environ, poids 650
  - metadonnees en ligne avec petites icones circulaires
- Actions sous metadata:
  - bouton Play primaire 112 x 44px, rayon 999px, fond Violet Frost, texte sombre `#14131A`
  - boutons icon-only secondaires 44px, cercle, fond Smoked Glass
  - gap 10-12px entre actions
- La zone hero laisse voir les cartes d'achievement et de stats en surimpression basse, mais celles-ci restent dans leur propre zone sans recouvrir le titre.

Dans le Selector, les cartes d'achievement et de stats sont absentes du premier niveau. Le titre passe à 64-76px, le bouton Play à 132 x 48px et le rail `Recently Played` devient la seule couche de contenu persistante au-dessus du bas du hero.

## 6. Composants

### Boutons

- **Primaire**: pill 44px haut, padding horizontal 24px, fond `linear-gradient(135deg, #F0EEFF, #BDB4FF)`, texte `#15131F`.
- **Icon-only**: 44px carre/cercle, fond `rgba(255,255,255,0.12)`, bordure `rgba(255,255,255,0.14)`.
- **Hover**: augmenter luminosite de 6%, bordure `rgba(255,255,255,0.22)`.
- **Active**: `transform: translateY(1px) scale(0.98)`.
- Pas d'outer glow neon. Les ombres restent sombres et diffuses.

### Cartes verre

- Fond: `rgba(32,31,34,0.58)` avec `backdrop-filter: blur(22px) saturate(130%)`.
- Bordure: `1px solid rgba(255,255,255,0.12)`.
- Rayon: 12px pour cartes standard, 14-16px pour widgets droite.
- Ombre: `0 18px 48px rgba(0,0,0,0.35)`.
- Padding:
  - petites cartes: 14-16px
  - widgets droite: 16-18px
  - cartes systeme: 14px vertical, 16px horizontal
- Les cartes sont translucides; leur couleur doit laisser deviner le hero ou les jaquettes derriere.

### Jaquettes de jeu

- Ratio principal: 16:9.
- Rayon: 8px.
- Bordure: `1px rgba(255,255,255,0.12)`.
- Overlay image: gradient bas `rgba(0,0,0,0.72)` vers transparent.
- Texte toujours en bas gauche, padding 12px.
- Etat selectionne: bordure Orivo Violet 1.5-2px, shadow interne/externes violettes tres discretes `rgba(125,84,244,0.35)`.
- Pas de cartes blanches, pas d'icones generiques a la place des images.

### Avatars & statuts

- Avatars ronds 28-36px avec image reelle ou illustration de personnage.
- Stack amis: avatars chevauches de 8-10px maximum, bordure sombre 2px.
- Statut online: point 7-8px vert, position bas droite, bordure sombre 2px.
- Statut away: point gold identique.
- Badge `+7`: cercle translucide 32px, texte 12px.

### Widgets droite

- Largeur 266-300px.
- Empilement vertical, gap 12-16px.
- Premier widget accueil: fond violet sombre translucide `rgba(72,43,130,0.38)`, icone/logo en bloc flou a droite.
- Liste amis: lignes 46-52px, avatar 32px, nom 13px, statut 12px.
- Activite recente: lignes 42-48px, icone/mini avatar, timestamp a droite en Ash Metadata.
- Systeme: labels 12px, barre 4px haut, track `rgba(255,255,255,0.10)`, valeur alignee droite.

### Barres de progression

- Hauteur: 3-5px selon contexte.
- Track sombre translucide.
- Remplissage violet par defaut, avec segments bleus/golds seulement pour stats systeme.
- Extremites arrondies.
- Pas de jauges circulaires.

## 7. Layout des Sections

- Les sections horizontales utilisent des rails de cartes, pas une grille uniforme à 3 cartes.
- **Selector**: le premier rail est `Recently Played`; il reste visible dans la scène et accompagne la sélection du hero.
- **Selector**: cartes de 220-240px de large, avec une carte active et des voisins immédiatement navigables.
- **Home / Game Hub**: `Continue playing` utilise des cartes plus grandes, 172-180px x 145-150px environ.
- **Home / Game Hub**: `Collections` utilise des cartes plus petites, 166-180px x 114px environ.
- **Home / Game Hub**: `Recently added` est un rail bas partiellement visible si l'écran manque de hauteur.
- Titres de section alignes a gauche, margin-bottom 10-12px.
- Bouton fleche de rail: icon-only discret a droite, jamais gros CTA textuel.
- Les cartes doivent garder une taille stable; le texte ne doit pas agrandir la carte.

## 8. Iconographie

- Style line icon, stroke 1.75-2px, coins arrondis.
- Taille nav: 17-18px.
- Taille metadata: 14-16px.
- Taille action: 16-18px.
- Couleur inactive `#C8C5CF`, active Moon White.
- Utiliser des icones simples: home, grid, archive, shopping bag, trophy, activity, users, download, settings, search, play, bookmark, more-horizontal, arrow-right.

## 9. Images & Direction Artistique

- Les images sont indispensables. Une interface Orivo sans jaquettes ni scene hero est invalide.
- Priorite aux visuels de jeux cinematiques: paysages, personnages, scenes contrastees.
- Chaque image recoit un overlay sombre pour garantir la lisibilite.
- Les couleurs des images peuvent apporter chaleur et variete, mais l'UI garde sa palette sombre + violet.
- Eviter les images stock abstraites, floues ou purement decoratives.

## 10. Responsive

- **Selector desktop large**: rester fullscreen, sans sidebar; conserver la lisibilité du bloc hero et laisser le rail déborder légèrement à droite.
- **Selector 16:10 / MacBook**: réduire les marges verticales du hero avant de réduire le titre; le rail reste ancré en bas.
- **Selector écran externe 16:9**: préserver le bloc texte à gauche et le sujet de l'artwork à droite; ne pas étirer l'image sans crop contrôlé.
- **Home / Game Hub desktop large**: conserver sidebar 240px + contenu + widgets droite.
- **Home / Game Hub desktop étroit**: réduire colonne droite à 260px, compacter gaps à 20px.
- **Tablet (< 1024px)**: transformer la colonne droite du Home en rail horizontal sous le hero.
- Mobile (< 768px):
  - sidebar devient bottom nav ou drawer.
  - search bar prend toute la largeur disponible.
  - hero garde image, titre 34-38px, actions sous le titre.
  - toutes les sections passent en rails horizontaux scrollables avec snap.
  - aucun overflow horizontal de page; seuls les rails internes peuvent scroller.
- Touch targets minimum 44px.

## 11. Motion & Interaction

- Transitions standard: 160-220ms, `cubic-bezier(0.2, 0.8, 0.2, 1)`.
- Hover carte: `translateY(-2px)`, bordure plus claire, image legerement scale `1.02`.
- Selection carte: apparition de bordure violette + shadow douce, 180ms.
- Ouverture panneau/widget: fade + translateY(6px) vers 0.
- Progress bars: remplissage anime uniquement au montage, 500-700ms.
- Animer seulement `transform` et `opacity`; ne pas animer width/height sauf progression initiale controlee.
- Pas de rebond excessif, pas de curseur custom, pas de particules.

### Motion spécifique au Selector

- Changement de jeu: crossfade du hero sur 180-220ms, avec parallax très léger uniquement si la performance reste stable.
- Carte sélectionnée: scale maximum `1.02`, bordure violette et luminosité légèrement renforcée; aucun glow néon.
- Vidéo: démarrage après l'affichage de l'image ou du placeholder, sans retarder la sélection ni le bouton Play.
- Navigation rapide: si l'utilisateur parcourt plusieurs jeux, réduire les transitions intermédiaires et afficher directement le dernier état stable.
- Le mode réduction des animations désactive parallax, scale et particules, et conserve uniquement les fondus.

## 12. Regles d'Implementation CSS

```css
:root {
  --void-window: #0B0B0D;
  --abyss-sidebar: #151618;
  --obsidian-surface: #1D1C1D;
  --moon-white: #F4F2F7;
  --mist-text: #C8C5CF;
  --ash-metadata: #9B97A3;
  --dim-hint: #6F6A76;
  --orivo-violet: #7D54F4;
  --violet-frost: #DAD8FF;
  --online-green: #43D66B;
  --away-gold: #F2C94C;
  --glass-fill: rgba(255,255,255,0.08);
  --glass-strong: rgba(255,255,255,0.13);
  --glass-border: rgba(255,255,255,0.14);
  --soft-divider: rgba(255,255,255,0.08);
}

.glass-panel {
  background: rgba(32,31,34,0.58);
  border: 1px solid var(--glass-border);
  -webkit-backdrop-filter: blur(22px) saturate(130%);
  backdrop-filter: blur(22px) saturate(130%);
  box-shadow: 0 18px 48px rgba(0,0,0,0.35);
}

@supports not (backdrop-filter: blur(1px)) {
  .glass-panel {
    background: rgba(29,28,32,0.88);
  }
}
```

## 13. Anti-Patterns Interdits

- Pas de fond clair dominant.
- Pas de noir pur `#000000`.
- Pas d'Inter.
- Pas de violet neon, glow externe ou gradient flashy.
- Pas de cartes opaques qui cassent l'effet verre depoli.
- Pas de hero centre ou de landing page marketing.
- Pas de grille generique de trois cartes egales.
- Pas de boutons rectangulaires massifs quand un pill ou icon-only suffit.
- Pas d'emojis.
- Pas de texte filler du type "Scroll to explore", "Next-gen", "Elevate", "Seamless", "Unleash".
- Pas de spinners circulaires; utiliser skeletons ou barres de chargement.
- Pas d'images manquantes ou placeholders abstraits.
- Pas d'elements qui se chevauchent de maniere incoherente; chaque bloc garde sa zone.

## 14. Content Honesty Rules

Orivo shows what it actually knows. These are product rules, not copy suggestions: they decide whether a component may render at all.

- **Never show a fabricated price.** An offer without a price or without a currency renders `Price unavailable` next to its provider — never a guessed figure, a rounded number, a `0`, `Free`, or a struck-through "was" price that was never verified. When a game has no offer at all, say so (`No offer` / `No verified offer`).
- **Flag stale or missing data explicitly.** An offer is stale when the provider marks it stale, when it has no verification timestamp, or when it was verified more than 24 hours ago. Stale offers are labelled inline (`may be outdated`, `not recently verified`) and carry a visible stale state. A stale price is never presented as a verified one.
- **Report provider health as it is.** `available`, `degraded`, `unavailable` and `not-configured` are distinct states with the provider's own message. A degraded or unconfigured provider is never rendered as healthy, and its absence is never hidden behind an empty result.
- **Render a section only when real data backs it.** Friends and Activity are the live examples: they exist in the layout only when the payload actually contains entries. No placeholder rows, no invented avatars, no "no friends yet" filler that implies a feature already ships. An absent section is more honest than an empty one.
- **Recommendation reasons must be factual.** They are derived from data on hand — tags, genres, platform support, session length, ownership — and are phrased as such (`Available on macOS`, `Works in short sessions`, `Story-rich campaign`). Three at most. Never any AI or cognitive claim: no "our AI picked this", no "learned from your habits", no "because you seem to…". If no factual reason exists, show none.
- Missing media is a stated fallback state, not an invented image: keep the final geometry, show the fallback surface, and never substitute unrelated artwork.

## 15. Accessibility Baseline

- **Visible focus, always.** `:focus-visible` draws a 2px `#c3b5ff` outline with offset. Focus styling is never removed; a component that needs a different indicator must provide one of at least equal visibility.
- **Everything is reachable by keyboard.** Interactive elements are real `button` / `input` elements, icon-only controls carry an `aria-label`, and decorative layers are `aria-hidden`. No click handler on a non-focusable element.
- **Only the active page is focusable.** Inactive page hosts are `hidden` and `inert`, so Tab never lands in a page the user cannot see.
- **`prefers-reduced-motion` is honoured, twice.** The system query collapses transitions and animations and forces `scroll-behavior: auto`. The Appearance preference mirrors it with `data-motion="reduced"` and wins over the system setting, so the app can be calmed down without changing macOS.
- **No autoplaying video, anywhere.** Trailers use `controls` and `preload="metadata"` with autoplay off, and start only on explicit user intent. Motion-only embellishments are attached only when motion is allowed.
- **Keyboard shortcuts are page-scoped.** The only shortcut shared by every page focuses the contextual search (`⌘K` / `Ctrl+K`, or `/` when not typing). Library shortcuts (arrows, Enter, `i`, `Shift+I`) return early unless Library is the active route, so they can never fire from the Store, a game detail or Settings. Escape belongs to whatever task is open — the Wine wizard first, then the library menu — and otherwise blurs the search field.
- **Announce what changes on its own.** Regions that update without a direct user action use `role="status"` / `aria-live="polite"`; nothing announces assertively.

## 16. Checklist de Conformite

- La premiere impression est une scene de jeu sombre et immersive.
- **Selector**: aucun panneau lateral permanent; la navigation est horizontale et le hero reste dominant.
- **Home / Game Hub** (surface non livrée, cf. §4.2): la sidebar est fixe, translucide, macOS-like, avec un etat actif visible. Ne s'applique pas au shell actuel, dont la navigation est la topbar globale.
- L'accent violet apparait sur le logo, l'action primaire, la selection et les progressions.
- Les cartes laissent percevoir l'arriere-plan par blur et transparence.
- Les jaquettes portent l'identite visuelle; aucun rail ne semble vide.
- **Home / Game Hub**: les widgets droite restent compacts, lisibles et alignes.
- **Selector**: le titre, les actions et le rail restent lisibles sans widgets secondaires concurrents.
- Les textes secondaires sont doux, jamais blanc pur massif.
- Les interactions sont tactiles mais discretes.
- L'interface reste utilisable en fullscreen desktop et sans overflow global.
- **Application pages**: the topbar is the same on every page and exactly one navigation link is `aria-current="page"`.
- **Application pages**: the only sidebar is inside Settings, and Settings has no backdrop, no close button, no focus trap and no Escape-to-close.
- **Application pages**: every hash resolves to a page, an unknown one included; ids in the URL stay opaque.
- **Application pages**: the inactive page is `hidden` and `inert`, and Back restores scroll, focus and filters.
- **Application pages**: nothing on screen is a price, a section or a reason that the data does not actually support.
