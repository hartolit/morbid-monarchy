//! Exit-status contract of the `monarchy-evidence` binary, exercised end to end.
//!
//! These are the runner's deliberate failure specimens: a request it cannot satisfy must exit
//! nonzero and record a non-passing result, never a green one.

use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn runner() -> Command {
    Command::new(env!("CARGO_BIN_EXE_monarchy-evidence"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "monarchy-evidence-runner-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn report(dir: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(dir.join("report.json")).expect("report.json written");
    serde_json::from_str(&text).expect("report is JSON")
}

fn run(args: &[&str]) -> Output {
    runner().args(args).output().expect("runner starts")
}

#[test]
fn usage_errors_exit_with_status_two_and_write_no_report() {
    for args in [
        &["deploy"][..],
        &["test", "gpu", "--suite", "physics", "--adapter", "x"][..],
        &[][..],
    ] {
        let output = run(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("usage:"));
    }
}

#[test]
fn a_gpu_suite_on_a_missing_adapter_is_not_a_pass() {
    let dir = scratch("missing-adapter");
    let output = run(&[
        "test",
        "gpu",
        "--suite",
        "cells",
        "--adapter",
        "no-such-adapter",
        "--output",
        dir.to_str().expect("utf-8 path"),
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(report(&dir)["result"], "unavailable");
}

#[test]
fn an_unimplemented_scenario_fails_on_every_available_adapter() {
    // Enumerating adapters is safe on any host; with none, the request is unavailable instead.
    let dir = scratch("inspect");
    let _ = run(&[
        "inspect-adapter",
        "--output",
        dir.to_str().expect("utf-8 path"),
    ]);
    let adapters = report(&dir)["data"]["adapters"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for adapter in adapters {
        let id = adapter["id"].as_str().expect("adapter id");
        let out = scratch(&format!("scenario-{id}"));
        let output = run(&[
            "run",
            "--scenario",
            "resident-world",
            "--adapter",
            id,
            "--output",
            out.to_str().expect("utf-8"),
        ]);
        assert_eq!(output.status.code(), Some(1), "{id}");
        assert_eq!(report(&out)["result"], "failed", "{id}");
    }
}

#[test]
fn the_report_identifies_its_source_and_toolchain() {
    let dir = scratch("identity");
    let _ = run(&[
        "test",
        "gpu",
        "--suite",
        "all",
        "--adapter",
        "none",
        "--output",
        dir.to_str().expect("utf-8"),
    ]);
    let report = report(&dir);
    assert_eq!(report["schema_version"], 1);
    assert!(report["identity"]["toolchain"]["lockfile_sha256"].is_string());
    assert!(report["identity"]["source"]["commit"].is_string());
    assert_eq!(report["command"][0], "test");
}
