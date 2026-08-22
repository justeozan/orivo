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
- import d'une source — **SteamSource implémenté** : scan local des installations et connexion de bibliothèque Steam directement depuis le desktop, avec secrets dans le Trousseau, synchronisation non bloquante, fusion AppID, preview/import idempotent et launch target typé;
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
