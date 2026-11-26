# Registre de plugins

Orivo embarque une liste minimale de plugins officiels et la complète par un
index distant **signé Ed25519** (`REGISTRY_INDEX_URL`, `src-tauri/src/plugin_index.rs`)
et, pour les plugins officiels, par leur dernière release GitHub (voir plus bas).
L'index est relu au plus toutes les 6 h tant que l'application tourne. Une mise à
jour n'est installée seule que si l'utilisateur a activé les mises à jour
automatiques ; sinon l'application affiche seulement une pastille.

## Où vit l'index

Dans ce dépôt, dossier `plugin-registry/` :

- `index.document.json` : le document qu'on édite ;
- `index.v1.json` : l'enveloppe signée que l'application télécharge depuis
  `main` (`REGISTRY_INDEX_URL`) ;
- `scripts/` : signature et vérification.

Jusqu'à la 0.4.4, l'index vivait dans le dépôt `justeozan/orivo-plugin-registry`,
et ces versions lisent toujours cette adresse-là. Tant qu'elles sont en
circulation, chaque index publié ici est aussi poussé tel quel dans
`index.v1.json` de ce dépôt-là.

## Plugins officiels : dernière release GitHub

Les plugins publiés par Orivo (`com.orivo.gamestream`, `com.orivo.quiky`)
n'attendent plus que l'index soit mis à jour. Leur entrée dans
`src-tauri/resources/plugin-registry.json` porte un champ `repository`
(`propriétaire/dépôt`), lu **uniquement** dans ce fichier compilé : ni l'index
ni une réponse réseau ne peuvent rendre un plugin officiel.

- Au même rythme que l'index (6 h), Orivo demande
  `https://github.com/<dépôt>/releases/latest` sans suivre la redirection et lit
  le tag dans `Location` (`v<X.Y.Z>` strict, préversions refusées). Pas d'API
  GitHub, donc pas de quota de 60 requêtes/h.
- Le paquet attendu est
  `https://github.com/<dépôt>/releases/download/v<X.Y.Z>/<id>-<X.Y.Z>.orivo-plugin`.
  Le workflow de release du plugin doit garder ce nom d'asset.
- La version la plus haute gagne entre l'index et la release ; à égalité,
  l'entrée de l'index (qui porte un `sha256`) est gardée.
- Aucun `sha256` n'est connu pour une release trouvée ainsi : c'est la
  signature du paquet par la clé de release qui couvre le contenu. Un paquet non
  signé ou signé par une autre clé est refusé. L'anti-retour, la vérification
  id/version du manifeste et `minOrivoVersion` s'appliquent comme pour l'index.
- Résultat mis en cache dans `plugins/.cache/official-releases.json` ; en cas
  d'échec réseau, la dernière version connue et l'index restent utilisés.

Publier une version d'un plugin officiel revient donc à publier une release
GitHub marquée « latest » avec le paquet signé ; l'index peut rester tel quel.

## Format de l'index (v1)

Enveloppe signée contenant un document :

```json
{
  "schemaVersion": 1,
  "sequence": 42,
  "expiresAtEpochMs": 1900000000000,
  "plugins": [
    {
      "id": "com.exemple.mon-plugin",
      "name": "Mon plugin",
      "version": "1.2.0",
      "summary": "Une phrase.",
      "url": "https://github.com/…/releases/download/v1.2.0/mon-plugin.orivo-plugin",
      "sha256": "<64 caractères hexadécimaux>",
      "sizeBytes": 123456,
      "category": "tools",
      "minOrivoVersion": "0.4.0"
    }
  ]
}
```

- `category` est facultatif : `emulation`, `streaming`, `compatibility`,
  `visuals`, `library`, `music`, `saves` ou `tools`. Une valeur inconnue est
  ignorée ; sans catégorie, l'application la déduit du nom.
- `url` : HTTPS, hôte de la liste compilée dans l'application.
- `sequence` ne décroît jamais (refus de retour en arrière).

## Publier un index

1. Éditer `plugin-registry/index.document.json` : l'URL vise une release sur un
   hôte autorisé, avec le `sha256` et la `sizeBytes` réels de l'archive.
2. **Monter `sequence`** (jamais reculer) et repousser `expiresAtEpochMs` si
   l'échéance approche : un index périmé est refusé par chaque client.
3. Signer puis vérifier :

   ```sh
   mkdir -p plugin-registry/keys
   cp /chemin/vers/orivo-release.key plugin-registry/keys/   # gitignorée, jamais committée
   pnpm registry:sign
   pnpm registry:verify
   ```

4. Committer les deux fichiers par une PR. `cargo test` vérifie l'index commité
   avec la clé compilée dans l'application.
5. Pousser le même `index.v1.json` dans `justeozan/orivo-plugin-registry`
   (voir plus haut).

Les clients relisent l'index au plus toutes les 6 h.

## Publier un plugin communautaire

1. Ouvrir une PR sur ce dépôt qui ajoute ou met à jour l'entrée du plugin dans
   `plugin-registry/index.document.json`, avec le gabarit
   `.github/pull_request_template.md`.
2. Le paquet est signé Ed25519 par son éditeur ; le `sha256` est celui du
   fichier publié.
3. Les mainteneurs relisent le code, les permissions demandées et le paquet.
4. Avant la fusion, les mainteneurs signent l'index avec la clé de release. Un
   index non signé par cette clé est rejeté par l'application.

Seul le canal officiel bénéficie de la mise à jour automatique.
