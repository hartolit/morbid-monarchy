//! GPU suites: cases that execute the production GPU owner on an explicit adapter.
//!
//! Each case runs in its own `gpu-suite` process (`src/bin/gpu-suite.rs`), so a device loss, a
//! crash or a hang affects only that case. The runner ([`crate::gpu`]) builds that binary, runs
//! every case of the requested suite with a time limit, and compares the cases it executed
//! with the coverage registry, so a case cannot disappear unnoticed. A case reports failures,
//! notes and metrics; it passes only when it recorded no failure.

pub mod alloc;
pub mod bodies;
pub mod cells;
pub mod driver;
pub mod probe;
pub mod transfers;

use std::{
    fmt::Debug,
    sync::atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::cli::GpuSuite;

/// One case of a suite.
pub struct Case {
    pub name: &'static str,
    pub summary: &'static str,
    pub run: fn(&str, &mut Check) -> Result<(), String>,
}

/// What a case process reports.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub suite: String,
    pub case: String,
    pub adapter: String,
    pub passed: bool,
    pub failures: Vec<String>,
    pub notes: Vec<String>,
    pub metrics: Map<String, Value>,
}

/// Accumulates a case's findings.
#[derive(Debug, Default)]
pub struct Check {
    failures: Vec<String>,
    notes: Vec<String>,
    metrics: Map<String, Value>,
}

impl Check {
    pub fn that(&mut self, condition: bool, what: impl FnOnce() -> String) {
        if !condition {
            self.failures.push(what());
        }
    }

    pub fn equal<T: PartialEq + Debug>(&mut self, what: &str, actual: T, expected: T) {
        if actual != expected {
            self.failures
                .push(format!("{what}: got {actual:?}, expected {expected:?}"));
        }
    }

    pub fn fail(&mut self, what: impl Into<String>) {
        self.failures.push(what.into());
    }

    pub fn note(&mut self, text: impl Into<String>) {
        self.notes.push(text.into());
    }

    pub fn metric(&mut self, key: &str, value: impl Into<Value>) {
        self.metrics.insert(key.to_owned(), value.into());
    }

    pub fn failures(&self) -> usize {
        self.failures.len()
    }
}

static ALLOW_SOFTWARE: AtomicBool = AtomicBool::new(false);

/// Lets cases open a software adapter; set by `gpu-suite` when the runner selected one
/// explicitly, so its results are still labelled software.
pub fn set_allow_software(allow: bool) {
    ALLOW_SOFTWARE.store(allow, Ordering::Relaxed);
}

pub fn allow_software() -> bool {
    ALLOW_SOFTWARE.load(Ordering::Relaxed)
}

pub fn cases(suite: GpuSuite) -> Option<&'static [Case]> {
    match suite {
        GpuSuite::Transfers => Some(transfers::CASES),
        GpuSuite::Cells => Some(cells::CASES),
        GpuSuite::Bodies => Some(bodies::CASES),
        _ => None,
    }
}

/// Runs one case in this process.
pub fn run_case(suite: GpuSuite, case: &Case, adapter: &str) -> CaseResult {
    let mut check = Check::default();
    if let Err(error) = (case.run)(adapter, &mut check) {
        check.fail(format!("stopped early: {error}"));
    }
    CaseResult {
        suite: suite.name().to_owned(),
        case: case.name.to_owned(),
        adapter: adapter.to_owned(),
        passed: check.failures.is_empty(),
        failures: check.failures,
        notes: check.notes,
        metrics: check.metrics,
    }
}
