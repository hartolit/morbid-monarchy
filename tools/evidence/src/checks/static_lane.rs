//! `check static`: formatting, compilation of each declared feature combination, Clippy and
//! compiler warnings against the finite baseline, warning-denying rustdoc, dependency policy
//! (`cargo deny`), unused dependencies (`cargo machete`) and repository contracts.

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

/// A dependency checker the lane requires, at the version CI installs. A missing or different
/// version fails the lane: a check that did not run is not a pass.
pub struct RequiredTool {
    pub id: &'static str,
    /// The cargo subcommand, as in `cargo <subcommand>`.
    pub subcommand: &'static str,
    /// The crate `cargo install` takes.
    pub package: &'static str,
    pub version: &'static str,
    pub args: &'static [&'static str],
    pub description: &'static str,
}

pub const REQUIRED_TOOLS: &[RequiredTool] = &[
    RequiredTool {
        id: "static.deny",
        subcommand: "deny",
        package: "cargo-deny",
        version: "0.20.2",
        args: &["--locked", "check"],
        description: "advisories, licenses, bans and sources (deny.toml)",
    },
    RequiredTool {
        id: "static.unused-dependencies",
        subcommand: "machete",
        package: "cargo-machete",
        version: "0.9.2",
        args: &[],
        description: "no declared dependency is unused",
    },
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

    for tool in REQUIRED_TOOLS {
        items.push(tool_item(root, run_dir, tool));
    }

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
    items.push(contracts::ci_item(root, REQUIRED_TOOLS));
    items
}

/// Runs a required checker after confirming its version.
fn tool_item(root: &Path, run_dir: &Path, tool: &RequiredTool) -> Item {
    let install = format!("cargo install --locked {}@{}", tool.package, tool.version);
    let version = run_logged(
        &Step::new("cargo", root)
            .args([tool.subcommand, "--version"])
            .without_runner_package_env(),
        run_dir,
        &format!("{}-version", tool.id),
    );
    let reported = version.stdout.split_whitespace().last().unwrap_or_default();
    if version.state != State::Passed || reported != tool.version {
        let found = if version.state == State::Passed {
            format!("version {reported}")
        } else {
            "not runnable".to_owned()
        };
        return Item::new(
            tool.id,
            State::Failed,
            format!(
                "{} {} is required, {found}; install it with `{install}`",
                tool.package, tool.version
            ),
        )
        .with_log(version.log);
    }
    let outcome = run_logged(
        &Step::new("cargo", root)
            .arg(tool.subcommand)
            .args(tool.args)
            .without_runner_package_env(),
        run_dir,
        tool.id,
    );
    step_item(
        tool.id,
        &format!("{} {}: {}", tool.package, tool.version, tool.description),
        outcome,
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_required_checker_fails_instead_of_passing() {
        let dir = std::env::temp_dir().join(format!(
            "monarchy-evidence-static-tool-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let tool = RequiredTool {
            id: "static.example",
            subcommand: "monarchy-no-such-checker",
            package: "monarchy-no-such-checker",
            version: "1.0.0",
            args: &[],
            description: "an absent checker",
        };
        let item = tool_item(&dir, &dir, &tool);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(item.state, State::Failed);
        assert!(
            item.summary
                .contains("cargo install --locked monarchy-no-such-checker@1.0.0"),
            "{}",
            item.summary
        );
    }
}
