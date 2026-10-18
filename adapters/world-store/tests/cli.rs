//! The `world-store` command line, run as a process against temporary files and the old
//! writer's fixture store.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "world-store-cli-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }

    fn path(&self, file: &str) -> PathBuf {
        self.0.join(file)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_world-store"))
        .args(args)
        .output()
        .expect("world-store runs")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn arg(path: &Path) -> &str {
    path.to_str().expect("utf-8 path")
}

#[test]
fn a_created_world_inspects_and_verifies_without_being_changed() {
    let scratch = Scratch::new("create");
    let store = scratch.path("world.redb");
    let created = run(&[
        "create",
        "--store",
        arg(&store),
        "--seed",
        "7",
        "--elevation-scale",
        "1.5",
        "--center",
        "-3,4",
        "--radius",
        "2",
    ]);
    assert!(created.status.success(), "{}", text(&created));
    assert!(text(&created).contains("seed 7, elevation scale 1.5"));
    let before = fs::read(&store).expect("bytes");
    let inspected = run(&["inspect", "--store", arg(&store)]);
    assert!(inspected.status.success(), "{}", text(&inspected));
    let report = text(&inspected);
    assert!(
        report.contains("latest checkpoint 1 at tick None"),
        "{report}"
    );
    assert!(report.contains("radius 2x2"), "{report}");
    assert!(report.contains("elevation scale 1.5"), "{report}");
    let verified = run(&["verify", "--store", arg(&store)]);
    assert!(verified.status.success(), "{}", text(&verified));
    assert_eq!(
        fs::read(&store).expect("bytes"),
        before,
        "inspection is read-only"
    );
    let again = run(&["create", "--store", arg(&store), "--seed", "7"]);
    assert_eq!(
        again.status.code(),
        Some(1),
        "an existing store is never overwritten"
    );
}

#[test]
fn a_legacy_import_needs_its_interpretation_and_reports_it() {
    let scratch = Scratch::new("import");
    let legacy = scratch.path("save.redb");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy-v0/legacy-chunks.redb"),
        &legacy,
    )
    .expect("copy fixture");
    let before = fs::read(&legacy).expect("bytes");
    let store = scratch.path("world.redb");
    let missing = run(&[
        "import-legacy",
        "--legacy",
        arg(&legacy),
        "--store",
        arg(&store),
        "--seed",
        "69",
    ]);
    assert_eq!(missing.status.code(), Some(1), "{}", text(&missing));
    assert!(
        text(&missing).contains("--elevation-scale"),
        "{}",
        text(&missing)
    );
    let refused = run(&[
        "import-legacy",
        "--legacy",
        arg(&legacy),
        "--store",
        arg(&store),
        "--seed",
        "69",
        "--elevation-scale",
        "0.5",
    ]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(text(&refused).contains("z != 0"), "{}", text(&refused));
    assert!(!store.exists());
    let imported = run(&[
        "import-legacy",
        "--legacy",
        arg(&legacy),
        "--store",
        arg(&store),
        "--seed",
        "69",
        "--elevation-scale",
        "0.5",
        "--skip-nonplanar-rows",
    ]);
    assert!(imported.status.success(), "{}", text(&imported));
    let report = text(&imported);
    assert!(
        report.contains("seed 69 and elevation scale 0.5 as supplied"),
        "{report}"
    );
    assert!(report.contains("rows imported: 4"), "{report}");
    assert_eq!(
        fs::read(&legacy).expect("bytes"),
        before,
        "the original is unchanged"
    );
    let verified = run(&["verify", "--store", arg(&store)]);
    assert!(
        text(&verified).contains("4 published chunks"),
        "{}",
        text(&verified)
    );
    let inspected = text(&run(&["inspect", "--store", arg(&store)]));
    assert!(inspected.contains("LegacyImport"), "{inspected}");
}

#[test]
fn usage_errors_are_refused_before_anything_is_opened() {
    for args in [
        vec![],
        vec!["mystery"],
        vec!["create", "--store"],
        vec!["create", "--seed", "1"],
        vec!["create", "--store", "x.redb", "--seed", "one"],
        vec![
            "create", "--store", "x.redb", "--seed", "1", "--colour", "red",
        ],
        vec!["create", "--store", "x.redb", "--seed", "1", "--seed", "2"],
        vec![
            "create", "--store", "x.redb", "--seed", "1", "--radius", "40",
        ],
        vec![
            "create",
            "--store",
            "x.redb",
            "--seed",
            "1",
            "--elevation-scale",
            "9",
        ],
        vec!["inspect"],
    ] {
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {}", text(&output));
        assert!(text(&output).contains("usage: world-store"), "{args:?}");
    }
    assert!(!Path::new("x.redb").exists(), "nothing was created");
    let missing = run(&["inspect", "--store", "/nonexistent/world.redb"]);
    assert_eq!(missing.status.code(), Some(1));
}
