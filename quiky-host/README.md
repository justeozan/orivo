# quiky-host — vendored copy of the Quiky plugin's host crate

This directory is a **vendored copy** of the privileged host crate from the
plugin repository, [`justeozan/orivo-plugin-quiky`](https://github.com/justeozan/orivo-plugin-quiky)
(`host/rust` + `host/tools` at the tag the copy was taken from).

## Why it is vendored

The Quiky `.orivo-plugin` package is deliberately data-only: no executables, no
native payloads. The pipeline that actually acquires and installs a FitGirl
repack — search, link scraping, aria2 download, MD5 verification, extraction
under Wine — therefore lives in a native Rust crate that Orivo links at build
time (`src-tauri/Cargo.toml`, macOS-only target dependency, exposed through the
`quiky_installer` tauri commands).

Orivo's CI and release workflows check out **only this repository**, so an
out-of-repo path dependency (`path = "../../orivo-plugin-quiky/..."`) cannot
resolve there, and a `git` dependency would require cross-repository credentials
for a private repository. Vendoring keeps every build — local, CI and release —
self-contained.

The plugin repository remains the source of truth: it owns the catalogue, the
package build/sign/verify pipeline, and the standalone test suite for this crate
(including the live-repack tests that are impractical to run here).

## Layout

```
quiky-host/
├── rust/      the crate (workspace member: quiky-host/rust)
│   ├── src/         pipeline modules (search, scrape, download, extract, fitgirl, …)
│   ├── tests/       integration tests (local TCP servers, fixtures)
│   └── examples/    quiky-step CLI for manual one-step debugging
└── tools/     host-side binaries embedded via include_bytes!
    ├── bin/srep.exe            SREP decompressor (runs under Wine)
    ├── bin/unarc_extract.exe   unarc helper (must sit next to unarc.dll)
    ├── bin/unarc               native patched unarc (macOS fallback)
    └── *.txt, unarc_extract.c  licenses and source for the above
```

The `rust/` + `tools/` nesting mirrors the plugin repository's `host/rust` +
`host/tools` layout **on purpose**: `include_bytes!("../../tools/bin/...")`
paths inside the crate resolve identically in both trees, so the crate's source
files are byte-identical to upstream and re-syncing stays a pure file copy.

Two integration tests also reach outside the crate for fixtures —
`extract_fixture.rs` and `scrape_fixture.rs` read `../../tests/fixtures/...`,
which lands at the **workspace root** in both repositories. The fixtures
(`sample.zip`, `sample-page.html`) are therefore vendored at `tests/fixtures/`
in this repository's root; re-sync them together with the crate.

## Re-syncing from the plugin repository

When the plugin repository's host crate changes:

```sh
PLUGIN=/path/to/orivo-plugin-quiky/threadfin
ORIVO=/path/to/orivo

rm -rf "$ORIVO/quiky-host/rust/src" "$ORIVO/quiky-host/rust/tests" \
       "$ORIVO/quiky-host/rust/examples" "$ORIVO/quiky-host/tools" \
       "$ORIVO/tests/fixtures"
cp "$PLUGIN/host/rust/Cargo.toml" "$ORIVO/quiky-host/rust/Cargo.toml"
cp -R "$PLUGIN/host/rust/src" "$PLUGIN/host/rust/tests" \
      "$PLUGIN/host/rust/examples" "$ORIVO/quiky-host/rust/"
cp -R "$PLUGIN/host/tools" "$ORIVO/quiky-host/tools"
cp -R "$PLUGIN/tests/fixtures" "$ORIVO/tests/fixtures"
```

Two deliberate deviations from upstream, both in `rust/Cargo.toml` — re-apply
them after a copy:

1. `edition`/`license` are inlined (`2021` / `UNLICENSED`) instead of
   `edition.workspace = true` / `license.workspace = true`, because Orivo's
   workspace does not define `[workspace.package]`.
2. Nothing else — the crate name (`orivo-plugin-quiky-host`), the lib name
   (`quiky_host`) and the feature set are unchanged, so `src-tauri`'s
   `quiky_host = { package = "orivo-plugin-quiky-host", ... }` dependency keeps
   working untouched.

After re-syncing, run `cargo test -p orivo-plugin-quiky-host` (tests use local
TCP servers and fixtures, never the live network) and `cargo test -p orivo` for
the host-side integration.

## Runtime requirements

The crate locates `aria2c`, `innoextract` and Wine on `PATH` (Finder-safe
prefixes included) or through `QUIKY_*` environment overrides. The bundled
`tools/bin` binaries are materialised per-install by
`quiky_host::fitgirl::ensure_bundled_tools`; they are compiled into the Orivo
binary, never shipped inside the plugin package.
