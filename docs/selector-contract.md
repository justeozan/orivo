# Orivo Selector — contrat local v0.1

Ce document fixe le périmètre du premier vertical slice : découvrir un jeu local, le sélectionner et le lancer depuis une scène fullscreen. Le contrat est volontairement indépendant du frontend Tauri TypeScript/CSS afin que le noyau Rust et le catalogue puissent ensuite être partagés avec le runtime Windows et une synchronisation distante.

## Version et compatibilité

- `schema_version` est un entier global, initialisé à `1`.
- Un fichier avec une version majeure inconnue est refusé sans être modifié.
- Une version mineure connue peut être chargée si tous ses champs obligatoires sont présents.
- Toute migration est explicite, déterministe et sauvegarde d'abord le fichier source en `.bak`.
- Une migration échouée laisse le catalogue original intact et expose une action `Restore backup`.
- Les champs inconnus sont conservés lors d'une lecture/écriture compatible ; le Selector n'en dépend jamais pour s'afficher.

## Entrée de catalogue

La source v0.1 peut être JSON ou SQLite, mais son modèle logique reste le même :

```text
Catalog {
  schema_version: 1
  games: [Game]
}

Game {
  id: string                 // stable, unique, non vide
  title: string              // non vide
  executable_path: path      // fichier local, obligatoire pour le lancement
  working_directory: path?   // sinon dossier parent de l'exécutable
  arguments: [string]        // arguments déjà tokenisés, jamais une commande shell
  description: string?       // sous-titre local affiché dans le hero
  metadata: string?           // informations compactes affichées sous le sous-titre
  artwork_path: path?        // hero image statique ou fond panoramique
  cover_path: path?          // jaquette verticale du rail
  logo_path: path?           // logo transparent affiché dans le hero
  hero_video_path: path?     // réservé au Gate 2
  last_played_at: timestamp?
  play_time_seconds: integer // >= 0
}
```

Règles importantes :

- Un lancement ne passe jamais par un shell. Le runtime construit directement le processus à partir de `executable_path`, `working_directory` et `arguments`.
- Le WebView ne transmet que `game_id` à la commande `launch_game`. Le backend résout ensuite le jeu, le chemin, le répertoire et les arguments depuis le catalogue ; il n'accepte jamais une commande, un chemin ou des arguments libres venant du frontend.
- Un chemin relatif est résolu par rapport au fichier de catalogue, puis normalisé et vérifié.
- Un exécutable absent produit une erreur récupérable ; il ne supprime pas le jeu.
- Un bundle macOS `.app` est accepté à l'import ; `CFBundleExecutable` est résolu vers `Contents/MacOS` et le nom d'affichage du bundle est utilisé quand il existe.
- Deux jeux qui pointent vers le même exécutable sont acceptés mais signalés comme doublons avant import.
- Les fichiers média sont optionnels. L'interface doit rester fonctionnelle avec une image de remplacement.

## Import manuel

Le flux v0.1 est : `Choose executable` → validation du chemin → aperçu des métadonnées → `Add game` → écriture atomique du catalogue. Le dialogue natif est ouvert par une commande Rust Tauri ; le frontend n'obtient pas un accès général au filesystem.

L'import n'exécute aucun fichier et ne scanne pas automatiquement les disques. L'utilisateur garde le contrôle sur les jeux et les médias ajoutés.

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

## Suite immédiate

1. Ajouter les migrations explicites et leurs fixtures versionnées.
2. Ajouter les tests de migration et la matrice d'erreurs avant le prochain gate fonctionnel.
