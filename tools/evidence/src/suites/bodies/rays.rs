//! Rays and the commands that act where a ray meets the world (checkpoint C).
//!
//! The live world resolves every ray on the device against the triangulated heightfield bodies
//! collide with; the CPU never reads terrain to place an edit. These cases compare its answers
//! with an independent `f64` reference over the terrain they built: a fine march along the ray
//! against the baseline's floor interpolation, refined by bisection.

use world_model::{
    body::BodyPreset,
    cell::WorldCell,
    command::{BodySpawn, Command, EditTool, MaterialEdit, Ray, Target},
    config::RAY_MAX_DISTANCE,
    glam::{DVec3, IVec2, Vec3},
    material::TerrainMat,
    observation::SurfaceHit,
    spatial::CHUNK_SIZE,
};
use world_runtime::port::CommandResult;

use super::{
    super::Check,
    scene::{Scene, Setup, scale},
};

/// Stone with a one-cell noise of up to eight elevation units.
fn bumps(cell: IVec2, column: &mut WorldCell) {
    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
    column.set_elevation(40 + noise(cell));
}

fn noise(cell: IVec2) -> u16 {
    (cell.x * 7 + cell.y * 13).rem_euclid(9) as u16
}

fn flat(_: IVec2, column: &mut WorldCell) {
    column.set_terrain_mat(TerrainMat::TERRAIN_STONE);
    column.set_elevation(40);
}

/// SplitMix64: a fixed stream of ray parameters.
struct Stream(u64);

impl Stream {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }

    fn between(&mut self, low: f64, high: f64) -> f64 {
        low + (high - low) * self.unit()
    }
}

/// What the reference expects of a ray.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Expected {
    Hit { distance: f64, point: DVec3 },
    NotResident { distance: f64 },
    Miss,
}

/// The resident terrain as the reference sees it: heights of the window's columns.
struct Terrain {
    min: IVec2,
    max: IVec2,
    shape: fn(IVec2) -> u16,
    scale: f64,
}

impl Terrain {
    fn new(setup: &Setup, shape: fn(IVec2) -> u16) -> Self {
        let view = setup.window().view();
        Self {
            min: view.min.origin_cell(),
            max: view.max.origin_cell() + IVec2::splat(CHUNK_SIZE as i32 - 1),
            shape,
            scale: f64::from(scale()),
        }
    }

    fn height(&self, cell: IVec2) -> Option<f64> {
        let inside = cell.cmpge(self.min).all() && cell.cmple(self.max).all();
        inside.then(|| f64::from((self.shape)(cell)) * self.scale)
    }

    /// The floor under render `(x, z)`: the baseline's interpolation over the quad's two
    /// triangles; `None` where a corner is not resident.
    fn floor(&self, x: f64, z: f64) -> Option<f64> {
        let grid = (x.floor(), (-z).floor());
        let cell = IVec2::new(grid.0 as i32, grid.1 as i32);
        let h00 = self.height(cell)?;
        let h10 = self.height(cell + IVec2::new(1, 0))?;
        let h01 = self.height(cell + IVec2::new(0, 1))?;
        let h11 = self.height(cell + IVec2::new(1, 1))?;
        let (tx, ty) = (x - grid.0, -z - grid.1);
        Some(if tx + ty <= 1.0 {
            h00 + (h10 - h00) * tx + (h01 - h00) * ty
        } else {
            h11 + (h01 - h11) * (1.0 - tx) + (h10 - h11) * (1.0 - ty)
        })
    }

    /// Marches the ray in steps of 1/200 unit: the first point at or below the floor is a hit,
    /// the first point over a quad with a missing corner is outside residency (every ray here
    /// runs far below the highest possible surface). Both are refined by bisection.
    fn trace(&self, ray: &Ray) -> Expected {
        let origin = ray.origin();
        let direction = ray.direction().as_dvec3();
        let length = f64::from(ray.max_distance());
        let at = |t: f64| origin + direction * t;
        let state = |t: f64| -> Option<bool> {
            let p = at(t);
            self.floor(p.x, p.z).map(|floor| p.y <= floor)
        };
        let step = 0.005;
        let mut previous = 0.0;
        let mut t = 0.0;
        loop {
            match state(t) {
                None => {
                    let edge = bisect(previous, t, |s| state(s).is_none());
                    return Expected::NotResident { distance: edge };
                }
                Some(true) => {
                    let hit = bisect(previous, t, |s| state(s) == Some(true));
                    return Expected::Hit {
                        distance: hit,
                        point: at(hit),
                    };
                }
                Some(false) => {}
            }
            if t >= length {
                return Expected::Miss;
            }
            previous = t;
            t = (t + step).min(length);
        }
    }
}

/// The first parameter in `(low, high]` where `reached` holds, given it holds at `high` and
/// not at `low`.
fn bisect(mut low: f64, mut high: f64, reached: impl Fn(f64) -> bool) -> f64 {
    for _ in 0..40 {
        let middle = 0.5 * (low + high);
        if reached(middle) {
            high = middle;
        } else {
            low = middle;
        }
    }
    high
}

fn ray(origin: DVec3, direction: Vec3, length: f32) -> Result<Ray, String> {
    Ray::new(origin, direction, length).map_err(|error| error.to_string())
}

/// Whether the GPU answer agrees with the reference: the same kind, the distance within 2e-3,
/// the hit point within 2e-3 and its quad cell the reference's (or a neighbour's when the hit
/// lies within 1e-3 of their shared edge).
fn agrees(actual: &SurfaceHit, expected: &Expected) -> Option<f64> {
    match (actual, expected) {
        (SurfaceHit::Miss, Expected::Miss) => Some(0.0),
        (
            SurfaceHit::NotResident { distance },
            Expected::NotResident {
                distance: reference,
            },
        ) => {
            let error = (f64::from(*distance) - reference).abs();
            (error <= 2e-3).then_some(error)
        }
        (
            SurfaceHit::Hit {
                cell,
                position,
                distance,
            },
            Expected::Hit {
                distance: reference,
                point,
            },
        ) => {
            let error = (f64::from(*distance) - reference)
                .abs()
                .max(position.distance(*point));
            let grid = DVec3::new(point.x, -point.z, 0.0);
            let near_edge =
                (grid.x - grid.x.round()).abs() < 1e-3 || (grid.y - grid.y.round()).abs() < 1e-3;
            let same = *cell == IVec2::new(grid.x.floor() as i32, grid.y.floor() as i32);
            (error <= 2e-3 && (same || near_edge)).then_some(error)
        }
        _ => None,
    }
}

/// Rays agree with an independent reference: random rays over bumpy stone hit the same
/// triangle at the same distance, leave the window as outside residency at its edge or run
/// out as misses; straight up and too short miss; a ray longer than the limit is refused. A
/// ray through a resting body reports it before the ground under it with the sphere's
/// distance, one passing beside it does not, and a body is reported even when the ray then
/// leaves residency.
pub fn ray_queries(adapter: &str, check: &mut Check) -> Result<(), String> {
    let setup = Setup::default();
    let terrain = Terrain::new(&setup, |cell| 40 + noise(cell));
    let mut scene = Scene::open(adapter, setup, bumps)?;
    let mut stream = Stream(0x0004_7A75);
    let mut rays = Vec::new();
    for _ in 0..96 {
        let origin = DVec3::new(
            stream.between(-50.0, 110.0),
            stream.between(30.0, 120.0),
            -stream.between(-50.0, 110.0),
        );
        let azimuth = stream.between(0.0, std::f64::consts::TAU);
        let descent = stream.between(4.0, 80.0).to_radians();
        let direction = DVec3::new(
            azimuth.cos() * descent.cos(),
            -descent.sin(),
            azimuth.sin() * descent.cos(),
        );
        rays.push(ray(origin, direction.as_vec3(), 400.0)?);
    }
    // Straight up, and one too short to reach the ground.
    rays.push(ray(DVec3::new(10.5, 40.0, -10.5), Vec3::Y, 400.0)?);
    rays.push(ray(DVec3::new(10.5, 60.0, -10.5), Vec3::NEG_Y, 5.0)?);
    // Low and level toward the east edge: outside residency at x = 127, the last quad whose
    // corners are all resident ending at 127.
    rays.push(ray(DVec3::new(100.25, 40.0, -20.75), Vec3::X, 400.0)?);
    check.that(
        Ray::new(DVec3::ZERO, Vec3::NEG_Y, RAY_MAX_DISTANCE * 1.01).is_err(),
        || "a ray beyond the distance limit was accepted".to_owned(),
    );

    let mut counts = [0u32; 3];
    let mut worst = 0.0f64;
    for batch in rays.chunks(32) {
        let commands: Vec<Command> = batch
            .iter()
            .map(|ray| Command::QueryRay { ray: *ray })
            .collect();
        let results = scene.step(&commands)?;
        for (ray, result) in batch.iter().zip(results) {
            let expected = terrain.trace(ray);
            let CommandResult::Ray { surface, body } = result else {
                check.fail(format!("{ray:?}: {result:?}"));
                continue;
            };
            check.that(body.is_none(), || format!("{ray:?} met a body: {body:?}"));
            match agrees(&surface, &expected) {
                Some(error) => worst = worst.max(error),
                None => check.fail(format!("{ray:?}: GPU {surface:?}, reference {expected:?}")),
            }
            counts[match expected {
                Expected::Hit { .. } => 0,
                Expected::NotResident { .. } => 1,
                Expected::Miss => 2,
            }] += 1;
        }
    }
    check.that(counts.iter().all(|count| *count > 0), || {
        format!("hits, outside residency and misses not all exercised: {counts:?}")
    });
    check.metric("rays", rays.len());
    check.metric("hits", counts[0]);
    check.metric("not_resident", counts[1]);
    check.metric("misses", counts[2]);
    check.metric("worst_error", worst);

    // A resting body: a ray down through its centre, one beside it, one level through it
    // toward the window's edge.
    let metal = BodyPreset::Metal.description();
    let body = scene.spawn(metal, 32.5, -32.5, 1.0)?;
    scene.run(90)?;
    let rest = scene.sample(body)?;
    // A query runs in the command pass, before the tick's body substeps: it sees the body where
    // the last sample left it.
    check.that(rest.grounded, || format!("not resting: {rest:?}"));
    let radius = f64::from(metal.radius());
    let center = rest.position;
    let down = ray(center + DVec3::new(0.0, 200.0, 0.0), Vec3::NEG_Y, 400.0)?;
    let beside = ray(
        center + DVec3::new(radius + 0.5, 200.0, 0.0),
        Vec3::NEG_Y,
        400.0,
    )?;
    let level = ray(center - DVec3::new(20.0, 0.0, 0.0), Vec3::X, 400.0)?;
    let results = scene.step(&[
        Command::QueryRay { ray: down },
        Command::QueryRay { ray: beside },
        Command::QueryRay { ray: level },
    ])?;
    let sphere_distance = |ray: &Ray| -> f64 {
        // Along the ray to the sphere's near surface, from the sample.
        let offset = ray.origin() - center;
        let along = offset.dot(ray.direction().as_dvec3());
        let c = offset.length_squared() - radius * radius;
        -along - (along * along - c).max(0.0).sqrt()
    };
    match results[0] {
        CommandResult::Ray {
            surface: SurfaceHit::Hit { distance, .. },
            body: Some(hit),
        } => {
            check.equal("body under the downward ray", hit.body, body);
            let error = (f64::from(hit.distance) - sphere_distance(&down)).abs();
            check.that(error < 1e-2, || format!("sphere distance off by {error}"));
            check.that(hit.distance < distance, || {
                "the body was not before the ground".to_owned()
            });
            check.metric("body_distance_error", error);
        }
        other => check.fail(format!("down through the body: {other:?}")),
    }
    match results[1] {
        CommandResult::Ray {
            surface: SurfaceHit::Hit { .. },
            body: None,
        } => {}
        other => check.fail(format!("beside the body: {other:?}")),
    }
    match results[2] {
        CommandResult::Ray {
            surface: SurfaceHit::NotResident { .. },
            body: Some(hit),
        } => {
            check.equal("body on the level ray", hit.body, body);
            let error = (f64::from(hit.distance) - sphere_distance(&level)).abs();
            check.that(error < 1e-2, || {
                format!("level sphere distance off by {error}")
            });
        }
        other => check.fail(format!("level through the body: {other:?}")),
    }
    scene.finish(check, "rays");
    Ok(())
}

/// Commands at a ray's hit, resolved on the device. In one tick: a column query before an edit
/// sees the old ground, the edit is acknowledged at the cell a query ray hit, and a query ray
/// and a column query after it see the raised ground; the next tick's totals grew by exactly
/// what the stroke added. A ray that misses or leaves residency is answered as such and changes
/// nothing; a spawn lands at the hit point, drop height above its column; attraction and lift
/// at a ray act at the hit point, and one whose ray missed acts on nothing.
pub fn ray_targeted_commands(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut scene = Scene::open(adapter, Setup::default(), flat)?;
    let ground = 40.0 * f64::from(scale());
    let before = scene.totals()?;
    let down = ray(DVec3::new(20.3, 200.0, -30.6), Vec3::NEG_Y, 400.0)?;
    let raise = MaterialEdit::at(EditTool::Raise, Target::Ray(down), 2, 20)
        .map_err(|error| error.to_string())?;
    let cell = IVec2::new(20, 30);
    let tool_before = scene.tool_solid_in;
    let results = scene.step(&[
        Command::QueryColumn { cell },
        Command::QueryRay { ray: down },
        Command::EditMaterial(raise),
        Command::QueryRay { ray: down },
        Command::QueryColumn { cell },
    ])?;
    let added = scene.tool_solid_in - tool_before;
    match (results[0], results[4]) {
        (CommandResult::Column { column: old, .. }, CommandResult::Column { column: new, .. }) => {
            check.equal("elevation before the edit", old.elevation(), 40);
            check.that(new.elevation() > 40, || {
                format!(
                    "the column after the edit was not raised: {}",
                    new.elevation()
                )
            });
        }
        other => check.fail(format!("column queries: {other:?}")),
    }
    let hit_cell = match results[1] {
        CommandResult::Ray {
            surface: SurfaceHit::Hit { cell, distance, .. },
            ..
        } => {
            check.that(
                (f64::from(distance) - (200.0 - ground)).abs() < 2e-3,
                || format!("flat ground hit at {distance}"),
            );
            Some(cell)
        }
        other => {
            check.fail(format!("query before the edit: {other:?}"));
            None
        }
    };
    match results[2] {
        CommandResult::Edited {
            center, changed, ..
        } => {
            check.equal("edit centre (the ray's quad cell)", Some(center), hit_cell);
            check.equal("edit centre", center, cell);
            check.that(changed > 0, || "the edit changed nothing".to_owned());
        }
        other => check.fail(format!("ray edit: {other:?}")),
    }
    match (results[1], results[3]) {
        (
            CommandResult::Ray {
                surface: SurfaceHit::Hit {
                    distance: first, ..
                },
                ..
            },
            CommandResult::Ray {
                surface:
                    SurfaceHit::Hit {
                        distance: second, ..
                    },
                ..
            },
        ) => check.that(second < first, || {
            format!("the raised ground is not nearer: {first} then {second}")
        }),
        other => check.fail(format!("queries around the edit: {other:?}")),
    }
    let after = scene.totals()?;
    check.that(added > 0, || {
        "the tick reported no tool material".to_owned()
    });
    check.equal(
        "bedrock added by the stroke",
        after.elevation - before.elevation,
        added,
    );

    // Misses and residency: nothing changes.
    let up = ray(DVec3::new(40.5, 30.0, -40.5), Vec3::Y, 400.0)?;
    let away = ray(DVec3::new(100.25, 25.0, -20.75), Vec3::X, 400.0)?;
    let sand_up = MaterialEdit::at(EditTool::Sand, Target::Ray(up), 2, 20)
        .map_err(|error| error.to_string())?;
    let sand_away = MaterialEdit::at(EditTool::Sand, Target::Ray(away), 2, 20)
        .map_err(|error| error.to_string())?;
    let metal = BodyPreset::Metal.description();
    let spawn_up =
        BodySpawn::with(metal, Target::Ray(up), 2.0).map_err(|error| error.to_string())?;
    let spawn_outside = BodySpawn::with(metal, Target::Column(IVec2::new(1000, 1000)), 2.0)
        .map_err(|error| error.to_string())?;
    let tool_before = (scene.tool_solid_in, scene.tool_fluid_in);
    let results = scene.step(&[
        Command::EditMaterial(sand_up),
        Command::EditMaterial(sand_away),
        Command::SpawnBody(spawn_up),
        Command::SpawnBody(spawn_outside),
    ])?;
    check.equal(
        "edit at an upward ray",
        results[0],
        CommandResult::TargetMissed(SurfaceHit::Miss),
    );
    match results[1] {
        CommandResult::TargetMissed(SurfaceHit::NotResident { distance }) => {
            check.that((distance - (127.0 - 100.25)).abs() < 2e-3, || {
                format!("left residency at {distance}")
            });
        }
        other => check.fail(format!("edit toward the edge: {other:?}")),
    }
    check.equal(
        "spawn at an upward ray",
        results[2],
        CommandResult::TargetMissed(SurfaceHit::Miss),
    );
    check.equal(
        "spawn outside the window",
        results[3],
        CommandResult::TargetNotResident {
            cell: IVec2::new(1000, 1000),
        },
    );
    check.equal(
        "tool material of the missed tick",
        (scene.tool_solid_in, scene.tool_fluid_in),
        tool_before,
    );
    check.equal("totals after the misses", scene.totals()?, after);
    check.equal(
        "live bodies after failed spawns",
        scene.driver.world.live_bodies(),
        0,
    );

    // A spawn at a ray's hit: the hit point, drop above the column's bedrock and loose material.
    let slanted = ray(
        DVec3::new(10.0, 120.0, -12.0),
        Vec3::new(0.6, -1.0, -0.3),
        400.0,
    )?;
    let spawn =
        BodySpawn::with(metal, Target::Ray(slanted), 3.0).map_err(|error| error.to_string())?;
    let results = scene.step(&[
        Command::QueryRay { ray: slanted },
        Command::SpawnBody(spawn),
    ])?;
    let first = match (results[0], results[1]) {
        (
            CommandResult::Ray {
                surface: SurfaceHit::Hit { position, .. },
                ..
            },
            CommandResult::BodySpawned {
                body,
                position: placed,
            },
        ) => {
            let expected = DVec3::new(position.x, ground + 3.0, position.z);
            let error = placed.distance(expected);
            check.that(error < 2e-3, || {
                format!("spawned at {placed}, expected {expected}")
            });
            body
        }
        other => return Err(format!("ray spawn: {other:?}")),
    };
    let second = scene.spawn(metal, 50.5, -50.5, 1.0)?;
    scene.run(60)?;

    // Attraction toward a ray's hit, then lift at the first body's column.
    let toward = ray(DVec3::new(35.5, 200.0, -40.5), Vec3::NEG_Y, 400.0)?;
    let results = scene.step(&[Command::AttractBodies {
        target: Target::Ray(toward),
    }])?;
    let point = DVec3::new(35.5, ground, -40.5);
    match results[0] {
        CommandResult::Attracted { bodies, target } => {
            check.equal("bodies attracted", bodies, 2);
            check.that(target.distance(point) < 2e-3, || {
                format!("attracted to {target}")
            });
        }
        other => check.fail(format!("ray attraction: {other:?}")),
    }
    for body in [first, second] {
        let sample = scene.sample(body)?;
        let toward = (point - sample.position).normalize().as_vec3();
        check.that(sample.velocity.dot(toward) > 0.0, || {
            format!("{body:?} is not moving toward the target: {sample:?}")
        });
    }
    scene.run(90)?;
    let lift_at = scene.sample(first)?.position;
    let over = ray(lift_at + DVec3::new(0.0, 200.0, 0.0), Vec3::NEG_Y, 400.0)?;
    let results = scene.step(&[Command::LiftBodies {
        target: Target::Ray(over),
    }])?;
    match results[0] {
        CommandResult::Lifted { bodies, .. } => {
            check.that(bodies >= 1, || "the lift reached no body".to_owned());
        }
        other => check.fail(format!("ray lift: {other:?}")),
    }
    let lifted = scene.sample(first)?;
    check.that(lifted.velocity.y > 0.0, || {
        format!("the lifted body is not rising: {lifted:?}")
    });
    // A missed ray attracts nothing.
    let results = scene.step(&[Command::AttractBodies {
        target: Target::Ray(up),
    }])?;
    check.equal(
        "attraction at an upward ray",
        results[0],
        CommandResult::TargetMissed(SurfaceHit::Miss),
    );
    scene.finish(check, "ray_commands");
    Ok(())
}
