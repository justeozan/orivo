# Plan — système de plugins Orivo

## Décision

Orivo doit être extensible sans devenir lent, fragile ou opaque. Le noyau reste
responsable de la bibliothèque, des données utilisateur, des permissions, du
lancement, du cache et de l'interface. Un plugin apporte une capacité précise,
mais ne reçoit jamais un accès général au système.

Le format cible est un **composant WebAssembly** exécuté par Wasmtime derrière
des contrats WIT versionnés. Un plugin ne fournit ni JavaScript injecté dans le
WebView, ni binaire natif chargé dans le processus principal.

Les adapters de première partie critiques, comme Steam, restent du Rust natif
pendant la phase de lancement du produit. Ils suivent les mêmes contrats que les
plugins afin de servir de référence et de tests, mais ne rendent pas le démarrage
dépendant du runtime plugin.

## État d’implémentation v0.3

Le socle est en place : target de lancement `Runner` et migration catalogue v5,
validation stricte des manifestes/capabilities, découverte lazy des composants
hashés, préflight Wasmtime Component Model dans un worker, contrat WIT v1 et
runner officiel Wine-Staging macOS. Wine est un adapter Rust natif de référence
: profils persistants isolés, dossiers accordés, inventaire privé et lancement
par `LaunchIntent` typé sans shell. Wine-Staging s'applique désormais
**automatiquement** à tout jeu local `.exe` via un profil géré par défaut
provisionné sans assistant (détection de l'engine, préfixe géré, dossier
accordé dérivé du seul répertoire de chaque `.exe`), avec une association
réversible qui conserve la fiche Direct d'origine. Le backend DXVK-macOS est le
défaut sur Apple Silicon (détecté via `hw.optional.arm64`) : archive épinglée
allowlistée, téléchargée puis hachée par le host, DLL copiées dans le seul
préfixe Orivo et override hôte fixe, sans GPTK ni CrossOver.

Restent volontairement hors de cette première intégration : l’installateur de
packages signés, l’invocation WIT de composants tiers avec grants, l’import de
ROMs et les runners GPTK/CrossOver. Les targets Runner tiers continuent donc à
échouer explicitement au lieu d’accepter une commande libre. Wine-Staging ne
charge pas un faux composant Wasm : il applique le contrat WIT `prepare-launch`
comme adapter natif, puis le host valide les IDs opaques et possède le processus.

## Les promesses à préserver

1. Le shell et la bibliothèque locale apparaissent sans attendre un plugin.
2. Une défaillance, un timeout ou une mise à jour de plugin ne bloque jamais la
   navigation, la recherche locale, ni un lancement direct/Steam.
3. Le WebView ne passe que des IDs Orivo. Il ne passe jamais de chemin, de ROM,
   d'argument de commande ou de capacité.
4. Les données canoniques appartiennent à Orivo. Le plugin ne renvoie que des
   propositions normalisées, validées et écrites transactionnellement par l'hôte.
5. Une capability est demandée, affichée à l'utilisateur, accordée pour un scope
   précis, révocable, puis journalisée.
6. Désactiver ou supprimer un plugin conserve les jeux, préférences et sessions
   déjà importés. Seules ses caches régénérables peuvent être supprimées.

## Modèle produit

Un plugin est un paquet signé qui expose une ou plusieurs extensions, mais une
seule responsabilité principale :

| Type | Exemple | Peut faire | Ne peut pas faire |
| --- | --- | --- | --- |
| `source` | Epic, GOG | découvrir et synchroniser des jeux | modifier directement le catalogue |
| `runner` | Ryujinx, PCSX2, Wine/CrossOver | préparer le lancement d'un jeu via un émulateur | recevoir une commande shell libre |
| `metadata` | IGDB, HowLongToBeat | proposer descriptions, tags et médias | écraser une donnée utilisateur |
| `search` | recherche de guides | proposer des résultats structurés | ralentir la recherche locale |
| `automation` | import/export, sauvegarde | exécuter un job explicite | tourner en boucle sans budget |
| `ui-contribution` | badge, carte, commande, réglage | fournir des données d'interface validées | injecter HTML, CSS ou JavaScript |

Une mini-app entièrement libre n'est pas une extension du premier SDK. Elle
arrive seulement après que les surfaces de données structurées auront prouvé
leurs limites.

## Architecture d'exécution

```text
WebView TypeScript
  └─ command/query Orivo avec IDs stables uniquement
       └─ application Rust
            ├─ catalogue SQLite + projections locales
            ├─ Launch service et platform service
            ├─ Job scheduler borné et annulable
            └─ Plugin host worker
                 ├─ validation manifest + grants
                 ├─ Wasmtime + WIT
                 ├─ limites mémoire, fuel, deadline et journal
                 └─ résultats typés → validation hôte → transaction SQLite
```

Le plugin host est un worker permanent, séparé du chemin de rendu. Au démarrage,
Orivo lit seulement les manifestes et l'état des plugins depuis SQLite. La
compilation/préparation d'un composant et toute synchronisation partent après le
premier rendu. Une seconde étape peut déplacer ce worker dans un helper process
`orivo-plugin-host` si les mesures ou la marketplace montrent qu'une isolation
de crash supplémentaire est nécessaire ; l'ABI WIT et la file de jobs restent
les mêmes.

## Contrat de performance

Les limites suivantes sont des critères d'acceptation de la première version,
à mesurer sur un Mac cible et à ajuster uniquement avec un benchmark enregistré.

| Chemin | Règle |
| --- | --- |
| Démarrage | aucun composant tiers n'est requis avant le premier shell utilisable |
| Navigation, rail, recherche locale | aucune invocation plugin synchrone |
| Appui sur Play | passage immédiat à `Launching`; résolution du runner dans un job visible et annulable |
| Invocation interactive | budget initial de 150 ms, puis état de progression plutôt qu'attente bloquante |
| Job de fond | concurrence globale bornée, une file par plugin et back-pressure |
| Composant bloqué | fuel + deadline Wasmtime ; trap, annulation et état `degraded`, jamais boucle de retry |
| Mémoire | limite par instance et plafond global ; les instances inactives sont évincées |
| Réseau | cache local, ETag/TTL, domaines explicitement accordés et absence de réseau sur le chemin d'affichage |

Wasmtime permet de précompiler/préparer les composants avant leur première
instanciation et d'appliquer back-pressure. Ses mécanismes de fuel et d'epochs
permettent aussi d'interrompre une exécution qui ne rend pas la main. Ces options
doivent être activées dans le host, pas laissées à chaque plugin.

## Package et confiance

Le format d'installation est un `.orivo-plugin`, archive signée contenant :

```text
manifest.json       # identité, version, ABI WIT, capabilities, hashes
component.wasm      # unique code exécutable du plugin
assets/             # icône, traductions et schémas de réglages, non exécutables
signature.ed25519   # signature du manifest et des hashes
```

Le manifest contient un identifiant stable inverse-DNS, une version sémantique,
une version minimale d'Orivo, les extensions annoncées, les capabilities et les
domaines réseau demandés. L'installation vérifie le hash, la signature,
compatibilité ABI et taille avant toute écriture durable.

Deux canaux existent :

- **officiel** : clé de signature connue et mises à jour automatiques après
  consentement global ;
- **développeur/local** : signature de test visible, mises à jour manuelles et
  bannière permanente. Il ne peut pas se faire passer pour officiel.

La marketplace ne distribue jamais un binaire natif, un script shell, une
extension Tauri ou une page Web privilégiée. Les mises à jour sont téléchargées,
vérifiées et préchauffées en arrière-plan ; un rollback conserve la version
précédente tant que le nouveau composant n'a pas passé son smoke test.

## Capabilities minimales

Les capabilities sont étroites et orientées tâche, jamais `filesystem:*` ou
`shell:*` :

| Capability | Scope utilisateur | Exemple |
| --- | --- | --- |
| `library.read` | jeux associés au plugin | lire les métadonnées utiles à un runner |
| `files.read` | dossiers choisis dans un picker natif | scanner une bibliothèque de ROMs |
| `network.fetch` | liste de domaines approuvés | récupérer une jaquette depuis IGDB |
| `secrets.read/write` | coffre propre au plugin et clés nommées | conserver un jeton Epic |
| `runner.prepare` | profils de lancement validés | préparer le lancement d'une ROM |
| `notifications.send` | notifications Orivo | signaler qu'un import est terminé |

Le plugin ne voit jamais un chemin non accordé, un secret d'un autre plugin ou
le Trousseau brut. Les secrets passent par un coffre hôte namespacé. Tous les
grants sont visibles dans Réglages → Plugins et peuvent être retirés sans
désinstaller le plugin.

## Intégration des émulateurs

L'option « Ajouter un émulateur » ouvre un futur flux hôte, pas un formulaire
propre à chaque plugin :

```text
Ajouter un émulateur
  → sélectionner un plugin runner installé
  → choisir l'application d'émulation via picker natif
  → choisir un ou plusieurs dossiers de jeux via picker natif
  → créer un profil runner validé
  → lancer un import en arrière-plan
  → jeux normalisés dans la bibliothèque Orivo
```

Le plugin runner connaît son format de bibliothèque et ses paramètres. Orivo
possède le profil, les chemins accordés, le jeu, son artwork, son état et la
décision finale de lancer un processus.

Le cycle de lancement devient :

```text
Orivo → game_id → LaunchTarget::Runner { runner_id, game_ref }
      → profil runner validé → plugin prépare une LaunchIntent typée
      → hôte valide l'intent et construit le processus sans shell
      → émulateur → jeu
```

`LaunchIntent` n'est jamais une chaîne de commande. C'est une structure fermée,
par exemple : `runner_id`, `game_ref`, `profile_id`, `launch_mode` et les options
déjà autorisées par le profil. Le host résout ensuite l'app d'émulation, le
répertoire de travail, les arguments tokenisés et le fichier jeu dans les scopes
accordés. Cette extension remplace le `launch_target` actuel `Direct | Steam`
par une union versionnée compatible :

```text
Direct { installation_id }
Steam { app_id }
Runner { runner_id, game_ref, profile_id }
```

Ainsi, une ROM ne devient jamais un faux exécutable et le plugin n'obtient jamais
le droit de lancer n'importe quoi sur le Mac.

## Contrats WIT v1

Le SDK commence petit. Chaque interface est versionnée séparément et les types
sont extensibles sans champs JSON opaques dans les zones de sécurité.

```text
plugin-core@1.0      identity, health-check, logging, settings schema
source@1.0           discover-page, sync-cursor, normalize records
runner@1.0           validate-profile, discover-page, prepare-launch
metadata@1.0         enrich(game references), media candidates
ui-contrib@1.0       commands, badges, settings schema, cards data
```

`discover-page` est paginé et reprend avec un curseur. Les synchronisations sont
idempotentes grâce à une clé de source externe stable. `prepare-launch` est
court, sans réseau et sans scan disque complet. Tout travail plus long est un job
retournant un `operation_id`, dont l'UI peut afficher l'avancement ou annuler.

Les contributions UI sont rendues par des composants Orivo : texte, icône
packagée, action déclarative, liste, badge, card et schéma de réglages. Elles ne
reçoivent pas le DOM, la CSP, les capabilities Tauri ou le pont IPC.

## Données locales et observabilité

SQLite reçoit les tables versionnées suivantes :

```text
plugins                id, version, channel, state, manifest_hash, installed_at
plugin_grants          plugin_id, capability, scope, granted_at, revoked_at
plugin_profiles        id, plugin_id, kind, encrypted_settings, status
plugin_jobs            id, plugin_id, kind, state, cursor, attempts, next_run_at
plugin_health          plugin_id, last_ok_at, failure_count, disabled_reason
external_refs          provider_id, external_id, orivo_entity_id, fingerprint
launch_targets         game_id, kind, runner_id?, profile_id?, opaque_game_ref
```

Les profils et les références externes survivent à une désactivation. Les caches,
logs techniques et résultats de recherche sont dérivés et purgeables.

Chaque job porte un `correlation_id`, un temps d'exécution, les octets lus/écrits,
un résultat normalisé et une erreur utilisateur. Après des échecs répétés, Orivo
met le plugin en pause avec un bouton de reprise : aucune relance infinie ni
toast à chaque démarrage.

## Parcours d'intégration

### Étape 0 — préparer le noyau

1. Extraire les types de catalogue/lancement de `src-tauri/src/catalog.rs` vers
   une frontière de domaine réutilisable.
2. Migrer le catalogue JSON v2 vers SQLite avec migrations et sauvegarde, sans
   perdre les launch targets `Direct` et `Steam`.
3. Ajouter `Runner` comme troisième launch target typé, mais sans runtime
   plugin ni interface utilisateur de configuration.
4. Mettre le lancement derrière un `LaunchService` qui valide le target et
   retourne des états `Launching`, `Running`, `Error` structurés.

### Étape 1 — un SDK interne, pas encore de marketplace

1. Écrire les WIT `plugin-core`, `source` et `runner` v1.
2. Implémenter le plugin host, les grants, les limites et le scheduler borné.
3. Créer un plugin runner de test qui ne peut lire qu'un dossier fixture et
   produit une `LaunchIntent` contrôlée.
4. Exécuter l'import et le lancement derrière le même contrat, avec tests de
   permission refusée, timeout, trap, annulation et reprise.

### Étape 2 — premier émulateur utile

1. Publier un plugin runner officiel pour **un seul** émulateur macOS dont le
   format de bibliothèque et le lancement sont vérifiables.
2. Terminer le flux « Ajouter un émulateur » : sélection de l'app, sélection
   des dossiers, création de profil, preview, import, réconciliation.
3. Ajouter l'écran Réglages → Plugins : état, permissions, profils, logs
   compréhensibles, désactivation et suppression du cache.
4. Mesurer démarrage, navigation, import et premier lancement avant d'ajouter
   une seconde intégration.

### Étape 3 — ouverture contrôlée

1. Signatures, registry officiel et mises à jour transactionnelles avec
   rollback.
2. Kit développeur, fixtures, simulateur de host, tests de compatibilité WIT
   et validation automatisée du manifest.
3. Plugins `metadata`, `search` et `ui-contrib` limités aux surfaces déclarées.
4. Canal développeur local, puis soumission/revue marketplace quand le modèle
   de confiance est éprouvé.

## Tests de sortie

- Orivo affiche la dernière bibliothèque locale même si tous les plugins sont
  absents, désactivés ou en erreur.
- Un plugin ne peut pas lire un second dossier, contacter un second domaine ou
  lancer un second binaire sans nouveau grant explicite.
- Un runner peut importer un jeu et le relancer après un redémarrage sans
  rescanner toute la bibliothèque.
- Un composant en boucle est interrompu, son job est marqué en échec et la
  navigation reste fluide.
- Un rollback de plugin conserve les jeux importés et les profils compatibles.
- La migration de `Direct | Steam` vers `Runner` est testée sur des fixtures et
  restaure une sauvegarde si elle échoue.
- Les benchmarks montrent que l'activation de plugins ne dégrade pas le premier
  rendu, le rail ni la recherche locale par rapport à la baseline sans plugin.

## Hors périmètre initial

- émuler directement une console dans Orivo ;
- accepter des scripts shell, DLL/dylib ou extensions Tauri tierces ;
- laisser un plugin dessiner librement dans le WebView ;
- synchroniser des ROMs ou fichiers de jeu vers un serveur Orivo ;
- marketplace publique avant signatures, rollback, limites et revue de package.

## Références

- [Wasmtime — composants WebAssembly, WASI et Component Model](https://docs.wasmtime.dev/)
- [Wasmtime — préparation et instanciation rapide](https://docs.wasmtime.dev/examples-fast-instantiation.html)
- [Wasmtime — interruption par fuel ou epochs](https://docs.wasmtime.dev/examples-interrupting-wasm.html)
