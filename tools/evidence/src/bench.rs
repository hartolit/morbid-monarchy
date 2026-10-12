//! `bench`: repeated release measurements of a frozen profile.
//!
//! In this baseline stage the measured application is the Bevy baseline, driven by the
//! `legacy-baseline` binary. Each run gets a fresh directory and an empty world store, so every
//! chunk is generated from the same seed and the user's world is never touched. A run counts
//! only if the driver completed the timeline on the requested adapter at the requested window
//! size with every frame sampled. One extra run per case enables GPU pass timestamps; it is
//! labelled diagnostic because timestamp queries and their readback add work to the frame.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    cli::BenchRequest,
    gpu::{self, AdapterRecord},
    measurement::{self, FrameSample, RunStatistics},
    paths,
    process::{Step, run_logged},
    report::{Item, State},
    scenario::{self, LoadedScenario},
};

const DRIVER_BINARY: &str = "legacy-baseline";
/// Interval between checks of a running driver.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// Interval between GPU memory samples of a running driver.
const GPU_MEMORY_INTERVAL: Duration = Duration::from_secs(2);
/// Slowest simulation rate a run may fall to before it is considered hung, in ticks per second.
const HUNG_TICKS_PER_SECOND: f64 = 2.0;
/// Frames the measurement protocol expects in a window, enough for stable 99th percentiles.
/// A slower run still counts, and is marked as below the protocol.
pub const PROTOCOL_MEASURED_FRAMES: usize = 1800;
/// Host CPU utilization below which a run may start uncontended, in percent of all cores.
const QUIET_CPU_BUSY_PERCENT: f64 = 15.0;
/// Longest wait for a quiet host before a run starts anyway and is marked contended.
const QUIET_WAIT_LIMIT: Duration = Duration::from_secs(120);
/// Extra attempts per case for runs rejected only for presentation (a paced or resized
/// window), which the desktop rather than the application causes. Every attempt is reported;
/// a crash, timeout, wrong adapter or tick error is never retried.
const PRESENTATION_RETRIES: u32 = 2;

pub fn run(request: &BenchRequest, root: &Path, run_dir: &Path) -> (Vec<Item>, Value) {
    let Some(profile) = scenario::profile(&request.profile) else {
        return failed(
            "bench.profile",
            format!("unknown profile `{}`", request.profile),
        );
    };
    let case_names: Vec<String> = if request.cases.is_empty() {
        scenario::CASES
            .iter()
            .map(|case| (*case).to_owned())
            .collect()
    } else {
        request.cases.clone()
    };
    if let Some(unknown) = case_names
        .iter()
        .find(|case| !scenario::CASES.contains(&case.as_str()))
    {
        return failed("bench.cases", format!("unknown case `{unknown}`"));
    }

    let mut items = Vec::new();
    let mut scenarios: Vec<LoadedScenario> = Vec::new();
    let scenarios_dir = root.join(paths::SCENARIOS);
    for case in &case_names {
        match scenario::load(&scenario::scenario_path(&scenarios_dir, profile.name, case)) {
            Ok(loaded) => scenarios.push(loaded),
            Err(error) => items.push(Item::new(
                format!("bench.{case}"),
                State::Failed,
                format!("not frozen: {error}"),
            )),
        }
    }
    if scenarios.is_empty() {
        return (items, Value::Null);
    }

    let records = gpu::enumerate();
    let adapter = match gpu::resolve(&records, &request.adapter) {
        Ok(adapter) => adapter.clone(),
        Err(item) => {
            items.push(item);
            return (items, Value::Null);
        }
    };
    if adapter.software {
        items.push(Item::new(
            "bench.adapter",
            State::Failed,
            format!(
                "{} is a software adapter; it cannot produce performance evidence",
                adapter.id
            ),
        ));
        return (items, Value::Null);
    }

    let build = run_logged(
        &Step::new("cargo", root).args([
            "build",
            "--locked",
            "--release",
            "-p",
            "monarchy-evidence",
            "--features",
            "legacy-baseline",
            "--bin",
            DRIVER_BINARY,
        ]),
        run_dir,
        "bench-build-driver",
    );
    if build.state != State::Passed {
        items.push(
            Item::new(
                "bench.driver",
                build.state,
                format!("driver build {}", build.reason.unwrap_or_default()),
            )
            .with_log(build.log),
        );
        return (items, Value::Null);
    }
    let driver = target_dir(root).join("release").join(DRIVER_BINARY);

    let mut cases_data = serde_json::Map::new();
    for loaded in &scenarios {
        let case = loaded.scenario.case.clone();
        let runs = request.runs.unwrap_or(loaded.scenario.run.runs);
        let mut results: Vec<RunResult> = Vec::new();
        let mut retries_left = PRESENTATION_RETRIES;
        let mut attempt = 0;
        let mut failed_slots = 0;
        for slot in 1..=runs {
            loop {
                attempt += 1;
                println!(
                    "bench {} {case}: run {slot}/{runs} (attempt {attempt})",
                    profile.name
                );
                let result = execute_run(
                    root,
                    run_dir,
                    &driver,
                    loaded,
                    &adapter,
                    &format!("run-{attempt}"),
                    false,
                );
                let retry = result.presentation_only_rejection() && retries_left > 0;
                if !result.problems.is_empty() && !retry {
                    failed_slots += 1;
                }
                results.push(result);
                if !retry {
                    break;
                }
                retries_left -= 1;
            }
        }
        println!(
            "bench {} {case}: diagnostic run with GPU timestamps",
            profile.name
        );
        let mut diagnostic =
            execute_run(root, run_dir, &driver, loaded, &adapter, "diagnostic", true);
        if diagnostic.presentation_only_rejection() {
            println!(
                "bench {} {case}: diagnostic run again after a presentation rejection",
                profile.name
            );
            diagnostic = execute_run(
                root,
                run_dir,
                &driver,
                loaded,
                &adapter,
                "diagnostic-2",
                true,
            );
        }

        let contended = results.iter().filter(|result| result.contended).count();
        let few_frames = results
            .iter()
            .filter(|result| result.below_frame_protocol)
            .count();
        let problems: Vec<String> = results
            .iter()
            .flat_map(|result| {
                result
                    .problems
                    .iter()
                    .map(move |problem| format!("{}: {problem}", result.label))
            })
            .collect();
        let valid: Vec<&RunStatistics> = results
            .iter()
            .filter_map(|result| result.statistics.as_ref())
            .collect();
        let state = if failed_slots == 0 && valid.len() == runs as usize {
            State::Passed
        } else {
            State::Failed
        };
        let summary = case_summary(&valid);
        items.push(
            Item::new(
                format!("bench.{case}"),
                state,
                format!(
                    "{}/{runs} valid runs in {} attempts ({contended} contended, {few_frames} below {PROTOCOL_MEASURED_FRAMES} frames); frame median {:.2} ms, worst p95 {:.2} ms, worst p99 {:.2} ms",
                    valid.len(),
                    results.len(),
                    summary.median_of_medians_ms,
                    summary.worst_p95_ms,
                    summary.worst_p99_ms
                ),
            )
            .with_details(problems),
        );
        let mut diagnostic_item = Item::new(
            format!("bench.{case}.gpu-timestamps"),
            if diagnostic.problems.is_empty() {
                State::Passed
            } else {
                State::Failed
            },
            if diagnostic.gpu_passes_recorded == 0 {
                "diagnostic run recorded no GPU pass timestamps (unavailable)".to_owned()
            } else {
                format!(
                    "diagnostic run recorded {} GPU passes",
                    diagnostic.gpu_passes_recorded
                )
            },
        )
        .with_details(diagnostic.problems.clone());
        diagnostic_item.required = false;
        items.push(diagnostic_item);

        cases_data.insert(
            case,
            json!({
                "scenario": loaded.path,
                "scenario_sha256": loaded.sha256,
                "summary": summary,
                "runs": results,
                "diagnostic": diagnostic,
            }),
        );
    }

    let data = json!({
        "profile": profile.name,
        "profile_spec": {
            "active_radius": profile.active_radius,
            "bodies": profile.bodies,
            "resolution": profile.resolution,
            "tick_rate_hz": profile.tick_rate_hz,
        },
        "adapter": adapter,
        "driver": driver,
        "measured_application": "Bevy baseline (morbid-app) in scripted input mode",
        "percentile_method": "nearest rank",
        "cases": cases_data,
    });
    (items, data)
}

fn failed(id: &str, summary: String) -> (Vec<Item>, Value) {
    (vec![Item::new(id, State::Failed, summary)], Value::Null)
}

fn target_dir(root: &Path) -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|dir| {
            if dir.is_absolute() {
                dir
            } else {
                root.join(dir)
            }
        })
        .unwrap_or_else(|| root.join("target"))
}

/// Cross-run summary of one case.
#[derive(Debug, Default, Serialize)]
pub struct CaseSummary {
    pub valid_runs: usize,
    pub median_of_medians_ms: f64,
    pub worst_p95_ms: f64,
    pub worst_p99_ms: f64,
    pub worst_max_ms: f64,
    pub run_medians_ms: Vec<f64>,
    pub run_p95_ms: Vec<f64>,
    pub ticks_per_second: Vec<f64>,
    pub simulation_median_ms: Vec<f64>,
    pub body_median_ms: Vec<f64>,
    pub full_copy_bytes_per_second: Vec<f64>,
    pub grid_upload_bytes_per_second: Vec<f64>,
    pub retained_bytes_max: u64,
    pub rss_bytes_max: Option<u64>,
}

fn case_summary(runs: &[&RunStatistics]) -> CaseSummary {
    let collect =
        |field: fn(&RunStatistics) -> f64| runs.iter().map(|run| field(run)).collect::<Vec<f64>>();
    let medians = collect(|run| run.frame_ms.median);
    CaseSummary {
        valid_runs: runs.len(),
        median_of_medians_ms: measurement::distribution(medians.iter().copied()).median,
        worst_p95_ms: runs.iter().map(|run| run.frame_ms.p95).fold(0.0, f64::max),
        worst_p99_ms: runs.iter().map(|run| run.frame_ms.p99).fold(0.0, f64::max),
        worst_max_ms: runs.iter().map(|run| run.frame_ms.max).fold(0.0, f64::max),
        run_medians_ms: medians,
        run_p95_ms: collect(|run| run.frame_ms.p95),
        ticks_per_second: collect(|run| run.ticks_per_second),
        simulation_median_ms: collect(|run| run.simulation_ms.median),
        body_median_ms: collect(|run| run.body_ms.median),
        full_copy_bytes_per_second: collect(|run| run.full_copy_bytes_per_second),
        grid_upload_bytes_per_second: collect(|run| run.grid_upload_bytes_per_second),
        retained_bytes_max: runs
            .iter()
            .map(|run| run.retained_bytes_max)
            .max()
            .unwrap_or(0),
        rss_bytes_max: runs.iter().filter_map(|run| run.rss_bytes_max).max(),
    }
}

/// One driver execution and its validation.
#[derive(Debug, Serialize)]
pub struct RunResult {
    pub label: String,
    pub directory: PathBuf,
    pub exit_code: Option<i32>,
    pub wall_seconds: f64,
    pub problems: Vec<String>,
    pub statistics: Option<RunStatistics>,
    pub driver_summary: Value,
    /// Largest GPU memory nvidia-smi attributed to the driver process, in MiB.
    pub gpu_memory_mib_max: Option<u64>,
    pub gpu_passes_recorded: usize,
    /// True when the window held fewer than [`PROTOCOL_MEASURED_FRAMES`] frames.
    pub below_frame_protocol: bool,
    /// How many of `problems` concern presentation (pacing, a resized window) rather than the
    /// application's behavior.
    pub presentation_problems: usize,
    /// Seconds spent waiting for the host to become quiet before starting.
    pub quiet_wait_seconds: f64,
    /// True when the host never fell below the quiet threshold; the run still counts, but its
    /// timings include other work.
    pub contended: bool,
    pub contention_before: Contention,
    pub contention_after: Contention,
}

impl RunResult {
    /// Rejected only because of the desktop's presentation, so another attempt is fair.
    pub fn presentation_only_rejection(&self) -> bool {
        !self.problems.is_empty() && self.problems.len() == self.presentation_problems
    }
}

fn execute_run(
    root: &Path,
    run_dir: &Path,
    driver: &Path,
    loaded: &LoadedScenario,
    adapter: &AdapterRecord,
    label: &str,
    gpu_timestamps: bool,
) -> RunResult {
    let directory = run_dir.join(&loaded.scenario.case).join(label);
    let mut result = RunResult {
        label: label.to_owned(),
        directory: directory.clone(),
        exit_code: None,
        wall_seconds: 0.0,
        problems: Vec::new(),
        statistics: None,
        driver_summary: Value::Null,
        gpu_memory_mib_max: None,
        gpu_passes_recorded: 0,
        below_frame_protocol: false,
        presentation_problems: 0,
        quiet_wait_seconds: 0.0,
        contended: false,
        contention_before: Contention::default(),
        contention_after: Contention::default(),
    };
    let waited = Instant::now();
    let mut busy = cpu_busy_percent(Duration::from_secs(1));
    while busy.is_some_and(|busy| busy >= QUIET_CPU_BUSY_PERCENT)
        && waited.elapsed() < QUIET_WAIT_LIMIT
    {
        busy = cpu_busy_percent(Duration::from_secs(1));
    }
    result.quiet_wait_seconds = waited.elapsed().as_secs_f64();
    result.contended = busy.is_none_or(|busy| busy >= QUIET_CPU_BUSY_PERCENT);
    result.contention_before = Contention::sample();
    if let Err(error) = fs::create_dir_all(&directory) {
        result
            .problems
            .push(format!("create {}: {error}", directory.display()));
        return result;
    }
    let samples_path = directory.join("samples.csv");
    let summary_path = directory.join("driver.json");
    let open_log = |name: &str| fs::File::create(directory.join(name));
    let (Ok(stdout), Ok(stderr)) = (open_log("driver.stdout.log"), open_log("driver.stderr.log"))
    else {
        result.problems.push("cannot create driver logs".to_owned());
        return result;
    };

    let mut command = Command::new(driver);
    command
        .current_dir(&directory)
        .arg("--scenario")
        .arg(&loaded.path)
        .arg("--world-dir")
        .arg(directory.join("world"))
        .arg("--samples")
        .arg(&samples_path)
        .arg("--summary")
        .arg(&summary_path)
        .arg("--asset-dir")
        .arg(root.join("crates/morbid-app/assets"))
        .env("WGPU_BACKEND", &adapter.backend)
        .env("WGPU_ADAPTER_NAME", &adapter.name)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr);
    if gpu_timestamps {
        command.arg("--gpu-timestamps");
    }

    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            result
                .problems
                .push(format!("could not start driver: {error}"));
            return result;
        }
    };
    let plan = &loaded.scenario.run;
    let ticks = (plan.warmup_ticks + plan.measured_ticks) as f64;
    let limit = Duration::from_secs_f64(
        plan.residency_timeout_seconds + ticks / HUNG_TICKS_PER_SECOND + 60.0,
    );
    let mut last_memory_sample = Instant::now() - GPU_MEMORY_INTERVAL;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(error) => {
                result.problems.push(format!("waiting for driver: {error}"));
                break None;
            }
        }
        if last_memory_sample.elapsed() >= GPU_MEMORY_INTERVAL {
            last_memory_sample = Instant::now();
            if let Some(mib) = gpu_memory_mib(child.id()) {
                result.gpu_memory_mib_max =
                    Some(result.gpu_memory_mib_max.map_or(mib, |max| max.max(mib)));
            }
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            result
                .problems
                .push(format!("timed out after {:.0} s", limit.as_secs_f64()));
            break None;
        }
        std::thread::sleep(POLL_INTERVAL);
    };
    result.wall_seconds = started.elapsed().as_secs_f64();
    result.exit_code = status.and_then(|status| status.code());
    result.contention_after = Contention::sample();

    validate_run(&mut result, &summary_path, &samples_path, loaded, adapter);
    // The scratch world is only the run's input and side effect; the samples are the evidence.
    let _ = fs::remove_dir_all(directory.join("world"));
    result
}

fn validate_run(
    result: &mut RunResult,
    summary_path: &Path,
    samples_path: &Path,
    loaded: &LoadedScenario,
    adapter: &AdapterRecord,
) {
    let summary: Value =
        match fs::read_to_string(summary_path).map(|text| serde_json::from_str(&text)) {
            Ok(Ok(summary)) => summary,
            _ => {
                result.problems.push(
                    "no driver summary: the window was closed or the driver failed".to_owned(),
                );
                return;
            }
        };
    if summary["status"] != "completed" {
        result
            .problems
            .push(format!("driver status {}", summary["status"]));
    }
    if summary["scenario_sha256"] != loaded.sha256.as_str() {
        result
            .problems
            .push("driver ran a different scenario file".to_owned());
    }
    let used = &summary["adapter"];
    if used["backend"] != adapter.backend.as_str()
        || used["vendor"] != adapter.vendor.as_str()
        || used["device"] != adapter.device.as_str()
    {
        result.problems.push(format!(
            "driver used adapter {used}, requested {}",
            adapter.id
        ));
    }
    if !summary["window_mismatch"].is_null() {
        result.problems.push(format!(
            "invalid presentation: {}",
            summary["window_mismatch"]
        ));
        result.presentation_problems += 1;
    }
    result.gpu_passes_recorded = summary["gpu_pass_elapsed_ms"]
        .as_object()
        .map_or(0, serde_json::Map::len);

    let samples: Result<Vec<FrameSample>, String> = fs::read_to_string(samples_path)
        .map_err(|error| error.to_string())
        .and_then(|text| {
            let mut lines = text.lines();
            if lines.next() != Some(measurement::CSV_HEADER) {
                return Err("unexpected samples header".to_owned());
            }
            lines.map(FrameSample::from_csv).collect()
        });
    match samples {
        Ok(samples) => {
            let plan = &loaded.scenario.run;
            let statistics = measurement::run_statistics(&samples);
            let window_end = plan.warmup_ticks + plan.measured_ticks;
            let final_tick = samples.last().map_or(0, |sample| sample.tick);
            // A frame is measured when it starts at or after the warm-up ticks, so the first
            // measured frame may start up to one catch-up batch after them.
            let latest_start =
                plan.warmup_ticks + u64::from(loaded.scenario.world.max_ticks_per_frame) - 1;
            match samples.iter().find(|sample| sample.measured) {
                None => result.problems.push("no measured frames".to_owned()),
                Some(first) => {
                    let first_start = first.tick - first.ticks;
                    if !(plan.warmup_ticks..=latest_start).contains(&first_start)
                        || final_tick < window_end
                        || statistics.ticks != final_tick - first_start
                    {
                        result.problems.push(format!(
                            "measured window from tick {first_start} to {final_tick} with {} ticks; expected a start in {}..={latest_start}, an end at or after {window_end}, and no missing ticks",
                            statistics.ticks, plan.warmup_ticks
                        ));
                    }
                }
            }
            result.below_frame_protocol = statistics.measured_frames < PROTOCOL_MEASURED_FRAMES;
            if let Some(pacing) = statistics.presentation_pacing {
                result.presentation_problems += 1;
                result.problems.push(format!(
                    "frames were paced at {:.2} ms, not by work ({} frames on it, {} beside, longest stretch {}); the window was throttled",
                    pacing.interval_ms,
                    pacing.frames_on_interval,
                    pacing.frames_beside_interval,
                    pacing.longest_stretch
                ));
            }
            if result.problems.is_empty() {
                result.statistics = Some(statistics);
            }
        }
        Err(error) => result.problems.push(format!("samples: {error}")),
    }
    result.driver_summary = summary;
}

/// Load on the host and the GPU around a run, so contended runs can be identified.
#[derive(Debug, Default, Serialize)]
pub struct Contention {
    /// Share of all cores busy over one second, from `/proc/stat`.
    pub cpu_busy_percent: Option<f64>,
    pub load_average: Option<String>,
    pub gpu_utilization_percent: Option<u64>,
    pub gpu_memory_used_mib: Option<u64>,
    /// Other processes nvidia-smi reports on the GPU: `pid, executable, MiB`, without arguments.
    pub gpu_processes: Vec<String>,
}

impl Contention {
    pub fn sample() -> Self {
        let gpu = nvidia_smi(&[
            "--query-gpu=utilization.gpu,memory.used",
            "--format=csv,noheader,nounits",
        ]);
        let mut fields = gpu
            .as_deref()
            .and_then(|text| text.lines().next())
            .map(|line| {
                line.split(',')
                    .map(|field| field.trim().parse::<u64>().ok())
                    .collect::<Vec<_>>()
            });
        let mut take = |index: usize| {
            fields
                .as_mut()
                .and_then(|fields| fields.get(index).copied().flatten())
        };
        Self {
            cpu_busy_percent: cpu_busy_percent(Duration::from_secs(1)),
            load_average: fs::read_to_string("/proc/loadavg")
                .ok()
                .map(|text| text.trim().to_owned()),
            gpu_utilization_percent: take(0),
            gpu_memory_used_mib: take(1),
            gpu_processes: nvidia_smi(&[
                "--query-compute-apps=pid,process_name,used_memory",
                "--format=csv,noheader,nounits",
            ])
            .map(|text| text.lines().filter_map(gpu_process_entry).collect())
            .unwrap_or_default(),
        }
    }
}

/// One `pid, process_name, used_memory` line from nvidia-smi as `pid, executable, MiB`. The name
/// is the full command line, whose arguments can carry private paths and tokens, so only the
/// executable's file name is kept.
fn gpu_process_entry(line: &str) -> Option<String> {
    let (pid, rest) = line.trim().split_once(',')?;
    let (name, mib) = rest.rsplit_once(',')?;
    let executable = name.split_whitespace().next()?;
    let executable = executable.rsplit('/').next().unwrap_or(executable);
    Some(format!("{}, {executable}, {}", pid.trim(), mib.trim()))
}

/// Aggregate CPU busy share over `window`: non-idle jiffies over all jiffies on the `cpu` line.
fn cpu_busy_percent(window: Duration) -> Option<f64> {
    let read = || -> Option<(u64, u64)> {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        let fields: Vec<u64> = stat
            .lines()
            .next()?
            .split_whitespace()
            .skip(1)
            .filter_map(|field| field.parse().ok())
            .collect();
        // user nice system idle iowait irq softirq steal; idle and iowait count as idle.
        let idle = fields.get(3)? + fields.get(4).copied().unwrap_or(0);
        Some((fields.iter().take(8).sum(), idle))
    };
    let (total_before, idle_before) = read()?;
    std::thread::sleep(window);
    let (total_after, idle_after) = read()?;
    let total = total_after.checked_sub(total_before)?;
    let idle = idle_after.checked_sub(idle_before)?;
    (total > 0).then(|| 100.0 * total.saturating_sub(idle) as f64 / total as f64)
}

fn nvidia_smi(args: &[&str]) -> Option<String> {
    let output = Command::new("nvidia-smi").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn gpu_memory_mib(pid: u32) -> Option<u64> {
    let text = nvidia_smi(&[
        "--query-compute-apps=pid,used_memory",
        "--format=csv,noheader,nounits",
    ])?;
    text.lines().find_map(|line| {
        let (process, memory) = line.split_once(',')?;
        (process.trim().parse::<u32>().ok()? == pid).then(|| memory.trim().parse::<u64>().ok())?
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_processes_keep_only_the_executable_name() {
        assert_eq!(
            gpu_process_entry(
                "228537, /opt/app/Discord --type=gpu-process --user-data-dir=/home/a/.config, 115"
            )
            .as_deref(),
            Some("228537, Discord, 115")
        );
        assert_eq!(
            gpu_process_entry(" 2963, /usr/bin/kwin_wayland, 215 ").as_deref(),
            Some("2963, kwin_wayland, 215")
        );
        assert_eq!(gpu_process_entry("no fields"), None);
    }

    #[test]
    fn case_summary_takes_worst_tails_across_runs() {
        let run = |median: f64, p95: f64| {
            let mut stats = measurement::run_statistics(&[]);
            stats.frame_ms.median = median;
            stats.frame_ms.p95 = p95;
            stats.frame_ms.p99 = p95 + 1.0;
            stats
        };
        let (a, b, c) = (run(10.0, 14.0), run(12.0, 20.0), run(11.0, 15.0));
        let summary = case_summary(&[&a, &b, &c]);
        assert_eq!(summary.median_of_medians_ms, 11.0);
        assert_eq!(summary.worst_p95_ms, 20.0);
        assert_eq!(summary.worst_p99_ms, 21.0);
    }
}
