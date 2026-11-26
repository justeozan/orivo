/**
 * Settings › About's "What's new", one title and one line per highlight of
 * `src/release-highlights.ts`. Drawn at 19px, so a few words each: a title
 * says what changed, the line says what you can do with it. Keys are named
 * after the change, never the version, so stamping a release renames nothing.
 *
 * This module imports nothing: `scripts/release-highlights.mjs` loads it with
 * plain Node to write the GitHub release notes.
 */
export const en = {
  "highlights.aboutHighlights.title": "What's new, in your language",
  "highlights.aboutHighlights.line": "A few short lines per release",

  "highlights.pluginStates.title": "Plugins show their state",
  "highlights.pluginStates.line": "Ready, Missing or To set up, at a glance",
  "highlights.streamingFlow.title": "Streaming, one question at a time",
  "highlights.streamingFlow.line": "Pick your PC, type the PIN, play",
  "highlights.storage.title": "Storage, in one look",
  "highlights.storage.line": "Empty the cache, your library stays",
  "highlights.about.title": "About, redone",
  "highlights.about.line": "Update and what's new, written large",

  "highlights.accounts.title": "Your accounts, at a glance",
  "highlights.accounts.line": "Games per store, sync one at a time",

  "highlights.general.title": "General, redone",
  "highlights.general.line": "Appearance joins it, one tap per choice",
  "highlights.motionAll.title": "Animations: All by default",
  "highlights.motionAll.line": "Or System, or Reduced, your call",

  "highlights.officialPlugins.title": "Official plugins, always current",
  "highlights.officialPlugins.line": "Taken from their latest GitHub release",

  "highlights.moonlightPlugin.title": "The Moonlight plugin is up to date",
  "highlights.moonlightPlugin.line": "The Store installs 0.2.1, streaming works",

  "highlights.pluginUpdates.title": "Plugins update themselves",
  "highlights.pluginUpdates.line": "Or a badge tells you one is waiting",
  "highlights.pluginCategories.title": "Plugins by category",
  "highlights.pluginCategories.line": "Filter the catalogue in one tap",

  "highlights.switchPlugin.title": "Your Switch games, in Orivo",
  "highlights.switchPlugin.line": "New plugin: install, update, add a game",

  "highlights.movedGames.title": "A moved game is found again",
  "highlights.movedGames.line": "Otherwise it dims, with Locate",
  "highlights.sourceBar.title": "Your library, by source",
  "highlights.sourceBar.line": "Sort by Recent, New or Most played",
  "highlights.windowsAdmin.title": "Admin games start on Windows",
  "highlights.windowsAdmin.line": "Windows asks, the game starts",
};

export const fr: Partial<Record<keyof typeof en, string>> = {
  "highlights.aboutHighlights.title": "Les nouveautés, en français",
  "highlights.aboutHighlights.line": "Quelques lignes courtes par version",

  "highlights.pluginStates.title": "Les plugins disent leur état",
  "highlights.pluginStates.line": "Prêt, Manquant ou À configurer, d'un coup d'œil",
  "highlights.streamingFlow.title": "Le streaming, une question à la fois",
  "highlights.streamingFlow.line": "Choisis ton PC, tape le PIN, joue",
  "highlights.storage.title": "Le stockage, en un regard",
  "highlights.storage.line": "Vide le cache, ta bibliothèque reste",
  "highlights.about.title": "À propos, refait",
  "highlights.about.line": "Mise à jour et nouveautés, en grand",

  "highlights.accounts.title": "Tes comptes, d'un coup d'œil",
  "highlights.accounts.line": "Les jeux de chaque store, synchro une par une",

  "highlights.general.title": "Général, refait",
  "highlights.general.line": "Apparence y entre, un geste par choix",
  "highlights.motionAll.title": "Animations : Toutes par défaut",
  "highlights.motionAll.line": "Ou Système, ou Réduites, à toi de voir",

  "highlights.officialPlugins.title": "Plugins officiels, toujours à jour",
  "highlights.officialPlugins.line": "Pris dans leur dernière version GitHub",

  "highlights.moonlightPlugin.title": "Le plugin Moonlight est à jour",
  "highlights.moonlightPlugin.line": "Le Store installe la 0.2.1, le streaming marche",

  "highlights.pluginUpdates.title": "Les plugins se mettent à jour",
  "highlights.pluginUpdates.line": "Ou une pastille te prévient",
  "highlights.pluginCategories.title": "Les plugins par catégorie",
  "highlights.pluginCategories.line": "Filtre le catalogue d'un geste",

  "highlights.switchPlugin.title": "Tes jeux Switch, dans Orivo",
  "highlights.switchPlugin.line": "Nouveau plugin : installe, mets à jour, ajoute un jeu",

  "highlights.movedGames.title": "Un jeu déplacé est retrouvé",
  "highlights.movedGames.line": "Sinon il passe en grisé, avec Localiser",
  "highlights.sourceBar.title": "Ta bibliothèque, par source",
  "highlights.sourceBar.line": "Trie par Récents, Nouveaux ou Plus joués",
  "highlights.windowsAdmin.title": "Les jeux admin démarrent sous Windows",
  "highlights.windowsAdmin.line": "Windows demande, le jeu se lance",
};
