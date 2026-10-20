//! `test cpu`: run every workspace test executable and compare results with the coverage
//! registry.
//!
//! Test executables are built with `cargo test --no-run --message-format=json`, which names
//! each executable's package and target exactly, then run directly. Every test in a covered
//! package must have a registry entry with a stable behavior ID; every entry must be executed
//! and pass. The original baseline tests are counted so a deletion cannot hide inside a rename.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use serde::Deserialize;
use serde_json::Value;

use crate::{
    checks::contracts::EVIDENCE_PACKAGE,
    process::{Step, run_logged},
    report::{Item, State},
};

/// The number of `#[test]` functions in the baseline source (source audit, prompt 00).
pub const BASELINE_TEST_COUNT: usize = 28;

/// Registry targets of GPU suite cases (`gpu:<suite>`), which the GPU lane checks.
pub const GPU_TARGET_PREFIX: &str = "gpu:";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    #[serde(rename = "behavior")]
    pub behaviors: Vec<Behavior>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Behavior {
    /// Stable coverage identity; it survives renames and moves of the test.
    pub id: String,
    pub package: String,
    /// `lib`, `bin:<name>` or `test:<name>`.
    pub target: String,
    /// The libtest path within the target.
    pub test: String,
    pub origin: Origin,
    pub expectation: String,
    pub destination: String,
    pub determinism: Determinism,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// One of the tests present before the transition.
    Baseline,
    /// Added during the transition.
    Transition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Determinism {
    /// Integer or otherwise exactly reproducible result.
    Exact,
    /// Floating body motion checked against physical tolerances.
    FloatTolerance,
    /// Depends on thread-seeded randomness; the assertion is probabilistic.
    StochasticThreadRng,
}

impl Registry {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        let registry: Self =
            toml::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))?;
        registry.validate()?;
        Ok(registry)
    }

    fn validate(&self) -> Result<(), String> {
        let mut ids = BTreeSet::new();
        let mut tests = BTreeSet::new();
        for behavior in &self.behaviors {
            if behavior.expectation.trim().is_empty() || behavior.destination.trim().is_empty() {
                return Err(format!(
                    "{} needs an expectation and a destination",
                    behavior.id
                ));
            }
            if !ids.insert(behavior.id.as_str()) {
                return Err(format!("duplicate behavior id {}", behavior.id));
            }
            if !tests.insert((&behavior.package, &behavior.target, &behavior.test)) {
                return Err(format!(
                    "{} registers a test that is already registered",
                    behavior.id
                ));
            }
        }
        Ok(())
    }
}

/// A test executable produced by `cargo test --no-run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestExecutable {
    pub package: String,
    pub package_dir: PathBuf,
    pub target: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Passed,
    Failed,
    Ignored,
}

/// Parses `compiler-artifact` messages into test executables.
pub fn parse_executables(output: &str) -> Vec<TestExecutable> {
    let mut executables = Vec::new();
    for line in output.lines() {
        let Ok(json) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if json["reason"] != "compiler-artifact" || json["profile"]["test"] != true {
            continue;
        }
        let Some(path) = json["executable"].as_str() else {
            continue;
        };
        let kind = json["target"]["kind"][0].as_str().unwrap_or_default();
        let name = json["target"]["name"].as_str().unwrap_or_default();
        let target = match kind {
            "lib" | "rlib" | "cdylib" | "staticlib" | "proc-macro" => "lib".to_owned(),
            other => format!("{other}:{name}"),
        };
        let manifest = json["manifest_path"].as_str().unwrap_or_default();
        executables.push(TestExecutable {
            package: package_name(&json),
            package_dir: Path::new(manifest)
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            target,
            path: PathBuf::from(path),
        });
    }
    executables
}

/// Package name from a package id such as `path+file:///w/crates/a#name@0.1.0` or
/// `path+file:///w/crates/name#0.1.0`.
fn package_name(json: &Value) -> String {
    let id = json["package_id"].as_str().unwrap_or_default();
    let fragment = id.rsplit_once('#').map_or(id, |(_, fragment)| fragment);
    match fragment.split_once('@') {
        Some((name, _)) => name.to_owned(),
        None => id
            .rsplit_once('#')
            .and_then(|(path, _)| path.rsplit('/').next())
            .unwrap_or(fragment)
            .to_owned(),
    }
}

/// Parses libtest's human output. Returns results by test path and the sum of the
/// `running N tests` headers, so a truncated parse can be detected.
pub fn parse_libtest(output: &str) -> (BTreeMap<String, Outcome>, usize) {
    let mut results = BTreeMap::new();
    let mut announced = 0;
    for line in output.lines() {
        if let Some(count) = line
            .strip_prefix("running ")
            .and_then(|rest| {
                rest.strip_suffix(" tests")
                    .or_else(|| rest.strip_suffix(" test"))
            })
            .and_then(|count| count.parse::<usize>().ok())
        {
            announced += count;
            continue;
        }
        let Some(rest) = line.strip_prefix("test ") else {
            continue;
        };
        let Some((name, verdict)) = rest.rsplit_once(" ... ") else {
            continue;
        };
        let outcome = match verdict.trim() {
            "ok" => Outcome::Passed,
            "FAILED" => Outcome::Failed,
            verdict if verdict.starts_with("ignored") => Outcome::Ignored,
            _ => continue,
        };
        results.insert(name.trim().to_owned(), outcome);
    }
    (results, announced)
}

/// Runs the lane. `run_dir` receives build and per-executable logs.
pub fn run(root: &Path, run_dir: &Path, registry_path: &Path) -> Vec<Item> {
    let registry = match Registry::load(registry_path) {
        Ok(registry) => registry,
        Err(error) => return vec![Item::new("cpu.registry", State::Failed, error)],
    };

    let build = Step::new("cargo", root).args([
        "test",
        "--workspace",
        "--all-features",
        "--locked",
        "--no-run",
        "--message-format=json-render-diagnostics",
    ]);
    let built = run_logged(&build, run_dir, "cpu-build");
    if built.state != State::Passed {
        return vec![
            Item::new(
                "cpu.build",
                built.state,
                format!("test build {}", built.reason.unwrap_or_default()),
            )
            .with_log(built.log),
        ];
    }
    let executables = parse_executables(&built.stdout);
    let mut items = vec![
        Item::new(
            "cpu.build",
            State::Passed,
            format!("{} test executables", executables.len()),
        )
        .with_log(built.log),
    ];

    let mut executed: BTreeMap<(String, String, String), Outcome> = BTreeMap::new();
    let mut execution_problems = Vec::new();
    for (index, executable) in executables.iter().enumerate() {
        let step = Step::new(executable.path.as_os_str(), &executable.package_dir)
            .env("CARGO_MANIFEST_DIR", executable.package_dir.as_os_str())
            .env("CARGO_PKG_NAME", executable.package.as_str());
        let log_name = format!(
            "cpu-{index:02}-{}-{}",
            executable.package,
            executable.target.replace(':', "-")
        );
        let outcome = run_logged(&step, run_dir, &log_name);
        let (results, announced) = parse_libtest(&outcome.stdout);
        if results.len() != announced {
            execution_problems.push(format!(
                "{} {}: announced {announced} tests, parsed {} results ({})",
                executable.package,
                executable.target,
                results.len(),
                outcome.log.clone().unwrap_or_default()
            ));
        }
        if outcome.state != State::Passed
            && !results.values().any(|result| *result == Outcome::Failed)
        {
            execution_problems.push(format!(
                "{} {} {} without a failing test line ({})",
                executable.package,
                executable.target,
                outcome.reason.unwrap_or_default(),
                outcome.log.unwrap_or_default()
            ));
        }
        for (name, result) in results {
            executed.insert(
                (executable.package.clone(), executable.target.clone(), name),
                result,
            );
        }
    }

    items.push(results_item(&executed, &execution_problems));
    items.push(registry_item(&registry, &executed));
    items
}

fn results_item(
    executed: &BTreeMap<(String, String, String), Outcome>,
    execution_problems: &[String],
) -> Item {
    let mut details: Vec<String> = execution_problems.to_vec();
    let mut counts = [0usize; 3];
    for ((package, target, name), outcome) in executed {
        match outcome {
            Outcome::Passed => counts[0] += 1,
            Outcome::Failed => {
                counts[1] += 1;
                details.push(format!("FAILED {package} {target} {name}"));
            }
            Outcome::Ignored => {
                counts[2] += 1;
                details.push(format!("ignored {package} {target} {name}"));
            }
        }
    }
    let state = if executed.is_empty()
        || counts[1] > 0
        || counts[2] > 0
        || !execution_problems.is_empty()
    {
        State::Failed
    } else {
        State::Passed
    };
    Item::new(
        "cpu.tests",
        state,
        format!(
            "{} passed, {} failed, {} ignored (ignored tests are not accepted)",
            counts[0], counts[1], counts[2]
        ),
    )
    .with_details(details)
}

/// Every test in a covered package is registered, every registered test ran and passed, and
/// all baseline behaviors are still present.
pub fn registry_item(
    registry: &Registry,
    executed: &BTreeMap<(String, String, String), Outcome>,
) -> Item {
    let mut problems = Vec::new();
    // `gpu:<suite>` behaviors are GPU suite cases; `test gpu` checks them, not this lane.
    let registered: BTreeMap<(&str, &str, &str), &Behavior> = registry
        .behaviors
        .iter()
        .filter(|behavior| !behavior.target.starts_with(GPU_TARGET_PREFIX))
        .map(|behavior| {
            (
                (
                    behavior.package.as_str(),
                    behavior.target.as_str(),
                    behavior.test.as_str(),
                ),
                behavior,
            )
        })
        .collect();

    let mut tooling_tests = 0;
    for (package, target, name) in executed.keys() {
        if package == EVIDENCE_PACKAGE {
            tooling_tests += 1;
            continue;
        }
        if !registered.contains_key(&(package.as_str(), target.as_str(), name.as_str())) {
            problems.push(format!("unregistered test {package} {target} {name}"));
        }
    }
    for ((package, target, name), behavior) in &registered {
        match executed.get(&(
            (*package).to_owned(),
            (*target).to_owned(),
            (*name).to_owned(),
        )) {
            None => problems.push(format!(
                "{} ({package} {target} {name}) was not executed",
                behavior.id
            )),
            Some(Outcome::Passed) => {}
            Some(other) => problems.push(format!("{} did not pass: {other:?}", behavior.id)),
        }
    }
    let baseline = registry
        .behaviors
        .iter()
        .filter(|behavior| behavior.origin == Origin::Baseline)
        .count();
    if baseline != BASELINE_TEST_COUNT {
        problems.push(format!(
            "{baseline} baseline behaviors registered; the baseline has {BASELINE_TEST_COUNT}"
        ));
    }
    if tooling_tests == 0 {
        problems.push(format!(
            "no {EVIDENCE_PACKAGE} tests ran; the runner's own checks are required"
        ));
    }

    let state = if problems.is_empty() {
        State::Passed
    } else {
        State::Failed
    };
    Item::new(
        "cpu.registry",
        state,
        format!(
            "{} registered CPU behaviors ({baseline} baseline), {tooling_tests} runner tests",
            registered.len()
        ),
    )
    .with_details(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBTEST: &str = "\nrunning 3 tests\ntest a::one ... ok\ntest a::two ... FAILED\ntest a::three ... ignored, slow\n\nfailures:\n\ntest result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out\n";

    #[test]
    fn libtest_output_is_parsed_and_counted() {
        let (results, announced) = parse_libtest(LIBTEST);
        assert_eq!(announced, 3);
        assert_eq!(results["a::one"], Outcome::Passed);
        assert_eq!(results["a::two"], Outcome::Failed);
        assert_eq!(results["a::three"], Outcome::Ignored);
    }

    #[test]
    fn artifacts_name_their_package_and_target() {
        let line = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "path+file:///w/crates/world-model#0.1.0",
            "manifest_path": "/w/crates/world-model/Cargo.toml",
            "target": {"kind": ["lib"], "name": "world_model"},
            "profile": {"test": true},
            "executable": "/w/target/debug/deps/world_model-1"
        });
        let named = serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": "path+file:///w/tools/evidence#monarchy-evidence@0.1.0",
            "manifest_path": "/w/tools/evidence/Cargo.toml",
            "target": {"kind": ["test"], "name": "runner"},
            "profile": {"test": true},
            "executable": "/w/target/debug/deps/runner-2"
        });
        let not_a_test = serde_json::json!({"reason": "compiler-artifact", "profile": {"test": false}, "executable": "/x"});
        let output = format!("{line}\n{named}\n{not_a_test}\n");
        let executables = parse_executables(&output);
        assert_eq!(executables.len(), 2);
        assert_eq!(
            (
                executables[0].package.as_str(),
                executables[0].target.as_str()
            ),
            ("world-model", "lib")
        );
        assert_eq!(
            (
                executables[1].package.as_str(),
                executables[1].target.as_str()
            ),
            ("monarchy-evidence", "test:runner")
        );
    }

    fn behavior(id: &str, test: &str, origin: Origin) -> Behavior {
        Behavior {
            id: id.to_owned(),
            package: "engine".to_owned(),
            target: "lib".to_owned(),
            test: test.to_owned(),
            origin,
            expectation: "e".to_owned(),
            destination: "d".to_owned(),
            determinism: Determinism::Exact,
        }
    }

    fn key(package: &str, name: &str) -> (String, String, String) {
        (package.to_owned(), "lib".to_owned(), name.to_owned())
    }

    #[test]
    fn registry_rejects_missing_unregistered_and_failed_tests() {
        let registry = Registry {
            behaviors: (0..BASELINE_TEST_COUNT)
                .map(|n| behavior(&format!("b{n}"), &format!("t{n}"), Origin::Baseline))
                .collect(),
        };
        let mut executed: BTreeMap<_, _> = (0..BASELINE_TEST_COUNT)
            .map(|n| (key("engine", &format!("t{n}")), Outcome::Passed))
            .collect();
        executed.insert(key(EVIDENCE_PACKAGE, "runner::works"), Outcome::Passed);
        assert_eq!(registry_item(&registry, &executed).state, State::Passed);

        let mut missing = executed.clone();
        missing.remove(&key("engine", "t3"));
        assert_eq!(registry_item(&registry, &missing).state, State::Failed);

        let mut extra = executed.clone();
        extra.insert(key("engine", "new_test"), Outcome::Passed);
        assert_eq!(registry_item(&registry, &extra).state, State::Failed);

        let mut failed = executed.clone();
        failed.insert(key("engine", "t0"), Outcome::Failed);
        assert_eq!(registry_item(&registry, &failed).state, State::Failed);

        let mut no_runner = executed.clone();
        no_runner.remove(&key(EVIDENCE_PACKAGE, "runner::works"));
        assert_eq!(registry_item(&registry, &no_runner).state, State::Failed);

        let shrunk = Registry {
            behaviors: registry.behaviors.into_iter().skip(1).collect(),
        };
        let mut fewer = executed;
        fewer.remove(&key("engine", "t0"));
        assert_eq!(
            registry_item(&shrunk, &fewer).state,
            State::Failed,
            "a removed baseline behavior is caught"
        );
    }

    #[test]
    fn empty_execution_fails() {
        assert_eq!(results_item(&BTreeMap::new(), &[]).state, State::Failed);
    }
}
