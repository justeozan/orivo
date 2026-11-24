//! Crash reports for the Rust half of the app.
//!
//! `src/sentry.ts` covers the webview. A panic on the Rust side — a plugin
//! runtime, a download, a launcher — never reaches it, so it is reported here
//! with the same DSN, the same release string and the same rule: a build made
//! without `VITE_SENTRY_DSN` has no client and opens no socket.

use std::time::Duration;

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

/// Reports a failure that is about to end the process, and waits for the report
/// to leave.
///
/// The client sends from a background thread, and a process that calls
/// `exit` straight after never gives that thread its turn. A startup failure is
/// the one crash a player cannot describe afterwards — there was no window to
/// photograph — so this is where waiting for the network is worth two seconds.
///
/// `kind` is a fixed label, not the error's text: that text names the player's
/// folders, and a count of how often startup fails is the lead, not the path.
pub fn report_fatal(kind: &'static str) {
    sentry::with_scope(
        |scope| scope.set_tag("startup_failure", kind),
        || {
            sentry::capture_message(&format!("Orivo could not start: {kind}"), sentry::Level::Fatal);
        },
    );
    if let Some(client) = sentry::Hub::current().client() {
        client.flush(Some(Duration::from_secs(2)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_startup_failure_is_reported_by_kind_and_not_by_text() {
        let events = sentry::test::with_captured_events(|| report_fatal("library"));

        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.level, sentry::Level::Fatal);
        assert_eq!(event.message.as_deref(), Some("Orivo could not start: library"));
        assert_eq!(event.tags.get("startup_failure").map(String::as_str), Some("library"));
    }

    #[test]
    fn without_a_client_reporting_a_startup_failure_does_nothing() {
        // The state of every build made without a DSN: no hub client, so there
        // is nothing to send and nothing to wait for.
        report_fatal("library");
    }
}
