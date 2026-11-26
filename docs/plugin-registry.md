# Registre de plugins

Orivo embarque une liste minimale de plugins officiels et la complète par un
index distant **signé Ed25519** (`REGISTRY_INDEX_URL`, `src-tauri/src/plugin_index.rs`)
et, pour les plugins officiels, par leur dernière release GitHub (voir plus bas).
L'index est relu au plus toutes les 6 h tant que l'application tourne. Une mise à
jour n'est installée seule que si l'utilisateur a activé les mises à jour
automatiques ; sinon l'application affiche seulement une pastille.

## Statut

Livré dans l'application : lecture de l'index signé, vérification périodique,
mise à jour automatique (opt-in) ou pastille. **Pas encore livré, décision
externe aux mainteneurs** : la création du dépôt `orivo-plugin-registry`,
l'hébergement de l'index à l'adresse `REGISTRY_INDEX_URL`, et l'outil qui
génère et signe l'index avec la clé de release. Tant que ce dépôt n'existe pas,
seul le registre embarqué répond.

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

## Publier un plugin

1. Ouvrir une PR sur le dépôt `orivo-plugin-registry` ajoutant ou mettant à
   jour l'entrée du plugin, avec le gabarit `.github/pull_request_template.md`.
2. Le paquet est signé Ed25519 par son éditeur ; le `sha256` est celui du
   fichier publié.
3. Les mainteneurs relisent le code, les permissions demandées et le paquet.
4. Après fusion, les mainteneurs régénèrent l'index et le signent avec la clé
   de release. Un index non signé par cette clé est rejeté par l'application.

Seul le canal officiel bénéficie de la mise à jour automatique.
