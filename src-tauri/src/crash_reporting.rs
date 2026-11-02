//! Crash reports for the Rust half of the app.
//!
//! `src/sentry.ts` covers the webview. A panic on the Rust side — a plugin
//! runtime, a download, a launcher — never reaches it, so it is reported here
//! with the same DSN, the same release string and the same rule: a build made
//! without `VITE_SENTRY_DSN` has no client and opens no socket.

/// Starts the client and returns the guard that keeps it alive; `None` without a
/// DSN. The caller holds the guard for as long as the app runs.
pub fn init() -> Option<sentry::ClientInitGuard> {
    // Read at compile time, like Vite does for the frontend: the DSN is part of
    // the build, so one binary cannot be pointed somewhere else afterwards.
    let dsn = option_env!("VITE_SENTRY_DSN")
        .map(str::trim)
        .filter(|dsn| !dsn.is_empty())?;

    // `ClientOptions` is non-exhaustive, so it is filled in after `default()`.
    let mut options = sentry::ClientOptions::default();
    options.release = Some(concat!("orivo@", env!("CARGO_PKG_VERSION")).into());
    options.environment = Some(if cfg!(debug_assertions) { "development" } else { "production" }.into());
    // A library has no sensitive payload, but no reason to send names or IP
    // addresses either.
    options.send_default_pii = false;
    let guard = sentry::init((dsn, options));
    sentry::configure_scope(|scope| scope.set_tag("runtime", "rust"));
    Some(guard)
}
