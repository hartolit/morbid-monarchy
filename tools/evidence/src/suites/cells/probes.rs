//! Rule probes: the WGSL column rules, hashes, shuffles and material tables, executed on the device
//! against world-model on boundary and arbitrary inputs. The baseline's five cell-test intentions
//! (inflow absorbed, larger inflow takes over, quench either way round, ice keeps volume,
//! draining clears) are among the inputs, as are the 03 ceiling cases.

use gpu_world::shader::{Assembly, SourceModule, rule_modules, shared_modules};
use world_model::{
    cell::WorldCell,
    glam::IVec2,
    material::{
        CompassFlags, FluidMat, GranularMat, SurfaceMat, TerrainMat, fluid_properties,
        granular_repose, is_combustible,
    },
    rules::{
        CARDINAL_DIRS, FlowPattern, NEIGHBOR_DIRS, RuleTick, ShuffledDirs, draw, fmix32,
        keyed_spatial_hash, keyed_spatial_hash_extended,
    },
};

use super::super::{
    Check,
    driver::{Driver, config, pattern},
    probe::{self, Binding},
};

const COLUMN_OUT_WORDS: usize = 15;

const COLUMN_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read> probe_inputs: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read_write> probe_out: array<u32>;
@group(0) @binding(2) var<uniform> probe_count: vec4<u32>;

fn put(base: u32, slot: u32, c: Changed) {
    probe_out[base + slot * 3u] = c.cell.x;
    probe_out[base + slot * 3u + 1u] = c.cell.y;
    probe_out[base + slot * 3u + 2u] = c.units;
}

@compute @workgroup_size(64, 1, 1)
fn probe_column(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= probe_count.x) {
        return;
    }
    let input = probe_inputs[gid.x];
    let c = input.xy;
    let base = gid.x * 15u;
    put(base, 0u, receive_fluid(c, input.z, input.w));
    put(base, 1u, quench(c));
    put(base, 2u, drain_fluid(c, input.w));
    put(base, 3u, Changed(freeze(c), 0u));
    put(base, 4u, Changed(with_granular_volume(c, input.w % 32u), thaw(c).y ^ thaw(c).x));
}
"#;

fn assembly(name: &'static str, path: &'static str, text: &'static str) -> Assembly {
    let mut modules = shared_modules();
    modules.extend(rule_modules());
    modules.push(SourceModule::file(path, text));
    Assembly::new(name, &modules)
}

/// What world-model says for one probe input, in the probe's output words.
fn column_expected(cell: WorldCell, material: u8, amount: u16) -> [u32; COLUMN_OUT_WORDS] {
    let words = |cell: WorldCell| cell.to_words();
    let mut out = [0u32; COLUMN_OUT_WORDS];
    let mut received = cell;
    let accepted = received.receive_fluid(FluidMat(material), amount);
    let mut quenched = cell;
    let converted = quenched.quench();
    let mut drained = cell;
    let removed = drained.drain_fluid(amount);
    let mut frozen = cell;
    frozen.freeze();
    let mut thawed = cell;
    thawed.thaw();
    let mut compacted = cell;
    compacted.set_granular_vol(amount % 32);
    let thawed = words(thawed);
    let rows = [
        (words(received), u32::from(accepted)),
        (words(quenched), u32::from(converted)),
        (words(drained), u32::from(removed)),
        (words(frozen), 0),
        (words(compacted), thawed[1] ^ thawed[0]),
    ];
    for (slot, (cell, units)) in rows.into_iter().enumerate() {
        out[slot * 3] = cell[0];
        out[slot * 3 + 1] = cell[1];
        out[slot * 3 + 2] = units;
    }
    out
}

fn column(elevation: u16, material: FluidMat, volume: u16) -> WorldCell {
    let mut cell = WorldCell::default();
    cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
    cell.set_elevation(elevation);
    cell.set_fluid_mat(material);
    cell.set_fluid_vol(volume);
    cell.set_compass(CompassFlags::FACING_E);
    cell
}

pub fn column_rules_match(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let mut inputs: Vec<(WorldCell, u8, u16)> = Vec::new();
    // Every resident and incoming material, at the floor, below and at the ceiling, empty,
    // shallow, deep and full, with nothing, a little, a lot and a full column arriving.
    for elevation in [0, 100, 4080, 4093, 4095] {
        for resident in 0..8 {
            for volume in [0, 1, 4, 20, 40, 510, 511] {
                for incoming in 0..8 {
                    for amount in [0, 1, 6, 8, 10, 511] {
                        inputs.push((
                            column(elevation, FluidMat(resident), volume),
                            incoming,
                            amount,
                        ));
                    }
                }
            }
        }
    }
    // Loose layers that compact, and arbitrary words.
    for (index, cell) in pattern(0x03).into_iter().take(2048).enumerate() {
        inputs.push((cell, (index % 8) as u8, (index * 37 % 600) as u16));
    }
    let mut words = Vec::with_capacity(inputs.len() * 4);
    for (cell, material, amount) in &inputs {
        let [x, y] = cell.to_words();
        words.extend([x, y, u32::from(*material), u32::from(*amount)]);
    }
    let count = inputs.len() as u32;
    let outputs = probe::run(
        &mut driver.world,
        &assembly("column probe", "probe/column.wgsl", COLUMN_PROBE),
        "probe_column",
        &[
            Binding::Storage(&words),
            Binding::Output(count * COLUMN_OUT_WORDS as u32),
            Binding::Uniform(&[count, 0, 0, 0]),
        ],
        [count.div_ceil(64), 1, 1],
    )?;
    let names = ["receive", "quench", "drain", "freeze", "compact"];
    let mut mismatches = 0;
    for (index, (cell, material, amount)) in inputs.iter().enumerate() {
        let expected = column_expected(*cell, *material, *amount);
        let got = &outputs[0][index * COLUMN_OUT_WORDS..(index + 1) * COLUMN_OUT_WORDS];
        for slot in 0..5 {
            if got[slot * 3..slot * 3 + 3] != expected[slot * 3..slot * 3 + 3] {
                mismatches += 1;
                if mismatches <= 8 {
                    check.fail(format!(
                        "{} of {:#018x} with {material}x{amount}: gpu {:x?}, world-model {:x?}",
                        names[slot],
                        cell.0,
                        &got[slot * 3..slot * 3 + 3],
                        &expected[slot * 3..slot * 3 + 3]
                    ));
                }
            }
        }
    }
    check.equal("column rule mismatches", mismatches, 0);
    check.metric("column_rule_inputs", inputs.len() as u64);
    Ok(())
}

const HASH_OUT_WORDS: usize = 24;

const HASH_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read> probe_inputs: array<vec4<u32>>;
@group(0) @binding(1) var<storage, read_write> probe_out: array<u32>;
@group(0) @binding(2) var<uniform> probe_count: vec4<u32>;

@compute @workgroup_size(64, 1, 1)
fn probe_hash(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= probe_count.x) {
        return;
    }
    let a = probe_inputs[2u * gid.x];
    let b = probe_inputs[2u * gid.x + 1u];
    let pos = vec2<i32>(bitcast<i32>(a.x), bitcast<i32>(a.y));
    let base = gid.x * 24u;
    probe_out[base] = keyed_hash(pos, a.z, a.w);
    probe_out[base + 1u] = keyed_hash_extended(pos, a.z, a.w, b.x, b.y);
    probe_out[base + 2u] = keyed_draw(pos, a.z, a.w, b.w);
    probe_out[base + 3u] = fmix32(a.x);
    let omni = shuffled(false, pos, a.z, a.w, b.x, b.y);
    let cardinal = shuffled(true, pos, a.z, a.w, b.x, b.y);
    let led = with_momentum(omni, b.z);
    for (var i = 0u; i < 8u; i++) {
        probe_out[base + 4u + i] = omni.dirs[i];
        probe_out[base + 12u + i] = led.dirs[i];
    }
    for (var i = 0u; i < 4u; i++) {
        probe_out[base + 20u + i] = cardinal.dirs[i];
    }
}
"#;

fn direction_index(dir: IVec2) -> u32 {
    NEIGHBOR_DIRS
        .iter()
        .position(|&d| d == dir)
        .map_or(u32::MAX, |index| index as u32)
}

pub fn rule_hashes_match(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let mut inputs: Vec<(IVec2, u32, u32, u32, u32, u8, u32)> = Vec::new();
    let positions = [
        IVec2::new(0, 0),
        IVec2::new(-1, -1),
        IVec2::new(63, -64),
        IVec2::new(i32::MIN, i32::MAX),
        IVec2::new(i32::MAX, i32::MIN),
        IVec2::new(-12_345, 67_890),
    ];
    let mut next = super::intentions::mixer(0xA5);
    for (index, pos) in positions.iter().enumerate() {
        for tick in [
            0u64,
            1,
            2,
            600,
            u64::from(u32::MAX),
            1 << 32,
            (1 << 32) + 7,
            u64::MAX,
        ] {
            for seed in [0u64, 1, 0x03C0_FFEE, u64::MAX] {
                let rule = RuleTick::new(seed, tick);
                let material = ((index as u64 + tick) % 16) as u32;
                let flags = (next() % 16) as u8;
                let salt = [101, 102, 103, 104, 105, 201, 202, 203][(tick % 8) as usize];
                inputs.push((
                    *pos,
                    rule.tick,
                    rule.key,
                    material,
                    (tick % 9) as u32,
                    flags,
                    salt,
                ));
            }
        }
    }
    for _ in 0..1024 {
        let pos = IVec2::new(next() as i32, next() as i32);
        inputs.push((
            pos,
            next() as u32,
            next() as u32,
            (next() % 16) as u32,
            (next() % 9) as u32,
            (next() % 16) as u8,
            next() as u32,
        ));
    }
    let mut words = Vec::new();
    for (pos, tick, key, material, cadence, flags, salt) in &inputs {
        words.extend([pos.x as u32, pos.y as u32, *tick, *key]);
        words.extend([*material, *cadence, u32::from(*flags), *salt]);
    }
    let count = inputs.len() as u32;
    let outputs = probe::run(
        &mut driver.world,
        &assembly("hash probe", "probe/hash.wgsl", HASH_PROBE),
        "probe_hash",
        &[
            Binding::Storage(&words),
            Binding::Output(count * HASH_OUT_WORDS as u32),
            Binding::Uniform(&[count, 0, 0, 0]),
        ],
        [count.div_ceil(64), 1, 1],
    )?;
    let mut mismatches = 0;
    for (index, (pos, tick, key, material, cadence, flags, salt)) in inputs.iter().enumerate() {
        let rule = RuleTick {
            tick: *tick,
            key: *key,
        };
        let mut expected = vec![
            keyed_spatial_hash(*pos, rule),
            keyed_spatial_hash_extended(*pos, rule, *material as u8, *cadence),
            draw(*pos, rule, *salt),
            fmix32(pos.x as u32),
        ];
        let omni = ShuffledDirs::new(FlowPattern::Omni, *pos, rule, *material as u8, *cadence);
        let led = ShuffledDirs::with_momentum(
            FlowPattern::Omni,
            *pos,
            rule,
            *material as u8,
            *cadence,
            *flags,
        );
        let cardinal =
            ShuffledDirs::new(FlowPattern::Cardinal, *pos, rule, *material as u8, *cadence);
        expected.extend(omni.get().iter().map(|d| direction_index(*d)));
        expected.extend(led.get().iter().map(|d| direction_index(*d)));
        expected.extend(cardinal.get().iter().map(|d| direction_index(*d)));
        let got = &outputs[0][index * HASH_OUT_WORDS..(index + 1) * HASH_OUT_WORDS];
        if got != expected.as_slice() {
            mismatches += 1;
            if mismatches <= 8 {
                check.fail(format!(
                    "hashes at {pos} tick {tick} key {key:#x}: gpu {got:x?}, world-model {expected:x?}"
                ));
            }
        }
    }
    check.equal("hash and shuffle mismatches", mismatches, 0);
    check.that(
        CARDINAL_DIRS
            .iter()
            .all(|d| direction_index(*d).is_multiple_of(2)),
        || "cardinal steps are the even directions".to_owned(),
    );
    check.metric("hash_inputs", inputs.len() as u64);
    Ok(())
}

const TABLE_PROBE: &str = r#"
@group(0) @binding(0) var<storage, read_write> probe_out: array<u32>;

@compute @workgroup_size(16, 1, 1)
fn probe_tables(@builtin(local_invocation_index) m: u32) {
    let base = m * 12u;
    probe_out[base] = fluid_flow_divisor(m);
    probe_out[base + 1u] = u32(fluid_levels_out(m));
    probe_out[base + 2u] = fluid_erosion_factor(m);
    probe_out[base + 3u] = u32(fluid_is_solid(m));
    probe_out[base + 4u] = u32(fluid_is_coolant(m));
    probe_out[base + 5u] = fluid_frozen(m);
    probe_out[base + 6u] = fluid_melted(m);
    probe_out[base + 7u] = u32(fluid_is_fuel(m));
    probe_out[base + 8u] = u32(surface_is_combustible(m));
    probe_out[base + 9u] = granular_repose(m);
    probe_out[base + 10u] = granular_to_terrain(m);
    probe_out[base + 11u] = terrain_to_granular(m);
}
"#;

pub fn material_tables_match(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let outputs = probe::run(
        &mut driver.world,
        &assembly("table probe", "probe/tables.wgsl", TABLE_PROBE),
        "probe_tables",
        &[Binding::Output(16 * 12)],
        [1, 1, 1],
    )?;
    let none = 0xFF;
    for m in 0..16u8 {
        let fluid = FluidMat(m % 8);
        let props = fluid_properties(fluid);
        // Fluid codes have 3 bits; the tables answer 8..15 like the last code (never used).
        let fluid_row = [
            props.flow_divisor,
            u32::from(props.levels_out),
            props.erosion_factor,
            u32::from(fluid.is_solid()),
            u32::from(fluid.is_coolant()),
            fluid.frozen().map_or(none, |f| u32::from(f.0)),
            fluid.melted().map_or(none, |f| u32::from(f.0)),
            u32::from(is_combustible(SurfaceMat::EMPTY, fluid)),
        ];
        let row = &outputs[0][usize::from(m) * 12..usize::from(m) * 12 + 12];
        if m < 8 {
            check.equal(&format!("fluid tables of {m}"), &row[..8], &fluid_row[..]);
        }
        let rest = [
            u32::from(is_combustible(SurfaceMat(m), FluidMat::EMPTY)),
            u32::from(granular_repose(GranularMat(m))),
            u32::from(GranularMat(m).to_terrain().0),
            TerrainMat(m).to_granular().map_or(none, |g| u32::from(g.0)),
        ];
        check.equal(&format!("4-bit tables of {m}"), &row[8..], &rest[..]);
    }
    Ok(())
}
