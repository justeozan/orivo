# Architecture cible: Orivo Gaming OS

> Document de reference construit a partir de `.context/the-idea-orivo.md`, `.context/the-idea-orivo-detailed.md` et `.context/tech-stack-research-report.md`.

## 1. Statut du document

Ce document decrit l'architecture cible d'Orivo et sert de reference aux implementations en cours.

Le workspace contient maintenant un premier vertical slice du Selector fullscreen: runtime Rust, interface Slint, catalogue local, lancement direct et pipeline media. Les modules decrits ci-dessous restent les frontieres a construire autour de ce prototype.

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
4. **Contrats stables**: les integrations et plugins passent par des interfaces structurees, jamais par des acces directs au renderer.
5. **GPU pour l'immersion**: Slint gere la structure de l'interface; wgpu gere la composition visuelle avancee.
6. **Progressive hydration**: la fenetre et les donnees recentes apparaissent immediatement, puis les donnees completes sont hydratees en arriere-plan.
7. **Accessibilite explicite**: roles, labels, focus clavier, contraste et reduction des animations font partie de chaque composant.

## 4. Vue d'ensemble

```text
+-----------------------------------------------------------------------+
|                         Orivo Desktop App                             |
|                                                                       |
|  +--------------------+       +----------------------+                |
|  | Presentation       |       | Application          |                |
|  | Slint              |<----->| Commands / Queries   |                |
|  | navigation, focus  |       | domain state         |                |
|  +---------+----------+       +----------+-----------+                |
|            |                             |                            |
|            v                             v                            |
|  +--------------------+       +----------------------+                |
|  | Visual compositor  |       | Local data platform  |                |
|  | wgpu               |       | SQLite + FTS5        |                |
|  | hero, glass, blur  |       | cache + projections  |                |
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

Le runtime principal est un binaire natif Rust. Il porte le cycle de vie de l'application, la configuration, les threads de travail, les permissions et les appels aux APIs du systeme d'exploitation.

Responsabilites:

- ouvrir et gerer la fenetre native;
- initialiser le renderer et les ressources GPU;
- demarrer la base locale;
- restaurer le dernier espace et la derniere selection;
- coordonner les workers et les plugins;
- exposer les actions applicatives a l'interface.

### 5.2 Presentation

Slint est la couche d'interface declarative.

Elle gere:

- la structure des pages et des panneaux;
- la navigation et les etats visuels;
- les textes, les controles et les listes;
- le focus clavier et la navigation manette;
- les roles et labels d'accessibilite;
- les etats loading, empty, error et offline.

Elle ne doit pas contenir la logique de synchronisation des fournisseurs, les requetes SQL complexes ni le code de rendu avance.

### 5.3 Composition visuelle

wgpu est utilise pour les surfaces qui donnent a Orivo son identite visuelle:

- hero images et backgrounds plein ecran;
- profondeur et parallax;
- blur pyramid et composition des panneaux glass;
- particules, bloom, color grading et overlays;
- lecture et presentation des medias;
- textures prechargees et caches GPU.

Le renderer reste une capacite interne de l'application. Un plugin ne peut pas injecter de shader, de render pass ou de callback dans la frame loop.

### 5.4 Domaine applicatif

Le domaine contient les regles metier, independantes de Slint, SQLite et des APIs externes.

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

### 6.2 Plugins

Le runtime plugin recommande est **Wasmtime + WebAssembly Component Model + WIT**.

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

- les assets produits integres a l'application;
- les thumbnails de bibliotheque;
- les versions hero et backdrop;
- les assets videos et captures;
- les textures pretes pour le GPU.

Les versions derivees sont generees en arriere-plan et peuvent etre supprimees puis reconstruites. L'ecran doit pouvoir afficher un placeholder stable sans modifier la mise en page.

## 8. Flux applicatifs principaux

### Demarrage

```text
Processus Rust
  -> Fenetre native et shell Slint
  -> Dernier etat local / hero cache
  -> Premier rendu interactif
  -> Ouverture SQLite et hydration des projections
  -> Workers: sources, media, intelligence, plugins
```

### Ouvrir l'accueil

```text
Home query service
  -> espace actif
  -> jeux recents et Continue Playing
  -> feed contextuel
  -> recommandations et smart collections
  -> view model Slint
  -> textures hero / glass composited par wgpu
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
- `degraded`: renderer, media ou provider en mode reduit;
- `error`: erreur recuperable avec action de reprise.

## 10. Performance, accessibilite et fiabilite

Objectifs de conception tires du rapport technique:

- viser 120 Hz quand le materiel le permet, soit 8,33 ms par frame;
- conserver le chemin input -> etat sous environ 2 ms;
- afficher les premiers resultats de recherche en moins d'une frame apres warm-up;
- rendre un shell utilisable avant l'hydration complete;
- maintenir les pipelines GPU et textures, sans les recreer a chaque frame;
- decharger les medias et projections qui ne sont plus visibles;
- supporter clavier, manette, navigation focus et reduction des animations;
- tester les roles et labels avec les outils d'accessibilite des plateformes ciblees;
- isoler les jobs lents dans des workers observables et annulables.

La promesse de demarrage sous 100 ms doit etre comprise comme un objectif de demarrage percu: fenetre, shell et contenu recent depuis le cache d'abord; hydration complete ensuite.

## 11. Arborescence cible du code

```text
orivo/
  Cargo.toml
  crates/
    app-shell/             # cycle de vie, fenetre, configuration
    domain/                # regles metier et modeles stables
    application/           # commands, queries, orchestration
    presentation/          # view models, navigation, focus
    ui/                    # fichiers Slint et composants visuels
    renderer/              # wgpu, blur, glass, hero, media GPU
    storage/               # SQLite, migrations, repositories, FTS5
    sources/               # adapters Steam, Epic, GOG, emulateurs...
    media/                 # cache, thumbnails, video et artworks
    jobs/                  # synchronisation et taches asynchrones
    plugins/               # Wasmtime, WIT, permissions, lifecycle
    intelligence/          # hardware, metrics, ONNX, recommandations
    platform/              # OS, processus, filesystem, fenetre, input
  wit/                     # contrats du SDK plugin
  migrations/              # schema SQLite versionne
  assets/
    product/               # icones, masks, noise, LUT, textures
    themes/                # tokens et ressources visuelles
  tests/
    contract/              # adapters et plugins
    integration/           # storage, launch, sync, search
    visual/                # screenshots et regression renderer
```

Cette arborescence est une proposition de decoupage. Elle ne doit etre creee qu'au moment ou chaque frontiere correspond a un besoin reel.

## 12. Ordre de construction recommande

### Phase 1: fondation utilisable

- runtime Rust et fenetre;
- shell Slint avec navigation clavier/manette;
- SQLite, migrations et catalogue minimal;
- import d'une source;
- recherche FTS5;
- lancement d'un jeu;
- cache de thumbnails et page Library.

### Phase 2: identite visuelle

- pipeline wgpu;
- hero plein ecran;
- panneaux glass, blur et overlays;
- transitions et motion;
- ecran Home avec Continue Playing;
- tests visuels sur les resolutions ciblees.

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

- Valider Slint contre Qt Quick sur un prototype de renderer et un test d'accessibilite reel.
- Choisir la strategie de fenetre sans bordure et d'integration de la barre de titre par OS.
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
