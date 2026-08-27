# Orivo Selector — contrat local v0.1

Ce document fixe le périmètre du premier vertical slice : découvrir un jeu local, le sélectionner et le lancer depuis une scène fullscreen. Le contrat est volontairement indépendant du frontend Tauri TypeScript/CSS afin que le noyau Rust et le catalogue puissent ensuite être partagés avec le runtime Windows et une synchronisation distante.

## Version et compatibilité

- `schema_version` est un entier global, actuellement à `5`.
- Un fichier avec une version majeure inconnue est refusé sans être modifié.
- Les formats v1 à v4 sont migrés explicitement vers v5 avant usage ; v2
  introduit les launch targets typés et les références média privées des
  sources, v3 ajoute le target `Runner` des émulateurs et v4 ajoute les
  profils Wine-Staging privés et leur inventaire d'exécutables accordés. v5
  ajoute le backend graphique Wine fermé. La politique par jeu est `Auto`
  (DXVK-macOS préféré, repli `WineD3D`) ; le défaut du profil est `WineD3D`,
  sauf sur un Mac Apple Silicon où le profil géré par défaut est directement en
  `DXVK-macOS` afin que l'utilisateur n'ait jamais à l'activer à la main.
- La migration v5 est additive : les jeux Direct, Steam et les targets Runner
  génériques restent inchangés; aucun jeu existant n'est basculé vers Wine.
- Toute migration est explicite, déterministe et sauvegarde d'abord le fichier source en `.bak`.
- Une migration échouée laisse le catalogue original intact ; le fichier `.bak` peut être restauré manuellement si nécessaire.
- Les champs inconnus sont conservés lors d'une lecture/écriture compatible ; le Selector n'en dépend jamais pour s'afficher.

## Entrée de catalogue

La source v0.1 peut être JSON ou SQLite, mais son modèle logique reste le même :

```text
Catalog {
  schema_version: 5
  games: [Game]
}

Game {
  id: string                 // stable, unique, non vide
  title: string              // non vide
  executable_path: path?     // fichier local, obligatoire pour une source direct
  working_directory: path?   // sinon dossier parent de l'exécutable
  arguments: [string]        // arguments déjà tokenisés, jamais une commande shell
  description: string?       // sous-titre local affiché dans le hero
  metadata: string?           // informations compactes affichées sous le sous-titre
  artwork_path: path?        // hero image statique ou fond panoramique
  artwork_source_path: path? // origine privée servant à régénérer le cache
  cover_path: path?          // jaquette verticale du rail
  cover_source_path: path?   // origine privée servant à régénérer le cache
  logo_path: path?           // logo transparent affiché dans le hero
  hero_video_path: path?     // réservé au Gate 2
  last_played_at: timestamp?
  play_time_seconds: integer // >= 0
  source: local | steam
  source_id: string?         // ID stable du fournisseur, e.g. Steam appId
  launch_target: direct | Steam { app_id } | Runner { runner_id, game_ref, profile_id }
  installation_path: path?   // conservé côté Rust, jamais exposé au WebView
}
```

Règles importantes :

- Un lancement ne passe jamais par un shell. Le runtime construit directement le processus à partir de `executable_path`, `working_directory` et `arguments`.
- Le WebView ne transmet que `game_id` à la commande `launch_game`. Le backend résout ensuite le jeu, le chemin, le répertoire et les arguments depuis le catalogue ; il n'accepte jamais une commande, un chemin ou des arguments libres venant du frontend.
- Un target `Runner` représente un jeu confié à un profil d'émulateur. Ses trois
  valeurs sont des identifiants opaques et validés : il ne contient ni ROM, ni
  exécutable, ni ligne de commande. Le host Orivo résout et valide ensuite le
  profil avant tout lancement.
- Pour le runner officiel Wine-Staging, le profil et l'inventaire privé
  contiennent les chemins Wine, préfixe, dossiers accordés et `.exe`; la carte
  publique ne contient que les IDs opaques du target `Runner`. Le WebView ne
  reçoit jamais ces chemins.
- Un chemin relatif est résolu par rapport au fichier de catalogue, puis normalisé et vérifié.
- Un exécutable absent produit une erreur récupérable ; il ne supprime pas le jeu.
- Un bundle macOS `.app` est accepté à l'import ; `CFBundleExecutable` est résolu vers `Contents/MacOS` et le nom d'affichage du bundle est utilisé quand il existe.
- Deux jeux qui pointent vers le même exécutable sont acceptés mais signalés comme doublons avant import.
- Les fichiers média sont optionnels. L'interface doit rester fonctionnelle avec une image de remplacement.

## Hero de la Bibliothèque

Sous le wordmark, le hero tient sur **une seule ligne** : la pill du genre, puis
le studio. Rien d'autre.

- La pill ne porte que le genre. `genre_for_game` renvoie `Library` quand aucun
  store n'en a publié un — c'est une absence, pas un libellé — donc un jeu sans
  genre n'affiche pas de pill, et « Library » n'est pas non plus une étagère du
  mode `Genre`.
- Le studio vient du champ `developer` de la vue, jamais de `metadata`.
  `metadata` est un champ mixte : un store y écrit le développeur, Steam l'état
  d'installation, Wine le nom du runner, la démo intégrée un compteur de succès.
- La source (Steam, Epic, GOG, …) n'est plus écrite sur l'artwork. Le rail, la
  page du jeu et le menu Sources la nomment déjà.
- Le temps de jeu et la dernière session appartiennent à la page du jeu.
- La compatibilité macOS n'a plus de pastille. Un jeu sans build macOS le dit
  dans le bouton Play, qui passe en grisé (`Windows only`). Seule une réponse
  réellement publiée par le store décide — `unknown` n'est pas « non » — et
  seulement sur un Mac : `macCompatibility` décrit le jeu, pas la machine.

## Plateformes d'un jeu

`supported_platforms` agrège deux clés `extra`, dans le même vocabulaire
(`windows` / `macos` / `linux`) :

- `STEAM_STORE_PLATFORMS_KEY`, rempli par les métadonnées Steam Store ;
- `SOURCE_PLATFORMS_KEY`, rempli par un connecteur dont le store publie la
  matrice. GOG la renvoie dans `content_system_compatibility` sur la fiche
  produit déjà récupérée par la synchronisation ; Epic la déduit de ses deux
  listes d'entitlements (Windows, plus macOS quand la liste Mac a répondu).

Une clé absente vaut « inconnu », jamais « aucune plateforme ». Le mode
`Platform` de la Bibliothèque lit cet agrégat, plus `macCompatibility` pour les
jeux dont le store n'a répondu que sur macOS, plus l'unique déduction sûre
côté runner : une entrée Wine est un build Windows.

## Import manuel

Le flux v0.1 est : `Choose executable` → validation du chemin → aperçu des métadonnées → `Add game` → écriture atomique du catalogue. Le dialogue natif est ouvert par une commande Rust Tauri ; le frontend n'obtient pas un accès général au filesystem.

L'import n'exécute aucun fichier et ne scanne pas automatiquement les disques. L'utilisateur garde le contrôle sur les jeux et les médias ajoutés.

## Runner officiel Wine-Staging (macOS)

Orivo applique Wine-Staging **automatiquement** à tout jeu local `.exe` : il
n'y a plus d'ajout manuel d'un jeu via Wine. Dès qu'un `.exe` est importé,
présent au démarrage ou lors d'un rechargement de bibliothèque, le host
l'associe à un unique profil Wine-Staging géré par défaut (`orivo-auto-wine`).
Ce profil est provisionné sans assistant : le host détecte l'installation
Wine-Staging (`detect_wine_staging`), la valide (`wine --version`), crée son
préfixe sous la racine gérée par Orivo, et n'accorde comme dossier que le
répertoire canonique propre à chaque `.exe` — jamais une arborescence plus
large et sans jamais scanner les disques. Orivo n'embarque pas Wine : sans
installation Wine-Staging détectée, le `.exe` reste une fiche Direct et le
lancement invite à installer Wine-Staging.

Un profil Wine possède un préfixe créé sous la racine gérée par Orivo et ne
peut ni adopter ni modifier un préfixe d'une autre application. Au lancement,
le host recanonise l'exécutable, vérifie qu'il est encore dans un dossier
accordé et construit `Command` avec des tokens fixes; il n'utilise jamais un
shell, une ligne de commande ou des arguments provenant du WebView.

L'association reste **réversible et non destructive** : la fiche Direct
d'origine est conservée (`origin_direct_game_id`) et masquée tant que la carte
`Runner` existe, puis réapparaît si le profil géré est supprimé. La carte
`Runner` ne porte que des IDs opaques, sans chemin ni arguments. Aucune
bibliothèque n'est perdue et aucun jeu n'est écrit avec un chemin exposé au
WebView.

Le backend graphique par défaut dépend de la machine. La politique par jeu est
`Auto` : sur macOS le host tente d'abord `DXVK-macOS` puis retombe sur
`WineD3D`. Sur un Mac **Apple Silicon** (détecté via `hw.optional.arm64`, donc
valide même sous Rosetta), le profil géré par défaut est directement en
`DXVK-macOS` afin que l'utilisateur n'ait jamais à l'activer à chaque fois. Le
runtime DXVK-macOS reste allowlisté et vérifié : le host télécharge la seule
archive épinglée depuis une URL fixe, la hache (`sha256`) et la lit sans
extraction libre avant d'en copier atomiquement les DLL D3D10/11 validées dans
le seul préfixe Orivo du profil, jamais dans l'installation Wine ni dans un
autre préfixe. Au lancement, le host applique uniquement l'override fixe
`WINEDLLOVERRIDES=d3d10core,d3d11=n,b`; le trajet est DirectX → Vulkan/MoltenVK
→ Metal. Ce téléchargement automatique est limité à cette archive épinglée et
vérifiée ; aucun GPTK, CrossOver, variable Wine libre ou support d'anti-cheat
n'est inclus. Un profil peut revenir à `WineD3D` (bouton d'override optionnel) :
les DLL restent alors dans son préfixe privé, mais ne sont plus surchargées au
lancement.

Au lancement, le host recanonise puis re-hache l'exécutable avec refus des
liens juste avant de créer le processus. Wine reçoit le chemin canonique
autorisé du `.exe`, plutôt qu'un descripteur `/dev/fd/*`, afin que les moteurs
comme Unity retrouvent leurs ressources voisines (`*_Data`). L'installation
DXVK utilise des descripteurs de dossiers privés et des opérations
`openat`/`renameat` sans suivi de liens : aucune écriture ne peut être
redirigée hors du préfixe Orivo.

## Cycle de lancement

```text
Idle --Play--> Launching --process started--> Running
Launching --validation/process error--> Error
Running --process exited--> Returning --catalog saved--> Idle
Error --Retry--> Launching
Error --Edit path--> Idle
Error --Dismiss--> Idle
```

Chaque erreur doit fournir une cause lisible et l'action la plus utile :

| Situation | Message | Action principale | Conservation des données |
| --- | --- | --- | --- |
| Exécutable manquant | Le fichier n'existe plus à cet emplacement | Edit path | Oui |
| Pas de permission | macOS/Windows bloque l'accès | Open permissions help | Oui |
| Processus impossible à démarrer | Le jeu n'a pas pu être lancé | Retry | Oui |
| Catalogue illisible | Version ou contenu non reconnu | Restore backup | Oui, fichier original intact |
| Média indisponible | Artwork non accessible | Use fallback | Oui |

## Critères Gate 0

- Une fenêtre Tauri v2 sans décoration s'ouvre en fullscreen sur macOS.
- Le WebView système compose le frontend TypeScript/CSS ; le verre utilise `backdrop-filter` avec une surface opaque de repli si le blur est indisponible.
- Le shell affiche un hero, un titre, une action Play, un rail et un HUD de navigation.
- La build frontend puis la vérification Rust/Tauri passent sans dépendance à un launcher externe ni à un service réseau.

## Gate 1 implémenté dans le prototype

- La commande `IMPORT GAME` ouvre le sélecteur de fichiers natif.
- L'exécutable choisi est ajouté au catalogue local, sauvegardé atomiquement et affiché dans le hero.
- Le bouton `PLAY` appelle `launch_game(game_id)` ; le backend lance le jeu résolu directement avec `Command`, sans shell.
- Les artworks locaux compatibles sont copiés dans le cache applicatif et exposés au WebView par un scope media Tauri limité ; BMP, JPEG, PNG et WebP conservent un fallback stable si le média est absent.
- Le hero, le logo et la cover sont des médias indépendants : l'absence de l'un ne masque pas les deux autres.
- Le chemin de catalogue peut être surchargé avec `ORIVO_CATALOG_PATH` pour les tests et le développement.

## Gate 2 — SteamSource local et bibliothèque de compte

Steam a deux chemins complémentaires, tous deux local-first. Ils ne nécessitent pas de backend Orivo hébergé.

- « Importer les jeux installés » dans le menu ancré au logo conserve le scan local et son aperçu de sélection.
- « Se connecter à une bibliothèque » ouvre une WebView Steam dédiée, non persistante et sans capability IPC. L'utilisateur se connecte directement auprès de Steam ; Orivo ne collecte jamais son mot de passe ni son Steam Guard. Si Steam termine sans rechargement de page, le bouton « I’ve signed in » relance explicitement la vérification au lieu de laisser l'interface attendre.
- Une fois l'identité et le jeton de bibliothèque obtenus localement, ils restent dans le Trousseau macOS. Ils ne sont ni ajoutés à `catalog.json`, ni renvoyés au WebView principal, ni journalisés. L'IPC ne reçoit que l'état public de connexion et les compteurs de synchronisation.
- La bibliothèque est récupérée directement auprès de Steam, en arrière-plan. L'alternative « clé API » accepte un SteamID64 et une clé Web API propres à l'utilisateur ; Orivo la vérifie auprès de Steam avant de remplacer une connexion existante, puis la stocke dans le même Trousseau. Elle sert de secours si la connexion web ne répond plus.
- Pour les jeux dont le catalogue n'a pas encore été enrichi, Orivo appelle directement l'endpoint public Steam Store avec au plus quatre requêtes simultanées. Il récupère la description courte, le genre et les plateformes natives (`Windows`, `macOS`, `Linux`) sans transmettre le jeton de compte, puis conserve ce résultat localement afin qu'une indisponibilité ponctuelle du Store ne réintroduise pas un texte générique.
- Orivo détecte le système qui exécute l'application côté Rust et compare cette valeur aux plateformes Steam déclarées. Le hero affiche le résultat uniquement lorsqu'il est connu. Il s'agit d'une compatibilité native annoncée par Steam, pas d'une promesse de fonctionnement via Proton, Wine ou une couche de virtualisation.
- La connexion web s'appuie sur un jeton présent dans la page Steam, pas sur un OAuth public documenté. Elle peut donc expirer ou évoluer : la reconnexion et la voie API-key restent des états produit explicites. Les challenges HTTPS externes de Steam Guard restent isolés dans la WebView sans capability ; seule une page `store.steampowered.com` peut fournir le jeton accepté.
- Sur macOS, Orivo découvre la racine Steam locale puis les bibliothèques secondaires déclarées dans `libraryfolders.vdf`.
- Seuls les manifests `appmanifest_<appid>.acf` complets, installés et associés à un dossier de jeu existant sont proposés. Les redistribuables, bandes-son, manifests incohérents et entrées incomplètes sont ignorés sans empêcher le reste du scan.
- Le panneau Steam reçoit uniquement un `appId`, un titre, un statut et des URLs de cache opaques ou des jaquettes Steam générées depuis un AppID numérique. Les chemins Steam, manifestes, répertoires d'installation et données source restent côté Rust.
- `import_steam_games(appIds)` relance un scan côté backend avant toute écriture ; il refuse toute entrée qui n'est plus découverte comme installée. Le frontend ne fournit jamais de chemin, commande, artwork ni argument de lancement.
- L'import est idempotent par `steam_app_id`, écrit le catalogue de façon atomique et préserve les données utilisateur ou les médias déjà mis en cache lors d'un refresh.
- Le scan et la préparation des médias tournent hors du chemin UI. Une courte photographie Rust-only de la découverte évite un second parcours des manifests lors de l'hydratation ; elle expire rapidement et ne traverse jamais l'IPC. La liste apparaît d'abord ; un maximum de 16 visuels visibles est hydraté ensuite en arrière-plan.
- Les médias sont optionnels, plafonnés à 20 MiB par fichier et 128 MiB par opération de cache. Une copie temporaire est renommée atomiquement avant d'être exposée ; une image illisible ou trop volumineuse laisse simplement le fallback visuel.
- Les mutations de catalogue sont sérialisées, mais les lectures de rail et de lancement ne gardent pas le verrou pendant l'écriture atomique sur disque.
- Un jeu Steam utilise un launch target typé `Steam { app_id }`, non un faux exécutable. Sur macOS, Orivo appelle le bundle Steam avec l'URI fixe `steam://run/<app_id>` sans shell, puis vérifie que macOS a accepté la demande avant de confirmer le lancement.
- La synchronisation de compte joint la liste de jeux possédés avec le scan local par AppID. Un jeu possédé mais absent du disque est conservé dans la sidebar avec `installation_path: None`, le statut « Not installed » et `launchable: false`; son bouton ouvre Steam via l'URI fixe `steam://install/<app_id>`, sans construire de commande depuis le WebView.
- Les trois rôles d'image Steam restent distincts : `library_600x900.jpg` pour la jaquette verticale 2:3, `library_hero.jpg` pour la carte horizontale sélectionnée et `capsule_616x353.jpg` pour le wallpaper officiel avec logo. `header.jpg` n'est qu'un fallback visuel si un asset manque.
- Une seule identité Steam est liée à une installation Orivo dans ce gate. Déconnecter Steam supprime seulement le secret du Trousseau ; les jeux déjà importés restent dans le catalogue local jusqu'à ce qu'une action produit explicite les retire.

Limites explicites de ce gate : le temps de jeu est celui renvoyé par Steam pour le compte lié ; les succès, le multi-compte, la gestion détaillée d'installation/désinstallation et les autres plateformes restent hors périmètre.

## Suite immédiate

1. Ajouter les migrations explicites et leurs fixtures versionnées.
2. Ajouter les tests de migration et la matrice d'erreurs avant le prochain gate fonctionnel.
3. Ajouter les succès et une stratégie multi-compte explicite avant d'étendre les données Steam sensibles.
