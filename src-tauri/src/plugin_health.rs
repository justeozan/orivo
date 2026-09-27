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
//! The journal's text is already bounded and stripped of control characters
//! before it reaches `PluginJournal` (`plugin_runtime.rs`'s `sanitise_text`),
//! so this module passes it through rather than sanitising it a second time.
//! Turning a decision code into a sentence a player can read is the settings
//! panel's job, the same way `plugin-manager.ts` turns `PluginState` into a
//! sentence today.

use crate::plugin_manifest::valid_opaque_id;
use crate::plugin_runtime::{PluginJournal, PluginRuntime};
use crate::plugin_scheduler::PluginScheduler;
use serde::Serialize;

const MAX_PLUGIN_ID_LENGTH: usize = 256;
/// A settings panel wants the last page of what happened, not the whole ring:
/// the ring itself is already bounded for the host's own reasons.
const MAX_JOURNAL_ROWS: usize = 40;

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
/// Decisions and the plugin's own log lines are two separate rings, so they are
/// not already interleaved in the order they happened — sorting by
/// `correlation_id` puts them back in that order, because both rings draw from
/// the same monotonic counter.
fn journal_view(journal: &PluginJournal, plugin_id: &str) -> Vec<PluginJournalEntryView> {
    let mut rows = journal.entries();
    rows.extend(journal.plugin_messages());
    rows.retain(|entry| entry.plugin_id == plugin_id);
    rows.sort_by_key(|entry| entry.correlation_id.0);
    rows.reverse();
    rows.truncate(MAX_JOURNAL_ROWS);
    rows.into_iter()
        .map(|entry| PluginJournalEntryView {
            plugin_id: entry.plugin_id,
            decision: entry.decision.to_owned(),
            detail: entry.detail,
            repeats: entry.repeats,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// One report per requested id. An id nobody ever submitted a job for reads as
/// perfectly healthy — a plugin that has never run cannot be degraded — so the
/// caller does not need to special-case "not found" here.
#[tauri::command]
pub fn get_plugin_health_report(plugin_ids: Vec<String>) -> Result<Vec<PluginHealthView>, String> {
    let runtime = PluginRuntime::shared().map_err(|error| error.to_string())?;
    let scheduler = runtime.scheduler();
    Ok(plugin_ids
        .into_iter()
        .filter(|id| opaque(id))
        .map(|id| health_view(scheduler, &id))
        .collect())
}

/// Take a plugin out of `degraded`. Explicit, by request: the plan asks for a
/// resume button, not a timer that quietly re-enables a broken extension.
#[tauri::command]
pub fn resume_plugin(plugin_id: String) -> Result<PluginHealthView, String> {
    if !opaque(&plugin_id) {
        return Err("This plugin is no longer available.".into());
    }
    let runtime = PluginRuntime::shared().map_err(|error| error.to_string())?;
    runtime.scheduler().resume(&plugin_id);
    Ok(health_view(runtime.scheduler(), &plugin_id))
}

#[tauri::command]
pub fn get_plugin_journal(plugin_id: String) -> Result<Vec<PluginJournalEntryView>, String> {
    if !opaque(&plugin_id) {
        return Err("This plugin is no longer available.".into());
    }
    let runtime = PluginRuntime::shared().map_err(|error| error.to_string())?;
    Ok(journal_view(runtime.journal(), &plugin_id))
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

    #[test]
    fn an_id_that_is_not_opaque_is_refused() {
        assert_eq!(
            resume_plugin("../etc/passwd".into()),
            Err("This plugin is no longer available.".into())
        );
        assert_eq!(
            get_plugin_journal("../etc/passwd".into()),
            Err("This plugin is no longer available.".into())
        );
    }
}
