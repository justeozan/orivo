//! Health and journal commands for Settings › Plugins.
//!
//! `PluginScheduler::health`, `::resume` and `PluginJournal::entries` /
//! `::plugin_messages` already exist — P1 and P2 wrote them and marked them
//! `#[allow(dead_code)]`, because nothing called them yet. This module is that
//! caller. It lives in its own file rather than in `plugin_runtime.rs` or
//! `plugin_scheduler.rs`, which two other lots are editing on this machine at
//! the same time (a compile cache, Windows sandbox parity): reading already
//! `pub` methods from a new file cannot conflict with either.
//!
//! Guest-authored text — a health-check message, a plugin's own log line — is
//! already bounded and stripped of control characters before it reaches
//! `PluginJournal` (`plugin_runtime.rs`'s `sanitise_text`), so this module
//! passes it through rather than sanitising it a second time. Host-authored
//! text (a panic message, a job's own error) is not guest input on its own,
//! but is not automatically innocent either: a `job-failed` detail can be the
//! `Display` of a `PluginRuntimeError::Plugin`, which embeds a message the
//! plugin chose — sanitised at the point it was captured, by the same
//! `sanitise_text` call a health-check message goes through, not by anything
//! in this module. Turning a decision code into a sentence a player can read
//! is the settings panel's job, the same way `plugin-manager.ts` turns a bare
//! `PluginState` into a sentence today.

use crate::plugin_manifest::valid_opaque_id;
use crate::plugin_runtime::{PluginJournal, PluginRuntime};
use crate::plugin_scheduler::PluginScheduler;
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_PLUGIN_ID_LENGTH: usize = 256;
/// A settings panel wants the last page of what happened, not the whole ring:
/// the ring itself is already bounded for the host's own reasons.
const MAX_JOURNAL_ROWS: usize = 40;
/// A batched health read is for the plugins Settings actually lists. Bounded
/// so a malformed or hostile caller cannot turn one IPC round trip into an
/// unbounded scan of the scheduler's table.
const MAX_HEALTH_REPORT_IDS: usize = 256;
/// The decision `PluginJournal::plugin_messages` always records under — never
/// used for a host decision, which is what lets `journal_view` tell a
/// component's own log line apart from the host's account of what happened.
const PLUGIN_LOG_DECISION: &str = "plugin-log";

fn opaque(value: &str) -> bool {
    valid_opaque_id(value, MAX_PLUGIN_ID_LENGTH)
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginHealthView {
    pub plugin_id: String,
    pub queued: usize,
    pub running: usize,
    pub consecutive_failures: u32,
    pub degraded: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginJournalEntryView {
    pub plugin_id: String,
    pub decision: String,
    pub detail: String,
    pub repeats: u32,
}

fn health_view(scheduler: &PluginScheduler, plugin_id: &str) -> PluginHealthView {
    let state = scheduler.health(plugin_id);
    PluginHealthView {
        plugin_id: plugin_id.to_owned(),
        queued: state.queued,
        running: state.running,
        consecutive_failures: state.consecutive_failures,
        degraded: state.degraded,
    }
}

/// One plugin's journal, newest first and bounded to [`MAX_JOURNAL_ROWS`].
///
/// Three things happen before the row budget is spent.
///
/// First, decisions and the plugin's own log lines are two separate rings,
/// ordered against each other by `correlation_id` — both draw from the same
/// monotonic counter, so this approximates the order they happened in, though
/// two entries sharing one correlation id are not resolved any further
/// against each other, since the host does not record a sub-order within one
/// call.
///
/// Second, the ring only collapses a decision reached twice under the *same*
/// correlation id; a decision reached on every call, each under its own id —
/// a plugin that simply never declared a capability logs `capability-unlinked`
/// exactly this way — is not collapsed there. Folding matching
/// `(decision, detail)` pairs together here, before truncating, is what stops
/// that from filling the window on its own — the merged row's own `repeats`
/// still says how often it was reached.
///
/// Third, a component earns `degraded` (or `job-failed`, `host-call-budget`,
/// ...) by making a call fail — the same call it may also have logged dozens
/// of lines during, all sharing that call's one correlation id. Sorting by
/// recency alone cannot separate them in that case, so the host's own
/// decisions are kept first, unconditionally, and the plugin's own log lines
/// only get whatever room is left: they can crowd each other out, but never
/// the one line that explains why the plugin is in the state it is in.
fn journal_view(journal: &PluginJournal, plugin_id: &str) -> Vec<PluginJournalEntryView> {
    let mut rows = journal.entries();
    rows.extend(journal.plugin_messages());
    rows.retain(|entry| entry.plugin_id == plugin_id);

    let mut merged: BTreeMap<(&'static str, String), (u32, u64)> = BTreeMap::new();
    for entry in &rows {
        let slot = merged
            .entry((entry.decision, entry.detail.clone()))
            .or_insert((0, entry.correlation_id.0));
        slot.0 = slot.0.saturating_add(entry.repeats);
        slot.1 = slot.1.max(entry.correlation_id.0);
    }

    let mut collapsed: Vec<(u64, PluginJournalEntryView)> = merged
        .into_iter()
        .map(|((decision, detail), (repeats, newest_correlation_id))| {
            (
                newest_correlation_id,
                PluginJournalEntryView {
                    plugin_id: plugin_id.to_owned(),
                    decision: decision.to_owned(),
                    detail,
                    repeats,
                },
            )
        })
        .collect();
    // Newest first. A plain `sort_by_key` here is stable, so two entries tied
    // on correlation id — the ordinary case for a plugin's own log lines from
    // one call — would otherwise keep whatever order `BTreeMap` happened to
    // iterate its `(decision, detail)` keys in, which is alphabetical and has
    // nothing to do with either recency or importance.
    collapsed.sort_by(|left, right| right.0.cmp(&left.0));

    let (host, plugin_log): (Vec<_>, Vec<_>) = collapsed
        .into_iter()
        .partition(|(_, view)| view.decision != PLUGIN_LOG_DECISION);
    let mut kept: Vec<(u64, PluginJournalEntryView)> =
        host.into_iter().take(MAX_JOURNAL_ROWS).collect();
    let remaining = MAX_JOURNAL_ROWS.saturating_sub(kept.len());
    kept.extend(plugin_log.into_iter().take(remaining));
    // `partition` preserved each half's own newest-first order, but the halves
    // themselves are no longer interleaved — restore that before returning, so
    // a call that needed no trimming at all still reads in plain recency order.
    kept.sort_by(|left, right| right.0.cmp(&left.0));
    kept.into_iter().map(|(_, view)| view).collect()
}

fn get_plugin_health_report_sync(plugin_ids: Vec<String>) -> Result<Vec<PluginHealthView>, String> {
    let runtime = PluginRuntime::shared().map_err(|error| error.to_string())?;
    let scheduler = runtime.scheduler();
    Ok(plugin_ids
        .into_iter()
        .filter(|id| opaque(id))
        .take(MAX_HEALTH_REPORT_IDS)
        .map(|id| health_view(scheduler, &id))
        .collect())
}

fn resume_plugin_sync(plugin_id: &str) -> Result<PluginHealthView, String> {
    if !opaque(plugin_id) {
        return Err("This plugin is no longer available.".into());
    }
    let runtime = PluginRuntime::shared().map_err(|error| error.to_string())?;
    runtime.scheduler().resume(plugin_id);
    Ok(health_view(runtime.scheduler(), plugin_id))
}

fn get_plugin_journal_sync(plugin_id: &str) -> Result<Vec<PluginJournalEntryView>, String> {
    if !opaque(plugin_id) {
        return Err("This plugin is no longer available.".into());
    }
    let runtime = PluginRuntime::shared().map_err(|error| error.to_string())?;
    Ok(journal_view(runtime.journal(), plugin_id))
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// One report per requested id. An id nobody ever submitted a job for reads as
/// perfectly healthy — a plugin that has never run cannot be degraded — so the
/// caller does not need to special-case "not found" here.
///
/// `spawn_blocking`, like `runner_commands::get_installed_runners`: the first
/// call into `PluginRuntime::shared()` in a process starts the scheduler's
/// worker threads, which does not belong on Tauri's command executor any more
/// than discovery does.
#[tauri::command]
pub async fn get_plugin_health_report(
    plugin_ids: Vec<String>,
) -> Result<Vec<PluginHealthView>, String> {
    tauri::async_runtime::spawn_blocking(move || get_plugin_health_report_sync(plugin_ids))
        .await
        .map_err(|_| "Orivo could not read plugin health.".to_string())?
}

/// Take a plugin out of `degraded`. Explicit, by request: the plan asks for a
/// resume button, not a timer that quietly re-enables a broken extension.
#[tauri::command]
pub async fn resume_plugin(plugin_id: String) -> Result<PluginHealthView, String> {
    tauri::async_runtime::spawn_blocking(move || resume_plugin_sync(&plugin_id))
        .await
        .map_err(|_| "Resuming this plugin did not finish. Try again.".to_string())?
}

#[tauri::command]
pub async fn get_plugin_journal(plugin_id: String) -> Result<Vec<PluginJournalEntryView>, String> {
    tauri::async_runtime::spawn_blocking(move || get_plugin_journal_sync(&plugin_id))
        .await
        .map_err(|_| "Orivo could not read this plugin's log.".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::next_correlation_id;
    use crate::plugin_scheduler::SchedulerLimits;
    use std::sync::Arc;

    #[test]
    fn a_plugin_that_never_ran_is_reported_healthy() {
        let scheduler = PluginScheduler::new(
            SchedulerLimits::default(),
            Arc::new(PluginJournal::default()),
        );
        let view = health_view(&scheduler, "com.orivo.never-ran");
        assert_eq!(
            view,
            PluginHealthView {
                plugin_id: "com.orivo.never-ran".into(),
                queued: 0,
                running: 0,
                consecutive_failures: 0,
                degraded: false,
            }
        );
    }

    #[test]
    fn resuming_clears_degraded_and_the_failure_count() {
        let scheduler = PluginScheduler::new(
            SchedulerLimits::default(),
            Arc::new(PluginJournal::default()),
        );
        // There is no public way to force a plugin into `degraded` without
        // running real jobs through the scheduler, which `plugin_scheduler.rs`
        // already covers. What this command layer owns is that `resume` is
        // reachable and its answer reflects the state right after — so the
        // property worth pinning here is idempotence: resuming a plugin that
        // was never degraded is a no-op, not an error.
        scheduler.resume("com.orivo.idle-runner");
        let view = health_view(&scheduler, "com.orivo.idle-runner");
        assert!(!view.degraded);
        assert_eq!(view.consecutive_failures, 0);
    }

    #[test]
    fn the_journal_is_filtered_by_plugin_and_newest_first() {
        let journal = PluginJournal::default();
        journal.record(
            next_correlation_id(),
            "com.orivo.a",
            "scope-refused",
            "first",
        );
        journal.record(
            next_correlation_id(),
            "com.orivo.b",
            "scope-refused",
            "not this plugin",
        );
        journal.record(next_correlation_id(), "com.orivo.a", "trap", "second");

        let rows = journal_view(&journal, "com.orivo.a");

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.plugin_id == "com.orivo.a"));
        assert_eq!(rows[0].detail, "second", "newest entry comes first");
        assert_eq!(rows[1].detail, "first");
    }

    #[test]
    fn the_journal_is_bounded_even_when_both_rings_hold_entries() {
        let journal = PluginJournal::default();
        for index in 0..(MAX_JOURNAL_ROWS + 10) {
            journal.record(
                next_correlation_id(),
                "com.orivo.chatty",
                "scope-refused",
                format!("attempt {index}"),
            );
        }
        let rows = journal_view(&journal, "com.orivo.chatty");
        assert_eq!(rows.len(), MAX_JOURNAL_ROWS);
    }

    /// The regression this module actually shipped with: a plugin that logs the
    /// same refusal on every call — which is exactly what happens when it never
    /// declares a capability it does not use — must not be able to fill the
    /// 40-row window with one reason and push out something that only happened
    /// once but mattered more.
    #[test]
    fn a_reason_repeated_under_many_correlation_ids_is_folded_into_one_row() {
        let journal = PluginJournal::default();
        for _ in 0..60 {
            journal.record(
                next_correlation_id(),
                "com.orivo.chatty",
                "capability-unlinked",
                "files_read is not declared, so its host import is absent",
            );
        }
        journal.record(
            next_correlation_id(),
            "com.orivo.chatty",
            "degraded",
            "paused after repeated failures; a resume is required",
        );

        let rows = journal_view(&journal, "com.orivo.chatty");

        assert_eq!(rows.len(), 2, "the repeated reason cost one row, not sixty");
        assert_eq!(
            rows[0].decision, "degraded",
            "the newest, distinct entry still leads"
        );
        assert_eq!(rows[1].decision, "capability-unlinked");
        assert_eq!(rows[1].repeats, 60);
    }

    /// `journal.entries()` and `journal.plugin_messages()` are two separate
    /// rings, and only `record()` (decisions) is public — a component's own log
    /// line only ever reaches the second ring through a real plugin invocation,
    /// which is what `record_plugin_message` being `pub(crate)` for tests is
    /// for. Without exercising both rings, a bug that only manifests when
    /// interleaving them (an off-by-one in the merge, a decision silently
    /// dropped) has nothing here to catch it.
    #[test]
    fn the_journal_actually_interleaves_both_rings_not_just_one() {
        let journal = PluginJournal::default();
        let first = next_correlation_id();
        journal.record(first, "com.orivo.a", "discover-page", "refused: no grant");
        let second = next_correlation_id();
        journal.record_plugin_message(second, "com.orivo.a", "info: starting import");

        let rows = journal_view(&journal, "com.orivo.a");

        assert_eq!(
            rows.len(),
            2,
            "a row from each ring, not just the public one"
        );
        assert_eq!(
            rows[0].detail, "info: starting import",
            "the plugin's own line is newer"
        );
        assert_eq!(rows[1].detail, "refused: no grant");
    }

    #[test]
    fn an_id_that_is_not_opaque_is_refused() {
        assert_eq!(
            resume_plugin_sync("../etc/passwd"),
            Err("This plugin is no longer available.".into())
        );
        assert_eq!(
            get_plugin_journal_sync("../etc/passwd"),
            Err("This plugin is no longer available.".into())
        );
    }

    #[test]
    fn a_health_report_is_bounded_even_when_every_id_is_valid() {
        let ids: Vec<String> = (0..(MAX_HEALTH_REPORT_IDS + 10))
            .map(|index| format!("com.orivo.plugin-{index}"))
            .collect();
        let report = get_plugin_health_report_sync(ids).unwrap();
        assert_eq!(report.len(), MAX_HEALTH_REPORT_IDS);
    }

    /// The invalid id has to sit well inside the bound, not past it: the
    /// filter and the `.take` compose left to right, so an invalid id placed
    /// beyond `MAX_HEALTH_REPORT_IDS` is never reached by either one and the
    /// assertion below would pass whether or not the opaque check exists.
    #[test]
    fn a_health_report_drops_an_id_that_fails_the_opaque_grammar() {
        let mut ids: Vec<String> = vec!["../etc/passwd".into()];
        ids.extend((0..MAX_HEALTH_REPORT_IDS).map(|index| format!("com.orivo.plugin-{index}")));
        let report = get_plugin_health_report_sync(ids).unwrap();
        assert_eq!(report.len(), MAX_HEALTH_REPORT_IDS);
        assert!(report.iter().all(|row| row.plugin_id != "../etc/passwd"));
    }

    /// The failing-first proof for the journal window's priority rule: a
    /// plugin that logs generously in the very call that fails must not be
    /// able to bury the host's own account of that failure.
    #[test]
    fn a_hosts_own_decision_survives_a_chatty_plugin_in_the_same_failing_call() {
        let journal = PluginJournal::default();
        let failing_call = next_correlation_id();
        for index in 0..(MAX_JOURNAL_ROWS + 10) {
            journal.record_plugin_message(
                failing_call,
                "com.orivo.chatty",
                format!("info: step {index}"),
            );
        }
        journal.record(
            failing_call,
            "com.orivo.chatty",
            "trap",
            "paused after repeated failures; a resume is required",
        );

        let rows = journal_view(&journal, "com.orivo.chatty");

        assert_eq!(rows.len(), MAX_JOURNAL_ROWS);
        assert!(
            rows.iter().any(|row| row.decision == "trap"),
            "the plugin's own fifty log lines from the same call must not bury it"
        );
    }
}
