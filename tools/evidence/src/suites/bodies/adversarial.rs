//! Cases the baseline never tested: body pairs and clusters, coincident and fast bodies,
//! contested deformation with brush strokes on the same cells, edits under a resting body, and
//! the residency edge.

use gpu_world::world::{Broadphase, PairIterations};
use world_model::{
    body::{BodyHandle, BodyPreset, DriveStrength, SphereDescription},
    cell::WorldCell,
    command::{BodySpawn, Command, Direction, Drive, EditTool, MaterialEdit, Target},
    glam::IVec2,
    material::{FluidMat, GranularMat, TerrainMat},
    observation::BodySample,
    spatial::ChunkKey,
};
use world_runtime::port::CommandResult;

use super::{
    super::Check,
    scene::{Scene, Setup, chunk_cells, scale},
};

fn metal() -> SphereDescription {
    BodyPreset::Metal.description()
}

fn wood() -> SphereDescription {
    BodyPreset::Wood.description()
}

fn stone(_: IVec2, column: &mut WorldCell) {
    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
    column.set_elevation(40);
}

fn sand(_: IVec2, column: &mut WorldCell) {
    column.set_elevation(40);
    column.set_granular_mat(GranularMat::GRANULAR_SAND);
    column.set_granular_vol(4);
}

fn drive(body: BodyHandle, x: f32, y: f32, strength: DriveStrength) -> Command {
    Command::DriveBody {
        body,
        drive: Drive::with(Direction::new(x, y).expect("nonzero"), strength),
    }
}

/// The deepest overlap any two sampled bodies have, computed exhaustively on the CPU.
fn deepest_overlap(samples: &[BodySample], radii: &[f32]) -> f64 {
    let mut deepest = 0.0f64;
    for (i, a) in samples.iter().enumerate() {
        for (j, b) in samples.iter().enumerate().skip(i + 1) {
            let reach = f64::from(radii[i] + radii[j]);
            deepest = deepest.max(reach - a.position.distance(b.position));
        }
    }
    deepest
}

/// Two bodies meeting in mid-air: horizontal momentum is kept (up to the drag both share),
/// kinetic energy does not grow, and they part at the restitution of their approach.
pub fn pair_collision(adapter: &str, check: &mut Check) -> Result<(), String> {
    for (label, west, east) in [
        ("equal", metal(), metal()),
        ("metal and wood", metal(), wood()),
    ] {
        let mut scene = Scene::open(adapter, Setup::default(), stone)?;
        let a = scene.spawn(west, 20.0, -32.0, 400.0)?;
        let b = scene.spawn(east, 44.0, -32.0, 400.0)?;
        let strength = DriveStrength::new(250.0, 20.0).map_err(|error| error.to_string())?;
        for _ in 0..5 {
            scene.step(&[drive(a, 1.0, 0.0, strength), drive(b, -1.0, 0.0, strength)])?;
        }
        let momentum = |scene: &Scene| -> Result<(f64, f64, f64), String> {
            let (sa, sb) = (scene.sample(a)?, scene.sample(b)?);
            let p = f64::from(west.mass() * sa.velocity.x + east.mass() * sb.velocity.x);
            let energy = 0.5
                * f64::from(
                    west.mass() * sa.velocity.x.powi(2) + east.mass() * sb.velocity.x.powi(2),
                );
            Ok((p, energy, f64::from(sa.velocity.x - sb.velocity.x)))
        };
        let (before, energy_before, closing) = momentum(&scene)?;
        let mut ticks = 0u32;
        let mut contact = false;
        while ticks < 60 {
            scene.step(&[])?;
            ticks += 1;
            contact |= scene.sample(a)?.touching > 0;
            if contact && scene.sample(a)?.velocity.x < scene.sample(b)?.velocity.x {
                break;
            }
        }
        scene.run(2)?;
        ticks += 2;
        let (after, energy_after, parting) = momentum(&scene)?;
        let drag = (-f64::from(world_model::body::AIR_DRAG_RATE) * f64::from(ticks) / 60.0).exp();
        check.that(contact, || format!("{label}: they never touched"));
        check.that(
            (after - before * drag).abs() <= 1e-3 * before.abs().max(1.0),
            || format!("{label}: momentum {before} became {after} (drag {drag})"),
        );
        check.that(energy_after <= energy_before * drag * drag + 1e-3, || {
            format!("{label}: kinetic energy grew from {energy_before} to {energy_after}")
        });
        let ratio = -parting / (closing * drag);
        check.that((ratio - 0.2).abs() < 0.05, || {
            format!("{label}: parted at {ratio} of the approach, restitution 0.2")
        });
        check.metric(&format!("{}_restitution", label.replace(' ', "_")), ratio);
        scene.finish(check, &label.replace(' ', "_"));
    }
    Ok(())
}

/// Bodies spawned with exactly coincident centres separate along a defined axis without a
/// non-finite value, and come to rest without overlapping.
pub fn coincident_centres(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), stone)?;
    let spawn = Command::SpawnBody(
        BodySpawn::new(BodyPreset::Metal, IVec2::new(32, 32), 6.0).expect("valid"),
    );
    scene.step(&[spawn; 8])?;
    check.equal("bodies", scene.bodies.len(), 8);
    scene.run(180)?;
    let samples: Vec<BodySample> = scene
        .bodies
        .iter()
        .map(|body| scene.sample(*body))
        .collect::<Result<_, _>>()?;
    let finite = samples
        .iter()
        .all(|sample| sample.position.is_finite() && sample.velocity.is_finite());
    check.that(finite, || "a coincident body went non-finite".to_owned());
    let overlap = deepest_overlap(&samples, &[4.0; 8]);
    check.that(overlap < 0.1, || format!("still overlapping by {overlap}"));
    let spread = samples
        .iter()
        .map(|s| s.position.x)
        .fold(f64::MIN, f64::max)
        - samples
            .iter()
            .map(|s| s.position.x)
            .fold(f64::MAX, f64::min);
    check.metric("spread", spread);
    check.metric("deepest_overlap", overlap);
    scene.finish(check, "coincident");
    Ok(())
}

/// A dense mixed-radius cluster: 512 bodies of radius 2 to 8 dropped on sand in two layers,
/// crushed together by 180 ticks of attraction toward one column, then left 240 ticks to
/// settle. Bodies move loose material and compact bedrock but create or destroy neither; the
/// bins and the all-pairs broadphase solve exactly the same pairs, so their results are
/// identical, and a second bins run repeats the first bit for bit. The declared convergence
/// limit, at the default iterations: once the pile has settled 60 ticks no pair overlaps by
/// more than 0.4 of its smaller radius (the solver's Jacobi averaging leaves light bodies
/// wedged between heavy ones slightly compressed; see `pairs.wgsl`). Fewer passes converge
/// less: the settle phase's mean count of pairs overlapping by more than 0.4 falls from 4x4 to
/// 8x8 to the default.
pub fn dense_cluster(adapter: &str, check: &mut Check) -> Result<(), String> {
    struct Pile {
        samples: Vec<BodySample>,
        // The deepest overlap relative to the smaller radius over the settled ticks, and the
        // mean count of pairs overlapping by more than 0.4 over the whole settle phase.
        settled_relative: f64,
        deep_pairs: f64,
    }
    let run = |broadphase: Broadphase,
               iterations: PairIterations,
               check: &mut Check|
     -> Result<Pile, String> {
        let setup = Setup {
            bodies: 512,
            samples: 512,
            broadphase,
            pair_iterations: iterations,
            ..Setup::default()
        };
        let label = format!(
            "{broadphase:?}_{}x{}",
            iterations.position, iterations.velocity
        );
        let mut scene = Scene::open(adapter, setup, sand)?;
        let before = scene.totals()?;
        let mut radii = Vec::new();
        let mut spawns = Vec::new();
        for k in 0..512 {
            let radius = 2.0 + (k % 7) as f32;
            radii.push(radius);
            let sphere = SphereDescription::new(radius * radius * 0.25, radius, 2.0)
                .map_err(|error| error.to_string())?;
            let (x, z) = (
                -20.0 + (k % 16) as f32 * 7.0,
                -(-20.0 + (k / 16 % 16) as f32 * 7.0),
            );
            let spawn = BodySpawn::with(
                sphere,
                Target::Ray(Scene::down_at(x, z)),
                5.0 + (k / 256) as f32 * 20.0,
            )
            .map_err(|error| error.to_string())?;
            spawns.push(Command::SpawnBody(spawn));
        }
        for batch in spawns.chunks(256) {
            scene.step(batch)?;
        }
        let target = Target::Column(IVec2::new(32, 32));
        for _ in 0..180 {
            scene.step(&[Command::AttractBodies { target }])?;
        }
        let crushed = overlaps(&scene, &radii)?;
        check.metric(&format!("{label}_crushed_overlap"), crushed.deepest);
        let mut settled_relative = 0.0f64;
        let mut deep_pairs = 0u64;
        let mut deepest_settled = 0.0f64;
        let mut windows = [0.0f64; 4];
        for tick in 1..=240 {
            scene.step(&[])?;
            let pile = overlaps(&scene, &radii)?;
            deep_pairs += pile.deep_pairs;
            if tick > 60 {
                settled_relative = settled_relative.max(pile.relative);
                deepest_settled = deepest_settled.max(pile.deepest);
            }
            windows[(tick as usize - 1) / 60] =
                windows[(tick as usize - 1) / 60].max(pile.relative);
        }
        let samples: Vec<BodySample> = scene
            .bodies
            .iter()
            .map(|body| scene.sample(*body))
            .collect::<Result<_, _>>()?;
        let after = scene.totals()?;
        // Bodies move loose material and compact bare bedrock; nothing else changes either.
        check.equal(
            &format!("{label}: loose material"),
            after.granular,
            before.granular,
        );
        check.equal(
            &format!("{label}: bedrock"),
            after.elevation + scene.prior.compaction,
            before.elevation,
        );
        let deep_pairs = deep_pairs as f64 / 240.0;
        check.metric(&format!("{label}_settled_overlap"), deepest_settled);
        for (window, relative) in windows.iter().enumerate() {
            check.metric(
                &format!(
                    "{label}_relative_overlap_ticks_{}_{}",
                    window * 60 + 1,
                    window * 60 + 60
                ),
                *relative,
            );
        }
        check.metric(
            &format!("{label}_settled_relative_overlap"),
            settled_relative,
        );
        check.metric(&format!("{label}_mean_pairs_over_0_4"), deep_pairs);
        let residual = scene.last.map(|last| last.residual_overlap).unwrap_or(0.0);
        check.metric(
            &format!("{label}_gpu_residual_overlap"),
            f64::from(residual),
        );
        check.metric(
            &format!("{label}_candidate_pairs"),
            scene.body_totals.candidate_pairs,
        );
        scene.finish(check, &label);
        Ok(Pile {
            samples,
            settled_relative,
            deep_pairs,
        })
    };
    let bins = run(Broadphase::Bins, PairIterations::DEFAULT, check)?;
    let finite = bins
        .samples
        .iter()
        .all(|s| s.position.is_finite() && s.velocity.is_finite());
    check.that(finite, || "a clustered body went non-finite".to_owned());
    check.that(bins.settled_relative <= 0.4, || {
        format!(
            "a settled pair overlaps by {} of its smaller radius",
            bins.settled_relative
        )
    });
    let identical = |a: &Pile, b: &Pile| {
        a.samples
            .iter()
            .zip(&b.samples)
            .filter(|(a, b)| a.position != b.position || a.velocity != b.velocity)
            .count()
    };
    if std::env::var("DIAG_ONLY").is_ok() {
        return Ok(());
    }
    let again = run(Broadphase::Bins, PairIterations::DEFAULT, check)?;
    check.equal(
        "bodies differing between two bins runs",
        identical(&bins, &again),
        0,
    );
    let all = run(Broadphase::AllPairs, PairIterations::DEFAULT, check)?;
    check.equal(
        "bodies differing between bins and all pairs",
        identical(&bins, &all),
        0,
    );
    // Convergence: the same pile with fewer passes is more compressed.
    let fewest = run(
        Broadphase::Bins,
        PairIterations {
            position: 4,
            velocity: 4,
        },
        check,
    )?;
    let fewer = run(
        Broadphase::Bins,
        PairIterations {
            position: 8,
            velocity: 8,
        },
        check,
    )?;
    check.that(
        fewest.deep_pairs > fewer.deep_pairs && fewer.deep_pairs > bins.deep_pairs,
        || {
            format!(
                "deep pairs do not fall with passes: 4x4 {}, 8x8 {}, 16x16 {}",
                fewest.deep_pairs, fewer.deep_pairs, bins.deep_pairs
            )
        },
    );
    Ok(())
}

struct Overlaps {
    deepest: f64,
    relative: f64,
    deep_pairs: u64,
}

/// The pile's overlaps from its latest samples: the deepest, the deepest relative to the
/// pair's smaller radius, and how many pairs overlap by more than 0.4.
fn overlaps(scene: &Scene, radii: &[f32]) -> Result<Overlaps, String> {
    let samples: Vec<BodySample> = scene
        .bodies
        .iter()
        .map(|body| scene.sample(*body))
        .collect::<Result<_, _>>()?;
    let mut out = Overlaps {
        deepest: 0.0,
        relative: 0.0,
        deep_pairs: 0,
    };
    for (i, a) in samples.iter().enumerate() {
        for (j, b) in samples.iter().enumerate().skip(i + 1) {
            let depth = f64::from(radii[i] + radii[j]) - a.position.distance(b.position);
            out.deepest = out.deepest.max(depth);
            out.relative = out.relative.max(depth / f64::from(radii[i].min(radii[j])));
            out.deep_pairs += u64::from(depth > 0.4);
        }
    }
    Ok(out)
}

/// Fast bodies do not tunnel: two small spheres attracted into each other at a combined speed
/// beyond twice their diameter per substep stay apart (swept contacts), and a fast sphere
/// thrown at a one-cell wall stays on its side (terrain sweep).
pub fn fast_bodies(adapter: &str, check: &mut Check) -> Result<(), String> {
    let small = SphereDescription::new(1.0, 2.0, 7.8).map_err(|error| error.to_string())?;
    // A small body dropped from high above and attracted downward reaches the speed limit
    // and lands on a small body resting below it: it must end above it, never through it.
    let mut scene = Scene::open(adapter, Setup::default(), stone)?;
    let below = scene.spawn(small, 32.0, -32.0, 2.0)?;
    scene.run(30)?;
    let spawn = BodySpawn::with(small, Target::Column(IVec2::new(32, 32)), 3_000.0)
        .map_err(|error| error.to_string())?;
    let falling = match scene.step(&[Command::SpawnBody(spawn)])?[0] {
        CommandResult::BodySpawned { body, .. } => body,
        other => return Err(format!("high spawn: {other:?}")),
    };
    let mut impact_speed = 0.0f32;
    let mut through = false;
    for _ in 0..600 {
        scene.step(&[Command::AttractBodies {
            target: Target::Column(IVec2::new(32, 32)),
        }])?;
        let (top, bottom) = (scene.sample(falling)?, scene.sample(below)?);
        if top.position.y > bottom.position.y + 10.0 {
            impact_speed = top.speed();
        }
        if !through && top.position.y < bottom.position.y - 0.01 {
            check.note(format!(
                "below at tick {}: falling {} v {}, resting {} v {}",
                scene.tick().value(),
                top.position,
                top.velocity,
                bottom.position,
                bottom.velocity
            ));
            through = true;
        }
        if top.touching > 0 && top.speed() < 1.0 {
            break;
        }
    }
    check.that(!through, || {
        "the falling body went through the resting one".to_owned()
    });
    check.metric("impact_speed", f64::from(impact_speed));
    scene.finish(check, "fall_onto");

    // Head-on at nearly the speed limit each, so they close 10 units a substep, more than
    // their 8 of combined diameter: without the swept test they would end a substep past each
    // other. A window of radius 8 gives them 500 units of run-up in mid-air toward a column
    // between them.
    let wide = Setup {
        radius: 8,
        ..Setup::default()
    };
    let mut scene = Scene::open(adapter, wide, stone)?;
    let a = scene.spawn(small, -450.0, -32.0, 900.0)?;
    let b = scene.spawn(small, 530.0, -32.0, 900.0)?;
    let target = Target::Column(IVec2::new(40, 32));
    let mut crossed = false;
    let mut fastest = 0.0f32;
    let mut met = false;
    for _ in 0..600 {
        let commands = if met {
            Vec::new()
        } else {
            vec![Command::AttractBodies { target }]
        };
        scene.step(&commands)?;
        let (sa, sb) = (scene.sample(a)?, scene.sample(b)?);
        fastest = fastest.max(sa.velocity.x - sb.velocity.x);
        crossed |= sa.position.x > sb.position.x + 1e-3;
        // Once they meet, attraction stops: they part and coast.
        met |= sa.touching > 0 || sa.velocity.x < sb.velocity.x;
        if met && sa.velocity.x < 0.0 && sb.velocity.x > 0.0 && sa.position.x + 50.0 < sb.position.x
        {
            break;
        }
    }
    check.that(met, || "the bodies never met".to_owned());
    check.that(!crossed, || {
        "the bodies passed through each other".to_owned()
    });
    check.metric("head_on_closing_speed", f64::from(fastest));
    check.metric(
        "swept_contacts",
        u64::from(scene.body_totals.swept_contacts),
    );
    scene.finish(check, "head_on");

    // A wall one cell thick and 500 units high (too high to bounce over) at x = 60; a small
    // body attracted toward a column beyond it from 500 units away arrives at several units
    // a substep: the terrain sweep must stop it at the wall.
    let mut scene = Scene::open(adapter, wide, |cell, column| {
        stone(cell, column);
        if cell.x == 60 {
            column.set_elevation(1040);
        }
    })?;
    let thrown = scene.spawn(small, -440.0, -32.0, 3.0)?;
    let beyond = Target::Column(IVec2::new(300, 32));
    let mut passed = false;
    let mut fastest = 0.0f32;
    for _ in 0..300 {
        scene.step(&[Command::AttractBodies { target: beyond }])?;
        let sample = scene.sample(thrown)?;
        fastest = fastest.max(sample.speed());
        passed |= sample.position.x > 60.0;
    }
    check.that(!passed, || "the body went through the wall".to_owned());
    check.that(fastest > 2.0 * 120.0, || {
        format!("too slow to test the sweep: {fastest}")
    });
    check.metric("wall_fastest", f64::from(fastest));
    check.metric("swept_bodies", u64::from(scene.body_totals.swept_bodies));
    scene.finish(check, "wall");
    Ok(())
}

/// Several bodies rolling side by side over sand, whose trails and berms overlap, with sand
/// strokes on the same cells in the same ticks; then several plunging into a lake beside water
/// strokes. Every unit is accounted for: the totals change by exactly the tools' sources and
/// the bodies' declared compaction, cells contested between bodies stay with their owner, and
/// every tick balances.
pub fn shared_deformation(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), sand)?;
    let before = scene.totals()?;
    let mut bodies = Vec::new();
    for k in 0..6 {
        bodies.push(scene.spawn(metal(), 8.0, -(14.0 + 7.0 * k as f32), 4.0)?);
    }
    for tick in 0..90 {
        let mut commands: Vec<Command> = bodies
            .iter()
            .map(|body| drive(*body, 1.0, 0.0, DriveStrength::CONTROLS))
            .collect();
        if tick % 10 == 5 {
            let x = scene.position(bodies[2])?.x as i32;
            commands.push(Command::EditMaterial(
                MaterialEdit::new(EditTool::Sand, IVec2::new(x, 32), 6, 3)
                    .map_err(|error| error.to_string())?,
            ));
        }
        scene.step(&commands)?;
    }
    let after = scene.totals()?;
    let sand_change = i128::from(after.granular) - i128::from(before.granular);
    let solid_change = i128::from(after.solid()) - i128::from(before.solid());
    let tools = i128::from(scene.tool_solid_in) - i128::from(scene.tool_solid_out);
    check.equal(
        "solid change against the tools and the compaction",
        solid_change,
        tools - i128::from(scene.prior.compaction),
    );
    check.that(scene.body_totals.claims_lost > 0, || {
        "no footprint cell was contested".to_owned()
    });
    check.metric("sand_change", sand_change as i64);
    check.metric("tool_solid_in", scene.tool_solid_in);
    scene.finish(check, "sand");

    let mut scene = Scene::open(adapter, Setup::default(), |_, column| {
        column.set_elevation(20);
        column.set_fluid_mat(FluidMat::FLUID_WATER);
        column.set_fluid_vol(30);
    })?;
    let water = scene.totals()?.fluid_volume();
    let floor = 20.0 * scale();
    for k in 0..5 {
        scene.spawn(metal(), 20.0 + 5.0 * k as f32, -32.0, 15.0 + 25.0 - floor)?;
    }
    for tick in 0..60 {
        let mut commands = Vec::new();
        if tick % 6 == 0 {
            commands.push(Command::EditMaterial(
                MaterialEdit::new(EditTool::Water, IVec2::new(30, 32), 8, 2)
                    .map_err(|error| error.to_string())?,
            ));
        }
        scene.step(&commands)?;
    }
    let after = scene.totals()?.fluid_volume();
    check.equal(
        "water change against the strokes",
        i128::from(after) - i128::from(water),
        i128::from(scene.tool_fluid_in),
    );
    check.that(scene.body_totals.splashes > 1, || {
        "fewer than two splashes".to_owned()
    });
    check.metric("splashes", u64::from(scene.body_totals.splashes));
    check.metric("lake_claims_lost", u64::from(scene.body_totals.claims_lost));
    scene.finish(check, "lake");
    Ok(())
}

/// Terrain raised under a resting body lifts it onto the new surface without burying it;
/// terrain lowered under it lets it fall and rest again.
pub fn edit_under_resting_body(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), stone)?;
    let body = scene.spawn(metal(), 32.0, -32.0, 4.0)?;
    scene.run(60)?;
    let resting = scene.position(body)?.y;
    let raise = MaterialEdit::new(EditTool::Raise, IVec2::new(32, 32), 7, 12)
        .map_err(|error| error.to_string())?;
    scene.step(&[Command::EditMaterial(raise)])?;
    let lifted = scene.position(body)?.y;
    let surface = 52.0 * scale();
    check.that(lifted >= surface + 4.0 - 0.05, || {
        format!("after the raise the centre is at {lifted}, the surface at {surface}")
    });
    scene.run(30)?;
    check.that(scene.sample(body)?.grounded, || {
        "not grounded on the raised ground".to_owned()
    });
    let lower = MaterialEdit::new(EditTool::Lower, IVec2::new(32, 32), 7, 30)
        .map_err(|error| error.to_string())?;
    scene.step(&[Command::EditMaterial(lower)])?;
    scene.run(90)?;
    let sample = scene.sample(body)?;
    let low_surface = 22.0 * scale();
    check.that(sample.grounded, || {
        "not grounded after the ground was lowered".to_owned()
    });
    check.that(
        (sample.position.y as f32 - (low_surface + 4.0)).abs() < 0.1,
        || {
            format!(
                "rests at {} over the lowered surface {low_surface}",
                sample.position.y
            )
        },
    );
    check.metric("rest_before", f64::from(resting));
    check.metric("lifted_to", f64::from(lifted));
    scene.finish(check, "edit_under_body");
    Ok(())
}

/// The residency-edge policy: a body suspends with its state when its reach meets a column
/// that is not resident, frozen (not falling into the missing chunk) and an immovable obstacle
/// to others; it resumes with the same state when the terrain is back; and its anchored
/// position survives the window moving away and back.
pub fn residency_edge(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), stone)?;
    // Driven east toward the window's edge (cell 128 is outside the window).
    let runner = scene.spawn(metal(), 100.0, -32.0, 4.0)?;
    let mut suspended_at = None;
    for _ in 0..240 {
        scene.step(&[drive(runner, 1.0, 0.0, DriveStrength::CONTROLS)])?;
        let sample = scene.sample(runner)?;
        if sample.suspended {
            suspended_at = Some(sample);
            break;
        }
    }
    let Some(frozen) = suspended_at else {
        return Err("the body never reached the edge".to_owned());
    };
    let reach = world_model::body::footprint_reach(4.0, 0.0);
    check.that(frozen.position.x as f32 + reach <= 128.0 + 1.0, || {
        format!("suspended only at x = {}", frozen.position.x)
    });
    scene.run(30)?;
    let later = scene.sample(runner)?;
    check.that(
        later.suspended && later.position == frozen.position && later.velocity == frozen.velocity,
        || format!("a suspended body changed: {frozen:?} -> {later:?}"),
    );

    // A resting body whose chunk is retired is suspended in place, an obstacle to a body
    // rolling into it, and resumes unchanged when the chunk returns.
    let resting = scene.spawn(metal(), 32.0, -32.0, 4.0)?;
    scene.run(60)?;
    let before = scene.sample(resting)?;
    scene.retire(ChunkKey::new(0, 0, 0))?;
    scene.step(&[])?;
    let held = scene.sample(resting)?;
    check.that(held.suspended && held.position == before.position, || {
        format!("retiring its chunk did not suspend the body in place: {held:?}")
    });
    scene.upload(
        ChunkKey::new(0, 0, 0),
        &chunk_cells(ChunkKey::new(0, 0, 0), &stone),
    )?;
    scene.run(5)?;
    let resumed = scene.sample(resting)?;
    check.that(!resumed.suspended, || "the body did not resume".to_owned());
    check.that(
        (resumed.position.y - before.position.y).abs() < 0.01,
        || {
            format!(
                "resumed at {} from {}",
                resumed.position.y, before.position.y
            )
        },
    );

    // The window moves far away and back: every chunk retired, two resizes, the chunks
    // re-uploaded. The suspended bodies keep their exact positions across it.
    let positions: Vec<_> = [runner, resting]
        .iter()
        .map(|body| scene.sample(*body).map(|s| s.position))
        .collect::<Result<_, _>>()?;
    let keys: Vec<ChunkKey> = scene.slots.iter().map(|(key, _)| *key).collect();
    for key in &keys {
        scene.retire(*key)?;
    }
    scene.step(&[])?;
    scene.run(4)?;
    let far =
        world_runtime::port::ResidentWindow::new(ChunkKey::new(1_000_000, -2_000_000, 0), 1, 1)
            .map_err(|error| format!("{error:?}"))?;
    resize(&mut scene, far)?;
    scene.run(3)?;
    let far_sample = scene.sample(runner)?;
    check.that(far_sample.suspended, || {
        "a body outside the window was not suspended".to_owned()
    });
    let home = scene.setup.window();
    resize(&mut scene, home)?;
    for key in &keys {
        scene.upload(*key, &chunk_cells(*key, &stone))?;
    }
    scene.run(3)?;
    for (body, position) in [runner, resting].iter().zip(&positions) {
        let sample = scene.sample(*body)?;
        check.that((sample.position - *position).length() < 0.01, || {
            format!(
                "{body:?} moved across the round trip: {position} -> {}",
                sample.position
            )
        });
    }
    scene.finish(check, "edge");
    Ok(())
}

fn resize(scene: &mut Scene, window: world_runtime::port::ResidentWindow) -> Result<(), String> {
    use world_runtime::port::{PhysicalWorld, ResizeRequest};
    let session = scene.driver.session();
    scene
        .driver
        .world
        .resize(&ResizeRequest { session, window })
        .map(|_| ())
        .map_err(|refusal| format!("resize: {refusal:?}"))
}
