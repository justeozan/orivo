# Performance bench (O1)

Reference numbers live in [`docs/performance.md`](../docs/performance.md); this
directory holds the tooling that produced them. Nothing here optimises
anything — it measures, so a later change can be judged against a real number
instead of a guess.

```
perf/
  bundle/    dist/ size by category (JS, CSS, media), plus its own test
  web/       Playwright bench for startup, navigation, search and rail pacing
```

Two rules that make this different from `e2e/`:

- **Never `pnpm test:e2e`.** `perf/web/playwright.perf.config.ts` is a separate
  Playwright config with its own port, its own `webServer`, and no golden
  screenshots — it must never run inside the suite that gates a pull request.
- **No new dependency.** The Rust side of this bench lives in
  `src-tauri/src/perf_bench.rs` (`#[cfg(test)]`, `#[ignore]`) and reuses
  `cargo test`; nothing here needed a benchmarking crate.

Run it:

```sh
pnpm exec vite build
node perf/bundle/measure-bundle.mjs
pnpm exec playwright test -c perf/web/playwright.perf.config.ts
cargo test --manifest-path src-tauri/Cargo.toml --release perf_bench -- --ignored --nocapture --test-threads=1
```
