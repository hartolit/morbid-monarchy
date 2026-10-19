//! The saved worlds the client offers: one `world-store` file per world in the worlds
//! directory, named `<name>.world`. Listing opens each read-only (no repair, no write), so a
//! damaged or foreign file is shown as such and never changed. Creating and importing write new
//! files only; an existing file is never overwritten.

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use world_model::{
    command::{AmbientTemperature, ElevationScale},
    config::DEFAULT_AMBIENT_TEMPERATURE_C,
    identity,
    rules::SimulationRules,
    spatial::ChunkKey,
};
use world_runtime::{
    ids::{SessionEpoch, Tick, WorldId},
    port::ResidentWindow,
    store::{
        CheckpointId, Generator, PhysicalConfig, Publication, WorldHeader, WorldOrigin, WorldStore,
    },
};
use world_store::{
    faults::StoreFaults,
    legacy::{self, ImportOptions, ImportReport, NonPlanarRows},
    store::{Access, OpenOptionsStore, RedbWorldStore},
};

pub const EXTENSION: &str = "world";
/// The window a new world opens with: the qualification profile's radius around the origin.
pub const NEW_WORLD_RADIUS: u32 = 6;

/// One file of the worlds directory.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldEntry {
    pub name: String,
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
    pub state: EntryState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EntryState {
    Ready {
        seed: u32,
        imported: bool,
        /// The latest checkpoint's tick; `None` for a world that has never run.
        tick: Option<Tick>,
        chunks: u64,
        bodies: u32,
    },
    /// The file could not be read as a world; the reason, for the user.
    Unreadable(String),
}

/// A world name: letters, digits, spaces, `-` and `_`, 1 to 64 characters.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name.chars().count() <= 64
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_')
}

pub fn world_path(worlds: &Path, name: &str) -> PathBuf {
    worlds.join(format!("{}.{EXTENSION}", name.trim()))
}

/// Every `.world` file in `worlds`, newest first. A missing directory lists nothing.
pub fn list(worlds: &Path) -> Result<Vec<WorldEntry>, String> {
    let entries = match std::fs::read_dir(worlds) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{}: {error}", worlds.display())),
    };
    let mut list = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(EXTENSION) {
            continue;
        }
        let name = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let modified = entry.metadata().ok().and_then(|meta| meta.modified().ok());
        list.push(WorldEntry {
            state: describe(&path),
            name,
            path,
            modified,
        });
    }
    list.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(list)
}

/// What a world file holds, read without changing it.
pub fn describe(path: &Path) -> EntryState {
    let options = OpenOptionsStore {
        access: Access::ReadOnly,
        ..OpenOptionsStore::default()
    };
    match RedbWorldStore::open(path, options) {
        Err(error) => EntryState::Unreadable(error.to_string()),
        Ok(store) => match store.latest() {
            Err(error) => EntryState::Unreadable(error.to_string()),
            Ok(latest) => EntryState::Ready {
                seed: store.header().generator.seed,
                imported: matches!(store.header().origin, WorldOrigin::LegacyImport(_)),
                tick: latest.tick,
                chunks: latest.chunks,
                bodies: latest.bodies,
            },
        },
    }
}

/// A seed from the clock, for a world created without one.
pub fn fresh_seed() -> u32 {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    (nanos ^ (nanos >> 32) ^ (nanos >> 64)) as u32
}

/// A world identity for a new world: its creation time, which distinguishes the worlds of one
/// person's directory.
fn fresh_world_id() -> WorldId {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(1, |elapsed| elapsed.as_nanos() as u64);
    WorldId::new(nanos.max(1))
}

fn default_config() -> Result<PhysicalConfig, String> {
    Ok(PhysicalConfig {
        rules: SimulationRules::ALL,
        ambient_temperature: AmbientTemperature::celsius(DEFAULT_AMBIENT_TEMPERATURE_C)
            .map_err(|error| format!("{error:?}"))?,
        elevation_scale: ElevationScale::DEFAULT,
    })
}

fn home_window() -> Result<ResidentWindow, String> {
    ResidentWindow::new(ChunkKey::new(0, 0, 0), NEW_WORLD_RADIUS, NEW_WORLD_RADIUS)
        .map_err(|error| format!("{error:?}"))
}

/// Creates a new world `name` in `worlds`, generated from `seed`: its first checkpoint has run
/// no tick, and every chunk is generated when it first becomes resident.
pub fn create(worlds: &Path, name: &str, seed: u32) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err(format!(
            "`{name}` is not a world name: use 1 to 64 letters, digits, spaces, - or _"
        ));
    }
    std::fs::create_dir_all(worlds).map_err(|error| format!("{}: {error}", worlds.display()))?;
    let path = world_path(worlds, name);
    if path.exists() {
        return Err(format!("a world named `{}` exists", name.trim()));
    }
    let header = WorldHeader {
        world: fresh_world_id(),
        generator: Generator {
            revision: identity::GENERATOR_REVISION,
            seed,
        },
        rule_seed: u64::from(seed),
        origin: WorldOrigin::Created,
    };
    RedbWorldStore::create(
        &path,
        &header,
        &Publication {
            id: CheckpointId::FIRST,
            tick: None,
            config: default_config()?,
            window: home_window()?,
            epoch: SessionEpoch::FIRST,
            heads: &[],
            bodies: 0,
        },
        StoreFaults::new(),
    )
    .map_err(|error| format!("{}: {error}", path.display()))?;
    Ok(path)
}

/// Imports the baseline's `save.redb` at `legacy` as a new world `name`. The original is only
/// read (an unclean one is repaired as a copy); the seed and elevation scale it was generated
/// and played with are the caller's interpretation, since the old format recorded neither.
pub fn import_legacy(
    worlds: &Path,
    name: &str,
    legacy: &Path,
    seed: u32,
    elevation_scale: f32,
) -> Result<ImportReport, String> {
    if !valid_name(name) {
        return Err(format!("`{name}` is not a world name"));
    }
    std::fs::create_dir_all(worlds).map_err(|error| format!("{}: {error}", worlds.display()))?;
    let destination = world_path(worlds, name);
    let mut config = default_config()?;
    config.elevation_scale = ElevationScale::new(elevation_scale)
        .map_err(|_| format!("elevation scale {elevation_scale} is outside 0.01 to 5"))?;
    let options = ImportOptions {
        world: fresh_world_id(),
        seed,
        rule_seed: u64::from(seed),
        config,
        window: home_window()?,
        non_planar: NonPlanarRows::Skip,
        rows_per_transaction: 64,
    };
    legacy::import(legacy, &destination, &options, StoreFaults::new())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_checked_and_worlds_never_overwritten() {
        assert!(valid_name("Valley 2"));
        assert!(!valid_name(""));
        assert!(!valid_name("../escape"));
        assert!(!valid_name(&"x".repeat(65)));
        let dir = std::env::temp_dir().join(format!("morbid-catalog-{}", std::process::id()));
        let path = create(&dir, "Valley", 69).expect("created");
        assert_eq!(path, world_path(&dir, "Valley"));
        assert!(
            create(&dir, "Valley", 1).is_err(),
            "an existing world is kept"
        );
        let listed = list(&dir).expect("listed");
        assert_eq!(listed.len(), 1);
        match &listed[0].state {
            EntryState::Ready {
                seed,
                tick,
                imported,
                ..
            } => {
                assert_eq!(*seed, 69);
                assert_eq!(*tick, None);
                assert!(!imported);
            }
            other => panic!("{other:?}"),
        }
        std::fs::write(dir.join("broken.world"), b"not a world").expect("write");
        let listed = list(&dir).expect("listed");
        assert!(
            listed
                .iter()
                .any(|entry| matches!(entry.state, EntryState::Unreadable(_)))
        );
        assert_eq!(
            std::fs::read(dir.join("broken.world")).expect("read"),
            b"not a world"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
