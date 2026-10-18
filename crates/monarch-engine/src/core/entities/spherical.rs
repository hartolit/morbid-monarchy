use bevy::{
    ecs::{
        component::Component,
        system::{Query, Res, ResMut},
    },
    math::{IVec2, Vec3},
    time::Time,
    transform::components::Transform,
};
use std::time::Instant;

use crate::core::{
    counters::EngineCounters,
    entities::{DeformationProfile, GlobalPhysicsConfig, KinematicProfile},
    physics::grid_physics::{DisplacementSlot, GridPhysicsApi},
    world::grid::ActiveWorldGrid,
};
pub use world_model::body::submerged_volume_fraction;
use world_model::{
    body::{BODY_RELATIVE_DENSITY, tuning::*},
    material::{FluidMat, GranularMat, fluid_properties},
    spatial::nearest_cell,
};

/// Lower bound on a body's relative density; keeps the buoyancy ratio finite.
const MIN_BODY_RELATIVE_DENSITY: f32 = *BODY_RELATIVE_DENSITY.start();

#[derive(Component, Debug, Clone)]
pub struct DynamicRigidSphere {
    pub velocity: Vec3,
    pub mass: f32,
    pub radius: f32,
    /// Density relative to water. A sphere floats in any fluid denser than this.
    pub relative_density: f32,
    /// Set by the contact solver while the sphere rests on or rolls along the ground.
    pub is_grounded: bool,
    /// Averaged ground contact normal from the latest substep; world up while airborne.
    pub ground_normal: Vec3,
    /// Fraction of the sphere volume below a liquid surface during the latest substep.
    pub submerged_fraction: f32,
    /// Submerged fraction one substep earlier; rising from zero marks entry into a liquid.
    pub previous_submerged_fraction: f32,
    /// Fractional bow-wave volume not yet displaced, carried between substeps.
    pub pending_wake_volume: f32,
    /// Filtered plane of the undisturbed ground under the sphere; see `GroundModel`.
    ground_reference: Option<ReferencePlane>,

    // Crater geometry
    pub rim_inner_radius_ratio: f32,
    pub rim_expansion_factor: f32,
    pub blade_ceiling_ratio: f32,
    pub max_rim_deposit_per_cell: u16,
    pub max_deformation_size_ratio: f32,

    // Contact response
    pub grounding_drift_buffer: f32,
    /// Impacts slower than this are absorbed without rebound, so rolling over uneven ground
    /// does not turn every facet into a hop.
    pub minimum_bounce_speed: f32,
}

impl DynamicRigidSphere {
    pub fn new(mass: f32, radius: f32, relative_density: f32) -> Self {
        Self {
            velocity: Vec3::ZERO,
            mass,
            radius,
            relative_density,
            is_grounded: false,
            ground_normal: Vec3::Y,
            submerged_fraction: 0.0,
            previous_submerged_fraction: 0.0,
            pending_wake_volume: 0.0,
            ground_reference: None,
            rim_inner_radius_ratio: CRATER.rim_inner_radius_ratio,
            rim_expansion_factor: CRATER.rim_expansion_factor,
            blade_ceiling_ratio: CRATER.blade_ceiling_ratio,
            max_rim_deposit_per_cell: CRATER.max_rim_deposit_per_cell,
            max_deformation_size_ratio: CRATER.max_deformation_size_ratio,
            grounding_drift_buffer: CRATER.grounding_drift_buffer,
            minimum_bounce_speed: CRATER.minimum_bounce_speed,
        }
    }
}

/// Closest point to `p` on triangle `abc`, by Voronoi region classification
/// (Ericson, Real-Time Collision Detection, 5.1.5).
fn closest_point_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }

    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }

    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }

    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }

    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }

    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }

    let inverse = 1.0 / (va + vb + vc);
    a + ab * (vb * inverse) + ac * (vc * inverse)
}

/// Grid cells whose mesh vertex lies within `reach` of `center` on the ground plane, with the
/// squared horizontal distance to that vertex. Vertices sit at integer coordinates, the same
/// points the landscape mesh is drawn through.
fn cells_within(center: Vec3, reach: f32) -> impl Iterator<Item = (IVec2, f32)> {
    let min_x = (center.x - reach).floor() as i32;
    let max_x = (center.x + reach).ceil() as i32;
    let min_y = (-center.z - reach).floor() as i32;
    let max_y = (-center.z + reach).ceil() as i32;
    let reach_sq = reach * reach;

    (min_y..=max_y).flat_map(move |gy| {
        (min_x..=max_x).filter_map(move |gx| {
            let dx = gx as f32 - center.x;
            let dz = -(gy as f32) - center.z;
            let distance_sq = dx * dx + dz * dz;
            (distance_sq <= reach_sq).then_some((IVec2::new(gx, gy), distance_sq))
        })
    })
}

/// Contacts between a sphere and the floor mesh near it.
#[derive(Clone, Copy)]
struct ContactManifold {
    normals: [Vec3; MAX_MANIFOLD_CONTACTS],
    /// Overlap of each contact; negative while only within the slop.
    depths: [f32; MAX_MANIFOLD_CONTACTS],
    count: usize,
    deepest_normal: Vec3,
    deepest_depth: f32,
}

impl ContactManifold {
    fn contacts(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.normals[..self.count]
            .iter()
            .copied()
            .zip(self.depths[..self.count].iter().copied())
    }

    fn ground_normal(&self) -> Vec3 {
        self.contacts()
            .filter(|(_, depth)| *depth >= self.deepest_depth - GROUND_NORMAL_BAND)
            .map(|(normal, _)| normal)
            .sum::<Vec3>()
            .try_normalize()
            .unwrap_or(self.deepest_normal)
    }

    fn is_touching(&self) -> bool {
        self.deepest_depth >= -CONSTRAINT_TOLERANCE
    }
}

/// Gathers the (yielded) floor triangles within `reach` of a sphere centre. The landscape is a height
/// field, so a closest point always lies below or beside the centre once it is above the
/// surface, and the direction from it is the separating normal.
fn probe_floor(
    physics_api: &GridPhysicsApi,
    ground: &GroundModel,
    center: Vec3,
    radius: f32,
    reach: f32,
) -> Option<ContactManifold> {
    let mut manifold = ContactManifold {
        normals: [Vec3::Y; MAX_MANIFOLD_CONTACTS],
        depths: [f32::NEG_INFINITY; MAX_MANIFOLD_CONTACTS],
        count: 0,
        deepest_normal: Vec3::Y,
        deepest_depth: f32::NEG_INFINITY,
    };
    let mut touching = false;

    let min_x = (center.x - reach).floor() as i32;
    let max_x = (center.x + reach).floor() as i32;
    let min_y = (-center.z - reach).floor() as i32;
    let max_y = (-center.z + reach).floor() as i32;

    for gy in min_y..=max_y {
        for gx in min_x..=max_x {
            let Some(triangles) = GridPhysicsApi::quad_triangles_with(IVec2::new(gx, gy), |cell| {
                ground.collision_height(physics_api, cell)
            }) else {
                continue;
            };
            for [a, b, c] in triangles {
                let closest = closest_point_on_triangle(center, a, b, c);
                let offset = center - closest;
                let distance = offset.length();
                if distance >= reach {
                    continue;
                }

                let raw_face = (b - a).cross(c - a).normalize_or(Vec3::Y);
                let face = if raw_face.y < 0.0 {
                    -raw_face
                } else {
                    raw_face
                };
                let normal = if distance > DEGENERATE_DISTANCE {
                    let direction = offset / distance;
                    if direction.dot(face) >= INTERNAL_EDGE_COSINE {
                        face
                    } else {
                        direction
                    }
                } else {
                    face
                };

                touching = true;
                let depth = radius - distance;
                if depth > manifold.deepest_depth {
                    manifold.deepest_depth = depth;
                    manifold.deepest_normal = normal;
                }

                let existing = (0..manifold.count)
                    .find(|&index| manifold.normals[index].dot(normal) >= DUPLICATE_NORMAL_COSINE);
                match existing {
                    Some(index) => manifold.depths[index] = manifold.depths[index].max(depth),
                    None if manifold.count < MAX_MANIFOLD_CONTACTS => {
                        manifold.normals[manifold.count] = normal;
                        manifold.depths[manifold.count] = depth;
                        manifold.count += 1;
                    }
                    None => {}
                }
            }
        }
    }

    touching.then_some(manifold)
}

type SphereQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static mut Transform,
        &'static mut DynamicRigidSphere,
        &'static KinematicProfile,
        &'static DeformationProfile,
    ),
>;

pub fn simulate_rigid_sphere_kinematics(
    mut spheres: SphereQuery<'_, '_>,
    mut grid: ResMut<ActiveWorldGrid>,
    time: Res<Time>,
    global_config: Res<GlobalPhysicsConfig>,
    counters: Option<ResMut<EngineCounters>>,
) {
    let frame_seconds = time.delta_secs();
    if frame_seconds <= 0.0 || spheres.is_empty() {
        return;
    }
    let started = Instant::now();
    let sphere_count = spheres.iter().len() as u64;
    let mut pair_contacts = 0;

    let substep_count = ((frame_seconds / global_config.max_substep_seconds).ceil() as u32)
        .clamp(1, global_config.max_substeps.max(1));
    let substep_seconds = frame_seconds / substep_count as f32;

    let mut physics_api = GridPhysicsApi::new(&mut grid, &global_config);

    for _ in 0..substep_count {
        integrate_body_forces(&mut spheres, &physics_api, &global_config, substep_seconds);
        pair_contacts += resolve_sphere_pairs(&mut spheres, &global_config);
        resolve_terrain_contacts(
            &mut spheres,
            &mut physics_api,
            &global_config,
            substep_seconds,
        );
        displace_liquids(&mut spheres, &mut physics_api, substep_seconds);
    }

    if let Some(mut counters) = counters {
        let substeps = u64::from(substep_count);
        counters.body_substeps += substeps;
        // `iter_combinations_mut` visits every unordered pair once per substep.
        counters.body_pair_tests += substeps * sphere_count * sphere_count.saturating_sub(1) / 2;
        counters.body_pair_contacts += pair_contacts;
        counters.body_nanos += started.elapsed().as_nanos() as u64;
    }
}

/// Semi-implicit Euler step for gravity, buoyancy and drag.
fn integrate_body_forces(
    spheres: &mut SphereQuery<'_, '_>,
    physics_api: &GridPhysicsApi,
    global_config: &GlobalPhysicsConfig,
    delta_time: f32,
) {
    for (mut transform, mut sphere, kinematic, _) in spheres.iter_mut() {
        let position = transform.translation;
        let center_cell = nearest_cell(position.x, position.z);

        let mut acceleration = global_config.gravity;
        let mut drag_rate = kinematic.air_drag_rate;
        let mut submerged_fraction = 0.0;

        if let Some((fluid, surface_height)) = physics_api.get_fluid_surface(center_cell) {
            let depth = surface_height - (position.y - sphere.radius);
            submerged_fraction = submerged_volume_fraction(depth, sphere.radius);

            if submerged_fraction > 0.0 {
                let fluid_props = fluid_properties(fluid);
                let density_ratio = fluid_props.relative_density
                    / sphere.relative_density.max(MIN_BODY_RELATIVE_DENSITY);

                // Archimedes: the displaced liquid pushes back with its own weight.
                acceleration -= global_config.gravity * (submerged_fraction * density_ratio);
                drag_rate = drag_rate * (1.0 - submerged_fraction)
                    + fluid_props.drag_rate * submerged_fraction;
            }
        }

        sphere.velocity += acceleration * delta_time;
        sphere.velocity *= (-drag_rate * delta_time).exp();
        sphere.previous_submerged_fraction = sphere.submerged_fraction;
        sphere.submerged_fraction = submerged_fraction;
        transform.translation += sphere.velocity * delta_time;
    }
}

/// Resolves every overlapping pair and returns how many overlapped.
fn resolve_sphere_pairs(
    spheres: &mut SphereQuery<'_, '_>,
    global_config: &GlobalPhysicsConfig,
) -> u64 {
    let mut contacts = 0;
    let mut combinations = spheres.iter_combinations_mut();
    while let Some([(mut t_a, mut s_a, k_a, _), (mut t_b, mut s_b, k_b, _)]) =
        combinations.fetch_next()
    {
        let pos_delta = t_a.translation - t_b.translation;
        let dist_sq = pos_delta.length_squared();
        let min_dist = s_a.radius + s_b.radius;

        if dist_sq < min_dist * min_dist {
            contacts += 1;
            let dist = dist_sq.sqrt();
            let normal = if dist > global_config.epsilon_distance {
                pos_delta / dist
            } else {
                Vec3::X
            };
            let overlap = min_dist - dist;
            let inv_mass_a = 1.0 / s_a.mass;
            let inv_mass_b = 1.0 / s_b.mass;
            let total_inv_mass = inv_mass_a + inv_mass_b;

            let correction = overlap / total_inv_mass;
            t_a.translation += normal * (correction * inv_mass_a);
            t_b.translation -= normal * (correction * inv_mass_b);

            let rel_vel = s_a.velocity - s_b.velocity;
            let vel_normal = rel_vel.dot(normal);

            if vel_normal < 0.0 {
                let impulse = -(1.0 + k_a.impact_restitution.max(k_b.impact_restitution))
                    * vel_normal
                    / total_inv_mass;
                s_a.velocity += normal * (impulse * inv_mass_a);
                s_b.velocity -= normal * (impulse * inv_mass_b);
            }
        }
    }
    contacts
}

/// Bounces off the ground, then removes any velocity still pointing into a contact and applies
/// rolling friction along the ground plane.
///
/// Gravity re-enters every substep; once its normal component is cancelled, the tangential
/// remainder `g - (g.n)n` accelerates the sphere down slopes, which is what makes it roll.
fn resolve_contact_velocity(
    sphere: &mut DynamicRigidSphere,
    kinematic: &KinematicProfile,
    global_config: &GlobalPhysicsConfig,
    manifold: &ContactManifold,
    delta_time: f32,
) {
    let ground_normal = manifold.ground_normal();

    let approach_speed = sphere.velocity.dot(ground_normal);
    if manifold.is_touching() && approach_speed < 0.0 {
        // Impacts slower than one substep of gravity are resting contact, not bounces;
        // reflecting them would make a grounded sphere vibrate.
        let settle_speed = (global_config.gravity.length() * delta_time
            + sphere.grounding_drift_buffer)
            .max(sphere.minimum_bounce_speed);
        let restitution = if -approach_speed > settle_speed {
            kinematic.impact_restitution
        } else {
            0.0
        };
        sphere.velocity -= (1.0 + restitution) * approach_speed * ground_normal;
    }

    // Each contact is a one-sided constraint; sweeping them removes motion into walls and
    // creases that the averaged ground normal alone would miss.
    for _ in 0..VELOCITY_ITERATIONS {
        for (normal, depth) in manifold.contacts() {
            let normal_speed = sphere.velocity.dot(normal);
            if depth >= -CONSTRAINT_TOLERANCE && normal_speed < 0.0 {
                sphere.velocity -= normal_speed * normal;
            }
        }
    }

    if ground_normal.y < GROUND_NORMAL_MIN_Y {
        return;
    }

    let normal_velocity = sphere.velocity.dot(ground_normal) * ground_normal;
    let tangential_velocity = sphere.velocity - normal_velocity;

    let horizontal_tilt = Vec3::new(ground_normal.x, 0.0, ground_normal.z).length();
    let slope_gradient = horizontal_tilt / ground_normal.y;
    let is_static = tangential_velocity.length() < kinematic.static_friction_speed
        && slope_gradient <= kinematic.static_friction_coefficient;

    let tangential_speed = tangential_velocity.length();
    let retained_tangential = if is_static || tangential_speed <= 0.0 {
        Vec3::ZERO
    } else {
        let rolling_loss = kinematic.rolling_resistance_coefficient
            * global_config.gravity.length()
            * ground_normal.y
            * delta_time;
        let damped_speed = (tangential_speed - rolling_loss).max(0.0)
            * (-kinematic.ground_damping_rate * delta_time).exp();
        tangential_velocity * (damped_speed / tangential_speed)
    };

    sphere.velocity = normal_velocity + retained_tangential;
}

/// Digs a crater under a sphere that hit the ground hard enough to deform it, returning the
/// energy the crater absorbed.
///
/// The sphere's underside is pushed down by an extension that grows with impact energy.
/// Bedrock above it is crushed (harder rock costs more, and so does digging below the
/// surrounding plateau), and loose material above it is thrown into a rim. Every displaced
/// granular unit lands in the rim, so cratering conserves loose material.
fn carve_impact_crater(
    physics_api: &mut GridPhysicsApi,
    center: Vec3,
    sphere: &DynamicRigidSphere,
    deformation: &DeformationProfile,
    impact_energy: f32,
) -> f32 {
    let radius = sphere.radius;
    let extension = (impact_energy * deformation.energy_to_deformation_scale)
        .sqrt()
        .min(radius * sphere.max_deformation_size_ratio);

    let center_cell = nearest_cell(center.x, center.z);
    let baseline = physics_api
        .get_floor_height(center_cell)
        .unwrap_or(center.y - radius);
    let resistance = physics_api.compute_outward_resistance(
        center_cell,
        baseline,
        RESISTANCE_SAMPLE_RINGS,
        RESISTANCE_SAMPLE_STRIDE,
        RESISTANCE_CLIFF_THRESHOLD,
        RESISTANCE_MULTIPLIER,
    );
    let crush_cost = deformation.cost_crush_terrain * resistance;

    let mut remaining = impact_energy;
    let mut loose: Vec<(GranularMat, DisplacementSlot)> = Vec::new();

    for (cell, distance_sq) in cells_within(center, radius) {
        let underside = center.y - (radius * radius - distance_sq).sqrt() - extension;

        physics_api.break_ice(cell, underside);
        let (_, spent) = physics_api.crush_bedrock(cell, underside, remaining, crush_cost);
        remaining = (remaining - spent).max(0.0);

        if let Some((material, units)) = physics_api.granular_above(cell, underside) {
            loose.push((material, DisplacementSlot { cell, units }));
        }
    }

    let affordable = (remaining / deformation.cost_displace_granular).floor() as u32;
    let moved = shove_into_rim(
        physics_api,
        &loose,
        RimShape {
            center,
            inner: radius * sphere.rim_inner_radius_ratio,
            outer: radius * sphere.rim_expansion_factor,
            ceiling: center.y + radius * sphere.blade_ceiling_ratio,
            max_per_cell: sphere.max_rim_deposit_per_cell,
            heading: None,
        },
        affordable,
    );
    remaining = (remaining - moved as f32 * deformation.cost_displace_granular).max(0.0);

    impact_energy - remaining
}

/// Where shoved loose material may land around a sphere.
struct RimShape {
    center: Vec3,
    inner: f32,
    outer: f32,
    /// Deposits stop at this world height.
    ceiling: f32,
    max_per_cell: u16,
    /// When set, only columns beside the direction of travel take material, so a rolling
    /// sphere builds berms along its trail instead of refilling it behind itself.
    heading: Option<Vec3>,
}

/// Moves the dominant loose material among `loose` into the rim, at most `limit` units, and
/// returns the units moved. One material is moved at a time because mixing would transmute it.
fn shove_into_rim(
    physics_api: &mut GridPhysicsApi,
    loose: &[(GranularMat, DisplacementSlot)],
    rim: RimShape,
    limit: u32,
) -> u32 {
    let dominant = loose
        .iter()
        .map(|(material, _)| *material)
        .max_by_key(|candidate| {
            loose
                .iter()
                .filter(|(material, _)| material == candidate)
                .map(|(_, slot)| slot.units as u32)
                .sum::<u32>()
        });
    let Some(material) = dominant else {
        return 0;
    };

    let sources: Vec<DisplacementSlot> = loose
        .iter()
        .filter(|(candidate, _)| *candidate == material)
        .map(|(_, slot)| *slot)
        .collect();

    let sinks: Vec<DisplacementSlot> = cells_within(rim.center, rim.outer)
        .filter(|(cell, distance_sq)| {
            if *distance_sq < rim.inner * rim.inner {
                return false;
            }
            rim.heading.is_none_or(|heading| {
                let offset = Vec3::new(
                    cell.x as f32 - rim.center.x,
                    0.0,
                    -(cell.y as f32) - rim.center.z,
                );
                offset.normalize_or_zero().dot(heading).abs() <= RUT_BERM_SIDE_COSINE
            })
        })
        .map(|(cell, _)| DisplacementSlot {
            cell,
            units: physics_api.granular_room(cell, material, rim.ceiling, rim.max_per_cell),
        })
        .filter(|slot| slot.units > 0)
        .collect();

    physics_api.displace_granular(material, &sources, &sinks, limit)
}

/// Plane through the undisturbed ground around a sphere, in grid coordinates.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ReferencePlane {
    origin: Vec3,
    height: f32,
    slope_x: f32,
    slope_y: f32,
}

impl ReferencePlane {
    fn at(&self, cell: IVec2) -> f32 {
        self.height
            + self.slope_x * (cell.x as f32 - self.origin.x)
            + self.slope_y * (cell.y as f32 - self.origin.y)
    }

    /// The same plane expressed around a new origin.
    fn moved_to(self, origin: Vec3) -> Self {
        let height = self.height
            + self.slope_x * (origin.x - self.origin.x)
            + self.slope_y * (origin.y - self.origin.y);
        Self {
            origin,
            height,
            ..self
        }
    }

    fn blend(self, target: Self, weight: f32) -> Self {
        let from = self.moved_to(target.origin);
        Self {
            origin: target.origin,
            height: from.height + (target.height - from.height) * weight,
            slope_x: from.slope_x + (target.slope_x - from.slope_x) * weight,
            slope_y: from.slope_y + (target.slope_y - from.slope_y) * weight,
        }
    }

    /// Least-squares plane through floor samples on a ring around `center`, refitted once
    /// without the samples far from the first fit. Those lie in the sphere's own trail or on
    /// its berms; dropping them keeps the plane on undisturbed ground while following slopes.
    fn fit(physics_api: &GridPhysicsApi, center: Vec3, ring_radius: f32) -> Option<Self> {
        let origin = Vec3::new(center.x, -center.z, 0.0);
        let samples: Vec<(IVec2, f32)> = (0..RUT_REFERENCE_SAMPLES)
            .filter_map(|index| {
                let angle = index as f32 / RUT_REFERENCE_SAMPLES as f32 * std::f32::consts::TAU;
                let cell = IVec2::new(
                    (origin.x + angle.cos() * ring_radius).round() as i32,
                    (origin.y + angle.sin() * ring_radius).round() as i32,
                );
                Some((cell, physics_api.get_floor_height(cell)?))
            })
            .collect();

        let first = Self::solve(origin, &samples)?;
        let undisturbed: Vec<(IVec2, f32)> = samples
            .iter()
            .copied()
            .filter(|(cell, height)| (*height - first.at(*cell)).abs() <= RUT_TRIM_DEPTH)
            .collect();
        Self::solve(origin, &undisturbed).or(Some(first))
    }

    /// Solves the 3x3 normal equations of `h = height + slope_x * dx + slope_y * dy`.
    fn solve(origin: Vec3, samples: &[(IVec2, f32)]) -> Option<Self> {
        if samples.len() < RUT_MIN_REFERENCE_SAMPLES {
            return None;
        }
        let (mut n, mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let (mut sh, mut shx, mut shy) = (0.0, 0.0, 0.0);
        for &(cell, height) in samples {
            let dx = cell.x as f32 - origin.x;
            let dy = cell.y as f32 - origin.y;
            n += 1.0;
            sx += dx;
            sy += dy;
            sxx += dx * dx;
            syy += dy * dy;
            sxy += dx * dy;
            sh += height;
            shx += height * dx;
            shy += height * dy;
        }

        let determinant_3 = |a: [[f32; 3]; 3]| {
            a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
                - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
                + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
        };
        let matrix = [[n, sx, sy], [sx, sxx, sxy], [sy, sxy, syy]];
        let determinant = determinant_3(matrix);
        if determinant.abs() < PLANE_SINGULARITY {
            return None;
        }

        // Cramer's rule: replace one column with the right-hand side per unknown.
        let rhs = [sh, shx, shy];
        let solve_column = |column: usize| {
            let mut replaced = matrix;
            for row in 0..3 {
                replaced[row][column] = rhs[row];
            }
            determinant_3(replaced) / determinant
        };
        Some(Self {
            origin,
            height: solve_column(0),
            slope_x: solve_column(1),
            slope_y: solve_column(2),
        })
    }
}

/// How the ground under one sphere yields this substep.
///
/// Soft columns are treated as already sunk by their softness below the undisturbed
/// reference, so collision happens against the yielded surface and the sphere glides through
/// soft ground at its sunk depth instead of bumping against lips. The visible trail is then
/// pressed down to meet it. Columns already pressed to the reference depth yield no further,
/// which keeps a trail from deepening on repeated passes.
struct GroundModel {
    plane: Option<ReferencePlane>,
    sinkage_per_softness: f32,
    max_sinkage: f32,
}

impl GroundModel {
    /// Fits this substep's reference and filters it against the sphere's previous one. A
    /// sphere that was airborne starts afresh, since its old reference describes other ground.
    fn new(
        physics_api: &GridPhysicsApi,
        center: Vec3,
        sphere: &mut DynamicRigidSphere,
        deformation: &DeformationProfile,
        delta_time: f32,
    ) -> Self {
        let fitted = ReferencePlane::fit(
            physics_api,
            center,
            sphere.radius * RUT_REFERENCE_RING_RATIO,
        );
        let weight = 1.0 - (-delta_time / RUT_REFERENCE_SMOOTHING_SECONDS).exp();
        let plane = match (sphere.ground_reference, fitted) {
            (Some(previous), Some(fitted)) if sphere.is_grounded => {
                Some(previous.blend(fitted, weight))
            }
            (_, fitted) => fitted,
        };
        sphere.ground_reference = plane;

        let load = (sphere.mass / deformation.rut_reference_mass)
            .max(0.0)
            .sqrt();
        Self {
            plane,
            sinkage_per_softness: deformation.rut_depth_scale * load,
            max_sinkage: sphere.radius * MAX_SINKAGE_RATIO,
        }
    }

    /// How far a column can sink and the height below the reference it bottoms out at, or
    /// `None` where it does not yield.
    fn column_yield(&self, physics_api: &GridPhysicsApi, cell: IVec2) -> Option<(f32, f32)> {
        let plane = self.plane?;
        let sinkage =
            (physics_api.ground_softness(cell) * self.sinkage_per_softness).min(self.max_sinkage);
        (sinkage > 0.0).then(|| (sinkage, plane.at(cell) - sinkage))
    }

    fn yield_floor(&self, physics_api: &GridPhysicsApi, cell: IVec2) -> Option<f32> {
        self.column_yield(physics_api, cell).map(|(_, floor)| floor)
    }

    /// Height the sphere collides with. A soft column gives way by up to its sinkage but not
    /// below its yield floor, so a fresh column sinks fully, a half-pressed one only the rest
    /// of the way, a finished trail not at all, and a mound only partially.
    fn collision_height(&self, physics_api: &GridPhysicsApi, cell: IVec2) -> Option<f32> {
        let top = physics_api.get_floor_height(cell)?;
        Some(match self.column_yield(physics_api, cell) {
            Some((sinkage, floor)) if top > floor => (top - sinkage).max(floor),
            _ => top,
        })
    }
}

/// Presses the visible trail down to where the sphere actually rides, and returns the kinetic
/// energy the work cost.
///
/// Only columns the sphere's underside has sunk into are pressed, each to the lower of the
/// underside and its yield floor, so the trail takes the sphere's rounded profile. Loose
/// material is shoved into berms beside the trail and conserved; bare soft bedrock compacts.
fn press_rut(
    physics_api: &mut GridPhysicsApi,
    ground: &GroundModel,
    center: Vec3,
    sphere: &DynamicRigidSphere,
    deformation: &DeformationProfile,
) -> f32 {
    let radius = sphere.radius;
    let Some(plane) = ground.plane else {
        return 0.0;
    };

    let mut loose: Vec<(GranularMat, DisplacementSlot)> = Vec::new();
    let mut compacted = 0u32;
    for (cell, distance_sq) in cells_within(center, radius * deformation.rut_width_ratio) {
        let (Some(top), Some(yield_floor)) = (
            physics_api.get_floor_height(cell),
            ground.yield_floor(physics_api, cell),
        ) else {
            continue;
        };
        let underside = center.y - (radius * radius - distance_sq).sqrt();
        let pressed_to = underside.max(yield_floor);
        if top <= pressed_to {
            continue;
        }

        match physics_api.granular_above(cell, pressed_to) {
            Some((material, units)) => loose.push((
                material,
                DisplacementSlot {
                    cell,
                    units: units.min(deformation.rut_press_per_substep),
                },
            )),
            None => {
                compacted += physics_api.compact_bedrock(
                    cell,
                    pressed_to,
                    deformation.rut_press_per_substep,
                ) as u32;
            }
        }
    }

    let horizontal = Vec3::new(sphere.velocity.x, 0.0, sphere.velocity.z);
    let heading = (horizontal.length() > RUT_HEADING_MIN_SPEED).then(|| horizontal.normalize());
    let shoved = shove_into_rim(
        physics_api,
        &loose,
        RimShape {
            center,
            inner: radius * RUT_BERM_INNER_RATIO,
            outer: radius * RUT_BERM_OUTER_RATIO,
            ceiling: plane.at(nearest_cell(center.x, center.z)) + radius * RUT_BERM_HEIGHT_RATIO,
            max_per_cell: sphere.max_rim_deposit_per_cell,
            heading,
        },
        u32::MAX,
    );

    (shoved + compacted) as f32 * deformation.rut_cost_per_unit
}

fn resolve_terrain_contacts(
    spheres: &mut SphereQuery<'_, '_>,
    physics_api: &mut GridPhysicsApi,
    global_config: &GlobalPhysicsConfig,
    delta_time: f32,
) {
    for (mut transform, mut sphere, kinematic, deformation) in spheres.iter_mut() {
        let radius = sphere.radius;
        let mut center = transform.translation;
        let was_grounded = sphere.is_grounded;

        // Terrain raised under a body (brush strokes, neighbouring craters) can bury its centre,
        // where closest-point normals would point back into the ground. Lift it out first.
        if let Some(surface) = physics_api.get_interpolated_floor_height(center.x, center.z) {
            center.y = center.y.max(surface);
        }

        for (cell, distance_sq) in cells_within(center, radius) {
            let underside = center.y - (radius * radius - distance_sq).sqrt();
            physics_api.clear_surface_organics(cell, underside);
        }

        let ground = GroundModel::new(physics_api, center, &mut sphere, deformation, delta_time);

        // Ground deforms in two ways. A landing from the air craters it in proportion to the
        // impact energy. While the sphere stays in contact, its weight presses a bounded trail
        // into soft ground instead, so bumps met while rolling never crater the surface.
        // Touching counts with the same slop the velocity solver uses; resting contact hovers
        // within that band and would otherwise never register as bearing on the ground.
        if let Some(manifold) =
            probe_floor(physics_api, &ground, center, radius, radius + CONTACT_SLOP)
        {
            let ground_normal = manifold.ground_normal();
            if ground_normal.y >= GROUND_NORMAL_MIN_Y {
                let approach_speed = -sphere.velocity.dot(ground_normal);
                let impact_energy = 0.5 * sphere.mass * approach_speed * approach_speed;

                if !was_grounded
                    && approach_speed > 0.0
                    && impact_energy > deformation.min_deformation_energy
                {
                    let absorbed = carve_impact_crater(
                        physics_api,
                        center,
                        &sphere,
                        deformation,
                        impact_energy,
                    );
                    let retained = ((impact_energy - absorbed) / impact_energy).max(0.0).sqrt();
                    sphere.velocity += ground_normal * (approach_speed * (1.0 - retained));
                } else {
                    let spent = press_rut(physics_api, &ground, center, &sphere, deformation);
                    let kinetic_energy = 0.5 * sphere.mass * sphere.velocity.length_squared();
                    if spent > 0.0 && kinetic_energy > 0.0 {
                        sphere.velocity *=
                            ((kinetic_energy - spent).max(0.0) / kinetic_energy).sqrt();
                    }
                }
            }
        }

        for _ in 0..CONTACT_ITERATIONS {
            match probe_floor(physics_api, &ground, center, radius, radius) {
                Some(manifold) if manifold.deepest_depth > 0.0 => {
                    center += manifold.deepest_normal * manifold.deepest_depth;
                }
                _ => break,
            }
        }

        match probe_floor(physics_api, &ground, center, radius, radius + CONTACT_SLOP) {
            Some(manifold) => {
                resolve_contact_velocity(
                    &mut sphere,
                    kinematic,
                    global_config,
                    &manifold,
                    delta_time,
                );
                let ground_normal = manifold.ground_normal();
                sphere.is_grounded = ground_normal.y >= GROUND_NORMAL_MIN_Y;
                sphere.ground_normal = if sphere.is_grounded {
                    ground_normal
                } else {
                    Vec3::Y
                };
            }
            None => {
                sphere.is_grounded = false;
                sphere.ground_normal = Vec3::Y;
            }
        }

        transform.translation = center;
    }
}

/// Pushes liquid out of a sphere's way: a splash crown when it plunges in, and a bow wave while
/// it ploughs along the surface. Displaced liquid is only moved, never removed, and the liquid
/// solver lets it rush back into the cavity afterwards.
fn displace_liquids(
    spheres: &mut SphereQuery<'_, '_>,
    physics_api: &mut GridPhysicsApi,
    delta_time: f32,
) {
    for (transform, mut sphere, _, _) in spheres.iter_mut() {
        let center = transform.translation;
        let radius = sphere.radius;
        let center_cell = IVec2::new(center.x.round() as i32, (-center.z).round() as i32);
        let Some((fluid, _, surface)) = physics_api.fluid_column(center_cell) else {
            sphere.pending_wake_volume = 0.0;
            continue;
        };

        let entering = sphere.previous_submerged_fraction <= 0.0 && sphere.submerged_fraction > 0.0;
        let plunge_speed = -sphere.velocity.y;

        if entering && plunge_speed > SPLASH_MIN_SPEED {
            let entry_energy = 0.5 * sphere.mass * plunge_speed * plunge_speed;
            let cavity_radius = radius * SPLASH_CAVITY_RATIO;

            // The cavity is scooped deepest under the centre and tapers to its edge.
            let sources: Vec<DisplacementSlot> = cells_within(center, cavity_radius)
                .filter_map(|(cell, distance_sq)| {
                    let (material, volume, _) = physics_api.fluid_column(cell)?;
                    let taper = 1.0 - distance_sq.sqrt() / cavity_radius;
                    let units = (volume as f32 * taper) as u16;
                    (material == fluid && units > 0).then_some(DisplacementSlot { cell, units })
                })
                .collect();
            let sinks = ring_room(
                physics_api,
                center,
                cavity_radius,
                radius * SPLASH_RING_OUTER_RATIO,
                fluid,
                SPLASH_MAX_PER_CELL,
            );

            let budget = (entry_energy * SPLASH_UNITS_PER_ENERGY) as u32;
            let moved = physics_api.displace_fluid(fluid, &sources, &sinks, budget);
            let spent = (moved as f32 * SPLASH_ENERGY_PER_UNIT).min(entry_energy);
            sphere.velocity.y *= ((entry_energy - spent) / entry_energy).sqrt();
            continue;
        }

        // The bow wave comes from the waterline: the circle where the sphere cuts the surface.
        // A sphere riding half out of the water has the widest waterline; one just below the
        // surface or fully under has little or none, and barely stirs it.
        let height_above_surface = center.y - surface;
        let horizontal = Vec3::new(sphere.velocity.x, 0.0, sphere.velocity.z);
        let speed = horizontal.length();
        if height_above_surface.abs() >= radius || speed < WAKE_MIN_SPEED {
            sphere.pending_wake_volume = 0.0;
            continue;
        }
        let waterline_radius =
            (radius * radius - height_above_surface * height_above_surface).sqrt();

        // The chord sets how wide a strip of surface the sphere meets. Of that strip, the
        // share shoved aside rather than flowing over the top is the horizontal component of
        // the surface normal along the waterline, `waterline_radius / radius`: a broadside
        // flank at the equator, almost nothing across a nearly submerged crown.
        let waterline_chord = 2.0 * waterline_radius;
        let shoved_share = waterline_radius / radius;
        sphere.pending_wake_volume +=
            waterline_chord * shoved_share * speed * delta_time * WAKE_LAYER_UNITS;
        let budget = sphere.pending_wake_volume.floor() as u32;
        if budget == 0 {
            continue;
        }

        // Only liquid ahead of the waterline is shoved aside; behind it the liquid solver lets
        // the surface close up again on its own.
        let heading = horizontal / speed;
        let scale = physics_api.global_config.elevation_scale;
        let sources: Vec<DisplacementSlot> = cells_within(center, waterline_radius)
            .filter_map(|(cell, distance_sq)| {
                let offset = Vec3::new(cell.x as f32 - center.x, 0.0, -(cell.y as f32) - center.z);
                if offset.dot(heading) <= 0.0 {
                    return None;
                }
                let (material, volume, column_surface) = physics_api.fluid_column(cell)?;
                let underside = center.y - (radius * radius - distance_sq).sqrt();
                let overlap_units = ((column_surface - underside) / scale).floor().max(0.0) as u16;
                let units = overlap_units.min(volume).min(WAKE_MAX_PER_CELL);
                (material == fluid && units > 0).then_some(DisplacementSlot { cell, units })
            })
            .collect();
        let sinks = ring_room(
            physics_api,
            center,
            waterline_radius,
            waterline_radius + WAKE_RING_WIDTH,
            fluid,
            WAKE_MAX_RING_PER_CELL,
        );
        let moved = physics_api.displace_fluid(fluid, &sources, &sinks, budget);
        sphere.pending_wake_volume =
            (sphere.pending_wake_volume - moved as f32).min(WAKE_MAX_PENDING);
    }
}

/// Columns in the ring between `inner` and `outer` around `center` that can take `fluid`.
fn ring_room(
    physics_api: &GridPhysicsApi,
    center: Vec3,
    inner: f32,
    outer: f32,
    fluid: FluidMat,
    max_per_cell: u16,
) -> Vec<DisplacementSlot> {
    cells_within(center, outer)
        .filter(|(_, distance_sq)| *distance_sq > inner * inner)
        .map(|(cell, _)| DisplacementSlot {
            cell,
            units: physics_api.fluid_room(cell, fluid, max_per_cell),
        })
        .filter(|slot| slot.units > 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::{entity::Entity, system::RunSystemOnce, world::World};
    use rand::{RngExt, SeedableRng, rngs::SmallRng};
    use std::time::Duration;
    use world_model::{
        body::{self, METAL_SPHERE, WOOD_SPHERE},
        cell::WorldCell,
        material::TerrainMat,
    };

    const GRID_SIZE: i32 = 64;
    const RADIUS: f32 = METAL_SPHERE.radius;
    const MASS: f32 = METAL_SPHERE.mass;
    const FRAME_SECONDS: f32 = 1.0 / 60.0;
    const STEEL_DENSITY: f32 = METAL_SPHERE.relative_density;
    const WOOD_DENSITY: f32 = WOOD_SPHERE.relative_density;
    /// The possession controls' drive acceleration and top speed.
    const DRIVE_ACCELERATION: f32 = body::DRIVE_ACCELERATION;
    const MAX_DRIVE_SPEED: f32 = body::DRIVE_MAX_SPEED;

    fn scale() -> f32 {
        GlobalPhysicsConfig::default().elevation_scale
    }

    fn world_with_terrain(shape: impl Fn(i32, i32, &mut WorldCell)) -> World {
        let mut grid = ActiveWorldGrid::new(GRID_SIZE, GRID_SIZE, IVec2::ZERO);
        for y in 0..GRID_SIZE {
            for x in 0..GRID_SIZE {
                let pos = IVec2::new(x, y);
                let mut cell = grid.get_cell(pos);
                cell.set_terrain_mat(TerrainMat::TERRAIN_DIRT);
                shape(x, y, &mut cell);
                grid.set_cell(pos, cell);
            }
        }

        let mut world = World::new();
        world.insert_resource(grid);
        world.insert_resource(GlobalPhysicsConfig::default());
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(FRAME_SECONDS));
        world.insert_resource(time);
        world
    }

    fn spawn_sphere(world: &mut World, position: Vec3, relative_density: f32) -> Entity {
        world
            .spawn((
                Transform::from_translation(position),
                DynamicRigidSphere::new(MASS, RADIUS, relative_density),
                KinematicProfile::default(),
                DeformationProfile::default(),
            ))
            .id()
    }

    fn step(world: &mut World) {
        world
            .run_system_once(simulate_rigid_sphere_kinematics)
            .expect("sphere system must run");
    }

    fn position(world: &World, entity: Entity) -> Vec3 {
        world
            .get::<Transform>(entity)
            .expect("sphere has a transform")
            .translation
    }

    fn body(world: &World, entity: Entity) -> DynamicRigidSphere {
        world
            .get::<DynamicRigidSphere>(entity)
            .expect("sphere component exists")
            .clone()
    }

    fn push(world: &mut World, entity: Entity, delta_velocity: Vec3) {
        world
            .get_mut::<DynamicRigidSphere>(entity)
            .expect("sphere component exists")
            .velocity += delta_velocity;
    }

    /// Mirrors the possession controls: accelerate along +X up to their top speed.
    fn drive(world: &mut World, entity: Entity, acceleration: f32) {
        let speed = body(world, entity).velocity.x;
        let gain = (acceleration * FRAME_SECONDS)
            .min(MAX_DRIVE_SPEED - speed)
            .max(0.0);
        push(world, entity, Vec3::X * gain);
    }

    fn grid_sum(world: &World, measure: impl Fn(&WorldCell) -> u64) -> u64 {
        world
            .resource::<ActiveWorldGrid>()
            .spatial
            .cells
            .iter()
            .map(measure)
            .sum()
    }

    #[test]
    fn closest_point_respects_triangle_regions() {
        let (a, b, c) = (Vec3::ZERO, Vec3::X * 2.0, Vec3::NEG_Z * 2.0);
        let above_face = closest_point_on_triangle(Vec3::new(0.5, 3.0, -0.5), a, b, c);
        assert!(above_face.distance(Vec3::new(0.5, 0.0, -0.5)) < 1e-6);
        assert_eq!(
            closest_point_on_triangle(Vec3::new(-1.0, 1.0, 1.0), a, b, c),
            a
        );
        let beside_edge = closest_point_on_triangle(Vec3::new(1.0, 0.0, 1.0), a, b, c);
        assert!(beside_edge.distance(Vec3::new(1.0, 0.0, 0.0)) < 1e-6);
    }

    #[test]
    fn submerged_fraction_matches_cap_volume() {
        assert_eq!(submerged_volume_fraction(0.0, RADIUS), 0.0);
        assert!((submerged_volume_fraction(RADIUS, RADIUS) - 0.5).abs() < 1e-6);
        assert!((submerged_volume_fraction(2.0 * RADIUS, RADIUS) - 1.0).abs() < 1e-6);
        assert_eq!(submerged_volume_fraction(3.0 * RADIUS, RADIUS), 1.0);
    }

    #[test]
    fn resting_sphere_settles_into_soft_ground_without_drifting() {
        let mut world = world_with_terrain(|_, _, cell| cell.set_elevation(40));
        let surface = 40.0 * scale();
        let start = Vec3::new(32.3, surface + RADIUS, -32.6);
        let sphere = spawn_sphere(&mut world, start, STEEL_DENSITY);

        for _ in 0..180 {
            step(&mut world);
        }

        let end = position(&world, sphere);
        let sinkage = start.y - end.y;
        assert!(body(&world, sphere).is_grounded);
        assert!(Vec3::new(end.x - start.x, 0.0, end.z - start.z).length() < 0.05);
        assert!(
            (0.0..=DeformationProfile::default().rut_depth_scale + 0.25).contains(&sinkage),
            "sank {sinkage} into soil"
        );
    }

    #[test]
    fn sphere_rolls_down_a_slope() {
        // One elevation unit per cell is a rise over run of 0.5, past the static friction limit.
        // Rock does not yield, so the sphere rolls without leaving a trail.
        let mut world = world_with_terrain(|x, _, cell| {
            cell.set_terrain_mat(TerrainMat::TERRAIN_STONE);
            cell.set_elevation((120 - x) as u16);
        });
        let start = Vec3::new(16.0, (120.0 - 16.0) * scale() + RADIUS * 1.15, -32.0);
        let sphere = spawn_sphere(&mut world, start, STEEL_DENSITY);
        let soil_before = grid_sum(&world, |c| c.elevation() as u64);

        for _ in 0..90 {
            step(&mut world);
        }

        let end = position(&world, sphere);
        assert!(body(&world, sphere).is_grounded);
        assert!(end.x > start.x + 3.0, "sphere stayed at {end}");
        assert!((end.z - start.z).abs() < 0.5, "sphere veered to {end}");
        assert_eq!(
            grid_sum(&world, |c| c.elevation() as u64),
            soil_before,
            "rolling must not dig"
        );
    }

    #[test]
    fn driven_sphere_crosses_bumpy_ground_without_hopping() {
        // One-unit cell-to-cell noise, rougher than the generated landscape's smooth relief.
        const BUMP_SEED: u64 = 0xB0B;
        const BASE: u16 = 40;
        const RELIEF: u16 = 2;
        let mut rng = SmallRng::seed_from_u64(BUMP_SEED);
        let bumps: Vec<u16> = (0..GRID_SIZE * GRID_SIZE)
            .map(|_| BASE + rng.random_range(0..RELIEF))
            .collect();
        let mut world = world_with_terrain(|x, y, cell| {
            cell.set_elevation(bumps[(y * GRID_SIZE + x) as usize]);
        });
        let start = Vec3::new(8.0, (BASE + RELIEF) as f32 * scale() + RADIUS, -32.0);
        let sphere = spawn_sphere(&mut world, start, STEEL_DENSITY);
        for _ in 0..30 {
            step(&mut world);
        }

        let finish_line = 52.0;
        let mut frames = 0;
        let mut grounded_frames = 0;
        while position(&world, sphere).x < finish_line && frames < 120 {
            drive(&mut world, sphere, DRIVE_ACCELERATION);
            step(&mut world);
            frames += 1;
            grounded_frames += body(&world, sphere).is_grounded as usize;
        }

        assert!(
            position(&world, sphere).x >= finish_line,
            "sphere stalled at {} after {frames} frames",
            position(&world, sphere)
        );
        assert!(
            grounded_frames * 10 >= frames * 9,
            "sphere was airborne for {} of {frames} frames",
            frames - grounded_frames
        );
    }

    #[test]
    fn full_speed_on_flat_rock_never_leaves_the_ground() {
        let mut world = world_with_terrain(|_, _, cell| {
            cell.set_terrain_mat(TerrainMat::TERRAIN_STONE);
            cell.set_elevation(40);
        });
        let sphere = spawn_sphere(
            &mut world,
            Vec3::new(4.0, 20.0 + RADIUS, -32.0),
            STEEL_DENSITY,
        );

        while position(&world, sphere).x < 56.0 {
            drive(&mut world, sphere, DRIVE_ACCELERATION);
            step(&mut world);
            let state = body(&world, sphere);
            assert!(state.is_grounded, "hopped at {}", position(&world, sphere));
            assert!(
                state.velocity.y.abs() < 0.05,
                "vertical kick {}",
                state.velocity.y
            );
        }
    }

    #[test]
    fn rolling_leaves_a_bounded_trail_with_conserved_berms() {
        const BED: u16 = 40;
        const SAND: u16 = 4;
        let mut world = world_with_terrain(|_, _, cell| {
            cell.set_elevation(BED);
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(SAND);
        });
        let surface = (BED + SAND) as f32 * scale();
        let sphere = spawn_sphere(
            &mut world,
            Vec3::new(8.0, surface + RADIUS, -32.0),
            STEEL_DENSITY,
        );
        let sand_before = grid_sum(&world, |c| c.granular_vol() as u64);

        while position(&world, sphere).x < 48.0 {
            drive(&mut world, sphere, DRIVE_ACCELERATION);
            step(&mut world);
        }

        let grid = world.resource::<ActiveWorldGrid>();
        let trail_depth = (16..40)
            .map(|x| (BED + SAND) as u32 - grid.get_cell(IVec2::new(x, 32)).solid_top())
            .max()
            .unwrap_or_default();
        let deepest = (0..GRID_SIZE * GRID_SIZE)
            .map(|i| {
                grid.get_cell(IVec2::new(i % GRID_SIZE, i / GRID_SIZE))
                    .solid_top()
            })
            .min()
            .unwrap_or_default();
        let sand_softness_units = (1.5 / scale()).ceil() as u32;

        assert!(trail_depth > 0, "rolling over sand left no trail");
        assert!(
            (BED + SAND) as u32 - deepest <= sand_softness_units,
            "trail dug to {deepest}"
        );
        assert_eq!(grid_sum(&world, |c| c.granular_vol() as u64), sand_before);
    }

    #[test]
    fn driven_sphere_climbs_a_hill() {
        const LOW: i32 = 40;
        const RAMP_START: i32 = 20;
        const RAMP_END: i32 = 36;
        let height_at = |x: i32| (LOW + (x - RAMP_START).clamp(0, RAMP_END - RAMP_START)) as u16;
        let mut world = world_with_terrain(|x, _, cell| cell.set_elevation(height_at(x)));
        let start = Vec3::new(8.0, LOW as f32 * scale() + RADIUS, -32.0);
        let sphere = spawn_sphere(&mut world, start, STEEL_DENSITY);
        let plateau_surface = height_at(RAMP_END) as f32 * scale();

        let mut reached_plateau = false;
        for _ in 0..180 {
            let now = position(&world, sphere);
            if now.x > (RAMP_END + 4) as f32 && now.y > plateau_surface {
                reached_plateau = true;
                break;
            }
            push(
                &mut world,
                sphere,
                Vec3::X * DRIVE_ACCELERATION * 0.5 * FRAME_SECONDS,
            );
            step(&mut world);
        }

        assert!(
            reached_plateau,
            "sphere stuck at {}",
            position(&world, sphere)
        );
    }

    #[test]
    fn hard_landing_craters_and_conserves_sand() {
        const SAND: u16 = 6;
        let mut world = world_with_terrain(|_, _, cell| {
            cell.set_elevation(40);
            cell.set_granular_mat(GranularMat::GRANULAR_SAND);
            cell.set_granular_vol(SAND);
        });
        let floor = (40 + SAND) as f32 * scale();
        let impact_point = IVec2::new(32, 32);
        let sphere = spawn_sphere(
            &mut world,
            Vec3::new(32.0, floor + 30.0, -32.0),
            STEEL_DENSITY,
        );
        let sand_before = grid_sum(&world, |c| c.granular_vol() as u64);

        for _ in 0..120 {
            step(&mut world);
        }

        assert!(body(&world, sphere).is_grounded);
        assert_eq!(grid_sum(&world, |c| c.granular_vol() as u64), sand_before);
        let crater_floor = world.resource::<ActiveWorldGrid>().get_cell(impact_point);
        assert!(
            crater_floor.granular_vol() < SAND,
            "no crater under the impact"
        );
    }

    #[test]
    fn light_sphere_floats_and_dense_sphere_sinks() {
        let bed_elevation = 20u16;
        let water_volume = 100u16;
        let mut world = world_with_terrain(|_, _, cell| {
            cell.set_elevation(bed_elevation);
            cell.set_fluid_mat(FluidMat::FLUID_WATER);
            cell.set_fluid_vol(water_volume);
        });
        let floor = bed_elevation as f32 * scale();
        let surface = (bed_elevation + water_volume) as f32 * scale();

        let wood = spawn_sphere(
            &mut world,
            Vec3::new(16.0, surface + 1.0, -32.0),
            WOOD_DENSITY,
        );
        let steel = spawn_sphere(
            &mut world,
            Vec3::new(48.0, surface + 1.0, -32.0),
            STEEL_DENSITY,
        );

        for _ in 0..600 {
            step(&mut world);
        }

        let wood_body = body(&world, wood);
        assert!(!wood_body.is_grounded);
        assert!(
            (wood_body.submerged_fraction - WOOD_DENSITY).abs() < 0.05,
            "floating equilibrium displaces its own weight, got {}",
            wood_body.submerged_fraction
        );

        let steel_body = body(&world, steel);
        assert!(steel_body.is_grounded);
        assert!(position(&world, steel).y <= floor + RADIUS + 0.1);
    }

    #[test]
    fn wake_scales_with_how_much_of_the_sphere_cuts_the_surface() {
        const BED: u16 = 20;
        const DEPTH: u16 = 100;
        let surface = (BED + DEPTH) as f32 * scale();

        // Neutral-ish buoyancy holds a sphere just under the surface; half density rides
        // half out of it.
        let stirred_volume = |relative_density: f32| -> u64 {
            let mut world = world_with_terrain(|_, _, cell| {
                cell.set_elevation(BED);
                cell.set_fluid_mat(FluidMat::FLUID_WATER);
                cell.set_fluid_vol(DEPTH);
            });
            let sphere = spawn_sphere(
                &mut world,
                Vec3::new(12.0, surface, -32.0),
                relative_density,
            );
            for _ in 0..240 {
                step(&mut world);
            }
            for _ in 0..60 {
                let speed = body(&world, sphere).velocity.x;
                push(&mut world, sphere, Vec3::X * (8.0 - speed));
                step(&mut world);
            }
            grid_sum(&world, |c| c.fluid_vol().abs_diff(DEPTH) as u64)
        };

        let barely_covered = stirred_volume(0.97);
        let half_out = stirred_volume(0.5);
        assert!(
            half_out > 0,
            "a sphere ploughing through the surface raised no wave"
        );
        assert!(
            barely_covered * 3 < half_out,
            "nearly submerged sphere stirred {barely_covered} units vs {half_out} at the surface"
        );
    }

    #[test]
    fn plunging_sphere_splashes_without_losing_water() {
        const BED: u16 = 20;
        const DEPTH: u16 = 40;
        let mut world = world_with_terrain(|_, _, cell| {
            cell.set_elevation(BED);
            cell.set_fluid_mat(FluidMat::FLUID_WATER);
            cell.set_fluid_vol(DEPTH);
        });
        let surface = (BED + DEPTH) as f32 * scale();
        let impact_point = IVec2::new(32, 32);
        spawn_sphere(
            &mut world,
            Vec3::new(32.0, surface + 20.0, -32.0),
            STEEL_DENSITY,
        );
        let water_before = grid_sum(&world, |c| c.fluid_vol() as u64);

        let mut deepest_cavity = DEPTH;
        for _ in 0..90 {
            step(&mut world);
            assert_eq!(grid_sum(&world, |c| c.fluid_vol() as u64), water_before);
            let at_impact = world.resource::<ActiveWorldGrid>().get_cell(impact_point);
            deepest_cavity = deepest_cavity.min(at_impact.fluid_vol());
        }

        assert!(
            deepest_cavity < DEPTH / 2,
            "the splash left the surface undisturbed"
        );
    }
}
