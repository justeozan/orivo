# Releasing Orivo

Orivo ships as a self-updating desktop app. A release is one GitHub Release that
carries the installers for every platform **plus** an updater manifest
(`latest.json`) that installed copies poll on startup.

Everything is automated by three workflows:

| Workflow | File | Trigger | What it does |
| --- | --- | --- | --- |
| CI | `.github/workflows/ci.yml` | push to `main`, every PR | `pnpm typecheck`, `pnpm test`, and `cargo check` on Linux / macOS / Windows |
| Tag release | `.github/workflows/tag.yml` | manual (`workflow_dispatch`) | bumps the version everywhere, commits to `main`, pushes `v<version>`, starts the release |
| Release | `.github/workflows/release.yml` | push of a `v*` tag, or manual | builds and publishes the bundles, signatures and `latest.json` |

---

## 1. One-time setup: repository secrets

Two secrets must exist under **Settings → Secrets and variables → Actions →
Repository secrets**. Without them the build still succeeds, but no `.sig` files
and no `latest.json` are produced, so the in-app updater silently stops working.

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | The **entire contents** of the minisign private key file `~/.orivo/orivo-updater.key`, including the `untrusted comment:` first line and the trailing newline. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | An **empty string**. The key was generated without a password, so the secret exists but has no value. |

With the GitHub CLI:

```sh
gh secret set TAURI_SIGNING_PRIVATE_KEY < ~/.orivo/orivo-updater.key
gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --body ""
```

`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` still has to be created even though it is
empty: the Tauri bundler prompts interactively for a password when the variable
is unset, which hangs the CI job.

### Where the private key lives

- Private key: `~/.orivo/orivo-updater.key` — outside every checkout, so it
  survives a workspace being archived.
- Public key: `~/.orivo/orivo-updater.key.pub`, and the same value is baked into
  `src-tauri/tauri.conf.json` under `plugins.updater.pubkey`:

  ```
  dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEI0NzlBNjdCQjI4OUJBOTgKUldTWXVvbXllNlo1dEhXcC9ZL2ZUQzV4L1g5MWNyUHFJbDA1dkttWXh5clpxcmJhbk1FVG5rZkgK
  ```

`.context/orivo-updater.key` is also read as a fallback, but `.context/` is
per-workspace and gitignored: the key was originally kept only there and went
missing the moment that workspace was archived, which breaks every local
`tauri build` with *"A public key has been found, but no private key"*.
`~/.orivo/` is the copy to keep.

**Put the private key in a password manager today.** It is the only thing that
lets you ship an update that already-installed copies of Orivo will accept. If
it is lost, every existing install is permanently orphaned: you would have to
generate a new key pair, ship a new `pubkey`, and ask every user to download and
reinstall the app by hand. A fresh clone never has it.

### Building locally

`bundle.createUpdaterArtifacts` is on, so any `tauri build` signs the updater
archive and fails when the key is missing. Wrap bundling commands so the key is
picked up from either location:

```sh
./scripts/with-signing-key.sh pnpm tauri build --debug --bundles app
```

`tauri dev`, `pnpm test` and anything that does not bundle need no key.

To regenerate a key pair (only if the current one leaks or is lost):

```sh
pnpm tauri signer generate -w ~/.orivo/orivo-updater.key
```

Then update `plugins.updater.pubkey` in `src-tauri/tauri.conf.json` and reset
both secrets.

---

## 2. Cutting a release

### Before you start

- Everything you want to ship is merged into `main` and CI is green.
- `CHANGELOG.md` has an entry for the new version (move the `[Unreleased]`
  items under a `## [x.y.z]` heading).

### The normal path (recommended)

1. Go to **Actions → Tag release → Run workflow**.
2. Enter the new version **without** the leading `v` (for example `0.4.0`).
   Pre-releases like `0.4.0-rc.1` are accepted.
3. Run it. The workflow:
   - refuses to continue if `v0.4.0` already exists locally or on `origin`;
   - rewrites the single version literal in `package.json`,
     `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` and `Cargo.lock`
     (formatting is left untouched);
   - commits `chore(release): v0.4.0` to `main`;
   - pushes the annotated tag `v0.4.0`;
   - starts **Release** at that tag.
4. Watch **Actions → Release**. Four builds run in parallel (macOS arm64, macOS
   x86_64, Linux, Windows); a cold cache takes roughly 20-30 minutes.
5. The release is published (not a draft) at
   `https://github.com/justeozan/orivo/releases/tag/v0.4.0`.

> The tag workflow starts the release workflow explicitly instead of relying on
> the tag push. GitHub deliberately does not start workflow runs for events
> created with the built-in `GITHUB_TOKEN`, and `workflow_dispatch` is one of
> the two events exempted from that rule.

### The manual path

Pushing a tag from your machine triggers **Release** directly:

```sh
git switch main && git pull
# bump the version in package.json, src-tauri/Cargo.toml,
# src-tauri/tauri.conf.json and Cargo.lock — all four must match
git commit -am "chore(release): v0.4.0"
git push origin main
git tag -a v0.4.0 -m "Orivo v0.4.0"
git push origin v0.4.0
```

You can also run **Release** by hand from the Actions tab. Leave the `version`
input empty to reuse the version already in `package.json`, or set it to
retarget the run at `v<version>`.

### What gets published

| Platform | Installers | Updater artifact |
| --- | --- | --- |
| macOS (Apple Silicon + Intel, built separately) | `.dmg`, `.app` | `.app.tar.gz` + `.sig` |
| Windows | NSIS `.exe`, `.msi` | NSIS `.exe` + `.sig` |
| Linux | `.AppImage`, `.deb`, `.rpm` | `.AppImage.tar.gz` + `.sig` |

Plus one `latest.json` shared by all four builds.

---

## 3. How the updater works

The app is configured (in `src-tauri/tauri.conf.json`) to poll a single
endpoint:

```
https://github.com/justeozan/orivo/releases/latest/download/latest.json
```

That URL is a GitHub redirect to the asset named `latest.json` on the newest
**published** release. Three things have to hold for it to resolve, and the
release workflow enforces all three:

1. `includeUpdaterJson: true` on `tauri-apps/tauri-action`, so `latest.json` is
   generated and uploaded as a release asset. Each matrix job merges its own
   platform into the existing file, so the final manifest lists
   `darwin-aarch64`, `darwin-x86_64`, `linux-x86_64` and `windows-x86_64`.
2. `releaseDraft: false`. `/releases/latest/download/` **404s for draft
   releases** — a draft release is invisible to that endpoint, and the updater
   would silently never find an update.
3. `prerelease: false`. `latest` also skips pre-releases.

`updaterJsonPreferNsis: true` makes the Windows entry point at the NSIS
installer rather than the MSI, which matches the `currentUser` / `passive`
install mode configured for the bundle (no admin prompt on update).

The bundler signs every updater artifact with the minisign private key and
writes a detached `.sig` next to it; the app verifies that signature against the
embedded `pubkey` before installing anything. A build without
`TAURI_SIGNING_PRIVATE_KEY` produces no `.sig` and no `latest.json`.

The `verify-updater-manifest` job at the end of the release workflow downloads
the published `latest.json` and fails the run if any of these is true, so a
broken updater cannot ship unnoticed:

- `latest.json` is not attached to the release;
- the release is a draft or a pre-release;
- the manifest's `version` does not match the tag (an install already on that
  version would never be offered the update);
- any of `darwin-aarch64`, `darwin-x86_64`, `linux-x86_64`, `windows-x86_64` is
  missing from `platforms` — one build failed, produced no signature, or a
  concurrent matrix job overwrote the shared manifest.

The release job also refuses to start a build whose tag disagrees with the
version in `package.json` and `src-tauri/tauri.conf.json`, which is the same
mistake caught 30 minutes earlier.

Note: the in-app updater covers macOS, Windows and the AppImage. People who
installed from the `.deb` or `.rpm` update through their package manager or by
downloading the new package — that is a Tauri limitation, not a configuration
gap.

---

## 4. Code signing: not yet

Orivo is **not** code-signed on macOS or Windows. The builds are usable, but the
operating systems warn about them. This is independent of the updater
signature above: minisign proves the update came from us, the OS wants a
certificate from a paid program.

### macOS

Users see *"Orivo can't be opened because Apple cannot check it for malicious
software"* (or, from a browser download, *"Orivo is damaged and can't be
opened"*). The workaround, worth repeating in the release notes:

1. Drag Orivo to **Applications**.
2. **Right-click** (or Control-click) the app → **Open** → **Open** in the
   dialog. macOS remembers the choice; later launches are normal.

If the "damaged" message shows up instead, the quarantine attribute has to be
cleared:

```sh
xattr -dr com.apple.quarantine /Applications/Orivo.app
```

### Windows

SmartScreen shows *"Windows protected your PC"* on the installer. Users click
**More info → Run anyway**. The warning fades on its own as the installer
accumulates downloads, but only per binary — every new release starts over
until the app is signed.

### Linux

No warning. The AppImage may need `chmod +x Orivo_*.AppImage`.

### The path to signed builds

Nothing in the workflow has to be restructured; signing is turned on with
secrets and a few config keys.

**macOS** — requires an Apple Developer Program membership (99 USD/year) and a
*Developer ID Application* certificate. Export it as a `.p12`, base64 it, and
add these to the `env:` block of the `Build and publish` step in
`release.yml`:

- `APPLE_CERTIFICATE` (base64 of the `.p12`)
- `APPLE_CERTIFICATE_PASSWORD`
- `APPLE_SIGNING_IDENTITY` (e.g. `Developer ID Application: Name (TEAMID)`)
- `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password), `APPLE_TEAM_ID` —
  these three enable notarization, which is what actually removes the Gatekeeper
  prompt.

`tauri-action` picks them up automatically and notarizes as part of the bundle
step.

**Windows** — either Azure Trusted Signing (cheapest credible option, no
hardware token) or an OV/EV code-signing certificate from a CA. Configure it
under `bundle.windows` in `src-tauri/tauri.conf.json` (`signCommand`, or the
`trustedSigning` block) and add the corresponding secrets to the same `env:`
block.

Until then, keep the Gatekeeper / SmartScreen instructions in the release notes
— the release body published by the workflow already contains them.

---

## 5. Troubleshooting

**`verify-updater-manifest` fails with "latest.json is missing".**
The signing secrets are missing or misnamed, or
`bundle.createUpdaterArtifacts` was turned off in `src-tauri/tauri.conf.json`.
Without a signing key the bundler skips updater artifacts entirely, and
`tauri-action` then has no manifest to upload.

**`verify-updater-manifest` fails with "missing platform entries".**
Either that platform's build failed (check the matrix job), or two jobs finished
within seconds of each other and one read `latest.json` before the other had
uploaded it. Re-running the whole **Release** workflow for the same tag fixes
it: `tauri-action` reuses the existing release and merges the platforms back in.

**A build fails immediately with "Tag vX.Y.Z does not match the app version".**
The tag was pushed by hand without bumping all four version files. Delete the
tag (`git push origin :refs/tags/vX.Y.Z`), then use the **Tag release**
workflow, which bumps them together.

**Installed apps never see the update.**
Open `https://github.com/justeozan/orivo/releases/latest/download/latest.json`
in a browser. A 404 means the newest release is a draft or a pre-release, or
`latest.json` was never attached. A stale version in the JSON means an older run
overwrote it — re-run the release workflow for the newest tag.

**`pnpm install --frozen-lockfile` fails.**
`pnpm-lock.yaml` is out of sync with `package.json`. Run `pnpm install` locally
and commit the lockfile.

**The tag workflow says "Tag v0.4.0 already exists".**
Versions are never reused. Pick the next version, or, if the tag was pushed by
mistake and nothing was published, delete both the tag and its release
(`git push origin :refs/tags/v0.4.0` and `gh release delete v0.4.0`) before
retrying.

**The tag workflow cannot push to `main`.**
Branch protection is rejecting the `github-actions[bot]` push. Either allow the
Actions bot to bypass the rule, or cut the release with the manual path above.

**A Rust build fails only on one platform.**
The matrix keeps going (`fail-fast: false`), so the other bundles still publish.
Fix the platform, then re-run the failed job — `tauri-action` reuses the
existing release for that tag instead of creating a second one.
