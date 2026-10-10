//! Repository contracts that source shape can establish: dependency direction, isolation of the
//! evidence leaf, and the toolchain/lock pins. They cannot prove physical correctness; those
//! claims are executed through the production owners.

use std::{fs, path::Path, process::Command};

use crate::{
    report::{Item, State},
    workspace::{DependencyKind, Workspace},
};

/// The evidence runner as named in the workspace.
pub const EVIDENCE_PACKAGE: &str = "monarchy-evidence";
/// The application composition root; nothing but the evidence leaf may import it.
pub const APPLICATION_PACKAGE: &str = "morbid-app";
/// The alias every recipe uses; `.cargo/config.toml` must define exactly this.
pub const EVIDENCE_ALIAS: &str = "run --locked --release -p monarchy-evidence --";

/// A package whose production dependency closure must avoid certain packages.
struct ForbiddenClosure {
    package: &'static str,
    forbidden_prefixes: &'static [&'static str],
    reason: &'static str,
}

/// Dependency-direction rules in force. Later stages add their packages here when they create
/// them, so a rule never passes vacuously for a package that does not exist.
const FORBIDDEN_CLOSURES: &[ForbiddenClosure] = &[ForbiddenClosure {
    package: EVIDENCE_PACKAGE,
    forbidden_prefixes: &["bevy", "morbid-app", "monarch-engine"],
    reason: "default evidence builds must not compile the Bevy baseline; it is behind `legacy-baseline`",
}];

/// `default_features` is the workspace resolved without `--all-features`.
pub fn dependency_item(all_features: &Workspace, default_features: &Workspace) -> Item {
    let mut problems = Vec::new();

    for package in all_features.members() {
        for dependency in all_features.direct(
            &package.id,
            &[
                DependencyKind::Normal,
                DependencyKind::Build,
                DependencyKind::Dev,
            ],
        ) {
            if dependency.name == EVIDENCE_PACKAGE {
                problems.push(format!(
                    "{} depends on {EVIDENCE_PACKAGE}; the evidence leaf is never a dependency",
                    package.name
                ));
            }
            if dependency.name == APPLICATION_PACKAGE && package.name != EVIDENCE_PACKAGE {
                problems.push(format!(
                    "{} depends on {APPLICATION_PACKAGE}; the application is the composition root",
                    package.name
                ));
            }
        }
    }

    for rule in FORBIDDEN_CLOSURES {
        let Some(package) = default_features.member(rule.package) else {
            problems.push(format!(
                "rule target {} is not a workspace member",
                rule.package
            ));
            continue;
        };
        for (id, path) in default_features.production_closure(&package.id) {
            let Some(dependency) = default_features.package(&id) else {
                continue;
            };
            if rule
                .forbidden_prefixes
                .iter()
                .any(|prefix| dependency.name.starts_with(prefix))
            {
                problems.push(format!(
                    "{} reaches {} via {}: {}",
                    rule.package,
                    dependency.name,
                    path.join(" -> "),
                    rule.reason
                ));
            }
        }
    }

    let state = if problems.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "static.contracts.dependencies",
        state,
        format!(
            "evidence leaf isolated, application not imported, {} closure rule(s)",
            FORBIDDEN_CLOSURES.len()
        ),
    )
    .with_details(problems)
}

pub fn pins_item(root: &Path) -> Item {
    let mut problems = Vec::new();

    match fs::read_to_string(root.join("rust-toolchain.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Table>(&text).ok())
    {
        Some(table) => {
            let channel = table
                .get("toolchain")
                .and_then(|toolchain| toolchain.get("channel"))
                .and_then(toml::Value::as_str)
                .unwrap_or_default();
            let exact = channel.split('.').count() == 3
                && channel
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
            if !exact {
                problems.push(format!(
                    "rust-toolchain.toml channel `{channel}` is not an exact x.y.z release"
                ));
            }
        }
        None => problems.push("rust-toolchain.toml is missing or invalid".to_owned()),
    }

    let tracked = Command::new("git")
        .current_dir(root)
        .args(["ls-files", "--error-unmatch", "Cargo.lock"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !tracked {
        problems.push("Cargo.lock is not tracked by git".to_owned());
    }
    let ignored = Command::new("git")
        .current_dir(root)
        .args(["check-ignore", "-q", "--no-index", "Cargo.lock"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if ignored {
        problems.push("Cargo.lock matches an ignore rule".to_owned());
    }

    let alias = fs::read_to_string(root.join(".cargo/config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<toml::Table>(&text).ok())
        .and_then(|table| {
            table
                .get("alias")?
                .get("evidence")?
                .as_str()
                .map(str::to_owned)
        });
    if alias.as_deref() != Some(EVIDENCE_ALIAS) {
        problems.push(format!(
            "`.cargo/config.toml` alias `evidence` must be `{EVIDENCE_ALIAS}`, found {alias:?}"
        ));
    }

    let state = if problems.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "static.contracts.pins",
        state,
        "exact toolchain, tracked lockfile and evidence alias",
    )
    .with_details(problems)
}
