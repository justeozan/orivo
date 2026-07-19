# Revue visuelle du Selector

Le Selector est maintenant un frontend TypeScript/CSS dans une fenêtre Tauri. Une suite navigateur telle que Playwright est donc la surface de régression adaptée : elle peut fixer le viewport, exercer les états du rail et comparer les captures au mock de référence.

## Capture de référence locale

La suite visuelle doit ouvrir le frontend à **1536 x 1024**, charger le catalogue de démonstration et produire un golden versionné dans `tests/visual/`. La cible de comparaison est `assets/moc-images/orivo-full-screen.png`.

Les tests frontend ne doivent pas dépendre d'un catalogue utilisateur, d'un dialogue natif ou d'un accès disque. Ils injectent un `SelectorViewModel` déterministe avec les assets de `public/media/`. Les commandes Tauri réelles sont couvertes séparément par les tests d'intégration Rust et IPC.

## Points comparés au mock

- barre de navigation horizontale et recherche flottante ;
- hero panoramique pleine largeur avec gradient de lisibilité ;
- bloc titre/logo, description, métadonnées et CTA ;
- flèches latérales de sélection ;
- rail `Recently Played` avec cover active ;
- HUD de navigation ancré au bas de la scène ;
- panneaux verre avec blur localisé, bordure et fallback opaque ;
- transitions du hero et de la carte active sans mouvement de layout.

La régression navigateur règle la géométrie, les couches CSS et le comportement reduced-motion. La validation finale se fait aussi dans la fenêtre Tauri empaquetée sur les WebViews cibles : fullscreen sans décoration, `backdrop-filter`, décodage média, clavier/manette et frame pacing sur macOS puis Windows.
