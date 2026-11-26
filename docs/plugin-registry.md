# Registre de plugins

Orivo embarque une liste minimale de plugins officiels et la complète par un
index distant **signé Ed25519** (`REGISTRY_INDEX_URL`, `src-tauri/src/plugin_index.rs`).
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
