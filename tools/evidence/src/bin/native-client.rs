//! Runs the native client in its real window under a script, for evidence. Not a person's way to
//! start the client (that is `morbid`); the evidence runner drives it.
//!
//! ```text
//! native-client capture --worlds DIR --world NAME --adapter ID --settings PATH --out DIR
//!               [--pose X,Y,Z,YAW_DEG,PITCH_DEG] [--frames N,N...] [--window WxH]
//! ```

use std::{path::PathBuf, process::ExitCode};

use monarchy_evidence::native::{CaptureDriver, Pose};
use morbid_app::options::{Autosave, ClientOptions};
use world_model::glam::DVec3;

fn value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("script") {
        return script(&args);
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
        other => {
            eprintln!("native-client: unknown script `{other}`");
            return ExitCode::from(2);
        }
    };
    let out = PathBuf::from(out);
    let worlds = PathBuf::from(worlds);
    let path = worlds.join(format!("{world}.world"));
    let driver = ScriptDriver::new(&name, world, path, out.clone(), cues)
        .starting_at(monarchy_evidence::native::SHORE_POSE);
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
    for failure in &report.failures {
        println!("FAIL {failure}");
    }
    if report.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
