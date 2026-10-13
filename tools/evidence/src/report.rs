//! Evidence results and the machine-readable report every command writes.
//!
//! A lane passes only when every required item passed. "Unavailable" (missing hardware or a
//! missing tool) and "not-run" are never success, so a lane with an absent GPU or an empty test
//! selection cannot be reported as green.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::identity::Identity;

/// Version of the `report.json` layout. Increment it when a field changes meaning.
pub const REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Passed,
    Failed,
    /// The check could not execute here: no adapter, a missing tool, an unsupported platform.
    Unavailable,
    NotRun,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Unavailable => "unavailable",
            Self::NotRun => "not-run",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    /// Stable dotted identifier, for example `static.clippy`.
    pub id: String,
    pub state: State,
    /// Whether the enclosing lane needs this item to pass.
    pub required: bool,
    pub summary: String,
    /// Individual findings: a broken link, a failed test, an unexpected lint.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub details: Vec<String>,
    /// Captured output of the nested command, relative to the run directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
}

impl Item {
    pub fn new(id: impl Into<String>, state: State, summary: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            state,
            required: true,
            summary: summary.into(),
            details: Vec::new(),
            log: None,
        }
    }

    pub fn with_details(mut self, details: Vec<String>) -> Self {
        self.details = details;
        self
    }

    pub fn with_log(mut self, log: Option<String>) -> Self {
        self.log = log;
        self
    }

    /// Marks an informational item the lane does not need to pass.
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }
}

/// The overall outcome of a set of items.
///
/// An empty set is a failure: a lane that executed nothing has not established anything.
pub fn overall(items: &[Item]) -> State {
    let required: Vec<&Item> = items.iter().filter(|item| item.required).collect();
    if required.is_empty() {
        return State::Failed;
    }
    if required.iter().all(|item| item.state == State::Passed) {
        return State::Passed;
    }
    if required.iter().any(|item| item.state == State::Failed) {
        return State::Failed;
    }
    if required.iter().any(|item| item.state == State::Unavailable) {
        return State::Unavailable;
    }
    State::NotRun
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub command: Vec<String>,
    pub result: State,
    pub started_at: String,
    pub finished_at: String,
    pub duration_seconds: f64,
    pub identity: Identity,
    pub items: Vec<Item>,
    /// Command-specific structured output, for example adapter inventories or measurements.
    #[serde(skip_serializing_if = "serde_json::Value::is_null")]
    pub data: serde_json::Value,
}

impl Report {
    pub fn write(&self, run_dir: &Path) -> Result<PathBuf, String> {
        let path = run_dir.join("report.json");
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| format!("serialize report: {error}"))?;
        fs::write(&path, json + "\n")
            .map_err(|error| format!("write {}: {error}", path.display()))?;
        Ok(path)
    }
}

/// Prints one line per item and the overall result to standard output.
pub fn print_summary(items: &[Item], result: State, report_path: &Path) {
    for item in items {
        let marker = if item.required { "" } else { " (optional)" };
        println!(
            "[{:>11}] {}{marker}: {}",
            item.state.label(),
            item.id,
            item.summary
        );
        for detail in item.details.iter().take(20) {
            println!("              - {detail}");
        }
        if item.details.len() > 20 {
            println!(
                "              … {} more in the report",
                item.details.len() - 20
            );
        }
        if let Some(log) = &item.log {
            println!("              log: {log}");
        }
    }
    println!("result: {} ({})", result.label(), report_path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(state: State) -> Item {
        Item::new("x", state, "")
    }

    #[test]
    fn empty_lane_is_not_success() {
        assert_eq!(overall(&[]), State::Failed);
        let mut optional = item(State::Passed);
        optional.required = false;
        assert_eq!(overall(&[optional]), State::Failed);
    }

    #[test]
    fn every_required_item_must_pass() {
        assert_eq!(
            overall(&[item(State::Passed), item(State::Passed)]),
            State::Passed
        );
        assert_eq!(
            overall(&[item(State::Passed), item(State::Failed)]),
            State::Failed
        );
        assert_eq!(
            overall(&[item(State::Passed), item(State::Unavailable)]),
            State::Unavailable
        );
        assert_eq!(
            overall(&[item(State::Passed), item(State::NotRun)]),
            State::NotRun
        );
        assert_eq!(
            overall(&[item(State::Unavailable), item(State::Failed)]),
            State::Failed
        );
    }

    #[test]
    fn optional_items_do_not_decide_the_lane() {
        let mut optional = item(State::Failed);
        optional.required = false;
        assert_eq!(overall(&[item(State::Passed), optional]), State::Passed);
    }
}
