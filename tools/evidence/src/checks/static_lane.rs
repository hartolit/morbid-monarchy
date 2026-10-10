//! `check static`: formatting, compilation of each declared feature combination, Clippy and
//! compiler warnings against the finite baseline, warning-denying rustdoc, and repository
//! contracts.

use std::path::Path;

use crate::{
    checks::{contracts, lints},
    process::{Step, run_logged},
    report::{Item, State},
    workspace::Workspace,
};

/// Feature combinations that must compile. `--all-features` alone would hide a crate that only
/// builds because another member enables its optional feature.
const COMPILE_COMBINATIONS: &[(&str, &[&str])] = &[
    (
        "static.check.default",
        &["check", "--workspace", "--all-targets", "--locked"],
    ),
    (
        "static.check.all-features",
        &[
            "check",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
        ],
    ),
    (
        "static.check.spatial-lib-minimal",
        &[
            "check",
            "-p",
            "spatial-lib",
            "--all-targets",
            "--no-default-features",
            "--locked",
        ],
    ),
];

pub fn run(root: &Path, run_dir: &Path, lint_baseline: &Path) -> Vec<Item> {
    let mut items = Vec::new();

    let fmt = run_logged(
        &Step::new("cargo", root).args(["fmt", "--all", "--", "--check"]),
        run_dir,
        "static-fmt",
    );
    items.push(step_item("static.fmt", "rustfmt reports no changes", fmt));

    for (id, args) in COMPILE_COMBINATIONS {
        let outcome = run_logged(&Step::new("cargo", root).args(*args), run_dir, id);
        items.push(step_item(id, &format!("cargo {}", args.join(" ")), outcome));
    }

    items.push(clippy_item(root, run_dir, lint_baseline));

    let doc = run_logged(
        &Step::new("cargo", root)
            .args([
                "doc",
                "--workspace",
                "--no-deps",
                "--all-features",
                "--locked",
            ])
            .env("RUSTDOCFLAGS", "-D warnings"),
        run_dir,
        "static-rustdoc",
    );
    items.push(step_item(
        "static.rustdoc",
        "rustdoc with warnings denied",
        doc,
    ));

    match (Workspace::load(root, true), Workspace::load(root, false)) {
        (Ok(all), Ok(default)) => items.push(contracts::dependency_item(&all, &default)),
        (Err(error), _) | (_, Err(error)) => {
            items.push(Item::new(
                "static.contracts.dependencies",
                State::Failed,
                error,
            ));
        }
    }
    items.push(contracts::pins_item(root));
    items
}

fn step_item(id: &str, description: &str, outcome: crate::process::Outcome) -> Item {
    let summary = match &outcome.reason {
        Some(reason) => format!("{description}: {reason}"),
        None => description.to_owned(),
    };
    Item::new(id, outcome.state, summary).with_log(outcome.log)
}

fn clippy_item(root: &Path, run_dir: &Path, lint_baseline: &Path) -> Item {
    let baseline = match lints::LintBaseline::load(lint_baseline) {
        Ok(baseline) => baseline,
        Err(error) => return Item::new("static.clippy", State::Failed, error),
    };
    let outcome = run_logged(
        &Step::new("cargo", root).args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--message-format=json",
        ]),
        run_dir,
        "static-clippy",
    );
    if outcome.state == State::Unavailable {
        return Item::new(
            "static.clippy",
            State::Unavailable,
            outcome.reason.unwrap_or_default(),
        )
        .with_log(outcome.log);
    }
    let diagnostics = lints::parse_diagnostics(&outcome.stdout, root);
    let mut problems = lints::compare(&diagnostics, &baseline);
    if outcome.state != State::Passed && problems.is_empty() {
        problems.push(format!(
            "cargo clippy {} without a reported diagnostic",
            outcome.reason.unwrap_or_default()
        ));
    }
    let excepted: usize = baseline
        .exceptions
        .iter()
        .map(|exception| exception.count)
        .sum();
    let state = if problems.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "static.clippy",
        state,
        format!(
            "{} diagnostics; {excepted} covered by {} baseline exceptions",
            diagnostics.len(),
            baseline.exceptions.len()
        ),
    )
    .with_details(problems)
    .with_log(outcome.log)
}
