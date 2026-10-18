//! `world-store`: create, import, inspect and verify saved worlds, without a GPU.
//!
//! ```text
//! world-store create --store PATH --seed N [--world ID] [--rule-seed N] [--elevation-scale S]
//!                    [--center X,Y] [--radius R]
//! world-store import-legacy --legacy PATH --store PATH --seed N --elevation-scale S
//!                    [--world ID] [--rule-seed N] [--center X,Y] [--radius R]
//!                    [--skip-nonplanar-rows] [--rows-per-transaction N]
//! world-store inspect --store PATH
//! world-store verify --store PATH
//! ```
//!
//! Every command names its store explicitly; nothing is opened relative to the working
//! directory. `create` and `import-legacy` write a new file and refuse an existing one; `inspect`
//! and `verify` open read-only and never change the file. A legacy store records no seed or
//! elevation scale, so `import-legacy` requires both and records them as its interpretation.
//! Unknown options are usage errors. The exit status is 0 on success, 1 on a store or import
//! failure, and 2 on a usage error.

use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use world_model::{
    command::{AmbientTemperature, ElevationScale},
    config::DEFAULT_AMBIENT_TEMPERATURE_C,
    identity,
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::{
    ids::{SessionEpoch, WorldId},
    port::ResidentWindow,
    store::{
        CheckpointId, Generator, PhysicalConfig, Publication, WorldHeader, WorldOrigin, WorldStore,
    },
};
use world_store::{
    faults::StoreFaults,
    legacy::{self, ImportOptions, NonPlanarRows},
    store::{Access, OpenOptionsStore, RedbWorldStore},
};

const USAGE: &str = "\
usage: world-store <command> [options]

commands:
  create --store PATH --seed N        a new world whose first checkpoint has run no tick
         [--world ID] [--rule-seed N] [--elevation-scale S] [--center X,Y] [--radius R]
  import-legacy --legacy PATH --store PATH --seed N --elevation-scale S
         [--world ID] [--rule-seed N] [--center X,Y] [--radius R]
         [--skip-nonplanar-rows] [--rows-per-transaction N]
                                      a new world from a legacy save.redb, which is not changed
  inspect --store PATH                header, latest checkpoint and row counts (read-only)
  verify --store PATH                 reads and checks every published row (read-only)

defaults: --world 1, --rule-seed the seed, --elevation-scale 0.5 (create), --center 0,0,
          --radius 6
";

#[derive(Debug)]
struct Usage(String);

struct Args {
    values: Vec<(String, Option<String>)>,
}

impl Args {
    /// Splits `--name value` pairs and `--flag`s; `flags` lists the options without a value.
    fn parse(raw: &[String], allowed: &[&str], flags: &[&str]) -> Result<Self, Usage> {
        let mut values = Vec::new();
        let mut index = 0;
        while index < raw.len() {
            let name = raw[index].as_str();
            if !allowed.contains(&name) {
                return Err(Usage(format!("unknown option `{name}`")));
            }
            if values.iter().any(|(seen, _)| seen == name) {
                return Err(Usage(format!("`{name}` given twice")));
            }
            if flags.contains(&name) {
                values.push((name.to_owned(), None));
                index += 1;
            } else {
                let value = raw
                    .get(index + 1)
                    .ok_or_else(|| Usage(format!("`{name}` needs a value")))?;
                values.push((name.to_owned(), Some(value.clone())));
                index += 2;
            }
        }
        Ok(Self { values })
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(seen, _)| seen == name)
            .and_then(|(_, value)| value.as_deref())
    }

    fn flag(&self, name: &str) -> bool {
        self.values.iter().any(|(seen, _)| seen == name)
    }

    fn required(&self, name: &str) -> Result<&str, Usage> {
        self.value(name)
            .ok_or_else(|| Usage(format!("`{name}` is required")))
    }

    fn number<T: std::str::FromStr>(&self, name: &str) -> Result<Option<T>, Usage> {
        self.value(name)
            .map(|text| {
                text.parse()
                    .map_err(|_| Usage(format!("`{name}` must be a number, not `{text}`")))
            })
            .transpose()
    }
}

fn path(args: &Args, name: &str) -> Result<PathBuf, Usage> {
    Ok(PathBuf::from(args.required(name)?))
}

fn window(args: &Args) -> Result<ResidentWindow, Usage> {
    let (x, y) = match args.value("--center") {
        None => (0, 0),
        Some(text) => {
            let (x, y) = text
                .split_once(',')
                .ok_or_else(|| Usage(format!("`--center` must be X,Y, not `{text}`")))?;
            let parse = |part: &str| {
                part.trim()
                    .parse::<i32>()
                    .map_err(|_| Usage(format!("`--center` must be X,Y, not `{text}`")))
            };
            (parse(x)?, parse(y)?)
        }
    };
    let radius = args.number::<u32>("--radius")?.unwrap_or(6);
    ResidentWindow::new(ChunkKey::new(x, y, 0), radius, radius)
        .map_err(|error| Usage(format!("unsupported window: {error:?}")))
}

fn config(args: &Args, default_scale: Option<f32>) -> Result<PhysicalConfig, Usage> {
    let scale = match (args.number::<f32>("--elevation-scale")?, default_scale) {
        (Some(scale), _) | (None, Some(scale)) => scale,
        (None, None) => return Err(Usage("`--elevation-scale` is required".to_owned())),
    };
    Ok(PhysicalConfig {
        rules: SimulationRules::ALL,
        ambient_temperature: AmbientTemperature::celsius(DEFAULT_AMBIENT_TEMPERATURE_C)
            .map_err(|error| Usage(format!("{error:?}")))?,
        elevation_scale: ElevationScale::new(scale)
            .map_err(|_| Usage(format!("`--elevation-scale` {scale} is outside 0.01..=5")))?,
    })
}

/// A validated `create`.
struct Create {
    store: PathBuf,
    header: WorldHeader,
    config: PhysicalConfig,
    window: ResidentWindow,
}

fn create_plan(args: &Args) -> Result<Create, Usage> {
    let store = path(args, "--store")?;
    let seed = args
        .number::<u32>("--seed")?
        .ok_or_else(|| Usage("`--seed` is required".to_owned()))?;
    let header = WorldHeader {
        world: WorldId::new(args.number("--world")?.unwrap_or(1)),
        generator: Generator {
            revision: identity::GENERATOR_REVISION,
            seed,
        },
        rule_seed: args.number("--rule-seed")?.unwrap_or(u64::from(seed)),
        origin: WorldOrigin::Created,
    };
    Ok(Create {
        store,
        header,
        config: config(args, Some(world_model::config::DEFAULT_ELEVATION_SCALE))?,
        window: window(args)?,
    })
}

fn create(plan: Create) -> Result<String, String> {
    RedbWorldStore::create(
        &plan.store,
        &plan.header,
        &Publication {
            id: CheckpointId::FIRST,
            tick: None,
            config: plan.config,
            window: plan.window,
            epoch: SessionEpoch::FIRST,
            heads: &[],
            bodies: 0,
        },
        StoreFaults::new(),
    )
    .map_err(|error| format!("{}: {error}", plan.store.display()))?;
    Ok(format!(
        "created {}: world {}, seed {}, elevation scale {}, checkpoint 1 at no tick",
        plan.store.display(),
        plan.header.world.value(),
        plan.header.generator.seed,
        plan.config.elevation_scale.value()
    ))
}

/// A validated `import-legacy`, except for the elevation scale, which the import cannot run
/// without and whose absence is reported as an import refusal with its reason.
struct Import {
    source: PathBuf,
    destination: PathBuf,
    options: Result<ImportOptions, String>,
}

fn import_plan(args: &Args) -> Result<Import, Usage> {
    let source = path(args, "--legacy")?;
    let destination = path(args, "--store")?;
    let seed = args.number::<u32>("--seed")?;
    let world = WorldId::new(args.number("--world")?.unwrap_or(1));
    let rule_seed = args.number::<u64>("--rule-seed")?;
    let window = window(args)?;
    let rows_per_transaction = args.number("--rows-per-transaction")?.unwrap_or(64);
    let non_planar = if args.flag("--skip-nonplanar-rows") {
        NonPlanarRows::Skip
    } else {
        NonPlanarRows::Refuse
    };
    let options = match (seed, args.value("--elevation-scale")) {
        (None, _) => Err("`--seed` is required: the legacy store does not record one".to_owned()),
        (Some(_), None) => {
            Err("`--elevation-scale` is required: the legacy store does not record one".to_owned())
        }
        (Some(seed), Some(_)) => Ok(ImportOptions {
            world,
            seed,
            rule_seed: rule_seed.unwrap_or(u64::from(seed)),
            config: config(args, None)?,
            window,
            non_planar,
            rows_per_transaction,
        }),
    };
    Ok(Import {
        source,
        destination,
        options,
    })
}

fn import(plan: Import) -> Result<String, String> {
    let options = plan.options?;
    let (source, destination) = (plan.source, plan.destination);
    let report = legacy::import(&source, &destination, &options, StoreFaults::new())
        .map_err(|error| match error {
            legacy::ImportError::CorruptRows(rows) => format!(
                "import refused: {} legacy rows do not decode (nothing was written): {}",
                rows.len(),
                rows.iter()
                    .map(|(key, detail)| format!("{:?}: {detail}", key.key))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            legacy::ImportError::NonPlanarRows(keys) => format!(
                "import refused: {} rows have z != 0 ({}); pass --skip-nonplanar-rows to leave them in the original",
                keys.len(),
                keys.iter()
                    .map(|key| format!("{:?}", key.key))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            other => other.to_string(),
        })?;
    let interpretation = report.interpretation;
    Ok(format!(
        "imported {} into {}\n  interpretation: seed {} and elevation scale {} as supplied (the legacy store records neither)\n  rows imported: {}, rows left in the original (z != 0): {} {:?}\n  legacy save times (session seconds, not kept): {:?}\n  read through a repaired copy: {}\n  published checkpoint {} at no tick",
        source.display(),
        destination.display(),
        interpretation.seed,
        interpretation.elevation_scale.value(),
        interpretation.rows_imported,
        interpretation.rows_refused,
        report
            .refused_keys
            .iter()
            .map(|key| key.key)
            .collect::<Vec<_>>(),
        report.last_simulated,
        report
            .read_repaired_copy
            .as_ref()
            .map_or("no".to_owned(), |copy| copy.display().to_string()),
        report.checkpoint.value(),
    ))
}

fn read_only(store: &Path) -> Result<RedbWorldStore, String> {
    RedbWorldStore::open(
        store,
        OpenOptionsStore {
            access: Access::ReadOnly,
            allow_incomplete: true,
            faults: StoreFaults::new(),
        },
    )
    .map_err(|error| format!("{}: {error}", store.display()))
}

fn inspect(store: &Path) -> Result<String, String> {
    let store = read_only(store)?;
    let header = store.header();
    let mut lines = vec![
        format!("store {}", store.path().display()),
        format!(
            "  world {}, generator revision {} seed {}, rule seed {}",
            header.world.value(),
            header.generator.revision,
            header.generator.seed,
            header.rule_seed
        ),
        format!("  origin {:?}", header.origin),
    ];
    match store.latest_row() {
        Ok(row) => {
            let manifest = row.manifest;
            lines.push(format!(
                "  latest checkpoint {} at tick {:?}: {} saved chunks, {} bodies, window {:?} radius {}x{}, epoch {}",
                manifest.id.value(),
                manifest.tick.map(|tick| tick.value()),
                manifest.chunks,
                manifest.bodies,
                manifest.window.center().key,
                manifest.window.radius_x(),
                manifest.window.radius_y(),
                manifest.epoch.value()
            ));
            lines.push(format!(
                "  configuration: rules {:?}, ambient {} °C, elevation scale {}",
                manifest.config.rules,
                manifest.config.ambient_temperature.degrees(),
                manifest.config.elevation_scale.value()
            ));
        }
        Err(error) => lines.push(format!("  no complete checkpoint: {error}")),
    }
    let [chunks, heads, bodies, manifests] =
        store.row_counts().map_err(|error| error.to_string())?;
    lines.push(format!(
        "  rows: {chunks} chunk revisions, {heads} heads, {bodies} body sets, {manifests} manifests"
    ));
    Ok(lines.join("\n"))
}

fn verify(store: &Path) -> Result<String, String> {
    let store = read_only(store)?;
    let report = store.verify().map_err(|error| error.to_string())?;
    Ok(format!(
        "verified checkpoint {}: {} published chunks and {} bodies read and checked",
        report.checkpoint.value(),
        report.chunks,
        report.bodies
    ))
}

fn run(raw: &[String]) -> Result<Result<String, String>, Usage> {
    let Some((command, rest)) = raw.split_first() else {
        return Err(Usage("a command is required".to_owned()));
    };
    Ok(match command.as_str() {
        "create" => {
            let args = Args::parse(
                rest,
                &[
                    "--store",
                    "--seed",
                    "--world",
                    "--rule-seed",
                    "--elevation-scale",
                    "--center",
                    "--radius",
                ],
                &[],
            )?;
            create(create_plan(&args)?)
        }
        "import-legacy" => {
            let args = Args::parse(
                rest,
                &[
                    "--legacy",
                    "--store",
                    "--seed",
                    "--elevation-scale",
                    "--world",
                    "--rule-seed",
                    "--center",
                    "--radius",
                    "--skip-nonplanar-rows",
                    "--rows-per-transaction",
                ],
                &["--skip-nonplanar-rows"],
            )?;
            import(import_plan(&args)?)
        }
        "inspect" => inspect(&path(&Args::parse(rest, &["--store"], &[])?, "--store")?),
        "verify" => verify(&path(&Args::parse(rest, &["--store"], &[])?, "--store")?),
        "-h" | "--help" => Ok(USAGE.to_owned()),
        other => return Err(Usage(format!("unknown command `{other}`"))),
    })
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match run(&raw) {
        Ok(Ok(text)) => {
            println!("{text}");
            ExitCode::SUCCESS
        }
        Ok(Err(error)) => {
            eprintln!("world-store: {error}");
            ExitCode::from(1)
        }
        Err(Usage(problem)) => {
            eprintln!("world-store: {problem}\n\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
