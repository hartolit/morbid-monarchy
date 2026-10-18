//! Morbid Monarchy's development evidence runner, invoked as `cargo evidence`.
//!
//! Each command executes one evidence lane and writes `report.json` into its run directory
//! (`target/evidence/<time>-<command>/` unless `--output` is given), alongside the logs of every
//! nested command. The process exits with status 0 only when every required item passed; see
//! [`report::overall`]. The command contracts are documented in `docs/development/workflow.md`.

pub mod bench;
pub mod checks;
pub mod cli;
pub mod gpu;
pub mod identity;
pub mod measurement;
pub mod process;
pub mod report;
pub mod scenario;
pub mod suites;
pub mod tests_cpu;
pub mod workspace;

use std::{
    fs,
    path::{Path, PathBuf},
};

use cli::{Command, Invocation};
use report::{Item, Report, State};

/// Exit status for a request that does not match the command grammar.
pub const EXIT_USAGE: i32 = 2;

/// Files the runner reads, relative to the repository root.
pub mod paths {
    pub const COVERAGE_REGISTRY: &str = "tools/evidence/coverage.toml";
    pub const LINT_BASELINE: &str = "tools/evidence/lint-baseline.toml";
    pub const SCENARIOS: &str = "tools/evidence/scenarios";
}

/// Finds the workspace root: the nearest ancestor of the current directory whose manifest
/// declares `[workspace]` and contains this runner, falling back to the runner's build location.
pub fn find_root() -> PathBuf {
    let compiled = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let Ok(current) = std::env::current_dir() else {
        return compiled;
    };
    current
        .ancestors()
        .find(|dir| {
            dir.join("tools/evidence/Cargo.toml").is_file()
                && fs::read_to_string(dir.join("Cargo.toml"))
                    .is_ok_and(|text| text.lines().any(|line| line.trim() == "[workspace]"))
        })
        .map(Path::to_path_buf)
        .unwrap_or(compiled)
}

/// Runs one invocation and returns the process exit status.
pub fn execute(invocation: &Invocation, raw_args: &[String], root: &Path) -> i32 {
    if invocation.command == Command::Help {
        print!("{}", cli::USAGE);
        return 0;
    }

    let started = identity::now_unix_seconds();
    let run_dir = match &invocation.output {
        Some(dir) if dir.is_absolute() => dir.clone(),
        Some(dir) => root.join(dir),
        None => root.join("target/evidence").join(format!(
            "{}-{}",
            identity::compact_utc(started),
            command_slug(&invocation.command)
        )),
    };
    if let Err(error) = fs::create_dir_all(&run_dir) {
        eprintln!("cannot create run directory {}: {error}", run_dir.display());
        return 1;
    }

    // Identity describes the source the lane ran against, so it is taken before the work. A
    // source change during a long run would make the results unattributable.
    let identity = identity::Identity::collect(root);
    let (mut items, data) = run_command(&invocation.command, root, &run_dir);
    let after = identity::Identity::collect(root).source;
    if after != identity.source {
        items.push(Item::new(
            "identity.stable",
            State::Failed,
            "the source changed while the command ran; its results describe no single revision",
        ));
    }
    let result = report::overall(&items);
    let finished = identity::now_unix_seconds();
    let report = Report {
        schema_version: report::REPORT_SCHEMA_VERSION,
        command: raw_args.to_vec(),
        result,
        started_at: identity::rfc3339_utc(started),
        finished_at: identity::rfc3339_utc(finished),
        duration_seconds: finished - started,
        identity,
        items,
        data,
    };
    let report_path = match report.write(&run_dir) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    report::print_summary(&report.items, result, &report_path);
    if result == State::Passed { 0 } else { 1 }
}

fn command_slug(command: &Command) -> String {
    match command {
        Command::Help => "help".to_owned(),
        Command::CheckDocs => "check-docs".to_owned(),
        Command::CheckStatic => "check-static".to_owned(),
        Command::CheckAll { .. } => "check-all".to_owned(),
        Command::TestCpu => "test-cpu".to_owned(),
        Command::TestGpu { suite, .. } => format!("test-gpu-{}", suite.name()),
        Command::InspectAdapter => "inspect-adapter".to_owned(),
        Command::Run { scenario, .. } => format!("run-{scenario}"),
        Command::Bench(request) => format!("bench-{}", request.profile),
    }
}

fn run_command(command: &Command, root: &Path, run_dir: &Path) -> (Vec<Item>, serde_json::Value) {
    let registry = root.join(paths::COVERAGE_REGISTRY);
    let lint_baseline = root.join(paths::LINT_BASELINE);
    match command {
        Command::Help => (Vec::new(), serde_json::Value::Null),
        Command::CheckDocs => (docs_items(root), serde_json::Value::Null),
        Command::CheckStatic => (
            checks::static_lane::run(root, run_dir, &lint_baseline),
            serde_json::Value::Null,
        ),
        Command::TestCpu => (
            tests_cpu::run(root, run_dir, &registry),
            serde_json::Value::Null,
        ),
        Command::InspectAdapter => {
            let records = gpu::enumerate();
            let data = serde_json::to_value(&records).unwrap_or_default();
            (
                gpu::inspect_items(&records),
                serde_json::json!({ "adapters": data }),
            )
        }
        Command::TestGpu { suite, adapter } => {
            let records = gpu::enumerate();
            match gpu::resolve(&records, adapter) {
                Ok(record) => {
                    let lane = gpu::LaneContext { root, run_dir };
                    let (items, results) = gpu::suite_items(*suite, record, &lane);
                    (
                        items,
                        serde_json::json!({ "adapter": record, "suites": results }),
                    )
                }
                Err(item) => (vec![item], serde_json::Value::Null),
            }
        }
        Command::Run { scenario, adapter } => {
            let records = gpu::enumerate();
            match gpu::resolve(&records, adapter) {
                Ok(record) => {
                    let lane = gpu::LaneContext { root, run_dir };
                    (
                        gpu::scenario_items(scenario, record, &lane),
                        adapter_data(record),
                    )
                }
                Err(item) => (vec![item], serde_json::Value::Null),
            }
        }
        Command::CheckAll { adapter } => {
            let mut items = docs_items(root);
            items.extend(checks::static_lane::run(root, run_dir, &lint_baseline));
            items.extend(tests_cpu::run(root, run_dir, &registry));
            let records = gpu::enumerate();
            match gpu::resolve(&records, adapter) {
                Ok(record) => {
                    let lane = gpu::LaneContext { root, run_dir };
                    let (suite_items, results) =
                        gpu::suite_items(cli::GpuSuite::All, record, &lane);
                    items.extend(suite_items);
                    items.extend(gpu::scenario_items("resident-world", record, &lane));
                    (
                        items,
                        serde_json::json!({ "adapter": record, "suites": results }),
                    )
                }
                Err(item) => {
                    items.push(item);
                    (items, serde_json::Value::Null)
                }
            }
        }
        Command::Bench(request) => bench::run(request, root, run_dir),
    }
}

fn docs_items(root: &Path) -> Vec<Item> {
    let workspace = workspace::Workspace::load(root, true);
    let mut items = checks::docs::run(root, workspace.as_ref().ok());
    if let Err(error) = workspace {
        items.push(Item::new("docs.workspace", State::Failed, error));
    }
    items
}

fn adapter_data(record: &gpu::AdapterRecord) -> serde_json::Value {
    serde_json::json!({ "adapter": record })
}
