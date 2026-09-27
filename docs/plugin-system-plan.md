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

Un second adapter natif existe désormais : le runner **Winlator** pour Android.
Winlator étant une application Android et non une bibliothèque, ce runner est un
passage de relais et non un processus possédé : le profil Orivo n’est qu’une
référence (distribution, container, dossiers accordés), parce que le préfixe Wine
vit dans un container du stockage privé de Winlator qu’Orivo ne peut ni créer ni
lire. Le lancement est une Intent Android explicite émise en JNI, construite par
une fonction pure dont les clés d’extra sont des constantes de compilation — donc
jamais une commande, jamais un shell, et testable sur l’hôte. Ce que Winlator
expose réellement, quelles distributions sont lançables et pourquoi, est consigné
dans [`docs/winlator-runner.md`](winlator-runner.md).

L’installateur de packages est désormais implémenté. Un plugin arrive sous
forme d’archive `.orivo-plugin` (tar gzippé) contenant `manifest.json`,
`component.wasm`, ses assets déclarés et une signature Ed25519 optionnelle. Le
host lit l’archive entièrement en mémoire, borne le nombre d’entrées et la
taille, rejette toute traversée de chemin, revalide chaque empreinte d’artefact
contre le manifeste, applique `validate_plugin_package`, fait passer le
composant par le préflight Wasmtime, puis matérialise le tout dans un
répertoire de staging renommé en place — de sorte qu’un échec ne laisse jamais
un plugin à moitié écrit derrière lui. Deux canaux, deux politiques de
signature : le registre embarqué exige la clé de release d’Orivo, tandis qu’un
paquet choisi à la main par l’utilisateur s’installe en `Development` et
s’affiche comme non signé partout. Un paquet ne peut pas transporter de binaire
natif : `blocked_payload_path` refuse `.exe`, `.dylib`, `.so`, `.dll` et les
scripts, et toute entrée non déclarée dans le manifeste invalide l’archive.

Une extension `installer` complète le contrat v1. Le plugin ne fournit que des
données — un catalogue borné de titres avec URL, empreinte et tailles — et le
host exécute lui-même chaque étape privilégiée : téléchargement HTTPS restreint
à l’allowlist du manifeste et revérifiée à chaque redirection, contrôle
SHA-256, extraction silencieuse de l’installeur Windows via un préfixe Wine
neuf par titre, puis enregistrement dans la bibliothèque en réutilisant la
présentation de la fiche Store d’origine. La WebView n’envoie jamais qu’un slug
opaque.

Le host de composants existe désormais pour de vrai. `plugin_runtime.rs` génère
les liaisons typées du monde `runner-plugin` depuis `wit/`, instancie un
composant vérifié et appelle `plugin-core` et `runner@1`. Trois règles y tiennent
ensemble, parce qu’aucune ne vaut seule.

**Le manifeste décide de ce qui est lié ; le grant décide de ce qui fonctionne.**
Un import que le manifeste n’a jamais déclaré est absent du `Linker`, donc un
composant qui le demande échoue à l’instanciation : il n’exécute pas une
instruction sous une permission que son paquet n’a pas montrée à l’utilisateur.
Une capability déclarée mais non accordée est liée et refuse chaque appel avec
une erreur WIT typée — c’est l’état de tout plugin entre son installation et sa
configuration, et c’est ce qui permet de l’identifier et de le sonder sans
autorité. Le scope reste vérifié à chaque appel : un plugin ne reçoit jamais un
chemin, seulement un identifiant de dossier accordé que le host résout. Il n’y a
aucun WASI, général ou non : un composant qui importe une horloge, une socket ou
un dossier préouvert est refusé avant qu’un `Store` existe.

**Les limites appartiennent au host.** Fuel et deadline viennent par paliers :
une sonde d’identité ou de santé (5 M de fuel, 250 ms), un appel interactif
(50 M, 1 s), une page de découverte (500 M, 5 s). Le budget interactif de 150 ms
du contrat de performance reste un budget d’*affichage* — passé ce délai
l’interface montre une progression — et non la deadline dure qui interrompt le
composant : un premier appel à froid le dépasse légitimement. La deadline se
compte en ticks d’epoch (10 ms), ce qui sert aussi à l’annulation : le même
drapeau interrompt un job en file et un appel déjà dans Wasmtime. Un
`ResourceLimiter` borne la mémoire par instance (64 Mio) et le total de toutes
les instances vivantes (256 Mio), plus les tables et le nombre d’instances. Une
boucle infinie est interrompue, et le thread de tick ne tourne que pendant un
appel.

Deux de ces limites ne sont pas là où on les attend. La deadline se déclenche au
premier des deux signaux : le compte de ticks, qui est déterministe, ou l’horloge,
qui est honnête — un tick réarmé pendant un appel hôte compte pour un, donc le
temps passé hors du wasm n’est visible que par l’horloge. Et `max_wasm_stack` n’est
une limite que si le thread est plus grand : Wasmtime place son seuil à
`sp - max_wasm_stack` sans le borner au thread, et ne compte pas les cadres hôte,
si bien qu’un dépassement pris dans du code hôte est un abort et non un trap. Le
code invité ne tourne donc que sur les workers du scheduler, dimensionnés à la pile
wasm plus sa marge, et `invoke` est privé pour que ce soit vrai par construction.

**Un résultat est non fiable jusqu’à validation.** Le host revalide la grammaire
des IDs opaques, les tailles, les doublons et les curseurs avant toute écriture,
et `LaunchIntent` est une structure fermée dont le `mode` est un enum : la chaîne
WIT ne peut pas devenir un argument de processus. Un intent qui parle d’un autre
profil ou d’un autre jeu que l’appel est rejeté.

Le scheduler (`plugin_scheduler.rs`) met tout cela hors du chemin de rendu :
concurrence globale bornée, un job à la fois par plugin, file bornée qui répond
`Busy` au lieu de grossir, `correlation_id`, états de job, annulation, et mise en
`degraded` après trois échecs consécutifs — avec reprise explicite, sans aucune
boucle de retry. Une annulation ou un refus de capability ne comptent pas comme
échec du plugin. Chaque décision est journalisée.

L’étape 1.3 est faite : `src-tauri/fixtures/runner-fixture` est un vrai composant
tiers, construit par un script reproductible et committé avec son SHA-256, qui ne
peut lire qu’un dossier fixture et produit une `LaunchIntent` contrôlée — et qui
sait mal se comporter sur demande, pour que chaque plafond soit prouvé plutôt
qu’affirmé. La découverte s’en sert : un paquet qui annonce l’extension `runner`
n’est plus « prêt à configurer » parce qu’il compile, mais parce que son
composant exporte le monde runner, n’importe rien que le host ne sache servir, ne
demande pas plus que son manifeste, et répond à `get-identity` et `health-check`
avec une identité qui concorde avec le paquet installé.

Wine-Staging ne charge pas un faux composant Wasm : il applique le contrat WIT
`prepare-launch` comme adapter natif, puis le host valide les IDs opaques et
possède le processus.

Un runner tiers est désormais utilisable de bout en bout, côté host. Le catalogue
passe au schéma **v8** et porte trois tables privées : un `RunnerProfile` — le
plugin qui prépare, l’application d’émulation choisie au sélecteur natif, les
dossiers accordés, les réglages et le statut que `validate-profile` a rendu —,
l’inventaire qui relie une référence de jeu opaque au fichier que le host a
résolu lui-même, et `plugin_grants`, le registre des permissions. Le lancement
suit l’ordre du contrat : profil accepté par le plugin, grant en vigueur,
`prepare-launch` sous le budget interactif, intent validé contre l’appel, puis —
seulement alors — le host résout l’application et le fichier de jeu, canonique,
sans jamais suivre un lien qui sort du dossier accordé, et construit un processus
sans shell dont l’unique argument est ce fichier. `launcher.rs` refuse toujours un
target `Runner` qui lui arrive non résolu ; c’est le filet, pas le chemin.
L’import passe par `discover-page` et le scheduler, une page par transaction,
avec un curseur persisté à côté de la page qu’il décrit : une annulation ou un
redémarrage reprend là où il s’était arrêté au lieu de reparcourir la
bibliothèque. Ce que ce palier n’apporte pas : l’interface « Ajouter un
émulateur » et l’écran Réglages → Plugins, qui consommeront ces commandes, ainsi
que l’import de ROMs par métadonnées et les runners GPTK/CrossOver.

Un écart assumé avec la suite de ce document : les tables SQLite décrites plus
bas (`plugin_jobs`, `plugin_health`…) n’existent pas. Le dépôt n’a aucune
dépendance SQLite et son catalogue est un JSON versionné (`catalog.rs`, schéma
v8). Les grants, les profils et les références externes y vivent, parce
qu’accorder un dossier et enregistrer la permission de le lire doivent réussir ou
échouer ensemble ; l’état des jobs reste dans le scheduler et le journal reste un
anneau borné en mémoire, parce que rien de ce que le host en tire n’avait besoin
de survivre au processus. Révoquer écrit une date au lieu de supprimer une ligne,
de sorte que « ce plugin pouvait lire ce dossier entre ces deux dates » reste une
question à laquelle le registre répond.

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
