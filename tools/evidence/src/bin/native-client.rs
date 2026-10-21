//! Runs the native client in its real window under a script, for evidence. Not a person's way to
//! start the client (that is `morbid`); the evidence runner drives it.
//!
//! ```text
//! native-client capture --worlds DIR --world NAME --adapter ID --settings PATH --out DIR
//!               [--pose X,Y,Z,YAW_DEG,PITCH_DEG] [--frames N,N...] [--window WxH]
//! native-client script features|loss|worlds ...
//! native-client bench --scenario FILE --worlds NEW_DIR --samples FILE --summary FILE
//!               --adapter ID --settings PATH [--gpu-timestamps] [--autosave TICKS]
//! ```
//!
//! The process counts its heap allocations (all threads, and per thread), so a run's summary can
//! report them; the count costs two relaxed atomic additions and a thread-local one each.
//! `--autosave` is a diagnostic: the frozen scenarios run with autosave off, as `cargo evidence
//! bench` starts them.

use std::{path::PathBuf, process::ExitCode};

use monarchy_evidence::{
    native::{CaptureDriver, Pose},
    suites::alloc::{self, CountingAllocator},
};
use morbid_app::options::{Autosave, ClientOptions};
use world_model::glam::DVec3;

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn main() -> ExitCode {
    alloc::mark_installed();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("script") {
        return script(&args);
    }
    if args.first().map(String::as_str) == Some("bench") {
        return bench(&args);
    }
    if args.first().map(String::as_str) != Some("capture") {
        eprintln!(
            "native-client: usage: native-client capture --worlds DIR --world NAME --adapter ID --settings PATH --out DIR"
        );
        return ExitCode::from(2);
    }
    let (Some(worlds), Some(world), Some(adapter), Some(settings), Some(out)) = (
        value(&args, "--worlds"),
        value(&args, "--world"),
        value(&args, "--adapter"),
        value(&args, "--settings"),
        value(&args, "--out"),
    ) else {
        eprintln!("native-client: --worlds, --world, --adapter, --settings and --out are required");
        return ExitCode::from(2);
    };
    let pose = value(&args, "--pose").and_then(|text| {
        let parts: Vec<f64> = text
            .split(',')
            .filter_map(|part| part.trim().parse().ok())
            .collect();
        (parts.len() == 5).then(|| Pose {
            position: DVec3::new(parts[0], parts[1], parts[2]),
            yaw: (parts[3] as f32).to_radians(),
            pitch: (parts[4] as f32).to_radians(),
        })
    });
    let frames: Vec<u64> = value(&args, "--frames")
        .map(|text| {
            text.split(',')
                .filter_map(|part| part.trim().parse().ok())
                .collect()
        })
        .unwrap_or_else(|| vec![30]);
    let window_size = value(&args, "--window").and_then(|text| {
        let (w, h) = text.split_once('x')?;
        Some([w.parse().ok()?, h.parse().ok()?])
    });
    let out = PathBuf::from(out);
    let captures = frames
        .iter()
        .map(|frame| (*frame, out.join(format!("frame-{frame}.png"))))
        .collect();
    let worlds = PathBuf::from(worlds);
    let path = worlds.join(format!("{world}.world"));
    let options = ClientOptions {
        adapter: Some(adapter),
        worlds,
        settings: PathBuf::from(settings),
        open: None,
        window_size,
        vsync: true,
        autosave: Autosave::Off,
    };
    let driver = Box::new(CaptureDriver::new(world, path, pose, captures));
    match morbid_app::client::run(options, Some(driver)) {
        Ok(summary) => {
            println!("frames {}", summary.frames);
            for close in &summary.closes {
                println!("{close}");
            }
            for error in &summary.errors {
                eprintln!("native-client: {error}");
            }
            if summary.errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("native-client: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `native-client script NAME --worlds DIR --world NAME --adapter ID --settings PATH --out DIR
/// [--window WxH]`: runs a named script and writes `report.json` to `--out`.
fn script(args: &[String]) -> ExitCode {
    use monarchy_evidence::native::{ScriptDriver, features};
    let name = args.get(1).cloned().unwrap_or_default();
    if name == "worlds" {
        let size = value(args, "--window")
            .and_then(|text| {
                let (w, h) = text.split_once('x')?;
                Some([w.parse().ok()?, h.parse().ok()?])
            })
            .unwrap_or([1600, 900]);
        return worlds_script(args, size);
    }
    let (Some(worlds), Some(world), Some(adapter), Some(settings), Some(out)) = (
        value(args, "--worlds"),
        value(args, "--world"),
        value(args, "--adapter"),
        value(args, "--settings"),
        value(args, "--out"),
    ) else {
        eprintln!("native-client: --worlds, --world, --adapter, --settings and --out are required");
        return ExitCode::from(2);
    };
    let size = value(args, "--window")
        .and_then(|text| {
            let (w, h) = text.split_once('x')?;
            Some([w.parse().ok()?, h.parse().ok()?])
        })
        .unwrap_or([1600, 900]);
    let cues = match name.as_str() {
        "features" => features(size),
        "loss" => monarchy_evidence::native::loss_and_recovery(size),
        other => {
            eprintln!("native-client: unknown script `{other}`");
            return ExitCode::from(2);
        }
    };
    let out = PathBuf::from(out);
    let worlds = PathBuf::from(worlds);
    let path = worlds.join(format!("{world}.world"));
    let mut driver = ScriptDriver::new(&name, world, path, out.clone(), cues)
        .starting_at(monarchy_evidence::native::SHORE_POSE);
    if name == "loss" {
        driver = driver.with_faults();
    }
    let report = std::rc::Rc::clone(&driver.report);
    let options = ClientOptions {
        adapter: Some(adapter),
        worlds,
        settings: PathBuf::from(settings),
        open: None,
        window_size: Some(size),
        vsync: true,
        autosave: Autosave::Off,
    };
    let summary = morbid_app::client::run(options, Some(Box::new(driver)));
    let mut report = report.borrow().clone();
    match summary {
        Ok(summary) => {
            report.failures.extend(summary.errors.iter().cloned());
            for close in &summary.closes {
                println!("{close}");
            }
        }
        Err(error) => report.failures.push(error),
    }
    let _ = std::fs::create_dir_all(&out);
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    let _ = std::fs::write(out.join("report.json"), text);
    for check in &report.checks {
        println!(
            "{} {:<48} {}",
            if check.passed { "pass" } else { "FAIL" },
            check.name,
            check.detail
        );
    }
    if name == "loss" {
        let reported = report
            .messages
            .iter()
            .find(|message| message.contains("device was lost"))
            .cloned();
        println!(
            "{} the loss is reported to the person: {reported:?}",
            if reported.is_some() { "pass" } else { "FAIL" },
        );
        if reported.is_none() {
            report
                .failures
                .push("the loss was not reported on screen".to_owned());
        }
        let text = serde_json::to_string_pretty(&report).unwrap_or_default();
        let _ = std::fs::write(out.join("report.json"), text);
    }
    for failure in &report.failures {
        println!("FAIL {failure}");
    }
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `native-client script worlds --legacy FILE --adapter ID --out DIR [--worlds DIR]
/// [--settings PATH]`: the start screen's create, import, refusal of a damaged save, open,
/// close and reopen through the client's handlers. Without `--worlds` and `--settings` the
/// client's own defaults apply (the user data and configuration directories), as for a person.
fn worlds_script(args: &[String], size: [u32; 2]) -> ExitCode {
    use monarchy_evidence::native::WorldsDriver;
    let (Some(legacy), Some(adapter), Some(out)) = (
        value(args, "--legacy"),
        value(args, "--adapter"),
        value(args, "--out"),
    ) else {
        eprintln!("native-client: --legacy, --adapter and --out are required");
        return ExitCode::from(2);
    };
    let out = PathBuf::from(out);
    let _ = std::fs::create_dir_all(&out);
    let damaged = out.join("damaged-legacy.redb");
    if let Err(error) = WorldsDriver::damage(std::path::Path::new(&legacy), &damaged) {
        eprintln!("native-client: the damaged copy: {error}");
        return ExitCode::FAILURE;
    }
    let driver = WorldsDriver::new(PathBuf::from(legacy), damaged);
    let report = std::rc::Rc::clone(&driver.report);
    let options = ClientOptions {
        adapter: Some(adapter),
        worlds: value(args, "--worlds")
            .map_or_else(morbid_app::options::default_worlds_dir, PathBuf::from),
        settings: value(args, "--settings")
            .map_or_else(morbid_app::options::default_settings_path, PathBuf::from),
        open: None,
        window_size: Some(size),
        vsync: true,
        autosave: Autosave::Off,
    };
    let summary = morbid_app::client::run(options, Some(Box::new(driver)));
    let mut report = report.borrow().clone();
    match summary {
        Ok(summary) => report.failures.extend(summary.errors),
        Err(error) => report.failures.push(error),
    }
    if report.checks.len() != 5 {
        report
            .failures
            .push(format!("{} of 5 steps ran", report.checks.len()));
    }
    let _ = std::fs::write(
        out.join("report.json"),
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );
    for check in &report.checks {
        println!(
            "{} {:<48} {}",
            if check.passed { "pass" } else { "FAIL" },
            check.name,
            check.detail
        );
    }
    for failure in &report.failures {
        println!("FAIL {failure}");
    }
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// `native-client bench`: one measured run of a frozen scenario (see
/// [`monarchy_evidence::native::bench`]). Succeeds when the timeline completed.
fn bench(args: &[String]) -> ExitCode {
    use monarchy_evidence::native::bench::{BenchDriver, create_world};
    let (Some(scenario), Some(worlds), Some(samples), Some(summary), Some(adapter), Some(settings)) = (
        value(args, "--scenario"),
        value(args, "--worlds"),
        value(args, "--samples"),
        value(args, "--summary"),
        value(args, "--adapter"),
        value(args, "--settings"),
    ) else {
        eprintln!(
            "native-client: --scenario, --worlds, --samples, --summary, --adapter and --settings are required"
        );
        return ExitCode::from(2);
    };
    let loaded = match monarchy_evidence::scenario::load(std::path::Path::new(&scenario)) {
        Ok(loaded) => loaded,
        Err(error) => {
            eprintln!("native-client: {error}");
            return ExitCode::from(2);
        }
    };
    // Every run starts from a new world, so every chunk is generated identically.
    let worlds = PathBuf::from(worlds);
    if worlds.exists() {
        eprintln!(
            "native-client: {} already exists; each run needs a new worlds directory",
            worlds.display()
        );
        return ExitCode::from(2);
    }
    if let Err(error) = create_world(&worlds, &loaded.scenario.case, &loaded.scenario) {
        eprintln!("native-client: {error}");
        return ExitCode::FAILURE;
    }
    let options = ClientOptions {
        adapter: Some(adapter.clone()),
        worlds: worlds.clone(),
        settings: PathBuf::from(settings),
        open: None,
        window_size: Some(loaded.scenario.presentation.resolution),
        vsync: BenchDriver::vsync(&loaded.scenario),
        autosave: match value(args, "--autosave").map(|ticks| ticks.parse::<u32>()) {
            None => Autosave::Off,
            Some(Ok(ticks)) if ticks > 0 => Autosave::Every(ticks),
            Some(_) => {
                eprintln!("native-client: --autosave takes a positive number of ticks");
                return ExitCode::from(2);
            }
        },
    };
    let summary_path = PathBuf::from(summary);
    let driver = BenchDriver::new(
        loaded,
        worlds,
        PathBuf::from(samples),
        summary_path.clone(),
        args.iter().any(|arg| arg == "--gpu-timestamps"),
        adapter,
    );
    let outcome = morbid_app::client::run(options, Some(Box::new(driver)));
    match &outcome {
        Ok(run) => {
            for error in &run.errors {
                eprintln!("native-client: {error}");
            }
        }
        Err(error) => eprintln!("native-client: {error}"),
    }
    // The summary exists once the timeline finished or timed out; closing the window early
    // leaves none, which the bench reports as a failed run.
    let completed = std::fs::read_to_string(&summary_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|summary| summary["status"] == "completed");
    if completed && outcome.is_ok_and(|run| run.errors.is_empty()) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
