//! What the version-1 fixture holds, computed without the store: the capture program writes it
//! through the store, and the tests compare what the store reads back from the committed file
//! with it. Changing anything here changes the expected content, not the fixture: the fixture is
//! immutable, and a capture with changed content is a new fixture set.

#![allow(dead_code)]

use world_model::{
    body::{BodyHandle, BodyPreset},
    cell::WorldCell,
    command::{AmbientTemperature, ElevationScale},
    generation::ChunkTheme,
    glam::{IVec2, Vec2, Vec3},
    identity,
    material::FluidMat,
    observation::BodyCounters,
    resume::{BodyResume, BodySet, BodySlot, GroundPlane, SlotState},
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, SessionEpoch, Tick, WorldId},
    port::ResidentWindow,
    store::{ChunkPayload, Generator, PhysicalConfig, WorldHeader, WorldOrigin},
};

/// The fixture, relative to the package.
pub const FIXTURE: &str = "tests/fixtures/v1/world-v1.redb";

pub fn header() -> WorldHeader {
    WorldHeader {
        world: WorldId::new(0x0F1C_7E01),
        generator: Generator {
            revision: identity::GENERATOR_REVISION,
            seed: 69,
        },
        rule_seed: 0x5EED_0F1C,
        origin: WorldOrigin::Created,
    }
}

pub fn window() -> ResidentWindow {
    ResidentWindow::new(ChunkKey::new(0, 0, 0), 1, 1).expect("window")
}

pub fn config(checkpoint: u64) -> PhysicalConfig {
    PhysicalConfig {
        rules: if checkpoint == 3 {
            SimulationRules {
                fire: false,
                ..SimulationRules::ALL
            }
        } else {
            SimulationRules::ALL
        },
        ambient_temperature: AmbientTemperature::celsius(if checkpoint == 3 { -4.5 } else { 20.0 })
            .expect("supported"),
        elevation_scale: if checkpoint == 3 {
            ElevationScale::new(0.75).expect("supported")
        } else {
            ElevationScale::DEFAULT
        },
    }
}

/// A checkpoint after the created one.
pub struct Checkpoint {
    pub id: u64,
    pub tick: Tick,
    pub epoch: SessionEpoch,
    /// The revisions it publishes.
    pub revisions: Vec<(ChunkKey, u64)>,
}

pub fn checkpoints() -> Vec<Checkpoint> {
    vec![
        Checkpoint {
            id: 2,
            tick: Tick::new(600),
            epoch: SessionEpoch::new(2),
            revisions: vec![
                (ChunkKey::new(0, 0, 0), 1),
                (ChunkKey::new(-1, -1, 0), 1),
                (ChunkKey::new(1, 0, 0), 1),
            ],
        },
        Checkpoint {
            id: 3,
            tick: Tick::new(1_234_567),
            epoch: SessionEpoch::new(3),
            revisions: vec![(ChunkKey::new(0, 0, 0), 2)],
        },
    ]
}

/// The published revision of every saved key after the last checkpoint.
pub fn published() -> Vec<(ChunkKey, u64)> {
    vec![
        (ChunkKey::new(0, 0, 0), 2),
        (ChunkKey::new(-1, -1, 0), 1),
        (ChunkKey::new(1, 0, 0), 1),
    ]
}

/// SplitMix64 words, so every bit pattern of both words occurs.
fn words(seed: u64) -> impl Iterator<Item = u64> {
    let mut state = seed;
    std::iter::repeat_with(move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    })
}

/// The cells of `key` at `revision`: the first cells hold boundary words (zero, all ones, each
/// word alone), the rest arbitrary words, so a field or word swap in the format shows.
pub fn cells(key: ChunkKey, revision: u64) -> Vec<WorldCell> {
    let seed = (key.key.x as u32 as u64) << 40 ^ (key.key.y as u32 as u64) << 8 ^ revision;
    let mut cells: Vec<WorldCell> = words(seed).take(CHUNK_CELL_COUNT).map(WorldCell).collect();
    cells[0] = WorldCell(0);
    cells[1] = WorldCell(u64::MAX);
    cells[2] = WorldCell(0xFFFF_FFFF);
    cells[3] = WorldCell(0xFFFF_FFFF_0000_0000);
    cells
}

pub fn theme(key: ChunkKey) -> ChunkTheme {
    ChunkTheme(((key.key.x + 2 * key.key.y).rem_euclid(8)) as u8)
}

pub fn payload(key: ChunkKey, revision: u64) -> ChunkPayload {
    ChunkPayload::new(cells(key, revision).into_boxed_slice(), theme(key)).expect("a whole chunk")
}

pub fn revision(value: u64) -> ChunkRevision {
    ChunkRevision::new(value)
}

fn body(index: u32, generation: u32, step: f32, preset: BodyPreset) -> BodyResume {
    BodyResume {
        handle: BodyHandle::new(index, generation),
        description: preset.description(),
        anchor: IVec2::new(-3 + index as i32, 2 - index as i32),
        local: Vec3::new(10.5 + step, 40.25 - step, 21.0 + step),
        velocity: Vec3::new(1.0, -2.0 * step, 0.5),
        grounded: index.is_multiple_of(2),
        suspended: index == 2,
        plane_valid: true,
        terrain_contact: index.is_multiple_of(2),
        ground_normal: Vec3::new(0.0, 1.0, 0.0),
        submerged: 0.25,
        previous_submerged: 0.125,
        pending_wake: 0.5 * step,
        plane: GroundPlane {
            origin: Vec2::new(10.0, 40.0),
            height: 20.0 + step,
            slope: Vec2::new(0.1, -0.2),
        },
        counters: BodyCounters {
            landings: 3 + index,
            liquid_entries: 1,
            body_contacts: 7 * index,
            craters: 2,
            splashes: 1,
            absorbed_energy: 1_000 + index,
        },
        touching: index,
        liquid: if index == 0 {
            FluidMat::FLUID_WATER
        } else {
            FluidMat::EMPTY
        },
        terrain_normal: 0x7FFF_0000 + index,
    }
}

/// The body set of checkpoint `checkpoint`: a live slot reused at a later generation, a free
/// slot, a retired one and live bodies in the others.
pub fn bodies(checkpoint: u64) -> BodySet {
    let step = checkpoint as f32;
    BodySet {
        slots: vec![
            BodySlot {
                generation: 4,
                state: SlotState::Live,
            },
            BodySlot {
                generation: 2,
                state: SlotState::Free,
            },
            BodySlot {
                generation: 0,
                state: SlotState::Live,
            },
            BodySlot {
                generation: u32::MAX,
                state: SlotState::Retired,
            },
        ],
        bodies: vec![
            body(0, 4, step, BodyPreset::Metal),
            body(2, 0, step, BodyPreset::Wood),
        ],
    }
}
