## Quoi

<!-- Changement en une ou deux phrases. -->

## Nouveautés (si un joueur le voit)

- [ ] Titre + ligne courts dans `src/locales/highlights.ts` (EN + FR, « tu »), item dans le bucket `"next"` de `src/release-highlights.ts`

Voir `docs/RELEASING.md` › Highlights. Un correctif ou du CI n'en ajoute pas.

## Plugin du registre (si applicable)

- [ ] `id`, `name`, `version`, `summary`
- [ ] `url` HTTPS sur un hôte autorisé, `sha256`, `sizeBytes`
- [ ] `category` (emulation, streaming, compatibility, visuals, library, music, saves, tools)
- [ ] `minOrivoVersion` si nécessaire
- [ ] Paquet signé Ed25519

Voir `docs/plugin-registry.md`.

## Tests

- [ ] `pnpm check`
