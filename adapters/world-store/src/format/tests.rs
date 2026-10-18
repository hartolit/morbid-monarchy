//! The format's bytes against values written out by hand, and its refusals.

use super::*;
use world_model::{
    body::BodyPreset,
    command::{AmbientTemperature, ElevationScale},
    glam::{IVec2, Vec2, Vec3},
};

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("four bytes"))
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().expect("eight bytes"))
}

/// Rewrites a row's checksum after a test changed its bytes on purpose.
fn reseal(bytes: &mut [u8]) {
    let end = bytes.len() - 4;
    let checksum = crc32c(&bytes[..end]);
    bytes[end..].copy_from_slice(&checksum.to_le_bytes());
}

#[test]
fn crc32c_matches_the_published_check_value() {
    // The CRC-32C check value of the ASCII digits 1 to 9 (RFC 3720, appendix B.4).
    assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    assert_eq!(crc32c(b""), 0);
}

fn header() -> WorldHeader {
    WorldHeader {
        world: WorldId::new(0x0102_0304_0506_0708),
        generator: Generator {
            revision: identity::GENERATOR_REVISION,
            seed: 69,
        },
        rule_seed: 0xAABB_CCDD_EEFF_0011,
        origin: WorldOrigin::LegacyImport(LegacyInterpretation {
            seed: 69,
            elevation_scale: ElevationScale::new(0.75).expect("supported"),
            rows_imported: 4,
            rows_refused: 1,
        }),
    }
}

#[test]
fn header_bytes_sit_where_the_format_says() {
    let bytes = encode_header(&header());
    assert_eq!(bytes.len(), HEADER_BYTES);
    assert_eq!(&bytes[0..4], b"MMWH");
    assert_eq!(u16::from_le_bytes([bytes[4], bytes[5]]), 1);
    assert_eq!(le64(&bytes, 8), 0x0102_0304_0506_0708);
    assert_eq!(le32(&bytes, 16), identity::CELL_FORMAT_REVISION);
    assert_eq!(le32(&bytes, 20), identity::PHYSICS_REVISION);
    assert_eq!(le64(&bytes, 24), identity::cell_layout_fingerprint());
    assert_eq!(le64(&bytes, 32), identity::physics_fingerprint());
    assert_eq!(le32(&bytes, 44), 69);
    assert_eq!(le64(&bytes, 48), 0xAABB_CCDD_EEFF_0011);
    assert_eq!(u16::from_le_bytes([bytes[56], bytes[57]]), 64);
    assert_eq!(bytes[60], 1, "legacy import");
    assert_eq!(le32(&bytes, 68), 0.75f32.to_bits());
    assert_eq!(le64(&bytes, 72), 4);
    assert_eq!(le64(&bytes, 80), 1);
    assert_eq!(le32(&bytes, 88), crc32c(&bytes[..88]));
    assert_eq!(decode_header(&bytes), Ok(header()));
}

#[test]
fn a_header_of_another_layout_physics_or_version_is_refused_unchanged() {
    let bytes = encode_header(&header());
    let mut layout = bytes.clone();
    layout[24] ^= 1;
    reseal(&mut layout);
    assert!(matches!(
        decode_header(&layout),
        Err(StoreError::Unsupported {
            detail: Unsupported::CellLayout { .. },
            ..
        })
    ));
    let mut physics = bytes.clone();
    physics[20] = 2;
    reseal(&mut physics);
    assert!(matches!(
        decode_header(&physics),
        Err(StoreError::Unsupported {
            detail: Unsupported::Physics { .. },
            ..
        })
    ));
    let mut future = bytes.clone();
    future[4] = 2;
    reseal(&mut future);
    assert_eq!(
        decode_header(&future),
        Err(StoreError::Unsupported {
            item: StoredItem::Header,
            detail: Unsupported::FormatVersion {
                stored: 2,
                supported: 1
            }
        })
    );
    let mut flipped = bytes;
    flipped[50] ^= 0x10;
    assert_eq!(
        decode_header(&flipped),
        Err(StoreError::Corrupt {
            item: StoredItem::Header,
            detail: "checksum mismatch"
        }),
        "a flipped bit without a new checksum is corruption, not a different world"
    );
}

fn payload() -> ChunkPayload {
    let mut payload = ChunkPayload::zeroed();
    for (index, cell) in payload.cells_mut().iter_mut().enumerate() {
        *cell = WorldCell((index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    }
    payload.cells_mut()[1] = WorldCell(u64::MAX);
    payload.set_theme(ChunkTheme(7));
    payload
}

#[test]
fn chunk_bytes_sit_where_the_format_says() {
    let key = ChunkKey::new(-3, 7, 0);
    let revision = ChunkRevision::new(0x0000_0001_0000_0002);
    let bytes = encode_chunk(key, revision, &payload());
    assert_eq!(bytes.len(), 32_812);
    assert_eq!(&bytes[0..4], b"MMCK");
    assert_eq!(le32(&bytes, 8) as i32, -3);
    assert_eq!(le32(&bytes, 12), 7);
    assert_eq!(le32(&bytes, 16), 0);
    assert_eq!(le64(&bytes, 24), 0x0000_0001_0000_0002);
    assert_eq!(bytes[32], 7, "theme");
    assert_eq!(le32(&bytes, 36), 4096);
    assert_eq!(le64(&bytes, 40), 0, "cell 0");
    assert_eq!(le64(&bytes, 48), u64::MAX, "cell 1");
    assert_eq!(
        le64(&bytes, 40 + 8 * 4095),
        4095u64.wrapping_mul(0x9E37_79B9_7F4A_7C15)
    );
    let mut into = ChunkPayload::zeroed();
    decode_chunk(&bytes, key, revision, &mut into).expect("decodes");
    assert_eq!(into, payload());
}

#[test]
fn damaged_chunk_rows_are_corrupt_and_never_partially_read() {
    let key = ChunkKey::new(2, 1, 0);
    let revision = ChunkRevision::new(3);
    let bytes = encode_chunk(key, revision, &payload());
    let mut into = ChunkPayload::zeroed();
    let corrupt =
        |result: Result<(), StoreError>| matches!(result, Err(StoreError::Corrupt { .. }));
    assert!(corrupt(decode_chunk(
        &bytes[..bytes.len() / 2],
        key,
        revision,
        &mut into
    )));
    assert!(corrupt(decode_chunk(&[], key, revision, &mut into)));
    let mut flipped = bytes.clone();
    flipped[1000] ^= 4;
    assert!(corrupt(decode_chunk(&flipped, key, revision, &mut into)));
    assert!(
        corrupt(decode_chunk(
            &bytes,
            ChunkKey::new(2, 2, 0),
            revision,
            &mut into
        )),
        "a row under another key"
    );
    assert!(corrupt(decode_chunk(
        &bytes,
        key,
        ChunkRevision::new(4),
        &mut into
    )));
    let mut count = bytes.clone();
    count[36] = 1;
    reseal(&mut count);
    assert!(corrupt(decode_chunk(&count, key, revision, &mut into)));
    let mut longer = bytes.clone();
    longer.splice(40..40, [0u8; 8]);
    reseal(&mut longer);
    assert!(corrupt(decode_chunk(&longer, key, revision, &mut into)));
    let mut future = bytes;
    future[4] = 9;
    reseal(&mut future);
    assert!(matches!(
        decode_chunk(&future, key, revision, &mut into),
        Err(StoreError::Unsupported { .. })
    ));
    assert_eq!(into, ChunkPayload::zeroed(), "no refused row wrote a cell");
}

fn body(index: u32, generation: u32) -> BodyResume {
    BodyResume {
        handle: BodyHandle::new(index, generation),
        description: BodyPreset::Wood.description(),
        anchor: IVec2::new(-1, 2),
        local: Vec3::new(3.5, 60.0, 30.25),
        velocity: Vec3::new(-4.0, 0.5, 12.0),
        grounded: false,
        suspended: true,
        plane_valid: true,
        terrain_contact: false,
        ground_normal: Vec3::new(0.0, 1.0, 0.0),
        submerged: 0.5,
        previous_submerged: 0.25,
        pending_wake: 1.5,
        plane: GroundPlane {
            origin: Vec2::new(3.0, 30.0),
            height: 22.0,
            slope: Vec2::new(0.0, 0.125),
        },
        counters: BodyCounters {
            landings: 1,
            liquid_entries: 2,
            body_contacts: 3,
            craters: 4,
            splashes: 5,
            absorbed_energy: u32::MAX,
        },
        touching: 2,
        liquid: FluidMat::FLUID_WATER,
        terrain_normal: 0x1234_ABCD,
    }
}

fn body_set() -> BodySet {
    BodySet {
        slots: vec![
            BodySlot {
                generation: 9,
                state: SlotState::Retired,
            },
            BodySlot {
                generation: 3,
                state: SlotState::Live,
            },
            BodySlot {
                generation: 1,
                state: SlotState::Free,
            },
        ],
        bodies: vec![body(1, 3)],
    }
}

#[test]
fn body_sets_keep_every_field_and_their_lengths_are_checked_before_allocation() {
    let checkpoint = CheckpointId::new(12);
    let bytes = encode_bodies(checkpoint, &body_set());
    assert_eq!(bytes.len(), 28 + 8 * 3 + BODY_BYTES);
    assert_eq!(&bytes[0..4], b"MMBS");
    assert_eq!(le64(&bytes, 8), 12);
    assert_eq!((le32(&bytes, 16), le32(&bytes, 20)), (3, 1));
    assert_eq!((le32(&bytes, 24), bytes[28]), (9, 2), "slot 0 retired");
    let first = 24 + 8 * 3;
    assert_eq!((le32(&bytes, first), le32(&bytes, first + 4)), (1, 3));
    assert_eq!(le32(&bytes, first + 8), 0.31f32.to_bits(), "wood mass");
    assert_eq!(
        le32(&bytes, first + 52),
        0b0110,
        "suspended and plane valid"
    );
    assert_eq!(le32(&bytes, first + 120), u32::MAX, "absorbed energy");
    assert_eq!(le32(&bytes, first + 124), 2, "touching");
    assert_eq!(le32(&bytes, first + 128), 1, "water");
    assert_eq!(le32(&bytes, first + 132), 0x1234_ABCD, "terrain normal");
    let (decoded, checksum) = decode_bodies(&bytes, checkpoint).expect("decodes");
    assert_eq!(decoded, body_set());
    assert_eq!(checksum, row_checksum(&bytes));

    // A row claiming four thousand bodies in a few bytes is refused by its length, before
    // anything is allocated for its claim.
    let mut claims = bytes.clone();
    claims[20..24].copy_from_slice(&3u32.to_le_bytes());
    reseal(&mut claims);
    assert!(matches!(
        decode_bodies(&claims, checkpoint),
        Err(StoreError::Corrupt { .. })
    ));
    let mut huge = bytes.clone();
    huge[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    reseal(&mut huge);
    assert!(matches!(
        decode_bodies(&huge, checkpoint),
        Err(StoreError::Corrupt { .. })
    ));
    let mut nan = bytes.clone();
    nan[first + 32..first + 36].copy_from_slice(&f32::NAN.to_bits().to_le_bytes());
    reseal(&mut nan);
    assert!(matches!(
        decode_bodies(&nan, checkpoint),
        Err(StoreError::Corrupt { .. })
    ));
    assert!(matches!(
        decode_bodies(&bytes, CheckpointId::new(11)),
        Err(StoreError::Corrupt { .. })
    ));
}

#[test]
fn manifests_round_trip_and_refuse_values_outside_the_model() {
    let manifest = CheckpointManifest {
        id: CheckpointId::new(5),
        tick: Some(Tick::new(1 << 40)),
        config: PhysicalConfig {
            rules: SimulationRules::DETERMINISTIC,
            ambient_temperature: AmbientTemperature::celsius(-12.5).expect("supported"),
            elevation_scale: ElevationScale::new(1.25).expect("supported"),
        },
        window: ResidentWindow::new(ChunkKey::new(-4, 9, 0), 6, 3).expect("window"),
        chunks: 1234,
        bodies: 2,
        epoch: SessionEpoch::new(3),
    };
    let row = ManifestRow {
        manifest,
        moved: 17,
        bodies_checksum: 0xDEAD_BEEF,
    };
    let bytes = encode_manifest(&row);
    assert_eq!(bytes.len(), MANIFEST_BYTES);
    assert_eq!(&bytes[0..4], b"MMCP");
    assert_eq!(bytes[16], 1);
    assert_eq!(le64(&bytes, 24), 1 << 40);
    assert_eq!(le32(&bytes, 32), 0b00111);
    assert_eq!(le32(&bytes, 36), (-12.5f32).to_bits());
    assert_eq!(le32(&bytes, 48) as i32, -4);
    assert_eq!((le32(&bytes, 56), le32(&bytes, 60)), (6, 3));
    assert_eq!(le64(&bytes, 64), 1234);
    assert_eq!(le32(&bytes, 84), 0xDEAD_BEEF);
    assert_eq!(decode_manifest(&bytes, manifest.id), Ok(row));

    let mut hot = bytes.clone();
    hot[36..40].copy_from_slice(&900.0f32.to_bits().to_le_bytes());
    reseal(&mut hot);
    assert!(matches!(
        decode_manifest(&hot, manifest.id),
        Err(StoreError::Corrupt { .. })
    ));
    let mut rules = bytes.clone();
    rules[32] = 0xFF;
    reseal(&mut rules);
    assert!(matches!(
        decode_manifest(&rules, manifest.id),
        Err(StoreError::Corrupt { .. })
    ));
    let mut radius = bytes;
    radius[56] = 40;
    reseal(&mut radius);
    assert!(matches!(
        decode_manifest(&radius, manifest.id),
        Err(StoreError::Corrupt { .. })
    ));
}

#[test]
fn heads_pointers_and_markers_are_checked() {
    let key = ChunkKey::new(1, -1, 0);
    let head = Head {
        revision: ChunkRevision::new(4),
        checkpoint: CheckpointId::new(2),
    };
    let bytes = encode_head(head);
    assert_eq!(bytes.len(), HEAD_BYTES);
    assert_eq!(decode_head(&bytes, key), Ok(head));
    let mut flipped = bytes;
    flipped[0] ^= 1;
    assert!(decode_head(&flipped, key).is_err());
    let latest = encode_latest(CheckpointId::new(77));
    assert_eq!(decode_latest(&latest), Ok(CheckpointId::new(77)));
    assert!(decode_latest(&latest[..10]).is_err());
    for marker in [SessionMarker::Open, SessionMarker::Closed] {
        assert_eq!(decode_session(&encode_session(marker)), Ok(marker));
    }
}
