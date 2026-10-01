# Architecture cible: Orivo Gaming OS

> Document de reference construit a partir de `.context/the-idea-orivo.md`, `.context/the-idea-orivo-detailed.md` et `.context/tech-stack-research-report.md`.

## 1. Statut du document

Ce document decrit l'architecture cible d'Orivo et sert de reference aux implementations en cours.

Le Selector fullscreen est desormais une application **Tauri v2** : un frontend TypeScript/CSS leger dans le WebView systeme, pilote par des commandes Rust pour le catalogue, l'import, le lancement et les medias. Le premier vertical slice conserve un catalogue local et le lancement direct, tandis que les modules decrits ci-dessous restent les frontieres a construire autour de lui.

## 2. Vision produit

Orivo est un **Gaming OS pour PC**, et non un simple launcher ou une bibliotheque de jeux.

Les plateformes existantes comme Steam, Epic, GOG et les emulateurs sont traitees comme des fournisseurs de contenu. Orivo centralise ensuite:

- la decouverte et les recommandations;
- la bibliotheque et le lancement;
- la progression et les statistiques;
- les mods et les integrations;
- le social contextuel;
- les performances materiel;
- les espaces de jeu;
- les plugins et les mini-apps;
- l'IA locale.

## 3. Principes d'architecture

1. **Local-first**: la bibliotheque, la recherche et les actions essentielles fonctionnent sans serveur distant.
2. **UI non bloquante**: l'interface ne doit jamais attendre une synchronisation, une jaquette ou un fournisseur externe.
3. **Une source canonique**: SQLite est la source de verite locale; les vues de feed, les recherches et les layouts sont des projections derivees.
4. **Contrats stables**: les integrations, plugins et le WebView passent par des interfaces structurees, jamais par des acces directs au domaine, au disque ou au processus de rendu.
5. **Composition native au WebView**: le CSS porte le hero, le verre, le blur et les animations; une surface WebGL/WebGPU n'est ajoutee que lorsqu'un profilage demontre qu'elle est necessaire.
6. **Progressive hydration**: la fenetre et les donnees recentes apparaissent immediatement, puis les donnees completes sont hydratees en arriere-plan.
7. **Accessibilite explicite**: roles, labels, focus clavier, contraste et reduction des animations font partie de chaque composant.

## 4. Vue d'ensemble

```text
+-----------------------------------------------------------------------+
|                         Orivo Desktop App                             |
|                                                                       |
|  +--------------------+       +----------------------+                |
|  | Presentation       |       | Application Rust     |                |
|  | TypeScript / CSS   |<----->| Commands / Queries   |                |
|  | navigation, focus  |  IPC  | domain state         |                |
|  +---------+----------+       +----------+-----------+                |
|            |                             |                            |
|            v                             v                            |
|  +--------------------+       +----------------------+                |
|  | Tauri WebView      |       | Local data platform  |                |
|  | CSS hero/glass/    |       | SQLite + FTS5        |                |
|  | blur + asset scope |       | cache + projections  |                |
|  +--------------------+       +----------+-----------+                |
|                                         |                            |
|                    +--------------------+--------------------+       |
|                    |                    |                    |       |
|                    v                    v                    v       |
|             Source adapters       Plugin runtime        AI runtime    |
|             Steam/Epic/GOG        Wasmtime + WIT        ONNX/Ollama  |
|             emulators/mods        capability sandbox    enrichment   |
|                                                                       |
|                    Platform services and workers                      |
|                    launch, files, GPU, hardware, media                |
+-----------------------------------------------------------------------+
```

## 5. Couches techniques

### 5.1 Runtime desktop

Le runtime principal est Tauri v2. Son processus Rust porte le cycle de vie de l'application, la configuration, les threads de travail, les permissions, les commandes IPC et les appels aux APIs du systeme d'exploitation. Le WebView systeme reste la surface de presentation, pas une source d'autorite sur le disque ou les processus.

Responsabilites:

- ouvrir et gerer la fenetre native et son WebView;
- declarer la CSP, les capabilities et les scopes de medias Tauri;
- demarrer la base locale;
- restaurer le dernier espace et la derniere selection;
- coordonner les workers et les plugins;
- exposer des commandes et evenements applicatifs minimaux a l'interface.

### 5.2 Presentation

Le frontend TypeScript/CSS est une couche de presentation legere chargee par Tauri. Il gere:

- la structure des pages et des panneaux;
- la navigation et les etats visuels;
- les textes, les controles et les listes;
- le focus clavier et la navigation manette;
- les roles et labels d'accessibilite;
- les etats loading, empty, error et offline;
- les medias recus sous forme d'URL autorisees, jamais de chemins locaux bruts.

Il ne doit pas contenir la logique de synchronisation des fournisseurs, les requetes SQL complexes, les chemins executables, les arguments de lancement ni une permission generale de filesystem ou de shell. Les mutations passent par des commandes Rust typees; les mises a jour asynchrones reviennent par des evenements ou par relecture d'un view model.

### 5.3 Composition visuelle

Le compositor du WebView et le CSS donnent a Orivo son identite visuelle:

- hero images et backgrounds plein ecran avec `object-fit` et overlays gradients;
- panneaux glass via `backdrop-filter`, applique uniquement aux surfaces compactes;
- profondeur, parallax optionnelle et transitions GPU-friendly via `transform` et `opacity`;
- lecture media, fallbacks et placeholders a geometrie stable;
- prechargement borne du jeu selectionne et de ses voisins immediats.

Le frontend ne doit pas recreer des canvases, decodages ou filtres plein ecran a chaque selection. Si `backdrop-filter` est indisponible ou trop couteux, une surface opaque conserve la meme geometrie et la meme lisibilite. Un plugin ne peut ni injecter du JavaScript privilegie, ni etendre les capabilities Tauri, ni contourner les commandes Rust. Une surface graphique specialisee reste possible plus tard, isolee dans le frontend et justifiee par une mesure, pas par defaut.

### 5.4 Domaine applicatif

Le domaine contient les regles metier, independantes de Tauri, du frontend, de SQLite et des APIs externes.

Modules proposes:

| Module | Responsabilite principale |
| --- | --- |
| `library` | Catalogue unifie des jeux, plateformes, installations et launch targets. |
| `discovery` | Gaming Feed, recommandations, smart collections et decouverte emotionnelle. |
| `game-hub` | Page detail d'un jeu, medias, guides, mods, succes, performances et activite. |
| `progression` | Sessions, mission courante, progression, temps restant et objectifs. |
| `timeline` | Historique personnel des jeux, captures, succes, moments et amis presents. |
| `statistics` | Temps de jeu, habitudes, genres, backlog et Gaming Wrapped permanent. |
| `spaces` | Espaces Solo, Coop, VR, Retro, Steam Deck et leurs layouts. |
| `commands` | Command Palette, recherche universelle et execution d'actions. |
| `installation` | Installation, configuration, mods, HDR, DLSS, cloud saves et optimisation. |
| `social` | Activite utile des amis et suggestions contextuelles sans messagerie centrale. |
| `marketplace` | Catalogue, installation et mise a jour des mini-apps. |
| `intelligence` | FPS, temperature, stockage, drivers, consommation et regressions. |
| `media` | Artworks, captures, clips, videos, thumbnails et hero assets. |

Chaque module expose des commandes et des requetes. Les ecrans consomment des view models prets a afficher plutot que d'acceder directement aux tables.

## 6. Modules d'integration

### 6.1 Source adapters

Les fournisseurs externes sont encapsules par des adapters uniformes. Un adapter peut importer des jeux, detecter une installation, lancer un executable, lire une progression ou synchroniser des metadonnees selon ses permissions.

Exemples de sources:

- Steam;
- Epic Games Store;
- GOG;
- launchers supplementaires;
- emulateurs;
- bibliotheques locales;
- fournisseurs de mods et de succes.

Flux general:

```text
Source externe -> Adapter -> Normalizer -> Catalog service -> SQLite
                                      -> Artwork pipeline
                                      -> Search index
                                      -> Feed projections
```

Le domaine ne depend jamais d'un format proprietaire de fournisseur. Les IDs externes sont conserves comme references d'import, avec un ID Orivo stable.

#### Comptes de boutiques connectes

`src-tauri/src/sources.rs` porte la frontiere commune des comptes connectes
(Epic Games, GOG, Ubisoft Connect, Xbox, Microsoft Store, Instant Gaming), et
chaque `source_*.rs` implemente un fournisseur. Trois regles y sont non
negociables:

1. **Aucun secret ne traverse l'IPC.** Les identifiants restent dans le trousseau
   du systeme. Le WebView apprend seulement si une source est connectee et sous
   quel nom d'affichage.
2. **Une reponse de fournisseur est une entree non fiable.** Les identifiants
   passent la grammaire de jetons opaques du catalogue et les URLs d'artwork une
   liste d'hotes autorises par fournisseur. Une reponse ne peut donc jamais
   devenir un chemin de fichier, un argument de processus ni une origine
   arbitraire dans la fenetre principale.
3. **Une reponse partielle reste une reponse.** Un jeu illisible est ignore et
   compte, jamais une synchronisation echouee. Le compte est affiche.

Deux styles de connexion coexistent, parce que la moitie de ces boutiques ne
publie pas d'API de compte utilisable:

| Style | Boutiques | Fonctionnement |
| --- | --- | --- |
| `Token` | Epic, GOG, Microsoft | La fenetre de connexion remet un code a usage unique, Rust l'echange contre un credential OAuth conserve dans le trousseau, puis chaque synchronisation est faite sans fenetre. |
| `Session` | Ubisoft Connect, Instant Gaming | Aucune API de compte publique. La fenetre de connexion reste connectee et la synchronisation s'execute *dans* cette origine authentifiee, en ne renvoyant qu'une liste compacte de jeux. Aucun secret durable ne sort de la fenetre. |

Un enregistrement issu d'un compte connecte utilise `LaunchTarget::Provider`.
Il ne porte ni chemin, ni repertoire de travail, ni arguments: l'hote transforme
son jeton opaque en une seule URI fixe et percent-encodee vers le client de la
boutique, et seulement si ce client est installe sur la machine.

### 6.2 Plugins

Le runtime plugin recommande est **Wasmtime + WebAssembly Component Model + WIT**.

Le plan d'exécution, le modèle de sécurité, les contrats initiaux et
l'intégration des runners d'émulation sont définis dans
[`docs/plugin-system-plan.md`](docs/plugin-system-plan.md). Ce document est la
référence avant d'introduire un runtime plugin dans le produit.

Les plugins peuvent fournir:

- une source de jeux;
- un fournisseur de metadonnees;
- un fournisseur de recherche;
- une integration sociale ou media;
- des commandes;
- des import/export et automatisations;
- des contributions UI structurees sous forme de donnees.

Les plugins ne peuvent pas:

- acceder librement au disque ou au reseau;
- executer du code natif dans le processus principal;
- modifier directement SQLite;
- dessiner dans le compositor;
- bloquer le thread UI.

Chaque plugin est instancie avec des capabilities explicites, des limites memoire et un delai d'execution. Les permissions sont visibles, revocables et journalisees.

Interfaces WIT a prevoir:

- `plugin-core`: cycle de vie, logs et permissions;
- `launcher-source`: enumeration et lancement;
- `metadata-provider`: enrichissement des jeux et artworks;
- `search-provider`: resultats structures;
- `ui-contrib`: cartes, badges, commandes et panneaux de reglages;
- `automation`: taches d'import/export et batch actions.

### 6.3 IA locale

Le runtime IA est separe du domaine et appele par des jobs asynchrones.

- **ONNX Runtime**: voie par defaut pour classification, tagging, embeddings et recommandations legeres.
- **Ollama**: backend optionnel pour les utilisateurs qui veulent des modeles locaux plus lourds.

L'IA produit des donnees derivees et expliquees: tags, scores, raisons de recommandation, palettes ou embeddings. Elle ne remplace pas les donnees canoniques ni les commandes utilisateur.

## 7. Donnees et stockage

### 7.1 SQLite

SQLite en mode WAL est la source locale canonique.

Groupes de tables:

- **catalogue**: jeux, plateformes, sources, installations, launch configs;
- **media**: artworks, thumbnails, videos, captures, clips et chemins de cache;
- **utilisateur**: favoris, etats, notes, jeux masques et preferences;
- **sessions**: sessions de jeu, progression, missions, succes et objectifs;
- **social**: amis, activites et relations contextuelles;
- **spaces**: espaces, layouts, widgets, fonds et raccourcis;
- **plugins**: manifestes, versions, capabilities, grants et executions;
- **system**: jobs, migrations, settings, telemetry locale et erreurs;
- **search**: donnees normalisees et index FTS5.

Les donnees de recherche, feed et home layout sont des projections regenerables. Une corruption de cache ne doit pas detruire le catalogue ou la progression.

### 7.2 Recherche

Le chemin initial est:

```text
Input clavier -> Command service -> SQLite FTS5 -> Ranking local -> View model
```

FTS5 couvre les titres, aliases, developpeurs, editeurs, tags, plateformes et sources. Tantivy peut etre ajoute plus tard pour les documents longs et la marketplace. Une recherche vectorielle embarquee ne doit etre ajoutee que lorsqu'un cas produit concret la justifie.

### 7.3 Cache media

Le pipeline distingue:

- les assets produits integres au bundle frontend;
- les thumbnails de bibliotheque;
- les versions hero, cover et backdrop derivees si elles sont necessaires;
- les assets videos et captures;
- les fichiers copies dans le cache applicatif et exposes au WebView par un scope Tauri explicite.

Les versions derivees sont generees hors du chemin d'input et peuvent etre supprimees puis reconstruites. Le frontend ne recoit que des URL media autorisees, jamais un acces generique aux chemins de l'utilisateur. L'ecran doit pouvoir afficher un placeholder stable sans modifier la mise en page.

## 8. Flux applicatifs principaux

### Demarrage

```text
Processus Tauri Rust
  -> Capabilities, CSP et fenetre WebView
  -> Dernier view model local / hero cache borne
  -> Premier rendu TypeScript/CSS interactif
  -> Commande Rust de lecture + hydration des projections
  -> Workers: sources, media, intelligence, plugins
```

### Ouvrir l'accueil

```text
Home query service
  -> espace actif
  -> jeux recents et Continue Playing
  -> feed contextuel
  -> recommandations et smart collections
  -> view model de presentation
  -> store TypeScript
  -> hero / glass composes par CSS dans le WebView
```

### Rechercher puis lancer

```text
Ctrl + Space
  -> Command Palette
  -> FTS5 + resultats providers
  -> Command launch(game_id)
  -> Launch service
  -> Adapter du fournisseur
  -> Processus du jeu
  -> Session monitor -> progression / stats / timeline
```

### Synchroniser une source

```text
Worker source
  -> fetch ou scan local
  -> normalisation des IDs
  -> transaction SQLite
  -> invalidation des projections concernees
  -> refresh feed/search/media sans bloquer l'UI
```

## 9. Etats de l'application

Les composants et services doivent traiter au minimum:

- `booting`: shell visible, donnees minimales;
- `ready`: donnees locales disponibles;
- `refreshing`: mise a jour en arriere-plan;
- `offline`: sources distantes indisponibles, fonctions locales actives;
- `empty`: aucune bibliotheque ou aucun resultat;
- `permission-required`: action bloquee par une capability;
- `degraded`: WebView, media ou provider en mode reduit;
- `error`: erreur recuperable avec action de reprise.

## 10. Performance, accessibilite et fiabilite

Objectifs de conception tires du rapport technique:

- viser 120 Hz quand le materiel le permet, soit 8,33 ms par frame;
- conserver le chemin input -> etat sous environ 2 ms;
- afficher les premiers resultats de recherche en moins d'une frame apres warm-up;
- rendre un shell utilisable avant l'hydration complete;
- ne decoder et ne precharger que le media selectionne et une petite fenetre de voisins;
- limiter `backdrop-filter` aux controles et cartes verre, jamais a une scene plein ecran;
- animer `transform` et `opacity`, et respecter `prefers-reduced-motion`;
- decharger les medias et projections qui ne sont plus visibles;
- supporter clavier, manette, navigation focus et reduction des animations;
- tester les roles et labels avec les outils d'accessibilite des plateformes ciblees;
- isoler les jobs lents dans des workers observables et annulables.

La promesse de demarrage sous 100 ms doit etre comprise comme un objectif de demarrage percu: fenetre, shell et contenu recent depuis le cache d'abord; hydration complete ensuite.

## 11. Arborescence cible du code

```text
orivo/
  package.json
  src/                      # frontend TypeScript/CSS
    app/                    # store, routes et orchestration UI
    selector/               # scene fullscreen, navigation et rail
    components/             # controles visuels accessibles
    styles/                 # tokens, glass, motion et responsive
  public/
    media/                  # assets de demonstration du frontend
  src-tauri/
    Cargo.toml
    build.rs
    tauri.conf.json
    capabilities/           # permissions minimales par fenetre
    src/
      lib.rs                # bootstrap Tauri, commandes et AppState
      catalog.rs            # modele, import et persistence locale
      launcher.rs           # lancement direct sans shell
  crates/
    domain/                # regles metier et modeles stables, si l'extraction devient utile
    application/           # commands, queries et orchestration, si l'extraction devient utile
    storage/               # SQLite, migrations, repositories, FTS5
    sources/               # adapters Steam, Epic, GOG, emulateurs...
    media/                 # cache, thumbnails, video et artworks
    jobs/                  # synchronisation et taches asynchrones
    plugins/               # Wasmtime, WIT, permissions, lifecycle
    intelligence/          # hardware, metrics, ONNX, recommandations
    platform/              # OS, processus, filesystem et input
  wit/                     # contrats du SDK plugin
  migrations/              # schema SQLite versionne
  assets/
    product/               # icones, masks, noise, LUT, textures
    themes/                # tokens et ressources visuelles
  tests/
    contract/              # adapters et plugins
    integration/           # storage, launch, sync, search et IPC
    visual/                # screenshots navigateur/WebView et regressions visuelles
```

Cette arborescence est une proposition de decoupage. Elle ne doit etre creee qu'au moment ou chaque frontiere correspond a un besoin reel.

## 12. Ordre de construction recommande

### Phase 1: fondation utilisable

- runtime Tauri v2, fenetre et capabilities minimales;
- shell TypeScript/CSS avec navigation clavier/manette;
- SQLite, migrations et catalogue minimal;
- import d'une source — **SteamSource implémenté** : connexion de bibliothèque Steam directement depuis le desktop, avec secrets dans le Trousseau, synchronisation non bloquante, fusion AppID, import idempotent et launch target typé ; le scan local des manifests ne dit plus quels jeux possédés sont déjà installés;
- recherche FTS5;
- lancement d'un jeu;
- cache media scope et page Library.

### Phase 2: identite visuelle

- hero plein ecran;
- panneaux glass CSS, blur et overlays;
- transitions et motion;
- ecran Home avec Continue Playing;
- tests visuels navigateur/WebView sur les resolutions ciblees.

### Phase 3: valeur produit

- sessions et progression;
- game hub;
- feed et smart collections;
- statistiques et timeline;
- Gaming Spaces;
- social contextuel.

### Phase 4: plateforme extensible

- SDK WIT;
- runtime Wasmtime sandboxe;
- marketplace;
- integrations mods, Discord, Spotify et autres;
- AI locale et recommandations contextuelles;
- installation intelligente et monitoring materiel.

## 13. Decisions encore ouvertes

- Valider le rendu CSS du verre, le frame pacing et l'accessibilite sur les WebViews macOS et Windows cibles.
- Choisir la strategie de fenetre sans bordure et d'integration de la barre de titre par OS.
- Definir le contrat IPC type entre le frontend, les commandes Rust et les evenements de rafraichissement.
- Fixer les scopes media et filesystem les plus etroits compatibles avec les imports utilisateur.
- Definir le premier fournisseur importe et son niveau de lancement/synchronisation.
- Determiner quelles donnees de progression sont fiables selon chaque fournisseur.
- Definir les limites du MVP pour le social, les mods et la marketplace.
- Fixer la politique de stockage et de consentement pour les metriques materiel.
- Decider si les videos de hero sont incluses au MVP ou ajoutees apres les images animees.
- Definir le modele de mise a jour et de signature des plugins.
- Formaliser les schemas d'evenements et les migrations SQLite avant le multi-provider.

## 14. Regle de coherence

Toute nouvelle fonctionnalite doit repondre a ces questions avant implementation:

1. Quel module de domaine en est proprietaire?
2. Quelle est la source canonique de ses donnees?
3. S'agit-il d'une commande, d'une requete ou d'un job asynchrone?
4. Quelle capability est necessaire si un plugin ou une integration est implique?
5. Que se passe-t-il hors ligne, sans artwork ou sans permission?
6. Quel est son impact sur le frame budget, le focus clavier et l'accessibilite?

---

## 15. Architecture UI et design system

> Statut: **recherche faite, decision a acter.** Cette section fige l'etude
> "equivalent React + Tailwind + shadcn pour Orivo" et le calendrier
> d'implementation. Elle ne modifie aucune decision des sections 1 a 14.

### 15.1 Constat: l'etat reel du frontend

Mesures sur un build frais (`pnpm build`, Vite 8.1.5):

| Indicateur | Valeur |
| --- | --- |
| Poids 1er ecran (gzip) | **~83 kB** (CSS 15.47 + JS 67.47) |
| Poids total app (gzip) | **~198 kB** (brut 730 kB) |
| Lignes CSS | **7 568** dans 5 fichiers |
| Lignes TS (hors tests/generes) | **19 262** |
| Classes CSS distinctes | **692** |
| Couleurs hex en dur | **242** + 309 `rgba()` |
| Composants `render*`/`build*` | **103** |
| Routes | 6 (`library`, `store`, `game`, `me`, `settings`, `not-found`) |

Repartition du CSS: `styles.css` (2 357 ln, global), puis une feuille par page
(`game-detail-page.css` 2 371, `me-page.css` 1 166, `store-page.css` 1 112,
`library-onboarding.css` 562) avec prefixes reserves `store-`, `gd-`, `me-`.

**Le trou principal:** `docs/DESIGN.md` definit une palette nommee
(`--void-window`, `--abyss-sidebar`, `--moon-white`, `--orivo-violet`,
`--glass-border`...) qui n'apparait **aucune fois** dans le CSS execute.
Le spec design et le code ont diverge; il n'existe aucun contrat entre les deux.

### 15.2 Probleme a resoudre

Deux besoins distincts, souvent confondus:

1. **Un fichier unique** qui definit le system design et dont toute l'app derive.
2. **Un systeme vierge** (boutons, dialogs, tabs...) que le style Orivo vient
   modifier, sans reimplementer l'a11y et le clavier a chaque fois.

Le besoin (1) est une couche **tokens**. Le besoin (2) est une couche
**primitives**. Tailwind seul ne couvre que (1).

### 15.3 Les possibilites etudiees

#### Couche A -- "le fichier unique"

| # | Option | Principe | Pertinence Orivo |
| --- | --- | --- | --- |
| A1 | **Tailwind v4 `@theme` (CSS-first)** | Un bloc `@theme {}` en CSS = tokens **et** generation des utility classes. Zero `tailwind.config.js`. | Le plus direct: le fichier unique est du CSS et alimente `var(--x)` + les classes |
| A2 | **Style Dictionary + W3C DTCG** (`tokens/*.json` -> `variables.css`) | Spec standard (stable oct. 2025). Sort **CSS + TS + doc + Figma**. | Le plus robuste si `DESIGN.md` doit derive du code |
| A3 | **CSS pur + `@layer`** (open-props) | `:root` + cascade layers, zero build | Le moins de risque, mais ne genere pas d'utilities |
| A4 | **twgen** (tokens TS -> codegen `@theme`) | Tokens types en TS, genere le CSS + switch runtime | Si le theme doit etre type dans le code |

A1/A3 sont non-exclusifs: `tokens.json` -> Style Dictionary -> `@theme` est le
montage courant.

#### Couche B -- "le systeme vierge"

| # | Option | Type | Fit TS vanilla |
| --- | --- | --- | --- |
| B1 | **shadcn-html** (`codylindley/shadcn-html`) | HTML sémantique + CSS tokens + JS vanilla, zero build; chaque composant = 1 dossier (`component-skill.md` + `.css` + `.js`) | Excellent |
| B2 | **5H3LL-UI** | shadcn-compatible **vanilla HTML/CSS/JS + Tailwind v4**, CLI de copy, 8 style packs remplaçables | Excellent -- le seul avec un vrai CLI type `shadcn add` |
| B3 | **Basecoat** | Classes CSS type shadcn via `@apply`, JS par Alpine.js | Bon, mais depend d'Alpine |
| B4 | **plain-elements** | Primitives **headless en Web Components light-DOM** (dialog, popover, tabs, tooltip...), aucun style | Excellent pour la couche comportement (a11y, focus, clavier) |
| B5 | **bast-ui** | Radix/BaseUI-like en web components (FAST), ~10.7 kB | Bon, mais Shadow DOM -> CSS global invisible |
| B6 | **Web Awesome (ex-Shoelace)** | 50+ composants, theming via `--wa-*`, `@layer` | Moyen -- look impose, `::part()` |
| B7 | **Whiskeyjack** | Design system **Tauri-first**, tokens -> CSS/JS/Swift/Kotlin, registry shadcn | Non -- React + Tailwind 3 |
| B8 | **zesdk** | Kit vanilla Tauri 2, tokens light/dark, composants promise-based | Correct -- mais maison, look impose |
| B9 | **rust-ui/ui (Leptos)** | Le vrai "shadcn de Rust" | Non -- reecriture totale |
| B10 | **React + shadcn + Tailwind v4** | Le standard absolu | Non -- 19 262 lignes TS a reecrire |

### 15.4 Schema d'architecture cible

```text
+---------------------------------------------------------------------+
|  SOURCE UNIQUE DE VERITE                                           |
|  tokens/orivo.tokens.json   (W3C DTCG: $type / $value / $description) |
|  +- palette, roles semantiques, typo, spacing, radius, shadow,      |
|     motion, breakpoints, form-factor                                |
+-----------------------------+---------------------------------------+
                              |  Style Dictionary (ou twgen)
                              v
+---------------------------------------------------------------------+
|  GENERES -- jamais edites a la main (header "DO NOT EDIT")          |
|  src/generated/tokens.css  -> @theme + :root + [data-theme]         |
|  src/generated/tokens.ts   -> types et valeurs pour le JS           |
|  docs/TOKENS.md            -> la doc design = le CSS reel           |
+-----------------------------+---------------------------------------+
                              v
+---------------------------------------------------------------------+
| COUCHE 1 -- PRIMITIFS VIERGES (le "shadcn" d'Orivo)                |
| src/ui/   copie en repo, stylet uniquement par var()                 |
|   button/   .ui-btn      [data-variant][data-size]                  |
|   card/     .ui-card                                             |
|   dialog/   <ui-dialog> (light DOM) + .ui-dialog                      |
|   tabs/ menu/ tooltip/ input/ switch/ badge/ toast/ skeleton/        |
|   AUCUNE couleur en dur. 100% var(--orivo-*)                        |
+---------------------------------------------------------------------+
| COUCHE 2 -- SKIN ORIVO                                              |
| src/styles/skin-orivo.css   <- TU ECRIS ICI TON STYLE              |
| .ui-btn[data-variant=primary] { ... }   (reecriture autorisee)     |
+---------------------------------------------------------------------+
| COUCHE 3 -- LAYOUT / PAGES (existant, migre progressivement)        |
| styles.css - store-page.css - gd-... - me-...                       |
|   remplace progressivement les classes par .ui-*                    |
|   garde data-form-factor, spatial-nav, page-lifecycle               |
+---------------------------------------------------------------------+

Ordre des @layer (evite les guerres de specificite):
  @layer theme, base, ui, skin, pages;
  -> les CSS pages (non-layered) gagnent par defaut, sans !important
```

**Regle d'or:** `grep -rE "#[0-9a-fA-F]{6}" src/ui/` doit renvoyer zero resultat.

### 15.5 Tableau comparatif des specs

`#1-#2` sont mesures, `#3-#8` sont estimes.

| # | Stack | Poids final (gzip) | Lignes source | Rapidite d'execution | Flexibilite | **Note /10** |
| --- | --- | --- | --- | --- | --- | :-: |
| 1 | **HTML + CSS pur** (statique) | **15-25 kB** total | ~6 000-8 000 | 10/10 -- zero runtime | 3/10 -- aucun etat ni logique | **4.0** |
| 2 | **Vanilla TS + CSS** (Orivo actuel) | **83 kB** 1er / **198 kB** total | **26 830** | 8/10 -- DOM direct; -1 pour le `innerHTML` de 554 ln | 7/10 -- tout controle, 0 contrat design | **6.5** |
| 3 | **Vanilla TS + tokens + primitives vierges** | **78-85 kB** 1er / **185-195 kB** total | **~24 000-26 000** | 8/10 -- runtime identique a #2 | 9/10 -- un fichier = tout le design | **9.0** |
| 4 | **shadcn + vanilla TS** (Tailwind v4 + 5H3LL-UI / shadcn-html) | **75-85 kB** 1er / **180-195 kB** total | **~22 000-25 000** | 8/10 -- meme runtime, Tailwind = build only | 9/10 -- CLI type `shadcn add` | **8.5** |
| 5 | **React 19 + shadcn + Tailwind v4** | **130-145 kB** 1er / **235-260 kB** total | **~26 000-32 000** | 7/10 -- +50 kB runtime, mount cost, vdom diff | 9.5/10 -- ecosysteme le plus large | **8.0** |
| 6 | **Web Components headless** (plain-elements + tokens) | **85-95 kB** 1er / **195-210 kB** total | **~25 000** | 8.5/10 -- light DOM natif, a11y gratuite | 8/10 -- ecosysteme jeune | **8.0** |
| 7 | **Web Awesome / Shoelace** | **150-170 kB** 1er / **280-310 kB** total | **~22 000** | 7/10 -- Lit + Shadow DOM | 5/10 -- look impose, `::part()` | **6.0** |
| 8 | **Leptos + rust-ui** | **180-250 kB** (WASM) | **~20 000-25 000 Rust** | 9/10 -- DOM compile | 4/10 -- quasi aucune lib | **5.0** |

Lecture:

- **#3 / #4 sont le point optimal**: meme poids que l'actuel (voire -5%), **-2 000
  a -5 000 lignes**, et la note monte parce que le CSS epars disparaît -- pas
  parce que le runtime accelererait.
- **#5 (React) coute +50 a +60 kB gzip** pour un gain de flexibilite marginal sur
  une app desktop 6 pages, plus la reecriture des 19 262 lignes TS.
- **#1 est un faux bond**: plus leger, mais les 103 builders, le router,
  `spatial-nav` et `page-lifecycle` seraient tous a refaire.
- **#7 est le seul qui alourdit reellement** (+70 a +110 kB) pour *moins* de
  flexibilite.

Reserves: "rapidite d'execution" = runtime avec code optimal. Sur une app 6 pages,
**les ecarts #2-#6 sont < 5 ms**; ce critere ne doit pas trancher. Les vrais
discriminants sont le **poids 1er ecran** et le **cout de migration**.

### 15.6 Recommandation

Stack composee, sans exclusivite:

```text
tokens/orivo.tokens.json --Style Dictionary--> tokens.css (@theme) + tokens.ts
        |
        +- source de truth pour docs/DESIGN.md (genere, plus jamais desynchronise)

src/ui/   = primitives vierges
            +- B2 (5H3LL-UI CLI) pour les composants "boite"
            +- B4 (plain-elements) pour dialog/popover/tabs/menu
              (a11y gratuite, light-DOM -> le CSS global continue de marcher)

src/styles/skin-orivo.css = le style Orivo, reecrit les .ui-* via var()
```

Pourquoi pas Tailwind seul: Tailwind ne resout que la couche A. La couche B --
"un systeme vierge que mon style modifiera" -- manquerait. D'ou le couple
**tokens + registry de primitives**.

Choix structurants a ne pas re-ouvrir:

- **Light DOM, pas de Shadow DOM** -- `styles.css` (2 357 lignes) ne voit pas
  l'interieur d'un shadow root.
- **Incrimental, pas de big-bang** -- migration page par page, cohabitation
  possible via les `@layer`.
- **Aucune couleur en dur dans `src/ui/`** -- c'est ce qui rend le fichier
  unique reellement unique.

### 15.7 Ce que ca permet d'ameliore

| Avant | Apres | Gain |
| --- | --- | --- |
| 242 hex + 309 `rgba()` en dur | 0 dans `src/ui/`, tout passe par `var(--orivo-*)` | Re-skin = 1 fichier, zero recherche/remplacement |
| `DESIGN.md` derive du CSS (0 correspondance) | `docs/TOKENS.md` **genere** depuis les tokens | Spec et code ne peuvent plus diverger |
| 7 568 ln CSS, 692 classes ad-hoc | ~4 500 ln CSS + primitives `ui-*` | **-40 % de CSS**, fini les classes jumellees entre pages |
| 103 builders qui refont l'a11y a la main | primitives B4 avec clavier/focus/roles fournis | Accessibilite (section 3.7 des principes) par defaut, pas par effort |
| Chaque page re-invente bouton/dialog/tabs | 1 seule source par composant | Ajouter un composant = 1 fois, pas 5 |
| Pas de guard lint | `grep -rE "#[0-9a-fA-F]{6}" src/ui/` = 0 en CI | Regression design bloquee avant merge |
| Poids 1er ecran 83 kB gzip | ~78-85 kB gzip | **Neutre a legerement meilleur** -- pas de taxe |

Ce que ca **n'ameliorera pas**: le runtime (meme DOM direct), les 19 262 lignes
de logique TS, ni le poids de `sentry-sdk` (49 kB gzip).

### 15.8 Quand l'implementer

Calendrier aligne sur l'**ordre de construction de la section 12**.

#### Phase 0 -- bloqueur technique (a faire avant toute migration)

| Tache | Pourquoi | Effort |
| --- | --- | --- |
| Remonter `build.target` de `safari13` dans `vite.config.ts` | `@layer` >= Safari 15.4, `@property` >= 16.4, `:has()` >= 15.4 -- rien de tout ca ne passe sous `safari13`. WebView2 (Chromium recent) n'est pas concerne; WKWebView suit la version de macOS. | 0.5 j + validation WebKit cible |
| Verifier la CSP (`style-src 'self' 'unsafe-inline'`) vs composants light-DOM | Empeche une mauvaise surprise en fin de migration | 0.5 j |
| Figler l'ordre `@layer theme, base, ui, skin, pages` | Sinon les pages existantes gagnent en ordre source et le skin est inappliquable | 0.5 j |

#### Phase 1 -- tokens (a lancer en parallele de la **Phase 1** produit, des que le catalogue est stable)

- Extraire `docs/DESIGN.md` -> `tokens/orivo.tokens.json` (W3C DTCG).
- Brancher Style Dictionary -> `src/generated/tokens.css` + `tokens.ts`.
- Ne **pas** migrer les pages: simplement alimenter `:root` et mesurer.
- **Critere de done:** `docs/TOKENS.md` genere, build vert, poids 1er ecran
  <= 85 kB gzip, aucun changement visuel (snapshot e2e `visual` identique).

#### Phase 2 -- primitives vierges (caler apres la **Phase 2 "identite visuelle"**)

Pourquoi ici: l'identite (hero, glass, blur, motion) est deja posee et stable.
Migrer avant = migrer deux fois; migrer maintenant = figer le vocabulaire.

- Introduire `src/ui/` avec 6 composants a fort renouvellement:
  `button`, `card`, `badge`, `switch`, `tabs`, `skeleton`.
- Skin Orivo dans `src/styles/skin-orivo.css`.
- **Critere de done:** 0 hex dans `src/ui/`, e2e vert, et au moins **une page**
  entierement `ui-*`.
- **Garde-fou:** ne pas toucher a `form-factor.ts` (33 selecteurs
  `data-form-factor`), `spatial-nav.ts`, `page-lifecycle.ts` pendant la
  migration.

#### Phase 3 -- migration des pages (au fil de la **Phase 3 "valeur produit"**)

- Ordre conseille: `settings` (le plus statique, risque minime) -> `me` ->
  `store` -> `game-detail` -> `library`/shell (le plus expose, en dernier).
- Chaque page: une PR dediee, critere de done = e2e vert + `git diff` CSS <= 0.
- **Ne pas migrer** tant qu'une page est en feature-freeze produit.

#### Ne pas faire maintenant

- **React (#5)**: reecriture de 19 262 lignes pour +50 kB gzip. A reevaluer
  uniquement si l'app depasse ~15 routes avec des etats clients complexes.
- **Leptos (#8)**: eco embryonnaire, poids WASM, big-bang total.
- **Web Awesome (#7)**: +70 a +110 kB pour moins de flexibilite.
- **Tailwind utilitaires partout**: ne l'adopter que si le cout des classes
  inline se revele inferieur au CSS actuel -- option #4, pas un acquis.

### 15.9 Decisions encore ouvertes (UI)

- [ ] A1/A2/A3: `@theme` pur, Style Dictionary, ou les deux en cascade.
- [ ] B2 vs B1 comme source de primitives (CLI 5H3LL-UI vs copy manuel shadcn-html).
- [ ] Version minimale de WebKit/macOS ciblee apres remontee de `build.target`.
- [ ] Si les tokens doivent aussi supporter un theme clair un jour (`[data-theme]`).
- [ ] Nom du prefixe: `.ui-*` propose, valide contre les 692 classes existantes.
