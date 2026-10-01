Dans l'ordre de priorité :

Emulation (plugin?) :
le launcher a pour but d'emuler facilement sur les appareil non compatible, par exemple jeu pc, mais sur mac alors emulateur automatique.

- emulateur windows pour mac pour les jeux pc non compatible.
- emulateur switch astris pensé pour mac
- voir plus tard les autres emulateur.

plugin

- system de plugin (architecture performante)
- recherche et intégration de plugin simplifié (à voir avec une page web)

Page store :

- Visiter les jeux par catégorie ou par source dispo sur les store sans devoir les ajouter à sa gallerie personnel ni les acheter/telecharger.
- ✅ Pouvoir voir les prix sur instant gaming. (badge prix + prix barré + % de réduction sur les cartes)
- Utiliser la reference moc image : orivo-store-clean.png
- prepaer les encadrés titres et informations pour la recommandation (l'image presente toute la structure)



Intelligence artificielle ;
Tres connecté avec le reste.

- aide à la suggestion de jeu dans le store.

Page détail de jeu :

- plus de détail
- sélecteur de fond d'écran et telechargeur facile.
- sélecteur de vidéo et telechargeur facile.
- idem icone et covers

Page moi :
image maquette de reference : orivo-cognitive-scan-page.png

- ✅ trouver les facteurs quantitatif pour mesurer chaque parametre. (v1 : engagement, régularité, diversité, intensité, équilibre — src/me-model.ts)
- ✅ santé cognitif en gros, tout est dans l'image. (v1 de la page "Moi" sur #/me, à raffiner avec la maquette)

Page ma gallerie :

- ✅ les jeux auquel j'ai le plus joué (rangée "Most Played" en haut de la bibliothèque)

Petits soucis que je vois à regler rapidement :

- ✅ remettre les 3 petits points en dessous du titre de jeu
- ✅ à l'ajout d'un jeu local, les images de jeu ne sont pas recherché, il prennent actuellement l'image d'elden ring tout le temps.
- ✅ Hozy Playtest n'a pas de couverture
- certain jeu n'ont pas de wallppaper 4k (ceux de steam) mais c'est normal ça venait avant le patch, et de toute facon ce sera reglable avec le selecteur de wallpaper.
- ✅ s'il n' ya pas dheure de jeu alors ne pas afficher
- ✅ afficher un vrai logo source plus grand (steam ou local)
- ✅ ne pas afficher la mention "wine-staging", "incompatible for macos", ni "installed" car le bouton play le mentionne deja
- ✅ on peut enlever le bouton bookmark
- ✅ le genre du jeu est parfois wrap, et la pill devient moche.
- ✅ les long titres de jeu sont wrap et donc coupé en plein milieu, il faut pas les wrap
- ✅ dans le modal en haut à gauche, dans le menu, il y a 2 boutons qui font doubblons: "importé les jeux installé (steam)" et "se connecter à une bibliothèque" , qui devrais donc les deux etres remplacé par "Sources" et apres Steam si on ajouter "steam", et "ajouter une nouvel source"



Ensuite, d'autres features non catégorisé :

- Creer le plugin quiky version Orivo
- Faire en sorte que tout soit navigable avec la manette et les flèche du clavier (petit detail en clavier: 'a' permet de rentrer dans la page d'entrer du jeu, et 'entrer' de le lancer direct)
- bundle app pour windows, linux et mac (prio). installable facilement, et un bouton qui check la version dans les parametre, et telechargé et update l'app automatiquement si une nouvelle version est détecté. avec CI et tag + release github automatique.
- 

---

j'étais en train de réfléchir en derniers :

- aux moyens d'optimiser l'écritures du style et design (rajouter peut être shadcn et tailwind) pour avoir un system de component facile à remettre partout, pur que ça permette également de réutiliser les styles de Glassmorphism/Blur/Vibrancy dans les elements.
- Implémenter réellement le système de plugin de Orivo, ça passe par du WIT si j'ai bien compris.
- Faire fonctionner des jeux windows sur android/iOS.
- 

---



État au 2026-09-30. Main `e6c7052`, vagues 1-3 mergées.

## Système de plugins — \~85 %

Mergé : runtime Wasm, sandbox fichiers, scheduling, compile cache (P5), manifeste signé, registre (index + mises à jour + rollback), UI Réglages → Plugins (E3), premier runner Ryujinx (P6).

- [ ] Extraire le crate SDK (l'app exporte tout aujourd'hui)
- [ ] `MINIMUM_INDEX_SEQUENCE` + expiration courte avant première publication de l'index
- [ ] Hébergement du registre + détenteur de clé
- [ ] Signer le plugin P6
- [ ] Pagination WIT v2 (cap 256 entrées par dossier)
- [ ] Politique FolderTrust sous Windows
- [ ] Élargir `runner-profile` et modes de lancement

## Émulation mobile (Android) — \~75 %

Mergé : RetroArch (NES/SNES/GB/GBA/Mega Drive) + PPSSPP (PSP) via SAF, consent preview, NES prouvé sur émulateur.

- [ ] Cert pinning RetroArch/PPSSPP (Play ≠ F-Droid)
- [ ] Hash disque complet à l'import
- [ ] Extraire la plomberie JNI de `winlator_saf`
- [ ] Comparaison noms de patch (faux refus, espaces)
- [ ] Paquet Galaxy Store erroné
- [ ] Perf aperçu d'import (\~6 min pour une collection GBA)
- [ ] PSP non prouvé (PPSSPP a refusé le PBP de test)

## Optimisation / perf — \~90 %

Mergé : compile cache HMAC (645→36 ms), import 1000 jeux (164→64 ms), bundle perf job, mesures step 2.4.

- [ ] JPEG 4:2:0 vs 4:4:4 (décision à prendre)
- [ ] `measure-bundle.test.mjs` dans le job perf node

## Parité Windows (sandbox) — \~90 %

Mergé : placeholders OneDrive, identité 128-bit (ReFS/Dev Drive), course deux ouvertures fermée, liste blanche balises reparse, CI Windows rouge→vert.

- [ ] 5 commentaires/README inexacts (non bloquants)
- [ ] E2 fixture bug (chemin Unix dans un test Windows)

## Wine (O2) — \~95 %

Mergé : auto-application sortie du démarrage, tâche de fond paginée et annulable.

- [ ] Événement pour rafraîchir les cartes converties après le premier rendu

## Vérifications manuelles

- [ ] Un vrai jeu Switch avec Ryujinx (procédure dans `docs/ryujinx-runner.md`)
- [ ] Un vrai jeu PSP sur Android (PPSSPP a refusé le PBP de test)
- [ ] Première ouverture Réglages → Plugins après mise à jour (invite trousseau macOS, builds ad hoc)

