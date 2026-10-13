//! Runs one case of a GPU suite in its own process, so a device loss, crash or hang affects
//! only that case. `cargo evidence test gpu` builds and drives this binary; it is not meant to
//! be run by hand, though it can be:
//!
//! ```text
//! gpu-suite --suite transfers --list
//! gpu-suite --suite transfers --adapter vulkan-10de-2c05 --case empty-batches --result out.json
//!           [--allow-software]
//! ```
//!
//! The process exits 0 only when the case passed; its result, with failures, notes and
//! metrics, is written as JSON to `--result`.

use std::process::ExitCode;

use monarchy_evidence::{
    cli::GpuSuite,
    suites::{self, alloc::CountingAllocator},
};

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn main() -> ExitCode {
    suites::alloc::mark_installed();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let Some(suite) = value("--suite").as_deref().and_then(GpuSuite::parse) else {
        eprintln!("gpu-suite: --suite NAME is required");
        return ExitCode::from(2);
    };
    let Some(cases) = suites::cases(suite) else {
        eprintln!("gpu-suite: suite `{}` has no cases", suite.name());
        return ExitCode::from(2);
    };
    if args.iter().any(|arg| arg == "--list") {
        for case in cases {
            println!("{}\t{}", case.name, case.summary);
        }
        return ExitCode::SUCCESS;
    }
    let (Some(adapter), Some(name), Some(result_path)) =
        (value("--adapter"), value("--case"), value("--result"))
    else {
        eprintln!("gpu-suite: --adapter, --case and --result are required");
        return ExitCode::from(2);
    };
    let Some(case) = cases.iter().find(|case| case.name == name) else {
        eprintln!("gpu-suite: unknown case `{name}`");
        return ExitCode::from(2);
    };
    suites::set_allow_software(args.iter().any(|arg| arg == "--allow-software"));
    let result = suites::run_case(suite, case, &adapter);
    for failure in &result.failures {
        println!("FAILED: {failure}");
    }
    for note in &result.notes {
        println!("note: {note}");
    }
    let json = serde_json::to_string_pretty(&result).unwrap_or_default();
    if let Err(error) = std::fs::write(&result_path, json) {
        eprintln!("gpu-suite: write {result_path}: {error}");
        return ExitCode::from(1);
    }
    if result.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
