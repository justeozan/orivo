# Plan de travail multi-agents

Ce document découpe ce qui reste à faire sur Orivo en lots confiables à des
agents distincts, avec le modèle et l'effort de réflexion attendus pour chacun.
Il se lit avec [`docs/plugin-system-plan.md`](plugin-system-plan.md), qui décrit
l'architecture visée, et [`docs/TODOS.md`](TODOS.md), qui porte les demandes
produit.

## D'où l'on part

Le système de plugins a **tout l'emballage et aucune exécution**. Le manifeste,
les capabilities, le registre, l'installateur signé Ed25519 et le contrat WIT v1
sont en place et testés ; `plugin_runtime.rs` fait 89 lignes et ne pratique
qu'un préflight Wasmtime. Un plugin tiers est donc validé, installé, vérifié —
et jamais invoqué. Les targets `Runner` tiers échouent explicitement sur
`RunnerUnavailable` (`src-tauri/src/launcher.rs:122`).

Wine occupe une place à part : ce n'est pas un plugin Wasm mais un **adapter
Rust natif** qui respecte le même contrat `prepare-launch`. C'est le précédent
qui permet d'ajouter un runner sans attendre le runtime de composants tiers.

Par rapport aux quatre étapes du plan plugins : l'étape 0 est faite, l'étape 1
est à moitié (contrats oui, exécution non), l'étape 2 est à peine entamée —
seul l'écran Réglages existe — et l'étape 3 n'a que les signatures.

## Les lots

| Lot | Tâche | Modèle | Effort | Dépend de |
|---|---|---|---|---|
| **P1** | Le vrai host de plugins : invocation WIT de composants tiers, grants, limites fuel/epoch, scheduler borné, annulation. Étapes 1.2 et 1.3 du plan plugins | Opus 5 | **max** | — |
| **P2** | Runner fixture + suite adversariale : permission refusée, timeout, trap, annulation, reprise | Opus 5 | high | P1 |
| **P3** | Registry officiel et mises à jour transactionnelles avec rollback | Opus 5 | high | P1 |
| **P4** | SDK développeur : fixtures, simulateur de host, validation automatisée du manifest, tests de compatibilité WIT | Sonnet 5 | medium | P1 |
| **E1** | Runner Winlator pour Android, en adapter Rust natif calqué sur `wine_runner.rs` | Opus 5 | high | — |
| **E2** | Flux « Ajouter un émulateur » et écran Réglages → Plugins complet : état, permissions, profils, logs lisibles, désactivation, purge du cache | Sonnet 5 | high | E1 |
| **Q1** | Réparer la suite e2e : 75 échecs hérités de deux réécritures de page, re-baseline des goldens, `pnpm test:e2e` dans la CI | Sonnet 5 | medium | — |
| **Q2** | Appliquer au flux Xbox/Microsoft le relais de navigation déjà fait pour Epic | Sonnet 5 | medium | — |
| **Q3** | Corriger `#/me` : `app-page-library` et `app-page-me` sont visibles simultanément, en desktop comme en compact | Haiku 4.5 | low | — |

### Pourquoi ces calibres

**P1 en effort maximum** parce que c'est le seul lot où une erreur devient une
faille : exécuter du code tiers sous grants, avec des limites qui doivent
réellement interrompre une boucle infinie. Quatre lots en dépendent, ce qui en
fait le goulot du chantier.

**P2 au même niveau que P1** : écrire les tests qui cherchent vraiment à casser
un bac à sable demande la même compétence que le bac à sable.

**E1 est indépendant de P1** et c'est tout l'intérêt : Wine a montré qu'un
adapter natif peut respecter le contrat sans runtime Wasm. Winlator est la même
pile (Box64 pour la traduction x86→ARM, Wine, DXVK sur Vulkan), empaquetée pour
Android.

**Q1 sert tout le monde** : tant que la suite e2e est à 75 échecs, aucun agent
ne peut démontrer qu'il n'a rien cassé.

## Ordre d'exécution

**Vague 1, en parallèle immédiat** : Q1, Q2, Q3, E1 et P1. Aucun de ces lots ne
touche les mêmes fichiers.

**Vague 2, après que P1 soit mergé** : P2, P3, P4, et E2 dès que E1 est en
place. Démarrer P2 ou P3 avant P1 revient à construire sur du sable.

**Un worktree git par lot.** Cinq agents dans le même dossier, ce sont cinq
agents qui se marchent dessus dans `lib.rs`.

## Le prompt d'orchestration

À donner à une session Opus 5, telle quelle.

```
Tu orchestres plusieurs agents sur Orivo, un lanceur de jeux Tauri
(Rust + TypeScript vanilla, pas de framework front).
Repo : /Users/spectre/orca/workspaces/orivo/port-louis, branche add-mobile-apk-build.

AVANT TOUT : lis docs/agent-workplan.md, docs/plugin-system-plan.md (en
particulier « État d'implémentation v0.3 » et « Parcours d'intégration »),
CONTRIBUTING.md, docs/ARCHITECTURE.md et docs/TODOS.md.
Ne lance aucun agent avant.

Règles non négociables, à répéter dans CHAQUE prompt d'agent :
- Un worktree git par lot. Jamais deux agents dans le même dossier.
- Un seul sujet par lot : pas de refactor opportuniste dans une PR de feature.
- Tout comportement vient avec un test. Un correctif sans test qui échoue
  d'abord est un correctif qui revient.
- Les commentaires expliquent le POURQUOI, jamais le QUOI, et épousent la
  densité et le ton du fichier édité. C'est une exigence forte de ce repo.
- Aucun secret ne doit atteindre la WebView. Les jetons vivent dans le
  trousseau système et sont lus par Rust.
- Aucune dépendance nouvelle sans justification écrite (gain et coût).
- Doit passer avant toute PR :
    pnpm typecheck && pnpm test
    cargo test --manifest-path src-tauri/Cargo.toml
    pnpm test:e2e
- Le rendu desktop 1536x1024 est protégé par des captures de référence
  Playwright : aucune PR ne doit les invalider sans le dire explicitement.
- Ouvre une PR. Ne merge jamais. Ne pousse pas sur main.

Contexte d'état, utile à tous :
- Le système de plugins a tout l'emballage (manifeste, capabilities, registre,
  installateur signé Ed25519, contrat WIT v1) mais AUCUNE exécution :
  plugin_runtime.rs ne fait qu'un préflight Wasmtime, 89 lignes. Les targets
  Runner tiers échouent sur RunnerUnavailable (launcher.rs:122).
- Wine est un adapter Rust NATIF qui respecte le contrat prepare-launch sans
  passer par Wasm — c'est le précédent à suivre pour tout nouveau runner.
- La suite e2e a 75 échecs hérités de deux réécritures de page. Ce n'est pas
  une régression : les specs et les goldens sont périmés.
- Un mode « compact » existe pour les écrans courts (src/form-factor.ts,
  attribut data-form-factor sur <html>, seuil 560px de haut), avec son propre
  projet Playwright android-914 et e2e/compact.spec.ts.

LOTS (modèle et effort imposés, ne les change pas) :

Vague 1, en parallèle immédiat :
  Q1 [Sonnet 5, medium] Réparer la suite e2e : remettre les specs en face du
      markup actuel, re-baseliner les goldens, ajouter pnpm test:e2e à la CI.
  Q2 [Sonnet 5, medium] Appliquer au flux Xbox/Microsoft le relais de
      navigation déjà fait pour Epic (source_epic.rs ANDROID_RELAY_SCRIPT +
      authorization_code_from_relay_url, câblé dans on_navigation).
  Q3 [Haiku 4.5, low] Corriger #/me : app-page-library et app-page-me sont
      visibles simultanément, en desktop comme en compact.
  E1 [Opus 5, high] Runner Winlator pour Android, en adapter Rust natif
      calqué sur wine_runner.rs : profils, préfixes, LaunchIntent typé, aucun
      shell. Vérifié sur l'émulateur Pixel_8 (emulator-5554).
  P1 [Opus 5, max] Le vrai host de plugins : invocation WIT de composants
      tiers avec grants, limites fuel/epoch, scheduler borné, annulation.
      Étape 1.2 et 1.3 de docs/plugin-system-plan.md. Seul sur
      plugin_runtime.rs. C'est le chemin critique de tout le reste.

Vague 2, seulement après que P1 soit mergé :
  P2 [Opus 5, high] Runner fixture + suite adversariale : permission refusée,
      timeout, trap, annulation, reprise.
  P3 [Opus 5, high] Registry officiel + mises à jour transactionnelles avec
      rollback, sans jamais laisser un plugin à moitié écrit.
  P4 [Sonnet 5, medium] SDK développeur : fixtures, simulateur de host,
      validation automatisée du manifest, tests de compatibilité WIT.
  E2 [Sonnet 5, high] Flux « Ajouter un émulateur » et écran Réglages →
      Plugins : état, permissions, profils, logs lisibles, désactivation,
      purge du cache. Après E1.

Ton rôle : lancer la vague 1, suivre les agents, relire chaque PR contre les
règles ci-dessus, puis lancer la vague 2. Rends-moi après chaque vague un état
en une page : ce qui est mergé, ce qui est bloqué, ce qui a dérivé du plan.
```

## Ce que le plan ne couvre pas

Les demandes de `docs/TODOS.md` encore ouvertes — sélecteurs de fond d'écran, de
vidéo, d'icônes et de jaquettes sur la fiche de jeu, suggestions de jeux par IA
dans le Store — ne sont pas dans les lots ci-dessus. Elles ne dépendent de rien
et peuvent partir à tout moment ; elles sont simplement moins structurantes que
le runtime de plugins.

Sur iOS, l'exécution locale de jeux Windows est hors d'atteinte : sans JIT, pas
de traduction x86 rapide, et Apple ne l'ouvre pas. Le streaming y reste la seule
réponse, et il est traité ailleurs.
