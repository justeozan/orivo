# Contributing to Orivo

Contributions are welcome. Read this page first — the licensing part is short
but it is not optional, and it is the one thing that cannot be fixed after a
pull request is merged.

## Before you write code

Orivo is written against three specifications, and a pull request that
contradicts one of them will be sent back with a pointer to the paragraph:

- [`docs/DESIGN.md`](docs/DESIGN.md) — the design system. Spacing, type, motion, and why
  a panel fades at the bottom instead of being cut off.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — the boundaries. What the WebView may
  know, what only Rust may do, and the performance budget.
- [`docs/selector-contract.md`](docs/selector-contract.md) — what the selector
  guarantees about focus, and what may never move under a player's thumb.

For anything larger than a bug fix, open an issue first. It is cheaper to
disagree about an approach in a paragraph than in a diff.

## The loop

```sh
pnpm install
pnpm tauri dev     # the desktop app
pnpm dev           # the frontend alone, in a browser, on fixture data
```

Everything below must pass before you open a pull request:

```sh
pnpm typecheck && pnpm test    # TypeScript + unit tests
pnpm test:e2e                  # Playwright, needs the dev server
cargo test --manifest-path src-tauri/Cargo.toml
```

CI is narrower than that — it runs `pnpm typecheck`, `pnpm test`, and
`cargo check` on Linux, macOS and Windows. The Rust tests and the Playwright
suite are yours to run locally, and a pull request that skipped them tends to
show it.

Requires Node 22, pnpm 11 and a stable Rust toolchain. On Linux you also need
`libwebkit2gtk-4.1-dev` and `libgtk-3-dev`.

## Working in a git worktree

A second worktree is cheap here because nothing that can be shared is copied:
the pnpm store, the Cargo registry, the Playwright browsers and the sccache
compilation cache all live outside every worktree, and `git worktree add` shares
the object store with the main checkout. What a new worktree actually costs is
its checkout (~123 MB) and its own `target/` directory.

```sh
# from the main checkout
git worktree add ../orivo-<name> -b <branch>

cd ../orivo-<name>
pnpm install            # ~1 s: files are linked from the shared store
pnpm exec vite build    # cargo needs ../dist — tauri::generate_context! reads it
```

Everything expensive is shared:

| What | Where it lives | Why it is not per-worktree |
| --- | --- | --- |
| JS packages | `pnpm store path` — global, linked into each `node_modules` | pnpm's default; an install is metadata only, `node_modules` adds no real disk |
| Cargo registry, git deps, toolchains | `$CARGO_HOME` (`~/.cargo`), `~/.rustup` | set by rustup; never override `CARGO_HOME` per worktree |
| Compilation cache | sccache, `~/Library/Caches/Mozilla.sccache` | wired by `$CARGO_HOME/config.toml`, machine-local and not versioned anywhere |
| Playwright browsers | `~/Library/Caches/ms-playwright` | Playwright's default, shared by every project on the machine |
| Gradle (Android) | `~/.gradle` | Gradle's default |
| Rust build directory | **per worktree**, `target/` at the repo root | deliberate — see below |

Two rules that make this hold:

- **Never point `CARGO_TARGET_DIR` at a directory shared between worktrees.**
  Cargo takes a lock on it, so two worktrees building at once would queue up
  behind each other, and alternating between branches would invalidate the
  fingerprints every time. `target/` stays private; sccache is what makes
  rebuilding it cheap.
- **Never copy the store into a worktree.** If `pnpm install` reports downloaded
  files instead of reused ones, the store is not being found — check
  `pnpm store path` rather than committing anything to fix it.

sccache cannot cache an incremental compilation, which is why `[profile.dev]` in
the workspace `Cargo.toml` sets `incremental = false`. The cost is that editing a
Rust file recompiles its crate instead of patching it — a few extra seconds per
edit — and it buys two things: a build directory without a session directory in
it, and compilations that travel between worktrees, so a worktree that has never
been built reuses the cache instead of recompiling the dependency graph.

Set it back to `true` in that same file if edit latency matters more to you than
either of those; the cache then covers only the crates outside this workspace,
and every worktree carries its own session directory again.

To build one thing without the cache, set the wrapper to the empty string:

```sh
RUSTC_WRAPPER="" cargo check --manifest-path src-tauri/Cargo.toml
```

Cleaning up: `git worktree remove ../orivo-<name>` (add `--force` if it is
dirty), then `git worktree prune`.

### Repository settings

These are local to a clone and are not committed, so run them once per clone if
you want the same behaviour:

```sh
git config feature.manyFiles true   # fsmonitor + untracked cache, faster `git status`
git config index.version 4          # smaller index; applied on the next index rebuild
git maintenance start               # scheduled background gc/prefetch/repack
```

## House style

- **Behaviour comes with a test.** A fix without a failing-then-passing test is
  a fix that comes back.
- **Comments explain why, not what.** The code already says what it does. Match
  the density and voice of the file you are editing.
- **Credentials never reach the WebView.** Tokens live in the system keychain
  and are read by Rust. If a change makes a secret visible to the frontend, it
  is the wrong change.
- **No new dependency without a reason in the pull request.** Say what it buys
  and what it costs; a bundle grows in one direction only.
- **One concern per pull request.** A refactor and a feature in the same diff
  are two reviews wearing one hat.

## Contributor licence

Orivo is source-available under the
[PolyForm Noncommercial License 1.0.0](LICENSE), and the copyright holder
offers it commercially — including as a hosted service. That only works if the
project can license the whole codebase, so contributions have to come with the
rights to do it.

**By submitting a contribution — a pull request, a patch, a snippet in an
issue — you agree to the following.**

1. **Ownership.** The contribution is your original work, or you have the right
   to submit it under these terms. If your employer has rights to work you
   produce, you have their permission to contribute it.
2. **Licence grant.** You grant Ozan Sahin (the copyright holder) **and their
   successors and assigns** a perpetual, worldwide, non-exclusive, irrevocable,
   royalty-free, transferable and sublicensable licence to reproduce, modify,
   adapt, publish, distribute and otherwise exploit your contribution, **under
   any licence terms, including commercial and proprietary ones**, and as part
   of a hosted or managed service.
3. **Patents.** You grant the same parties a perpetual, worldwide,
   irrevocable, royalty-free patent licence covering any patent claims you own
   or control that your contribution would otherwise infringe.
4. **You keep your copyright.** This is a licence, not an assignment. You may
   use your own contribution however you like, elsewhere, forever.
5. **No warranty.** You provide the contribution as is, without warranty of
   any kind.

If you cannot agree to all five, please do not open a pull request — describe
the idea in an issue instead. That is a genuinely useful contribution and it
carries none of this.

Third-party code may only be added if its licence permits noncommercial *and*
commercial redistribution — in practice MIT, Apache-2.0, BSD, ISC, Zlib or the
public domain. Copyleft code (GPL, AGPL, LGPL when statically linked) cannot be
merged. Say the licence in the pull request.

## Reporting a security issue

Do not open a public issue. Write to contact@oneiby.com with what you found and
how to reproduce it, and give it a reasonable window before disclosing.
