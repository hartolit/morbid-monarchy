//! Repository contracts that source shape can establish: dependency direction, isolation of the
//! evidence leaf, and the toolchain/lock pins. They cannot prove physical correctness; those
//! claims are executed through the production owners.

use std::{fs, path::Path, process::Command};

use crate::{
    checks::static_lane::RequiredTool,
    report::{Item, State},
    workspace::{DependencyKind, Workspace},
};

/// The hosted CI workflow, which runs the same lanes through the runner.
pub const CI_WORKFLOW: &str = ".github/workflows/evidence.yml";

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
    /// Exact package names a forbidden prefix would otherwise match.
    allowed: &'static [&'static str],
    reason: &'static str,
}

/// The standalone ECS and the crates it brings with `default-features = false, features =
/// ["std"]` (`cargo tree -p world-runtime`). Every other `bevy*` package is the engine.
const STANDALONE_ECS: &[&str] = &[
    "bevy_ecs",
    "bevy_ecs_macros",
    "bevy_ecs_macro_logic",
    "bevy_macro_utils",
    "bevy_platform",
    "bevy_ptr",
    "bevy_tasks",
    "bevy_utils",
];

/// Dependency-direction rules in force. Later stages add their packages here when they create
/// them, so a rule never passes vacuously for a package that does not exist.
const FORBIDDEN_CLOSURES: &[ForbiddenClosure] = &[
    ForbiddenClosure {
        package: EVIDENCE_PACKAGE,
        forbidden_prefixes: &["bevy"],
        allowed: STANDALONE_ECS,
        reason: "the evidence runner drives and measures the native client; it links no Bevy engine crate",
    },
    ForbiddenClosure {
        package: APPLICATION_PACKAGE,
        forbidden_prefixes: &["bevy", "rayon", EVIDENCE_PACKAGE],
        allowed: STANDALONE_ECS,
        reason: "the native client's production closure has no Bevy engine, render, app, window, input, time or task crate and no CPU simulation; the standalone ECS arrives only through world-runtime",
    },
    ForbiddenClosure {
        package: "world-render",
        forbidden_prefixes: &[
            "bevy",
            "winit",
            "egui",
            "redb",
            "bitcode",
            "rayon",
            "flume",
            "morbid-app",
            "world-store",
            EVIDENCE_PACKAGE,
        ],
        allowed: STANDALONE_ECS,
        reason: "the renderer draws the GPU owner's view: no window, UI, store, engine or baseline; the ECS arrives only through world-runtime",
    },
    ForbiddenClosure {
        package: "world-model",
        forbidden_prefixes: &[
            "bevy",
            "wgpu",
            "naga",
            "winit",
            "egui",
            "redb",
            "bitcode",
            "rayon",
            "flume",
            "morbid-app",
            "world-runtime",
            "gpu-world",
            "world-store",
            EVIDENCE_PACKAGE,
        ],
        allowed: &[],
        reason: "the world model is framework-free: no engine, GPU API, window, UI, store, codec, worker pool or other workspace package",
    },
    ForbiddenClosure {
        package: "world-runtime",
        forbidden_prefixes: &[
            "bevy",
            "wgpu",
            "naga",
            "winit",
            "egui",
            "redb",
            "bitcode",
            "rayon",
            "flume",
            "morbid-app",
            "gpu-world",
            "world-store",
            EVIDENCE_PACKAGE,
        ],
        allowed: STANDALONE_ECS,
        reason: "the runtime's ports name no GPU, window, UI or store type; its only framework is the standalone ECS",
    },
    ForbiddenClosure {
        package: "gpu-world",
        forbidden_prefixes: &[
            "bevy",
            "winit",
            "egui",
            "redb",
            "bitcode",
            "rayon",
            "flume",
            "morbid-app",
            "world-store",
            EVIDENCE_PACKAGE,
        ],
        allowed: STANDALONE_ECS,
        reason: "the GPU owner has no window, UI, store, engine or baseline dependency; the ECS arrives only through world-runtime",
    },
    ForbiddenClosure {
        package: "world-store",
        forbidden_prefixes: &[
            "bevy",
            "wgpu",
            "naga",
            "winit",
            "egui",
            "rayon",
            "flume",
            "morbid-app",
            "gpu-world",
            EVIDENCE_PACKAGE,
        ],
        allowed: STANDALONE_ECS,
        reason: "the store owns encoding and redb transactions only: no GPU, window, UI, engine, baseline or worker framework; the ECS arrives only through world-runtime",
    },
];

/// `package_graph` returns the graph one package resolves when built alone with its default
/// features ([`Workspace::load_package`]). Workspace-wide metadata would unify features with
/// every other member, so one member's features could appear in another's closure.
pub fn dependency_item(
    all_features: &Workspace,
    package_graph: &dyn Fn(&str) -> Result<Workspace, String>,
) -> Item {
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

    problems.extend(closure_violations(package_graph, FORBIDDEN_CLOSURES));

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

/// CI runs the CPU and static lanes through this runner and installs exactly the checker
/// versions the static lane requires, so the two cannot drift apart.
pub fn ci_item(root: &Path, tools: &[RequiredTool]) -> Item {
    let problems = match fs::read_to_string(root.join(CI_WORKFLOW)) {
        Ok(text) => ci_problems(&text, tools),
        Err(error) => vec![format!("{CI_WORKFLOW}: {error}")],
    };
    let state = if problems.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "static.contracts.ci",
        state,
        format!("{CI_WORKFLOW} runs the runner's lanes with the pinned checkers"),
    )
    .with_details(problems)
}

fn ci_problems(workflow: &str, tools: &[RequiredTool]) -> Vec<String> {
    let mut problems = Vec::new();
    for lane in [
        "cargo evidence check docs",
        "cargo evidence check static",
        "cargo evidence test cpu",
    ] {
        if !workflow.contains(lane) {
            problems.push(format!("{CI_WORKFLOW} does not run `{lane}`"));
        }
    }
    for tool in tools {
        let pin = format!("{}@{}", tool.package, tool.version);
        if !workflow.contains(&pin) {
            problems.push(format!("{CI_WORKFLOW} does not install {pin}"));
        }
    }
    problems
}

fn closure_violations(
    package_graph: &dyn Fn(&str) -> Result<Workspace, String>,
    rules: &[ForbiddenClosure],
) -> Vec<String> {
    let mut problems = Vec::new();
    for rule in rules {
        let workspace = match package_graph(rule.package) {
            Ok(workspace) => workspace,
            Err(error) => {
                problems.push(format!("{}: {error}", rule.package));
                continue;
            }
        };
        let Some(package) = workspace.member(rule.package) else {
            problems.push(format!(
                "rule target {} is not a workspace member",
                rule.package
            ));
            continue;
        };
        for (id, path) in workspace.production_closure(&package.id) {
            let Some(dependency) = workspace.package(&id) else {
                continue;
            };
            if rule
                .forbidden_prefixes
                .iter()
                .any(|prefix| dependency.name.starts_with(prefix))
                && !rule.allowed.contains(&dependency.name.as_str())
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
    problems
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A model that depends on `glam`, and through `extra` on whatever the case adds.
    fn workspace(extra: Option<&str>) -> Workspace {
        let mut model_deps =
            vec![serde_json::json!({"pkg": "glam", "dep_kinds": [{"kind": null}]})];
        let mut packages = vec![
            serde_json::json!({"id": "world-model", "name": "world-model", "manifest_path": "/w/m/Cargo.toml"}),
            serde_json::json!({"id": "glam", "name": "glam", "manifest_path": "/r/glam/Cargo.toml"}),
        ];
        let mut nodes = vec![serde_json::json!({"id": "glam", "deps": []})];
        if let Some(name) = extra {
            model_deps.push(serde_json::json!({"pkg": "helper", "dep_kinds": [{"kind": null}]}));
            packages.push(serde_json::json!({"id": "helper", "name": "helper", "manifest_path": "/r/h/Cargo.toml"}));
            packages.push(
                serde_json::json!({"id": name, "name": name, "manifest_path": "/r/x/Cargo.toml"}),
            );
            nodes.push(serde_json::json!({"id": "helper", "deps": [{"pkg": name, "dep_kinds": [{"kind": null}]}]}));
            nodes.push(serde_json::json!({"id": name, "deps": []}));
        }
        nodes.push(serde_json::json!({"id": "world-model", "deps": model_deps}));
        Workspace::from_metadata(&serde_json::json!({
            "workspace_members": ["world-model"],
            "packages": packages,
            "resolve": {"nodes": nodes}
        }))
        .expect("metadata")
    }

    fn rule(package: &str) -> &'static [ForbiddenClosure] {
        let index = FORBIDDEN_CLOSURES
            .iter()
            .position(|rule| rule.package == package)
            .expect("a rule for the package");
        &FORBIDDEN_CLOSURES[index..index + 1]
    }

    /// The violations of `package`'s rule when it depends directly on one package `name`.
    fn direct(package: &'static str, name: &str) -> Vec<String> {
        let workspace = Workspace::from_metadata(&serde_json::json!({
            "workspace_members": [package],
            "packages": [
                {"id": package, "name": package, "manifest_path": "/w/r/Cargo.toml"},
                {"id": name, "name": name, "manifest_path": "/r/x/Cargo.toml"}
            ],
            "resolve": {"nodes": [
                {"id": package, "deps": [{"pkg": name, "dep_kinds": [{"kind": null}]}]},
                {"id": name, "deps": []}
            ]}
        }))
        .expect("metadata");
        closure_violations(&|_| Ok(workspace.clone()), rule(package))
    }

    #[test]
    fn the_standalone_ecs_is_allowed_where_listed_and_the_engine_never() {
        assert!(direct("world-runtime", "bevy_ecs").is_empty());
        assert!(direct("world-runtime", "bevy_tasks").is_empty());
        assert_eq!(direct("world-runtime", "bevy_render").len(), 1);
        assert_eq!(direct("world-runtime", "bevy").len(), 1);
        assert_eq!(direct("world-runtime", "wgpu-core").len(), 1);
    }

    #[test]
    fn the_native_client_closure_has_no_bevy_engine_crate() {
        for engine in [
            "bevy",
            "bevy_render",
            "bevy_winit",
            "bevy_window",
            "bevy_input",
            "bevy_app",
            "bevy_time",
            "bevy_egui",
            "rayon",
        ] {
            assert_eq!(direct(APPLICATION_PACKAGE, engine).len(), 1, "{engine}");
        }
        for allowed in ["bevy_ecs", "bevy_tasks", "winit", "egui", "wgpu"] {
            assert!(direct(APPLICATION_PACKAGE, allowed).is_empty(), "{allowed}");
        }
        assert_eq!(direct("world-render", "winit").len(), 1);
        assert_eq!(direct("world-render", "egui").len(), 1);
    }

    #[test]
    fn the_model_may_not_reach_an_engine_or_store_even_indirectly() {
        let model_rule = rule("world-model");
        assert_eq!(model_rule[0].package, "world-model");
        assert!(closure_violations(&|_| Ok(workspace(None)), model_rule).is_empty());
        for forbidden in ["bevy_ecs", "wgpu-core", "redb", "rayon"] {
            let problems = closure_violations(&|_| Ok(workspace(Some(forbidden))), model_rule);
            assert_eq!(problems.len(), 1, "{forbidden}: {problems:?}");
            assert!(
                problems[0].contains("helper"),
                "the path is reported: {problems:?}"
            );
        }
    }

    #[test]
    fn ci_must_run_the_lanes_with_the_pinned_checkers() {
        let tools = crate::checks::static_lane::REQUIRED_TOOLS;
        let good = "run: cargo install --locked cargo-deny@0.20.2 cargo-machete@0.9.2\n\
                    run: cargo evidence check docs\nrun: cargo evidence check static\n\
                    run: cargo evidence test cpu\n";
        assert_eq!(ci_problems(good, tools), Vec::<String>::new());
        let drifted = good.replace("cargo-deny@0.20.2", "cargo-deny@0.19.0");
        assert_eq!(ci_problems(&drifted, tools).len(), 1);
        let weaker = good.replace("run: cargo evidence test cpu\n", "run: cargo test\n");
        assert_eq!(ci_problems(&weaker, tools).len(), 1);
    }

    #[test]
    fn package_graphs_come_from_one_package_s_own_resolution() {
        let tree = "0world-runtime v0.1.0 (/w/crates/world-runtime)\n1bevy_ecs v0.19.0\n2bevy_ptr v0.19.0\n1world-model v0.1.0 (/w/crates/world-model)\n2glam v0.32.1\n2bevy_ptr v0.19.0 (*)\n";
        let workspace = Workspace::from_tree(tree).expect("tree");
        let rule = rule("world-runtime");
        assert!(closure_violations(&|_| Ok(workspace.clone()), rule).is_empty());
        let engine = "0world-runtime v0.1.0 (/w)\n1bevy_ecs v0.19.0\n2bevy_reflect v0.19.0\n";
        let workspace = Workspace::from_tree(engine).expect("tree");
        let problems = closure_violations(&|_| Ok(workspace.clone()), rule);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("bevy_ecs -> bevy_reflect"),
            "{problems:?}"
        );
    }

    #[test]
    fn a_rule_for_a_missing_package_fails_instead_of_passing_vacuously() {
        let problems = closure_violations(&|_| Ok(workspace(None)), &FORBIDDEN_CLOSURES[..1]);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("not a workspace member"));
    }
}
