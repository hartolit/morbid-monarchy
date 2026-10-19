//! Adapter inventory and GPU lanes.
//!
//! `inspect-adapter` lists every adapter on every backend with the stable identifier
//! `gpu-world` assigns; GPU commands then name that identifier explicitly. Nothing here picks
//! a "preferred" GPU. A suite runs only when its owner stage has registered real cases; the
//! others fail explicitly, so a requested GPU lane can never pass on an empty selection.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Serialize;

use crate::{
    cli::{Autosave, GpuSuite, SessionRequest},
    paths,
    process::{Step, run_logged},
    report::{Item, State},
    suites::{self, CaseResult, scenarios::ScenarioResult, session::SessionResult},
    tests_cpu::Registry,
};

/// The binary that runs one suite case per process.
const SUITE_BINARY: &str = "gpu-suite";
/// The longest one case may run before it is killed and failed.
const CASE_TIME_LIMIT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize)]
pub struct AdapterRecord {
    /// `<backend>-<vendor>-<device>` in lowercase hex, with `-<n>` when several adapters share
    /// it. Pass this to `--adapter`.
    pub id: String,
    pub name: String,
    pub backend: String,
    pub device_type: String,
    /// True for CPU/software rasterizers, which can add correctness coverage but never qualify
    /// the hardware performance target.
    pub software: bool,
    pub vendor: String,
    pub device: String,
    pub pci_bus: String,
    pub driver: String,
    pub driver_info: String,
    pub features: Vec<String>,
    /// `Limits` field names and values exactly as wgpu reports them.
    pub limits: Vec<(String, String)>,
}

/// Enumerates adapters on all compiled backends, through the GPU owner's inventory.
pub fn enumerate() -> Vec<AdapterRecord> {
    gpu_world::context::AdapterInventory::enumerate()
        .records()
        .map(|record| AdapterRecord {
            id: record.id.clone(),
            name: record.name.clone(),
            backend: record.backend.clone(),
            device_type: record.device_type.clone(),
            software: record.software,
            vendor: format!("0x{:04x}", record.vendor),
            device: format!("0x{:04x}", record.device),
            pci_bus: record.pci_bus.clone(),
            driver: record.driver.clone(),
            driver_info: record.driver_info.clone(),
            features: record.features.clone(),
            limits: record.limits.clone(),
        })
        .collect()
}

pub fn inspect_items(records: &[AdapterRecord]) -> Vec<Item> {
    if records.is_empty() {
        return vec![Item::new(
            "gpu.adapters",
            State::Unavailable,
            "no adapter on any backend; GPU lanes cannot run on this host",
        )];
    }
    let details = records
        .iter()
        .map(|record| {
            format!(
                "{}: {} ({}, {}{}), driver {} {}",
                record.id,
                record.name,
                record.backend,
                record.device_type,
                if record.software { ", software" } else { "" },
                record.driver,
                record.driver_info
            )
        })
        .collect();
    vec![
        Item::new(
            "gpu.adapters",
            State::Passed,
            format!("{} adapter(s) enumerated", records.len()),
        )
        .with_details(details),
    ]
}

/// Resolves `--adapter`. A missing adapter is unavailable, not a pass.
pub fn resolve<'a>(records: &'a [AdapterRecord], id: &str) -> Result<&'a AdapterRecord, Item> {
    records
        .iter()
        .find(|record| record.id == id)
        .ok_or_else(|| {
            let known: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
            Item::new(
                "gpu.adapter",
                State::Unavailable,
                format!("adapter `{id}` not found; run `cargo evidence inspect-adapter`"),
            )
            .with_details(known)
        })
}

/// Where a lane builds and finds its helper binaries.
pub struct LaneContext<'a> {
    pub root: &'a Path,
    pub run_dir: &'a Path,
}

/// Executes a GPU suite (or all of them) on `adapter`.
pub fn suite_items(
    suite: GpuSuite,
    adapter: &AdapterRecord,
    lane: &LaneContext<'_>,
) -> (Vec<Item>, serde_json::Value) {
    let requested: Vec<GpuSuite> = match suite {
        GpuSuite::All => GpuSuite::NAMED.to_vec(),
        single => vec![single],
    };
    let mut items = Vec::new();
    let mut data = serde_json::Map::new();
    if adapter.software {
        items.push(
            Item::new(
                "gpu.adapter.software",
                State::Passed,
                format!("{} is a software adapter: correctness coverage only, never hardware qualification", adapter.id),
            )
            .optional(),
        );
    }
    for suite in requested {
        match suites::cases(suite) {
            Some(cases) => {
                let (suite_items, results) = run_suite(suite, cases, adapter, lane);
                items.extend(suite_items);
                data.insert(
                    suite.name().to_owned(),
                    serde_json::to_value(results).unwrap_or_default(),
                );
            }
            None => items.push(Item::new(
                format!("gpu.suite.{}", suite.name()),
                State::Failed,
                format!(
                    "suite `{}` is not implemented; prompt {} registers it (adapter {})",
                    suite.name(),
                    suite.owner_stage(),
                    adapter.id
                ),
            )),
        }
    }
    (items, serde_json::Value::Object(data))
}

/// The registry's behaviors for `suite`: entries whose target is `gpu:<suite>`.
pub fn registered_cases(registry: &Registry, suite: GpuSuite) -> BTreeSet<String> {
    let target = format!("gpu:{}", suite.name());
    registry
        .behaviors
        .iter()
        .filter(|behavior| behavior.target == target)
        .map(|behavior| behavior.test.clone())
        .collect()
}

/// Differences between the cases a suite defines and the ones the registry expects.
pub fn registry_problems(defined: &BTreeSet<String>, registered: &BTreeSet<String>) -> Vec<String> {
    let mut problems = Vec::new();
    for missing in registered.difference(defined) {
        problems.push(format!(
            "registered case `{missing}` is not defined by the suite"
        ));
    }
    for unregistered in defined.difference(registered) {
        problems.push(format!(
            "case `{unregistered}` has no coverage registry entry"
        ));
    }
    if registered.is_empty() {
        problems.push("the registry expects no case: an empty suite cannot pass".to_owned());
    }
    problems
}

fn suite_binary(root: &Path) -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|dir| {
            if dir.is_absolute() {
                dir
            } else {
                root.join(dir)
            }
        })
        .unwrap_or_else(|| root.join("target"));
    target.join("release").join(SUITE_BINARY)
}

fn run_suite(
    suite: GpuSuite,
    cases: &[suites::Case],
    adapter: &AdapterRecord,
    lane: &LaneContext<'_>,
) -> (Vec<Item>, Vec<CaseResult>) {
    let name = suite.name();
    let mut items = Vec::new();
    let registry = match Registry::load(&lane.root.join(paths::COVERAGE_REGISTRY)) {
        Ok(registry) => registry,
        Err(error) => {
            return (
                vec![Item::new(
                    format!("gpu.{name}.registry"),
                    State::Failed,
                    error,
                )],
                Vec::new(),
            );
        }
    };
    let defined: BTreeSet<String> = cases.iter().map(|case| case.name.to_owned()).collect();
    let problems = registry_problems(&defined, &registered_cases(&registry, suite));
    items.push(
        Item::new(
            format!("gpu.{name}.registry"),
            if problems.is_empty() {
                State::Passed
            } else {
                State::Failed
            },
            format!("{} cases defined and registered", defined.len()),
        )
        .with_details(problems),
    );

    let build = run_logged(
        &Step::new("cargo", lane.root).args([
            "build",
            "--locked",
            "--release",
            "-p",
            "monarchy-evidence",
            "--bin",
            SUITE_BINARY,
        ]),
        lane.run_dir,
        &format!("gpu-{name}-build"),
    );
    if build.state != State::Passed {
        items.push(
            Item::new(
                format!("gpu.{name}.build"),
                build.state,
                format!("suite build {}", build.reason.unwrap_or_default()),
            )
            .with_log(build.log),
        );
        return (items, Vec::new());
    }
    let binary = suite_binary(lane.root);
    let results_dir = lane.run_dir.join(format!("gpu-{name}"));
    if let Err(error) = std::fs::create_dir_all(&results_dir) {
        items.push(Item::new(
            format!("gpu.{name}.results"),
            State::Failed,
            error.to_string(),
        ));
        return (items, Vec::new());
    }
    let mut results = Vec::new();
    for case in cases {
        let result_path = results_dir.join(format!("{}.json", case.name));
        let _ = std::fs::remove_file(&result_path);
        let mut step = Step::new(binary.as_os_str(), lane.root)
            .args([
                "--suite",
                name,
                "--adapter",
                &adapter.id,
                "--case",
                case.name,
                "--result",
            ])
            .arg(result_path.as_os_str())
            .timeout(CASE_TIME_LIMIT);
        if adapter.software {
            step = step.arg("--allow-software");
        }
        let outcome = run_logged(&step, lane.run_dir, &format!("gpu-{name}-{}", case.name));
        let id = format!("gpu.{name}.{}", case.name);
        let parsed = std::fs::read_to_string(&result_path)
            .ok()
            .and_then(|text| serde_json::from_str::<CaseResult>(&text).ok());
        let item = match parsed {
            Some(result) => {
                let state = if result.passed && outcome.state == State::Passed {
                    State::Passed
                } else {
                    State::Failed
                };
                let mut details: Vec<String> = result
                    .failures
                    .iter()
                    .map(|failure| format!("FAILED {failure}"))
                    .collect();
                details.extend(result.notes.iter().map(|note| format!("note: {note}")));
                let item = Item::new(id, state, case.summary)
                    .with_details(details)
                    .with_log(outcome.log);
                results.push(result);
                item
            }
            None => Item::new(
                id,
                State::Failed,
                format!(
                    "{}: no result ({})",
                    case.summary,
                    outcome
                        .reason
                        .unwrap_or_else(|| "exited without writing one".to_owned())
                ),
            )
            .with_log(outcome.log),
        };
        items.push(item);
    }
    (items, results)
}

/// Builds the release `gpu-suite` binary that scenarios and sessions run in; a failure is the
/// `<id>.build` item.
fn build_suite_binary(lane: &LaneContext<'_>, id: &str, label: &str) -> Result<(), Item> {
    if !lane.root.join("Cargo.toml").is_file() {
        return Err(Item::new(
            format!("{id}.build"),
            State::Failed,
            format!("{} is not the workspace", lane.root.display()),
        ));
    }
    let build = run_logged(
        &Step::new("cargo", lane.root).args([
            "build",
            "--locked",
            "--release",
            "-p",
            "monarchy-evidence",
            "--bin",
            SUITE_BINARY,
        ]),
        lane.run_dir,
        &format!("{label}-build"),
    );
    if build.state != State::Passed {
        return Err(Item::new(
            format!("{id}.build"),
            build.state,
            format!("{label} build {}", build.reason.unwrap_or_default()),
        )
        .with_log(build.log));
    }
    Ok(())
}

/// A headless session over the store the request names, in its own `gpu-suite` process: one
/// item each for opening, running and closing. Relative store paths are resolved against the
/// invoking directory, not the workspace.
pub fn session_items(
    request: &SessionRequest,
    adapter: &AdapterRecord,
    lane: &LaneContext<'_>,
) -> Vec<Item> {
    let store = if request.store.is_absolute() {
        request.store.clone()
    } else {
        match std::env::current_dir() {
            Ok(dir) => dir.join(&request.store),
            Err(error) => {
                return vec![Item::new(
                    "session.open",
                    State::Failed,
                    format!("cannot resolve {}: {error}", request.store.display()),
                )];
            }
        }
    };
    if let Err(item) = build_suite_binary(lane, "session", "session") {
        return vec![item];
    }
    let result_path = lane.run_dir.join("session.json");
    let _ = std::fs::remove_file(&result_path);
    let mut step = Step::new(suite_binary(lane.root).as_os_str(), lane.root)
        .args(["--session", "--store"])
        .arg(store.as_os_str())
        .args([
            "--adapter",
            &adapter.id,
            "--ticks",
            &request.ticks.to_string(),
        ])
        .timeout(Duration::from_secs(300 + u64::from(request.ticks) / 20));
    if let Some(((x, y), radius)) = request.window {
        step = step.args([
            "--center".to_owned(),
            format!("{x},{y}"),
            "--radius".to_owned(),
            radius.to_string(),
        ]);
    }
    for edit in &request.edits {
        step = step.args([
            "--edit".to_owned(),
            format!("{},{},{}", edit.name(), edit.cell.0, edit.cell.1),
        ]);
    }
    match request.autosave {
        Autosave::Default => {}
        Autosave::Off => step = step.args(["--autosave", "off"]),
        Autosave::Every(ticks) => step = step.args(["--autosave".to_owned(), ticks.to_string()]),
    }
    step = step.arg("--result").arg(result_path.as_os_str());
    if adapter.software {
        step = step.arg("--allow-software");
    }
    let outcome = run_logged(&step, lane.run_dir, "session");
    let parsed = std::fs::read_to_string(&result_path)
        .ok()
        .and_then(|text| serde_json::from_str::<SessionResult>(&text).ok());
    let Some(result) = parsed else {
        return vec![
            Item::new(
                "session",
                State::Failed,
                format!(
                    "no result ({})",
                    outcome
                        .reason
                        .unwrap_or_else(|| "exited without writing one".to_owned())
                ),
            )
            .with_log(outcome.log),
        ];
    };
    let mut items: Vec<Item> = result
        .steps
        .iter()
        .map(|step| {
            let mut details: Vec<String> = step
                .failures
                .iter()
                .map(|failure| format!("FAILED {failure}"))
                .collect();
            details.extend(step.notes.iter().map(|note| format!("note: {note}")));
            details.extend(
                step.metrics
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}")),
            );
            Item::new(
                format!("session.{}", step.name),
                if step.passed {
                    State::Passed
                } else {
                    State::Failed
                },
                step.summary.clone(),
            )
            .with_details(details)
            .with_log(outcome.log.clone())
        })
        .collect();
    for name in ["open", "run", "close"] {
        if !result.steps.iter().any(|step| step.name == name) {
            items.push(Item::new(
                format!("session.{name}"),
                State::Failed,
                "not reached: an earlier step stopped the session",
            ));
        }
    }
    items
}

/// The longest a connected scenario may run before it is killed and failed.
const SCENARIO_TIME_LIMIT: Duration = Duration::from_secs(300);

/// Connected scenarios run through the production runtime, in their own `gpu-suite` process.
/// Each step is one item: a step its owner stage has not delivered fails, so the scenario fails
/// until every step exists.
pub fn scenario_items(
    scenario: &str,
    adapter: &AdapterRecord,
    lane: &LaneContext<'_>,
) -> Vec<Item> {
    let id = format!("scenario.{scenario}");
    if scenario != "resident-world" {
        return vec![Item::new(
            id,
            State::Failed,
            format!("unknown scenario `{scenario}`; registered: resident-world"),
        )];
    }
    if let Err(item) = build_suite_binary(lane, &id, scenario) {
        return vec![item];
    }
    let result_path = lane.run_dir.join(format!("{scenario}.json"));
    let _ = std::fs::remove_file(&result_path);
    let mut step = Step::new(suite_binary(lane.root).as_os_str(), lane.root)
        .args(["--scenario", scenario, "--adapter", &adapter.id, "--result"])
        .arg(result_path.as_os_str())
        .timeout(SCENARIO_TIME_LIMIT);
    if adapter.software {
        step = step.arg("--allow-software");
    }
    let outcome = run_logged(&step, lane.run_dir, scenario);
    let parsed = std::fs::read_to_string(&result_path)
        .ok()
        .and_then(|text| serde_json::from_str::<ScenarioResult>(&text).ok());
    let Some(result) = parsed else {
        return vec![
            Item::new(
                id,
                State::Failed,
                format!(
                    "no result ({})",
                    outcome
                        .reason
                        .unwrap_or_else(|| "exited without writing one".to_owned())
                ),
            )
            .with_log(outcome.log),
        ];
    };
    result
        .steps
        .iter()
        .map(|step| {
            let mut details: Vec<String> = step
                .failures
                .iter()
                .map(|failure| format!("FAILED {failure}"))
                .collect();
            details.extend(step.notes.iter().map(|note| format!("note: {note}")));
            details.extend(
                step.metrics
                    .iter()
                    .map(|(key, value)| format!("{key}: {value}")),
            );
            Item::new(
                format!("{id}.{}", step.name),
                if step.passed {
                    State::Passed
                } else {
                    State::Failed
                },
                format!("({}) {}", step.owner, step.summary),
            )
            .with_details(details)
            .with_log(outcome.log.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(id: &str) -> AdapterRecord {
        AdapterRecord {
            id: id.to_owned(),
            name: String::new(),
            backend: String::new(),
            device_type: String::new(),
            software: false,
            vendor: String::new(),
            device: String::new(),
            pci_bus: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            features: Vec::new(),
            limits: Vec::new(),
        }
    }

    #[test]
    fn unimplemented_suites_and_scenarios_fail_and_missing_adapters_are_unavailable() {
        let adapter = named("vulkan-10de-2c05");
        let dir =
            std::env::temp_dir().join(format!("monarchy-evidence-gpu-{}", std::process::id()));
        let lane = LaneContext {
            root: &dir,
            run_dir: &dir,
        };
        for suite in [
            GpuSuite::Cells,
            GpuSuite::Bodies,
            GpuSuite::Streaming,
            GpuSuite::Render,
        ] {
            let (items, _) = suite_items(suite, &adapter, &lane);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].state, State::Failed, "{}", items[0].id);
        }
        assert_eq!(
            scenario_items("resident-world", &adapter, &lane)[0].state,
            State::Failed
        );
        let missing = resolve(&[adapter], "gl-0000-0000").expect_err("unknown adapter");
        assert_eq!(missing.state, State::Unavailable);
    }

    #[test]
    fn a_suite_must_match_its_registry_and_cannot_be_empty() {
        let set = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<BTreeSet<String>>()
        };
        assert!(registry_problems(&set(&["a", "b"]), &set(&["a", "b"])).is_empty());
        assert_eq!(
            registry_problems(&set(&["a"]), &set(&["a", "b"])).len(),
            1,
            "a registered case vanished"
        );
        assert_eq!(
            registry_problems(&set(&["a", "b"]), &set(&["a"])).len(),
            1,
            "an unregistered case"
        );
        assert!(
            !registry_problems(&set(&[]), &set(&[])).is_empty(),
            "an empty suite"
        );
    }

    #[test]
    fn every_implemented_suite_case_is_registered() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let registry = Registry::load(&root.join("coverage.toml")).expect("registry");
        for (suite, cases) in [
            (GpuSuite::Transfers, suites::transfers::CASES),
            (GpuSuite::Cells, suites::cells::CASES),
            (GpuSuite::Bodies, suites::bodies::CASES),
        ] {
            let defined: BTreeSet<String> = cases.iter().map(|case| case.name.to_owned()).collect();
            assert_eq!(
                registry_problems(&defined, &registered_cases(&registry, suite)),
                Vec::<String>::new(),
                "{}",
                suite.name()
            );
        }
    }
}
