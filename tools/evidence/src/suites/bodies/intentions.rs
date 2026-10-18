//! The baseline's sphere tests (`monarch-engine` `core::entities::spherical::tests`), run on the
//! GPU body solver against the same ground, starts, controls and tolerances.
//!
//! A baseline frame is one 60 Hz tick here: both split it into two substeps of 1/120 s. The
//! baseline tests change velocity directly between frames; here every change is a command:
//! the controls' drive, or a weaker drive where a test pushed less (the hill climb's half
//! push, the wake's 8 units per second). The ground is the tests' 64 × 64 grid as the centre
//! chunk of a 3 × 3 window, continued outside it, so the residency edge stays out of reach.

use world_model::{
    body::{BodyPreset, DriveStrength, SphereDescription},
    cell::WorldCell,
    command::{Command, Direction, Drive},
    glam::{IVec2, Vec3},
    material::{FluidMat, GranularMat, TerrainMat},
};

use super::{
    super::Check,
    scene::{Scene, Setup, scale},
};

const RADIUS: f32 = 4.0;

fn metal() -> SphereDescription {
    BodyPreset::Metal.description()
}

fn wood() -> SphereDescription {
    BodyPreset::Wood.description()
}

/// The controls' drive along +x.
fn drive_east(scene: &Scene, strength: DriveStrength) -> Vec<Command> {
    let direction = Direction::new(1.0, 0.0).expect("unit");
    scene
        .bodies
        .iter()
        .map(|body| Command::DriveBody {
            body: *body,
            drive: Drive::with(direction, strength),
        })
        .collect()
}

/// `resting_sphere_settles_into_soft_ground_without_drifting`.
pub fn resting_soft_ground(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), |_, cell| cell.set_elevation(40))?;
    let body = scene.spawn(metal(), 32.3, -32.6, RADIUS)?;
    let start = scene.position(body)?;
    check.that((start.y - (20.0 + RADIUS)).abs() < 1e-4, || {
        format!("spawned at {start}, expected the surface plus the radius")
    });
    scene.run(180)?;
    let end = scene.position(body)?;
    let sample = scene.sample(body)?;
    check.that(sample.grounded, || {
        "not grounded after 180 ticks".to_owned()
    });
    let drift = Vec3::new(end.x - start.x, 0.0, end.z - start.z).length();
    check.that(drift < 0.05, || format!("drifted {drift}"));
    let sinkage = start.y - end.y;
    check.that((0.0..=1.0 + 0.25).contains(&sinkage), || {
        format!("sank {sinkage} into soil")
    });
    check.metric("drift", f64::from(drift));
    check.metric("sinkage", f64::from(sinkage));
    scene.finish(check, "rest");
    Ok(())
}

/// `sphere_rolls_down_a_slope`.
pub fn slope_roll(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), |cell, column| {
        column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
        column.set_elevation((120 - cell.x).clamp(0, 4095) as u16);
    })?;
    let before = scene.totals()?;
    let body = scene.spawn(metal(), 16.0, -32.0, RADIUS * 1.15)?;
    let start = scene.position(body)?;
    scene.run(90)?;
    let end = scene.position(body)?;
    check.that(scene.sample(body)?.grounded, || "not grounded".to_owned());
    check.that(end.x > start.x + 3.0, || format!("stayed at {end}"));
    check.that((end.z - start.z).abs() < 0.5, || format!("veered to {end}"));
    let after = scene.totals()?;
    check.equal(
        "elevation (rolling must not dig)",
        after.elevation,
        before.elevation,
    );
    check.equal("loose material", after.granular, before.granular);
    check.metric("rolled", f64::from(end.x - start.x));
    scene.finish(check, "slope");
    Ok(())
}

/// One-unit noise per cell: SplitMix64 of the cell, 0 or 1 (the baseline drew the same range
/// from a seeded SmallRng; any rough, fixed noise serves the intention).
fn bump(cell: IVec2) -> u16 {
    let mut z = (cell.x as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((cell.y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
        .wrapping_add(0xB0B);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    ((z ^ (z >> 31)) & 1) as u16
}

/// `driven_sphere_crosses_bumpy_ground_without_hopping`.
pub fn bumpy_drive(adapter: &str, check: &mut Check) -> Result<(), String> {
    const BASE: u16 = 40;
    const RELIEF: u16 = 2;
    let mut scene = Scene::open(adapter, Setup::default(), |cell, column| {
        column.set_elevation(BASE + bump(cell));
    })?;
    // Start at the noise's top plus the radius, as the baseline does.
    let floor = f32::from(BASE + bump(IVec2::new(8, 32))) * scale();
    let top = f32::from(BASE + RELIEF) * scale();
    let body = scene.spawn(metal(), 8.0, -32.0, top - floor + RADIUS)?;
    scene.run(30)?;
    let finish_line = 52.0;
    let mut ticks = 0;
    let mut grounded = 0;
    while scene.position(body)?.x < finish_line && ticks < 120 {
        let drive = drive_east(&scene, DriveStrength::CONTROLS);
        scene.step(&drive)?;
        ticks += 1;
        grounded += usize::from(scene.sample(body)?.grounded);
    }
    let end = scene.position(body)?;
    check.that(end.x >= finish_line, || {
        format!("stalled at {end} after {ticks} ticks")
    });
    check.that(grounded * 10 >= ticks * 9, || {
        format!("airborne for {} of {ticks} ticks", ticks - grounded)
    });
    check.metric("ticks", ticks as u64);
    check.metric("grounded_ticks", grounded as u64);
    scene.finish(check, "bumpy");
    Ok(())
}

/// `full_speed_on_flat_rock_never_leaves_the_ground`.
pub fn flat_rock_full_speed(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), |_, column| {
        column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
        column.set_elevation(40);
    })?;
    let body = scene.spawn(metal(), 4.0, -32.0, RADIUS)?;
    let mut ticks = 0;
    let mut worst_kick = 0.0f32;
    let mut hops = 0;
    while scene.position(body)?.x < 56.0 && ticks < 600 {
        let drive = drive_east(&scene, DriveStrength::CONTROLS);
        scene.step(&drive)?;
        ticks += 1;
        let sample = scene.sample(body)?;
        hops += usize::from(!sample.grounded);
        worst_kick = worst_kick.max(sample.velocity.y.abs());
    }
    check.that(scene.position(body)?.x >= 56.0, || {
        format!("reached only {}", scene.position(body).unwrap_or_default())
    });
    check.equal("ticks not grounded", hops, 0);
    check.that(worst_kick < 0.05, || format!("vertical kick {worst_kick}"));
    check.metric("ticks", ticks as u64);
    check.metric("worst_vertical_speed", f64::from(worst_kick));
    scene.finish(check, "flat_rock");
    Ok(())
}

/// `rolling_leaves_a_bounded_trail_with_conserved_berms`.
pub fn rut_conserved(adapter: &str, check: &mut Check) -> Result<(), String> {
    const BED: u16 = 40;
    const SAND: u16 = 4;
    let mut scene = Scene::open(adapter, Setup::default(), |_, column| {
        column.set_elevation(BED);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(SAND);
    })?;
    let before = scene.totals()?;
    let body = scene.spawn(metal(), 8.0, -32.0, RADIUS)?;
    let mut ticks = 0;
    while scene.position(body)?.x < 48.0 && ticks < 600 {
        let drive = drive_east(&scene, DriveStrength::CONTROLS);
        scene.step(&drive)?;
        ticks += 1;
    }
    let after = scene.totals()?;
    check.equal("sand volume", after.granular, before.granular);
    check.equal("bedrock", after.elevation, before.elevation);
    let cells = scene.capture_all()?;
    let center = &cells[&(0, 0)];
    let top = |x: usize, y: usize| center[y * 64 + x].solid_top();
    let surface = u32::from(BED + SAND);
    let trail = (16..40)
        .map(|x| surface - top(x, 32).min(surface))
        .max()
        .unwrap_or(0);
    let deepest = center.iter().map(WorldCell::solid_top).min().unwrap_or(0);
    let softness_units = (1.5 / scale()).ceil() as u32;
    check.that(trail > 0, || "rolling over sand left no trail".to_owned());
    check.that(surface - deepest.min(surface) <= softness_units, || {
        format!("trail dug to {deepest}")
    });
    check.metric("trail_depth", u64::from(trail));
    check.metric("ticks", ticks as u64);
    scene.finish(check, "rut");
    Ok(())
}

/// `driven_sphere_climbs_a_hill`.
pub fn hill_climb(adapter: &str, check: &mut Check) -> Result<(), String> {
    const LOW: i32 = 40;
    const RAMP_START: i32 = 20;
    const RAMP_END: i32 = 36;
    let height_at = |x: i32| (LOW + (x - RAMP_START).clamp(0, RAMP_END - RAMP_START)) as u16;
    let mut scene = Scene::open(adapter, Setup::default(), |cell, column| {
        column.set_elevation(height_at(cell.x));
    })?;
    let body = scene.spawn(metal(), 8.0, -32.0, RADIUS)?;
    let plateau = f32::from(height_at(RAMP_END)) * scale();
    // The baseline pushed half the drive's acceleration each frame.
    let push = DriveStrength::new(world_model::body::DRIVE_ACCELERATION * 0.5, 40.0)
        .map_err(|error| error.to_string())?;
    let mut reached = None;
    for tick in 0..180 {
        let now = scene.position(body)?;
        if now.x > (RAMP_END + 4) as f32 && now.y > plateau {
            reached = Some(tick);
            break;
        }
        let drive = drive_east(&scene, push);
        scene.step(&drive)?;
    }
    check.that(reached.is_some(), || {
        format!("stuck at {}", scene.position(body).unwrap_or_default())
    });
    check.metric("ticks_to_plateau", reached.unwrap_or(180));
    scene.finish(check, "hill");
    Ok(())
}

/// `hard_landing_craters_and_conserves_sand`.
pub fn crater_conserved(adapter: &str, check: &mut Check) -> Result<(), String> {
    const SAND: u16 = 6;
    let mut scene = Scene::open(adapter, Setup::default(), |_, column| {
        column.set_elevation(40);
        column.set_granular_mat(GranularMat::GRANULAR_SAND);
        column.set_granular_vol(SAND);
    })?;
    let before = scene.totals()?;
    let body = scene.spawn(metal(), 32.0, -32.0, 30.0)?;
    scene.run(120)?;
    check.that(scene.sample(body)?.grounded, || "not grounded".to_owned());
    let after = scene.totals()?;
    check.equal("sand volume", after.granular, before.granular);
    let floor = scene.column(IVec2::new(32, 32))?;
    check.that(floor.granular_vol() < SAND, || {
        format!("no crater under the impact: {} units", floor.granular_vol())
    });
    // A rebound can land hard enough to crater again, as in the baseline; at least the first
    // landing must.
    let counters = scene.sample(body)?.counters;
    check.that(counters.craters >= 1, || {
        "the landing made no crater".to_owned()
    });
    check.metric("craters", u64::from(counters.craters));
    check.metric("crater_floor_units", u64::from(floor.granular_vol()));
    check.metric(
        "absorbed_energy_sixteenths",
        u64::from(counters.absorbed_energy),
    );
    scene.finish(check, "crater");
    Ok(())
}

/// `light_sphere_floats_and_dense_sphere_sinks`.
pub fn float_and_sink(adapter: &str, check: &mut Check) -> Result<(), String> {
    const BED: u16 = 20;
    const WATER: u16 = 100;
    let mut scene = Scene::open(adapter, Setup::default(), |_, column| {
        column.set_elevation(BED);
        column.set_fluid_mat(FluidMat::FLUID_WATER);
        column.set_fluid_vol(WATER);
    })?;
    let floor = f32::from(BED) * scale();
    let surface = f32::from(BED + WATER) * scale();
    let wood_body = scene.spawn(wood(), 16.0, -32.0, surface + 1.0 - floor)?;
    let steel = scene.spawn(metal(), 48.0, -32.0, surface + 1.0 - floor)?;
    scene.run(600)?;
    let floating = scene.sample(wood_body)?;
    check.that(!floating.grounded, || "wood is grounded".to_owned());
    let density = wood().relative_density();
    check.that((floating.submerged_fraction - density).abs() < 0.05, || {
        format!(
            "floating equilibrium displaces its own weight, got {}",
            floating.submerged_fraction
        )
    });
    let sunk = scene.sample(steel)?;
    check.that(sunk.grounded, || "steel is not on the bed".to_owned());
    check.that(sunk.position.y as f32 <= floor + RADIUS + 0.1, || {
        format!("steel rests at {}", sunk.position.y)
    });
    check.metric("wood_submerged", f64::from(floating.submerged_fraction));
    scene.finish(check, "float");
    Ok(())
}

/// `wake_scales_with_how_much_of_the_sphere_cuts_the_surface`.
pub fn wake_scaling(adapter: &str, check: &mut Check) -> Result<(), String> {
    const BED: u16 = 20;
    const DEPTH: u16 = 100;
    let floor = f32::from(BED) * scale();
    let surface = f32::from(BED + DEPTH) * scale();
    let stirred = |relative_density: f32, check: &mut Check| -> Result<u64, String> {
        let mut scene = Scene::open(adapter, Setup::default(), |_, column| {
            column.set_elevation(BED);
            column.set_fluid_mat(FluidMat::FLUID_WATER);
            column.set_fluid_vol(DEPTH);
        })?;
        let sphere = SphereDescription::new(4.0, RADIUS, relative_density)
            .map_err(|error| error.to_string())?;
        scene.spawn(sphere, 12.0, -32.0, surface - floor)?;
        scene.run(240)?;
        let eight = DriveStrength::new(world_model::body::DRIVE_ACCELERATION, 8.0)
            .map_err(|error| error.to_string())?;
        for _ in 0..60 {
            let drive = drive_east(&scene, eight);
            scene.step(&drive)?;
        }
        let cells = scene.capture_all()?;
        scene.finish(check, &format!("wake_{relative_density}"));
        Ok(cells
            .values()
            .flatten()
            .map(|cell| u64::from(cell.fluid_vol().abs_diff(DEPTH)))
            .sum())
    };
    let barely_covered = stirred(0.97, check)?;
    let half_out = stirred(0.5, check)?;
    check.that(half_out > 0, || {
        "a sphere ploughing the surface raised no wave".to_owned()
    });
    check.that(barely_covered * 3 < half_out, || {
        format!("nearly submerged stirred {barely_covered} units vs {half_out} at the surface")
    });
    check.metric("stirred_barely_covered", barely_covered);
    check.metric("stirred_half_out", half_out);
    Ok(())
}

/// `plunging_sphere_splashes_without_losing_water`.
pub fn splash_conserved(adapter: &str, check: &mut Check) -> Result<(), String> {
    const BED: u16 = 20;
    const DEPTH: u16 = 40;
    let mut scene = Scene::open(adapter, Setup::default(), |_, column| {
        column.set_elevation(BED);
        column.set_fluid_mat(FluidMat::FLUID_WATER);
        column.set_fluid_vol(DEPTH);
    })?;
    let floor = f32::from(BED) * scale();
    let surface = f32::from(BED + DEPTH) * scale();
    let water = scene.totals()?.fluid_volume();
    scene.spawn(metal(), 32.0, -32.0, surface + 20.0 - floor)?;
    let impact = IVec2::new(32, 32);
    let mut deepest = DEPTH;
    let mut changed_ticks = 0;
    for _ in 0..90 {
        // Both read the end of the previous tick, after its body substeps.
        let results = scene.step(&[
            Command::MeasureTotals,
            Command::QueryColumn { cell: impact },
        ])?;
        if let world_runtime::port::CommandResult::Totals(totals) = results[0] {
            changed_ticks += u32::from(totals.fluid_volume() != water);
        }
        if let world_runtime::port::CommandResult::Column { column, .. } = results[1] {
            deepest = deepest.min(column.fluid_vol());
        }
    }
    check.equal("ticks whose water total changed", changed_ticks, 0);
    check.that(deepest < DEPTH / 2, || {
        format!("the splash left the surface undisturbed (deepest cavity {deepest})")
    });
    check.metric("deepest_cavity", u64::from(deepest));
    scene.finish(check, "splash");
    Ok(())
}
