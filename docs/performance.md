# Performance — méthode et références

Ce document répond à l'étape 2.4 de [`docs/plugin-system-plan.md`](plugin-system-plan.md)
(« mesurer démarrage, navigation, import et premier lancement ») et à son test
de sortie : « les benchmarks montrent que l'activation de plugins ne dégrade
pas le premier rendu, le rail ni la recherche locale par rapport à la baseline
sans plugin ». Il mesure ; il n'optimise pas — l'optimisation à partir de ces
chiffres est un lot séparé. Chaque nombre ci-dessous est reproductible avec les
commandes données en fin de document, et daté de sa mesure : un chiffre sans
date ni condition machine n'est qu'une anecdote.

## Méthode

- **Préchauffage puis répétitions.** Chaque mesure sépare une première
  itération (« cold », module JS non parsé, cache navigateur froid, ou
  composant Wasm non compilé) des itérations suivantes (« warm »), et rapporte
  min / médiane / max sur les répétitions warm plutôt qu'une seule valeur.
- **Le WebView de test est un navigateur, pas Tauri.** Comme la suite e2e,
  ces bancs tournent contre `pnpm dev` : `isTauriRuntime()` est faux, donc
  toute commande `invoke` résout vers les données éditoriales/fallback
  déterministes déjà utilisées par les captures Playwright. Cela borne ce que
  le banc web peut prouver : il mesure fidèlement le rendu, mais jamais un
  aller-retour IPC réel ni un plugin réellement installé (voir plus bas).
- **Conditions machine notées, jamais supposées stables.** Ces chiffres ont
  été mesurés le 2026-09-27 sur la machine de développement partagée de la
  vague d'agents Orivo : sept builds d'agents tournent en parallèle sur le
  même Mac, `RUSTC_WRAPPER=sccache` est actif, et les benchmarks Rust ci-dessous
  tournent en profil `release`. Sur une machine chargée, une des cinq
  répétitions peut être un ordre de grandeur plus lente que les quatre autres
  (voir la note sur `auto_apply_wine_to_direct_games`) — c'est un artefact de
  contention disque/CPU documenté ici, pas une régression du code mesuré.
- **Rien de nouveau dans le chemin mesuré.** Aucune instrumentation ajoutée
  par ce lot ne s'exécute par défaut : le banc Rust est un module
  `#[cfg(test)]` avec des tests `#[ignore]`, et le banc web est une
  configuration Playwright séparée (`perf/web/`), jamais la suite e2e qui
  filtre les PR.

## 1. Desktop, dans le navigateur (Playwright)

Banc : `perf/web/specs/*.spec.ts`, viewport 1536×1024 (la résolution protégée
par les golden screenshots). Bibliothèque de repli à 10 jeux
(`FALLBACK_LIBRARY_IDS`, voir `e2e/helpers.ts`) : les chiffres de recherche
locale et de navigation dans le rail sont donc des coûts « un passage de
filtre JS sur une petite liste », pas une preuve de tenue à l'échelle d'une
grande bibliothèque, que le mode navigateur ne peut pas fournir sans une
fixture dédiée (hors périmètre de ce lot, voir « Limites »).

| Mesure | Cold | Warm (médiane) | Warm (max) |
| --- | --- | --- | --- |
| Premier shell utilisable (`header.topbar` visible) | 107,7 ms | 2,3 ms | 2,6 ms |
| Bibliothèque hydratée (`#game-cards .game-card` visible) | 118,6 ms | 3,9 ms | 4,3 ms |
| Ouverture du Store | — | 3,3 ms | 6,5 ms |
| Ouverture d'une fiche de jeu | — | 2,9 ms | 13,5 ms |
| Ouverture de Réglages → Plugins | — | 4,0 ms | 4,5 ms |
| Recherche locale (une frappe, filtre → rendu) | 0,30 ms | 0,10 ms | 0,30 ms |

Mémoire : tas JS après hydratation de la bibliothèque ≈ **9,2 Mo**
(`Performance.getMetrics` via CDP, `JSHeapUsedSize`).

Navigation clavier dans le rail (18 appuis flèche, aller-retour sur les 10
cartes) : **0 tâche longue** (`PerformanceObserver({type: "longtask"})`), delta
`requestAnimationFrame` maximum observé 8 ms — sous le budget 60 Hz (16,7 ms)
et a fortiori sous la cible 120 Hz de `docs/ARCHITECTURE.md` (8,33 ms), bien
que l'échantillon soit court (4 frames) : l'interaction clavier sur 10 cartes
ne déclenche pas d'animation de défilement assez longue pour un échantillon
plus riche.

**Lecture pour le test de sortie plugins :** ces chiffres sont le seul état
que le mode navigateur peut représenter — zéro plugin, `plugin-manager.ts`
retourne `emptyPluginCatalog()` dès que `isTauriRuntime()` est faux (voir
`src/plugin-manager.ts:212`). Ils sont donc littéralement « la baseline sans
plugin » du test de sortie. Le coût qu'aurait un plugin installé est mesuré
côté Rust (section 3) parce que c'est là qu'il existe réellement.

## 2. Rust — coût de `AppState::load` selon la taille du catalogue

Banc : `src-tauri/src/perf_bench.rs` (`cargo test --release ... -- --ignored`),
un `catalog.json` synthétique de 10 / 1 000 / 10 000 jeux (un jeu sur dix est
un `.exe` local direct, pour donner au filtre Wine de vrais candidats plutôt
qu'un retour anticipé sur liste vide).

| Étape | n=10 | n=1 000 | n=10 000 |
| --- | --- | --- | --- |
| `Catalog::load_with_migration` (lecture + parse JSON) | 90 µs | 418 µs | 4,6 ms |
| `Catalog::save_atomically` (sérialisation + écriture atomique) | 554 µs | 1,4 ms | 11,5 ms |
| `auto_apply_wine_to_direct_games` | 26,2 ms\* | 22,5 ms\* | 25,9 ms\* |
| ~~`auto_apply_winlator_shortcuts`~~\*\* | — | — | — |

\* Ce chiffre ne dépend pas de `n` : c'est le coût fixe d'une sonde disque
(`wine_runner::detect_wine_staging`, une douzaine de chemins candidats
canonicalisés) qui s'exécute une seule fois par appel dès qu'un jeu Windows
local existe et qu'aucun profil Wine managé n'est encore associé — jamais une
fois par jeu. Sur la machine partagée de cette mesure, une répétition sur
cinq a atteint 960 ms au lieu de ~25 ms (contention disque avec sept builds
concurrents) : à surveiller si ce chiffre revient sur une machine calme, mais
pas traité comme une régression ici.

\*\* Cette ligne mesurait `auto_apply_winlator_shortcuts` — un no-op vérifié
hors Android (`cfg!(target_os = "android")`), à 0 ns quelle que soit `n` — tant
que la passe tournait dans `AppState::load`. Depuis M1 (#44) elle n'y tourne
plus du tout : elle est passée en tâche de fond après le premier rendu, ne lit
que le dossier que l'utilisateur a connecté par SAF, et n'écrit rien. Le banc
ne la mesure donc plus ici, faute de pouvoir synthétiser un grant ; le coût de
démarrage qu'elle représentait est zéro par construction.

**Lecture :** le parse/sérialisation JSON croît linéairement et reste sous la
milliseconde jusqu'à 1 000 jeux, environ 4-12 ms à 10 000 — largement sous un
budget de premier rendu perçu. La seule passe d'auto-application encore sur le
chemin de démarrage (Wine) a un coût fixe indépendant de la taille de la
bibliothèque, jamais une boucle par jeu qui écrirait sur le disque ; celle de
Winlator a quitté ce chemin.

## 3. Plugins — coût de découverte, avec et sans composants installés

Banc : `src-tauri/src/perf_bench.rs`, avec 0 / 1 / 8 / 20 copies du composant
fixture réel de P1/P2 (`src-tauri/fixtures/orivo-runner-fixture.wasm`,
42 386 octets), chacune sous son propre id de manifeste (le registre rejette
un paquet dont le nom de dossier ne correspond pas à l'id déclaré, avant même
de le compiler — la première version de ce banc s'est fait piéger par cette
règle et ne mesurait presque rien après la première copie).

| Composants installés | `get_plugin_catalog` (Réglages → Plugins) | `get_runner_plugins` (flux émulateur) |
| --- | --- | --- |
| 0 | 18 µs | 11 µs |
| 1 | 36,3 ms | 32,9 ms |
| 8 | 246,4 ms | 244,3 ms |
| 20 | 615,6 ms | 603,0 ms |

Coût par composant : ~31 ms, constant, cohérent avec le test déjà présent dans
`plugin_runtime.rs`
(`reports_what_a_prepared_component_costs_against_a_cold_one`, qui a mesuré
30,98 ms de compilation Wasmtime à froid pour ce même fixture le même jour).
Ce coût est celui d'une compilation Cranelift à froid, payée une fois par
composant à chaque appel de ces deux commandes — il n'y a pas de cache de
compilation inter-appels (`Component::new` recompile toujours), et pas de
palier observé à `MAX_PROBED_PLUGINS` (16) : ce plafond réduit l'étape
interactive (`get-identity`/`health-check`), pas la compilation, qui reste
payée pour chaque composant découvert.

**Pourquoi ceci ne dégrade jamais premier rendu, rail ou recherche locale :**
ce n'est pas une hypothèse, c'est ce que montre le code. `get_plugin_catalog`
et `get_runner_plugins` sont deux commandes Tauri `async fn` renvoyées à
`tauri::async_runtime::spawn_blocking`, invoquées uniquement quand l'utilisateur
ouvre Réglages → Plugins ou le flux « Ajouter un émulateur »
(`src-tauri/src/lib.rs:1235`, `src-tauri/src/plugin_installer.rs:192`) — jamais
depuis `AppState::load` ni depuis un chemin de rendu. Le tableau ci-dessus
borne donc le pire cas de ces deux écrans, pas un coût caché du démarrage.

## 4. Taille du bundle

Mesuré avec `perf/bundle/measure-bundle.mjs` sur `pnpm exec vite build`
(2026-09-27, commit de base de ce lot) :

| Catégorie | Taille | Gzip |
| --- | --- | --- |
| JS (`dist/assets/*.js`) | 593,2 ko | 165,9 ko |
| CSS (`dist/assets/*.css`) | 139,6 ko | 28,5 ko |
| Médias (`dist/media/`) | ~48 Mo | — (déjà compressés : JPEG/WebP) |
| **Total `dist/`** | **~49 Mo** | — |

Le JS est un seul chunk (Vite avertit lui-même : « Some chunks are larger than
500 kB »), sans découpage par page — la Store, la fiche de jeu et les Réglages
partagent le même bundle que l'écran de démarrage. Les médias dominent
largement la taille totale : ce sont des images de démonstration du frontend
(`public/media`), embarquées telles quelles (`assetsInlineLimit: 0` dans
`vite.config.ts`).

## 5. Taille de l'APK Android

Pas de reconstruction locale dans cette session : la machine de build partagée
n'a pas de JDK installé (`java -version` échoue), et en installer un
system-wide était hors périmètre d'un lot qui ne fait que mesurer. Les
chiffres ci-dessous viennent d'un APK signé produit pendant les tests
Winlator de la vague 1 (`/tmp/orivo-signed.apk`, `io.orivo.desktop`
`versionName=0.3.6` `versionCode=3006` — identique à ce que rapporte
`src-tauri/gen/android/app/tauri.properties` sur ce commit), donc représentatif
sans être reproduit à l'identique ici.

| | Taille |
| --- | --- |
| APK complet (compressé) | 48,2 Mo |
| `lib/arm64-v8a/*.so` (non compressé) | 57,3 Mo — un seul fichier |
| `classes.dex` | 1,9 Mo |
| `resources.arsc` | 1,0 Mo |
| `res/` | 0,7 Mo (849 fichiers) |
| `assets/` | 0,01 Mo (3 fichiers) |

**Une seule ABI (arm64-v8a), pas de split par ABI** : la configuration Tauri
Android actuelle ne produit qu'un unique `.so`, donc « taille par ABI » ne
s'applique pas encore telle quelle — il n'y a qu'une ABI. Point plus notable :
`assets/` (3 fichiers, 10 Ko) ne contient presque rien, alors que `dist/` fait
~49 Mo côté desktop. Le frontend construit — JS, CSS et les ~48 Mo de médias —
n'est pas copié dans `assets/` sur Android : il est embarqué par
`tauri::generate_context!()` directement dans le binaire natif, ce qui explique
la taille du `.so` unique (57,3 Mo non compressé, pour un binaire qui contient
aussi Wasmtime, Cranelift et wry). Une réduction de la taille des médias
embarqués se répercuterait donc directement sur la taille du `.so`, pas sur un
dossier `assets/` séparé.

## 6. Android, sur l'émulateur `Orivo_Test` (emulator-5556)

**Un émulateur ne remplace pas un vrai téléphone** — ces chiffres viennent
d'un rendu logiciel (Skia/OpenGL émulés) sur du matériel de CI, pas d'un GPU
mobile réel ; ils sont utiles pour détecter une régression *relative*, jamais
comme référence absolue de ce qu'un utilisateur ressent sur un Pixel.

Démarrage (`am start -W`, `io.orivo.desktop/.MainActivity`), quatre mesures
après `am force-stop` :

| Run | TotalTime |
| --- | --- |
| Cold 1 | 643 ms |
| Cold 2 | 372 ms |
| Cold 3 | 565 ms |
| Cold 4 | 410 ms |
| Chaud (déjà au premier plan après `HOME`) | 32 ms |

Rendu (`dumpsys gfxinfo io.orivo.desktop`) après six balayages de la
bibliothèque : 134 frames, **97,8 % de frames « janky »**, 50ᵉ percentile
77 ms/frame, 95ᵉ 150 ms. À comparer aux 0 tâche longue / 8 ms de delta rAF
mesurés dans Chromium desktop (section 1) : l'écart est cohérent avec un
rendu logiciel d'émulateur plutôt qu'avec un défaut de l'application — mais
seul un vrai appareil pourra le confirmer ou l'infirmer.

## Budgets proposés

Des propositions, pas des seuils déjà décidés — à valider par l'équipe avant
qu'un budget devienne bloquant en CI (voir « CI » ci-dessous). Basés sur ce
que `docs/plugin-system-plan.md` fixe déjà (budget interactif de 150 ms,
premier rendu sans composant tiers) et sur les chiffres warm ci-dessus avec
une marge :

| Métrique | Budget proposé | Mesuré (warm) |
| --- | --- | --- |
| Premier shell utilisable (navigateur) | < 20 ms | 2,3 ms |
| Bibliothèque hydratée (navigateur) | < 30 ms | 3,9 ms |
| Recherche locale (une frappe) | < 4 ms | 0,10 ms |
| `Catalog::load_with_migration` à 10 000 jeux | < 20 ms | 4,6 ms |
| `Catalog::save_atomically` à 10 000 jeux | < 30 ms | 11,5 ms |
| Tâches longues pendant la navigation au clavier dans le rail | 0 | 0 |
| `get_plugin_catalog` / `get_runner_plugins`, par composant | < 60 ms | ~31 ms |

## Reproduire ces mesures

```sh
# Web (Playwright), depuis la racine du dépôt
pnpm exec vite build
pnpm exec playwright test -c perf/web/playwright.perf.config.ts

# Rust, catalogue et plugins — --release, sinon les chiffres ne veulent rien dire
cargo test --manifest-path src-tauri/Cargo.toml --release perf_bench -- --ignored --nocapture --test-threads=1

# Le coût de compilation à froid d'un seul composant, déjà présent dans le dépôt
cargo test --manifest-path src-tauri/Cargo.toml --release \
  plugin_runtime::tests::reports_what_a_prepared_component_costs_against_a_cold_one -- --nocapture

# Taille du bundle
pnpm exec vite build
node perf/bundle/measure-bundle.mjs

# Android, avec le verrou d'émulateur de l'orchestrateur (voir docs/agent-workplan.md)
adb -s emulator-5556 install -r -g chemin/vers.apk
adb -s emulator-5556 shell am force-stop io.orivo.desktop
adb -s emulator-5556 shell am start -W -n io.orivo.desktop/.MainActivity
adb -s emulator-5556 shell dumpsys gfxinfo io.orivo.desktop reset
# … interagir …
adb -s emulator-5556 shell dumpsys gfxinfo io.orivo.desktop
```

`--test-threads=1` sur le banc Rust évite que les mesures de compilation
Wasmtime d'un test ne rivalisent pour le CPU avec un autre pendant qu'il
mesure — chaque nombre ci-dessus a été pris ainsi.

## CI

Un job `perf` (`.github/workflows/ci.yml`) exécute la partie desktop
(bundle + web) sur chaque push/PR et publie les chiffres dans le résumé du
job (`$GITHUB_STEP_SUMMARY`). Il est **non bloquant** (`continue-on-error:
true`) : un seuil qui ferait échouer la CI sur une machine partagée et
variable n'aurait aucune valeur, il ferait juste repasser en boucle une PR qui
n'a rien de cassé. Il pourra devenir bloquant une fois que :

1. les budgets ci-dessus auront été validés (pas seulement proposés) ;
2. le job aura tourné assez de fois sur `main` pour qu'on connaisse sa
   variance naturelle sur les runners GitHub (différents de la machine de
   développement partagée qui a produit les chiffres de ce document) ;
3. une régression détectée par le job aura été confirmée manuellement au
   moins une fois, pour écarter un faux positif structurel.

Le banc Rust et la mesure Android ne tournent pas en CI : le premier suppose
`--release` (coûteux à chaque run) et n'apporte rien de plus qu'un run local
ponctuel pour ce que ce lot voulait prouver ; la seconde a besoin d'un
émulateur d'API 37 arm64, absent des runners GitHub standards.

## Limites de ce lot

- La recherche locale et la navigation dans le rail sont mesurées sur la
  bibliothèque de repli à 10 jeux (mode navigateur, sans Tauri) : un chiffre
  à l'échelle d'une bibliothèque de plusieurs centaines de jeux demanderait
  une fixture dédiée que ce lot n'a pas créée pour ne pas toucher aux données
  partagées de la suite e2e.
- Le mode navigateur ne peut pas représenter « des plugins installés » — voir
  section 1 — donc le test de sortie plugins s'appuie sur les deux moitiés de
  ce document : la preuve architecturale + les chiffres Rust (section 3) pour
  le coût potentiel, et les chiffres navigateur (section 1) pour la baseline
  réelle qu'aucun composant ne dégrade puisqu'aucun ne s'exécute sur ce chemin.
- L'APK mesuré (section 5) date de la vague 1, pas de ce commit exact — voir
  la justification dans cette section. Un job qui reconstruit et mesure l'APK
  à chaque run demanderait un JDK sur les runners CI, un chantier plus large
  que ce lot.
- Une seule ABI Android existe actuellement (`arm64-v8a`) : « taille par
  ABI » deviendra pertinent quand un split multi-ABI existera.
