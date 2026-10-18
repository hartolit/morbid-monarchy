//! The saved-world format, version 1: the exact bytes of every row the store writes.
//!
//! Every value is little-endian. Every row is an envelope:
//!
//! ```text
//! offset 0   magic        4 bytes, names the row kind
//!        4   version      u16, FORMAT_VERSION
//!        6   reserved     u16, 0
//!        8   body         kind-specific, fixed layout for the version
//!        n   checksum     u32, CRC-32C (Castagnoli) of bytes 0..n
//! ```
//!
//! The envelope (magic, version, checksum algorithm) is the same for every version, so a
//! reader checks the checksum first (a mismatch is corruption), then the magic, then the
//! version (another version is unsupported and left unmodified), then the exact length the
//! version defines, before it allocates anything sized by the row. No GPU record, padding,
//! handle or ECS identity is ever written; the bodies are the semantic values of
//! `world_model::resume`. The row kinds and their bodies, with byte offsets from the start of
//! the row:
//!
//! **Header** (`MMWH`, 92 bytes): 8 world id u64; 16 cell format revision u32; 20 physics
//! revision u32; 24 cell layout fingerprint u64; 32 physics fingerprint u64; 40 generator
//! revision u32; 44 generator seed u32; 48 rule seed u64; 56 chunk edge u16 (64); 58 axis
//! convention u16 (1: cell `y` is render `-z`, keys planar with `z = 0`); 60 origin u8 (0
//! created, 1 legacy import); 61 three zero bytes; 64 legacy seed u32; 68 legacy elevation scale
//! f32; 72 legacy rows imported u64; 80 legacy rows refused u64 (the four legacy fields are zero
//! for a created world); 88 checksum.
//!
//! **Chunk** (`MMCK`, 32,812 bytes): 8 key x, y, z i32; 20 zero u32; 24 revision u64; 32 theme
//! u8; 33 three zero bytes; 36 cell count u32 (4,096); 40 the cells, row-major, each the packed
//! `WorldCell` u64 (bit 0 is the low bit of the first byte); 32,808 checksum. The row repeats
//! its key and revision, so a row read under another key is corrupt.
//!
//! **Head** (20 bytes, no magic: one per saved key, under the key): 0 revision u64; 8
//! checkpoint that published it u64; 16 checksum.
//!
//! **Body set** (`MMBS`): 8 checkpoint id u64; 16 slot count `S` u32 (at most 4,096); 20 body
//! count `B` u32 (at most `S`); 24 `S` slots of 8 bytes (generation u32, state u8: 0 free, 1
//! live, 2 retired, three zero bytes); then `B` bodies of [`BODY_BYTES`] (below); then the
//! checksum: `28 + 8 S + 136 B` bytes in all.
//!
//! **Body** (136 bytes, 34 four-byte fields): index, generation u32; mass, radius, relative
//! density f32; anchor x, y i32; local x, y, height f32; velocity x, y, z f32; flags u32 (bit 0
//! grounded, 1 suspended, 2 plane valid, 3 terrain contact); ground normal x, y, z f32;
//! submerged, previous submerged, pending wake f32; plane origin x, y, height, slope x, y f32;
//! counters u32 (landings, liquid entries, body contacts, craters, splashes, absorbed energy);
//! touching u32; liquid u32 (a fluid material, 0 for none); terrain normal u32 (two 16-bit
//! snorms, octahedral).
//!
//! **Manifest** (`MMCP`, 92 bytes): 8 checkpoint id u64; 16 has tick u8; 17 seven zero bytes;
//! 24 tick u64 (zero without one); 32 rules u32 (`SimulationRules::bits`); 36 ambient
//! temperature f32 (°C); 40 elevation scale f32; 44 zero u32; 48 window centre x, y i32; 56
//! window radius x, y u32; 64 saved keys u64; 72 live bodies u32; 76 session epoch u32; 80 keys
//! this checkpoint moved u32; 84 the body set row's checksum u32; 88 checksum.
//!
//! **Latest** (`MMLT`, 20 bytes): 8 latest published checkpoint id u64; 16 checksum.
//!
//! **Session marker** (`MMSN`, 20 bytes): 8 state u32 (1 open, 2 closed cleanly); 12 zero u32;
//! 16 checksum.
//!
//! Floats are stored as their IEEE-754 bits and validated against the model's ranges on
//! reading; a non-finite or out-of-range value is corruption, never a clamp.

use world_model::{
    body::{BodyHandle, SphereDescription},
    cell::WorldCell,
    command::{AmbientTemperature, ElevationScale},
    generation::ChunkTheme,
    glam::{IVec2, Vec2, Vec3},
    identity,
    material::FluidMat,
    observation::BodyCounters,
    resume::{BodyResume, BodySet, BodySlot, GroundPlane, SlotState},
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, CHUNK_SIZE, ChunkKey},
};
use world_runtime::{
    ids::{ChunkRevision, SessionEpoch, Tick, WorldId},
    port::ResidentWindow,
    store::{
        CheckpointId, CheckpointManifest, ChunkPayload, Generator, LegacyInterpretation,
        PhysicalConfig, StoreError, StoredItem, Unsupported, WorldHeader, WorldOrigin,
    },
};

/// The format version this build writes and reads.
pub const FORMAT_VERSION: u16 = 1;
/// The axis convention of `world_model::spatial`: cell `y` grows toward render `-z`, and keys
/// are planar (`z = 0`).
pub const AXIS_CONVENTION: u16 = 1;

const ENVELOPE: usize = 8;
const CHECKSUM: usize = 4;

pub const HEADER_BYTES: usize = 92;
pub const CHUNK_BYTES: usize = 40 + CHUNK_CELL_COUNT * 8 + CHECKSUM;
pub const HEAD_BYTES: usize = 20;
pub const BODY_BYTES: usize = 136;
pub const MANIFEST_BYTES: usize = 92;
pub const LATEST_BYTES: usize = 20;
pub const SESSION_BYTES: usize = 20;

const HEADER_MAGIC: [u8; 4] = *b"MMWH";
const CHUNK_MAGIC: [u8; 4] = *b"MMCK";
const BODIES_MAGIC: [u8; 4] = *b"MMBS";
const MANIFEST_MAGIC: [u8; 4] = *b"MMCP";
const LATEST_MAGIC: [u8; 4] = *b"MMLT";
const SESSION_MAGIC: [u8; 4] = *b"MMSN";

/// CRC-32C (Castagnoli, reflected polynomial 0x82F63B78), table-driven.
pub fn crc32c(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut index = 0;
        while index < 256 {
            let mut crc = index as u32;
            let mut bit = 0;
            while bit < 8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0x82F6_3B78
                } else {
                    crc >> 1
                };
                bit += 1;
            }
            table[index] = crc;
            index += 1;
        }
        table
    };
    let mut crc = !0u32;
    for byte in bytes {
        crc = TABLE[((crc ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// Appends little-endian fields.
struct Writer(Vec<u8>);

impl Writer {
    fn with_envelope(magic: [u8; 4], capacity: usize) -> Self {
        let mut bytes = Vec::with_capacity(capacity);
        bytes.extend_from_slice(&magic);
        bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        Self(bytes)
    }

    fn u8(&mut self, value: u8) {
        self.0.push(value);
    }

    fn zeros(&mut self, count: usize) {
        self.0.resize(self.0.len() + count, 0);
    }

    fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn i32(&mut self, value: i32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }

    fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }

    fn finish(mut self) -> Vec<u8> {
        let checksum = crc32c(&self.0);
        self.0.extend_from_slice(&checksum.to_le_bytes());
        self.0
    }
}

/// Reads little-endian fields of a row whose length was already checked.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], at: usize) -> Self {
        Self { bytes, at }
    }

    fn take<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        out.copy_from_slice(&self.bytes[self.at..self.at + N]);
        self.at += N;
        out
    }

    fn u8(&mut self) -> u8 {
        self.take::<1>()[0]
    }

    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take())
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take())
    }

    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take())
    }

    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take())
    }

    fn f32(&mut self) -> f32 {
        f32::from_bits(self.u32())
    }

    fn skip(&mut self, count: usize) {
        self.at += count;
    }
}

/// Checks a row's envelope: length, checksum, magic and version, in that order. Returns the
/// row's checksum.
fn open_envelope(
    bytes: &[u8],
    magic: [u8; 4],
    item: StoredItem,
    exact: Option<usize>,
) -> Result<u32, StoreError> {
    let corrupt = |detail| StoreError::Corrupt { item, detail };
    if bytes.len() < ENVELOPE + CHECKSUM {
        return Err(corrupt("shorter than the row envelope"));
    }
    let (content, stored) = bytes.split_at(bytes.len() - CHECKSUM);
    let stored = u32::from_le_bytes([stored[0], stored[1], stored[2], stored[3]]);
    if crc32c(content) != stored {
        return Err(corrupt("checksum mismatch"));
    }
    if content[..4] != magic {
        return Err(corrupt("wrong row kind"));
    }
    let version = u16::from_le_bytes([content[4], content[5]]);
    if version != FORMAT_VERSION {
        return Err(StoreError::Unsupported {
            item,
            detail: Unsupported::FormatVersion {
                stored: version,
                supported: FORMAT_VERSION,
            },
        });
    }
    if let Some(length) = exact
        && bytes.len() != length
    {
        return Err(corrupt("wrong length for its format version"));
    }
    Ok(stored)
}

pub fn encode_header(header: &WorldHeader) -> Vec<u8> {
    let mut w = Writer::with_envelope(HEADER_MAGIC, HEADER_BYTES);
    w.u64(header.world.value());
    w.u32(identity::CELL_FORMAT_REVISION);
    w.u32(identity::PHYSICS_REVISION);
    w.u64(identity::cell_layout_fingerprint());
    w.u64(identity::physics_fingerprint());
    w.u32(header.generator.revision);
    w.u32(header.generator.seed);
    w.u64(header.rule_seed);
    w.u16(CHUNK_SIZE as u16);
    w.u16(AXIS_CONVENTION);
    match header.origin {
        WorldOrigin::Created => {
            w.u8(0);
            w.zeros(3);
            w.u32(0);
            w.f32(0.0);
            w.u64(0);
            w.u64(0);
        }
        WorldOrigin::LegacyImport(legacy) => {
            w.u8(1);
            w.zeros(3);
            w.u32(legacy.seed);
            w.f32(legacy.elevation_scale.value());
            w.u64(legacy.rows_imported);
            w.u64(legacy.rows_refused);
        }
    }
    w.finish()
}

/// Decodes a header and checks that this build reads the world it describes.
pub fn decode_header(bytes: &[u8]) -> Result<WorldHeader, StoreError> {
    let item = StoredItem::Header;
    open_envelope(bytes, HEADER_MAGIC, item, Some(HEADER_BYTES))?;
    let corrupt = |detail| StoreError::Corrupt { item, detail };
    let unsupported = |detail| StoreError::Unsupported { item, detail };
    let mut r = Reader::new(bytes, ENVELOPE);
    let world = WorldId::new(r.u64());
    let cell_format = r.u32();
    let physics = r.u32();
    let layout = r.u64();
    let physics_print = r.u64();
    let generator_revision = r.u32();
    let seed = r.u32();
    let rule_seed = r.u64();
    let chunk_edge = r.u16();
    let axes = r.u16();
    let origin = r.u8();
    r.skip(3);
    let legacy_seed = r.u32();
    let legacy_scale = r.f32();
    let rows_imported = r.u64();
    let rows_refused = r.u64();

    let supported_layout = identity::cell_layout_fingerprint();
    if cell_format != identity::CELL_FORMAT_REVISION || layout != supported_layout {
        return Err(unsupported(Unsupported::CellLayout {
            stored: layout,
            supported: supported_layout,
        }));
    }
    let supported_physics = (identity::PHYSICS_REVISION, identity::physics_fingerprint());
    if (physics, physics_print) != supported_physics {
        return Err(unsupported(Unsupported::Physics {
            stored: (physics, physics_print),
            supported: supported_physics,
        }));
    }
    if generator_revision != identity::GENERATOR_REVISION {
        return Err(unsupported(Unsupported::Generator {
            stored: generator_revision,
            supported: identity::GENERATOR_REVISION,
        }));
    }
    if chunk_edge as usize != CHUNK_SIZE || axes != AXIS_CONVENTION {
        return Err(corrupt("unknown chunk edge or axis convention"));
    }
    let origin = match origin {
        0 if legacy_seed == 0
            && legacy_scale.to_bits() == 0
            && rows_imported == 0
            && rows_refused == 0 =>
        {
            WorldOrigin::Created
        }
        1 => WorldOrigin::LegacyImport(LegacyInterpretation {
            seed: legacy_seed,
            elevation_scale: ElevationScale::new(legacy_scale)
                .map_err(|_| corrupt("legacy elevation scale outside the supported range"))?,
            rows_imported,
            rows_refused,
        }),
        _ => return Err(corrupt("unknown world origin")),
    };
    Ok(WorldHeader {
        world,
        generator: Generator {
            revision: generator_revision,
            seed,
        },
        rule_seed,
        origin,
    })
}

pub fn encode_chunk(key: ChunkKey, revision: ChunkRevision, payload: &ChunkPayload) -> Vec<u8> {
    let mut w = Writer::with_envelope(CHUNK_MAGIC, CHUNK_BYTES);
    w.i32(key.key.x);
    w.i32(key.key.y);
    w.i32(key.key.z);
    w.u32(0);
    w.u64(revision.value());
    w.u8(payload.theme().0);
    w.zeros(3);
    w.u32(CHUNK_CELL_COUNT as u32);
    for cell in payload.cells() {
        w.u64(cell.0);
    }
    w.finish()
}

/// Decodes a chunk row stored under `key` and `revision` into `into`.
pub fn decode_chunk(
    bytes: &[u8],
    key: ChunkKey,
    revision: ChunkRevision,
    into: &mut ChunkPayload,
) -> Result<(), StoreError> {
    let item = StoredItem::Chunk(key, revision);
    open_envelope(bytes, CHUNK_MAGIC, item, Some(CHUNK_BYTES))?;
    let corrupt = |detail| StoreError::Corrupt { item, detail };
    let mut r = Reader::new(bytes, ENVELOPE);
    let stored_key = ChunkKey::new(r.i32(), r.i32(), r.i32());
    let zero = r.u32();
    let stored_revision = ChunkRevision::new(r.u64());
    if stored_key != key || stored_revision != revision {
        return Err(corrupt("the row names another key or revision"));
    }
    let theme = r.u8();
    let padding = r.take::<3>();
    let count = r.u32();
    if zero != 0 || padding != [0; 3] || count as usize != CHUNK_CELL_COUNT {
        return Err(corrupt("nonzero reserved bytes or a wrong cell count"));
    }
    for cell in into.cells_mut() {
        *cell = WorldCell(r.u64());
    }
    into.set_theme(ChunkTheme(theme));
    Ok(())
}

/// A key's published revision and the checkpoint that published it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Head {
    pub revision: ChunkRevision,
    pub checkpoint: CheckpointId,
}

pub fn encode_head(head: Head) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEAD_BYTES);
    bytes.extend_from_slice(&head.revision.value().to_le_bytes());
    bytes.extend_from_slice(&head.checkpoint.value().to_le_bytes());
    let checksum = crc32c(&bytes);
    bytes.extend_from_slice(&checksum.to_le_bytes());
    bytes
}

pub fn decode_head(bytes: &[u8], key: ChunkKey) -> Result<Head, StoreError> {
    let corrupt = |detail| StoreError::Corrupt {
        item: StoredItem::Head(key),
        detail,
    };
    if bytes.len() != HEAD_BYTES {
        return Err(corrupt("wrong length"));
    }
    let (content, stored) = bytes.split_at(HEAD_BYTES - CHECKSUM);
    if crc32c(content) != u32::from_le_bytes([stored[0], stored[1], stored[2], stored[3]]) {
        return Err(corrupt("checksum mismatch"));
    }
    let mut r = Reader::new(bytes, 0);
    let revision = ChunkRevision::new(r.u64());
    let checkpoint = CheckpointId::new(r.u64());
    if revision.value() == 0 || checkpoint.value() == 0 {
        return Err(corrupt("a zero revision or checkpoint"));
    }
    Ok(Head {
        revision,
        checkpoint,
    })
}

/// The row length a body set of `slots` slots and `bodies` bodies has.
pub fn body_set_bytes(slots: usize, bodies: usize) -> usize {
    ENVELOPE + 16 + slots * 8 + bodies * BODY_BYTES + CHECKSUM
}

pub fn encode_bodies(checkpoint: CheckpointId, set: &BodySet) -> Vec<u8> {
    let mut w = Writer::with_envelope(
        BODIES_MAGIC,
        body_set_bytes(set.slots.len(), set.bodies.len()),
    );
    w.u64(checkpoint.value());
    w.u32(set.slots.len() as u32);
    w.u32(set.bodies.len() as u32);
    for slot in &set.slots {
        w.u32(slot.generation);
        w.u8(match slot.state {
            SlotState::Free => 0,
            SlotState::Live => 1,
            SlotState::Retired => 2,
        });
        w.zeros(3);
    }
    for body in &set.bodies {
        w.u32(body.handle.index());
        w.u32(body.handle.generation());
        w.f32(body.description.mass());
        w.f32(body.description.radius());
        w.f32(body.description.relative_density());
        w.i32(body.anchor.x);
        w.i32(body.anchor.y);
        for value in body.local.to_array() {
            w.f32(value);
        }
        for value in body.velocity.to_array() {
            w.f32(value);
        }
        w.u32(
            u32::from(body.grounded)
                | u32::from(body.suspended) << 1
                | u32::from(body.plane_valid) << 2
                | u32::from(body.terrain_contact) << 3,
        );
        for value in body.ground_normal.to_array() {
            w.f32(value);
        }
        w.f32(body.submerged);
        w.f32(body.previous_submerged);
        w.f32(body.pending_wake);
        w.f32(body.plane.origin.x);
        w.f32(body.plane.origin.y);
        w.f32(body.plane.height);
        w.f32(body.plane.slope.x);
        w.f32(body.plane.slope.y);
        let c = body.counters;
        for value in [
            c.landings,
            c.liquid_entries,
            c.body_contacts,
            c.craters,
            c.splashes,
            c.absorbed_energy,
        ] {
            w.u32(value);
        }
        w.u32(body.touching);
        w.u32(u32::from(body.liquid.0));
        w.u32(body.terrain_normal);
    }
    w.finish()
}

/// Decodes the body set of `checkpoint`, checking its declared counts against the row's length
/// before allocating, and its values against the supported envelope.
pub fn decode_bodies(bytes: &[u8], checkpoint: CheckpointId) -> Result<(BodySet, u32), StoreError> {
    let item = StoredItem::Bodies(checkpoint);
    let checksum = open_envelope(bytes, BODIES_MAGIC, item, None)?;
    let corrupt = |detail| StoreError::Corrupt { item, detail };
    if bytes.len() < body_set_bytes(0, 0) {
        return Err(corrupt("shorter than an empty body set"));
    }
    let mut r = Reader::new(bytes, ENVELOPE);
    let stored = CheckpointId::new(r.u64());
    let slots = r.u32() as usize;
    let count = r.u32() as usize;
    if stored != checkpoint {
        return Err(corrupt("the row names another checkpoint"));
    }
    if slots > world_model::body::MAX_BODIES as usize || count > slots {
        return Err(corrupt("more slots or bodies than a world holds"));
    }
    if bytes.len() != body_set_bytes(slots, count) {
        return Err(corrupt("length disagrees with the declared counts"));
    }
    let mut set = BodySet {
        slots: Vec::with_capacity(slots),
        bodies: Vec::with_capacity(count),
    };
    for _ in 0..slots {
        let generation = r.u32();
        let state = match r.u8() {
            0 => SlotState::Free,
            1 => SlotState::Live,
            2 => SlotState::Retired,
            _ => return Err(corrupt("unknown slot state")),
        };
        if r.take::<3>() != [0; 3] {
            return Err(corrupt("nonzero reserved bytes"));
        }
        set.slots.push(BodySlot { generation, state });
    }
    for _ in 0..count {
        let handle = BodyHandle::new(r.u32(), r.u32());
        let description = SphereDescription::new(r.f32(), r.f32(), r.f32())
            .map_err(|_| corrupt("a body description outside the supported envelope"))?;
        let anchor = IVec2::new(r.i32(), r.i32());
        let local = Vec3::new(r.f32(), r.f32(), r.f32());
        let velocity = Vec3::new(r.f32(), r.f32(), r.f32());
        let flags = r.u32();
        if flags & !0b1111 != 0 {
            return Err(corrupt("unknown body flags"));
        }
        let ground_normal = Vec3::new(r.f32(), r.f32(), r.f32());
        let submerged = r.f32();
        let previous_submerged = r.f32();
        let pending_wake = r.f32();
        let plane = GroundPlane {
            origin: Vec2::new(r.f32(), r.f32()),
            height: r.f32(),
            slope: Vec2::new(r.f32(), r.f32()),
        };
        let counters = BodyCounters {
            landings: r.u32(),
            liquid_entries: r.u32(),
            body_contacts: r.u32(),
            craters: r.u32(),
            splashes: r.u32(),
            absorbed_energy: r.u32(),
        };
        let touching = r.u32();
        let liquid = r.u32();
        let terrain_normal = r.u32();
        let liquid = u8::try_from(liquid).map_err(|_| corrupt("unknown liquid"))?;
        set.bodies.push(BodyResume {
            handle,
            description,
            anchor,
            local,
            velocity,
            grounded: flags & 1 != 0,
            suspended: flags & 2 != 0,
            plane_valid: flags & 4 != 0,
            terrain_contact: flags & 8 != 0,
            ground_normal,
            submerged,
            previous_submerged,
            pending_wake,
            plane,
            counters,
            touching,
            liquid: FluidMat(liquid),
            terrain_normal,
        });
    }
    set.validate(world_model::body::MAX_BODIES)
        .map_err(|_| corrupt("the body set is inconsistent or outside the envelope"))?;
    Ok((set, checksum))
}

/// The checksum of an encoded row: its last four bytes.
pub fn row_checksum(bytes: &[u8]) -> u32 {
    let tail = &bytes[bytes.len() - CHECKSUM..];
    u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]])
}

/// What a manifest row adds to the semantic manifest.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ManifestRow {
    pub manifest: CheckpointManifest,
    /// Keys whose head this checkpoint moved.
    pub moved: u32,
    /// The checksum of the checkpoint's body set row.
    pub bodies_checksum: u32,
}

pub fn encode_manifest(row: &ManifestRow) -> Vec<u8> {
    let manifest = &row.manifest;
    let mut w = Writer::with_envelope(MANIFEST_MAGIC, MANIFEST_BYTES);
    w.u64(manifest.id.value());
    w.u8(u8::from(manifest.tick.is_some()));
    w.zeros(7);
    w.u64(manifest.tick.map_or(0, Tick::value));
    w.u32(manifest.config.rules.bits());
    w.f32(manifest.config.ambient_temperature.degrees());
    w.f32(manifest.config.elevation_scale.value());
    w.u32(0);
    let center = manifest.window.center();
    w.i32(center.key.x);
    w.i32(center.key.y);
    w.u32(manifest.window.radius_x());
    w.u32(manifest.window.radius_y());
    w.u64(manifest.chunks);
    w.u32(manifest.bodies);
    w.u32(manifest.epoch.value());
    w.u32(row.moved);
    w.u32(row.bodies_checksum);
    w.finish()
}

pub fn decode_manifest(bytes: &[u8], id: CheckpointId) -> Result<ManifestRow, StoreError> {
    let item = StoredItem::Manifest(id);
    open_envelope(bytes, MANIFEST_MAGIC, item, Some(MANIFEST_BYTES))?;
    let corrupt = |detail| StoreError::Corrupt { item, detail };
    let mut r = Reader::new(bytes, ENVELOPE);
    let stored = CheckpointId::new(r.u64());
    let has_tick = r.u8();
    let padding = r.take::<7>();
    let tick = r.u64();
    let rules = r.u32();
    let ambient = r.f32();
    let scale = r.f32();
    let zero = r.u32();
    let center = ChunkKey::new(r.i32(), r.i32(), 0);
    let (radius_x, radius_y) = (r.u32(), r.u32());
    let chunks = r.u64();
    let bodies = r.u32();
    let epoch = r.u32();
    let moved = r.u32();
    let bodies_checksum = r.u32();
    if stored != id {
        return Err(corrupt("the row names another checkpoint"));
    }
    if padding != [0; 7] || zero != 0 || has_tick > 1 || (has_tick == 0 && tick != 0) {
        return Err(corrupt("nonzero reserved bytes"));
    }
    let config = PhysicalConfig {
        rules: SimulationRules::from_bits(rules).ok_or(corrupt("unknown rule bits"))?,
        ambient_temperature: AmbientTemperature::celsius(ambient)
            .map_err(|_| corrupt("ambient temperature outside the supported range"))?,
        elevation_scale: ElevationScale::new(scale)
            .map_err(|_| corrupt("elevation scale outside the supported range"))?,
    };
    let window = ResidentWindow::new(center, radius_x, radius_y)
        .map_err(|_| corrupt("an unsupported resident window"))?;
    Ok(ManifestRow {
        manifest: CheckpointManifest {
            id,
            tick: (has_tick == 1).then_some(Tick::new(tick)),
            config,
            window,
            chunks,
            bodies,
            epoch: SessionEpoch::new(epoch),
        },
        moved,
        bodies_checksum,
    })
}

pub fn encode_latest(id: CheckpointId) -> Vec<u8> {
    let mut w = Writer::with_envelope(LATEST_MAGIC, LATEST_BYTES);
    w.u64(id.value());
    w.finish()
}

pub fn decode_latest(bytes: &[u8]) -> Result<CheckpointId, StoreError> {
    open_envelope(bytes, LATEST_MAGIC, StoredItem::Current, Some(LATEST_BYTES))?;
    let id = CheckpointId::new(Reader::new(bytes, ENVELOPE).u64());
    if id.value() == 0 {
        return Err(StoreError::Corrupt {
            item: StoredItem::Current,
            detail: "checkpoint 0",
        });
    }
    Ok(id)
}

/// Whether the session that last opened a store for writing closed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionMarker {
    Open,
    Closed,
}

pub fn encode_session(marker: SessionMarker) -> Vec<u8> {
    let mut w = Writer::with_envelope(SESSION_MAGIC, SESSION_BYTES);
    w.u32(match marker {
        SessionMarker::Open => 1,
        SessionMarker::Closed => 2,
    });
    w.u32(0);
    w.finish()
}

pub fn decode_session(bytes: &[u8]) -> Result<SessionMarker, StoreError> {
    open_envelope(
        bytes,
        SESSION_MAGIC,
        StoredItem::Header,
        Some(SESSION_BYTES),
    )?;
    let mut r = Reader::new(bytes, ENVELOPE);
    match (r.u32(), r.u32()) {
        (1, 0) => Ok(SessionMarker::Open),
        (2, 0) => Ok(SessionMarker::Closed),
        _ => Err(StoreError::Corrupt {
            item: StoredItem::Header,
            detail: "unknown session marker",
        }),
    }
}

#[cfg(test)]
mod tests;
