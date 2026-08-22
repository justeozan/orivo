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
