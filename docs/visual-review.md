# Revue visuelle du Selector

Orivo étant une application native Slint/WGPU, Playwright n'est pas la bonne surface d'inspection. Le projet utilise le backend headless de Slint pour rendre la scène en PNG à taille fixe, puis la capture peut être ouverte avec un viewer d'image.

## Capture de référence locale

```bash
cargo test renders_selector_snapshot_for_visual_review -- --nocapture
```

La capture est écrite dans `.context/orivo-selector-snapshot.png` afin de rester hors du suivi Git. Le test injecte les assets Unrailed! du dossier `assets/test-unrailed-assets` et vérifie donc la composition avec un vrai hero, un logo et une cover.

## Points comparés au mock

- barre de navigation horizontale et recherche flottante ;
- hero panoramique pleine largeur avec gradient de lisibilité ;
- bloc titre/logo, description, métadonnées et CTA ;
- flèches latérales de sélection ;
- rail `Recently Played` avec cover active ;
- HUD de navigation ancré au bas de la scène.

Le snapshot headless sert à régler la géométrie et les couches. La validation finale de fluidité, du fullscreen et du rendu Metal reste à faire sur la fenêtre native du MacBook.
