#!/usr/bin/env bash
# Prints the body of a GitHub Release: the version's highlights when it has
# some (src/release-highlights.ts), one line per pull request merged since the
# previous release, then the download notes.
#
#   scripts/release-notes.sh v0.4.3
#
# Reads everything from git, so it needs the tags (a full clone). When the tag
# does not exist yet, HEAD stands in for it.
set -euo pipefail

tag="${1:?usage: release-notes.sh <tag>}"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  head="$tag"
else
  head="HEAD"
fi

# The previous stable tag reachable from this one. Pre-releases are skipped so
# a stable release lists everything since the last stable one.
previous=$(git tag --merged "$head" --list 'v[0-9]*' --sort=-v:refname \
  | grep -vx "$tag" | grep -v -- '-' | head -n1 || true)
range="${previous:+$previous..}$head"

# Merge commits ("Merge pull request #N", the PR title on the next line) and
# squash merges ("<title> (#N)") on main's first-parent line, oldest first.
lines=""
for commit in $(git log --first-parent --reverse --format='%H' "$range"); do
  subject=$(git log -1 --format='%s' "$commit")
  if [[ "$subject" =~ ^Merge\ pull\ request\ \#([0-9]+)\  ]]; then
    number="${BASH_REMATCH[1]}"
    title=$(git log -1 --format='%b' "$commit" | sed -n '/./{p;q;}')
    title="${title:-$subject}"
  elif [[ "$subject" =~ ^(.*)\ \(\#([0-9]+)\)$ ]]; then
    number="${BASH_REMATCH[2]}"
    title="${BASH_REMATCH[1]}"
  else
    continue
  fi
  case "$lines" in *"- #$number :"*) continue ;; esac
  lines+="- #$number : $title"$'\n'
done

# Highlights are a nicety: a Node too old to load them, or any other failure,
# leaves the PR list alone rather than stopping a release.
highlights=$(node "$(dirname "$0")/release-highlights.mjs" notes "$tag") || {
  echo "release-notes.sh: no highlights for $tag (see above)." >&2
  highlights=""
}
if [ -n "$highlights" ]; then
  printf '%s\n\n## Pull requests\n\n' "$highlights"
fi

if [ -n "$lines" ]; then
  printf '%s' "$lines"
else
  echo "- No pull request since ${previous:-the first release}."
fi

cat <<'NOTES'

---

## Downloads

- **macOS:** download the `.dmg` matching your Mac (Apple Silicon or Intel).
- **Windows:** use the `-setup.exe` installer (or `.msi` for managed installs).
- **Linux:** use `.AppImage`, `.deb` or `.rpm` for your distribution.
- **Android:** download the `.apk` to install on your device (requires "Install from unknown sources").
- **iPhone & iPad:** Orivo comes from the App Store. The `_ios-sideload.ipa` is the sideload edition, for AltStore, SideStore or Sideloadly: see https://orivo.evey.run/sideload.

The `.app.tar.gz` and updater archives are for Orivo's verified updater; they are not manual installers.
The `.apk` is the build Google Play signs, so it updates an install from Google Play and the other way round. The `.aab` (Android App Bundle) is what Google Play received; it is not an installer.

Orivo updates itself: existing installs pick this release up automatically the next time they check for updates.
NOTES
