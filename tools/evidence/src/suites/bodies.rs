//! The `bodies` GPU suite: sphere bodies on the GPU world, coupled to its terrain and liquids,
//! executed on the selected adapter.
//!
//! The baseline's twelve sphere tests and its apportionment test keep their scenarios and
//! tolerances ([`intentions`], [`probes`]); the other cases cover what the GPU solver adds:
//! identity and the fixed clock, a complete broadphase against an exhaustive oracle, clusters,
//! fast bodies, shared deformation, the residency edge, commands and rays, and the CPU's
//! observations.

pub mod adversarial;
pub mod consumer;
pub mod identity;
pub mod intentions;
pub mod measure;
pub mod oracle;
pub mod probes;
pub mod rays;
pub mod scene;

use super::Case;

pub const CASES: &[Case] = &[
    Case {
        name: "closest-point-regions",
        summary: "the WGSL closest point lies on the face, vertex or edge, against an independent f64 reference",
        run: probes::closest_point_regions,
    },
    Case {
        name: "cap-volume",
        summary: "the WGSL submerged fraction is 0, 1/2 and 1 at 0, r and 2r and matches a numerical integral",
        run: probes::cap_volume,
    },
    Case {
        name: "apportionment",
        summary: "the GPU split of displaced units is exact, fits every slot and equals world-model's",
        run: probes::apportionment,
    },
    Case {
        name: "broadphase-oracle",
        summary: "bins visit every pair an exhaustive bound requires and find the all-pairs contacts",
        run: oracle::broadphase_oracle,
    },
    Case {
        name: "body-handles",
        summary: "handles carry generations: capacity refusal, stale after removal, reuse, freed failed spawns, gone samples",
        run: identity::body_handles,
    },
    Case {
        name: "fixed-tick-substeps",
        summary: "ticks split into substeps of at most 1/120 s at 10-240 Hz, free fall matches its scheme, speed is capped at 600",
        run: identity::fixed_tick_substeps,
    },
    Case {
        name: "body-presentation-rates",
        summary: "scheduled spawns and drives at 30, 60, 144 Hz and with stalls give bit-identical bodies at one tick",
        run: identity::body_presentation_rates,
    },
    Case {
        name: "resting-soft-ground",
        summary: "a steel sphere settles into soil without drifting and sinks no deeper than its rut scale",
        run: intentions::resting_soft_ground,
    },
    Case {
        name: "slope-roll",
        summary: "a sphere rolls down a rock slope without veering or digging",
        run: intentions::slope_roll,
    },
    Case {
        name: "bumpy-drive",
        summary: "a driven sphere crosses one-unit bumps grounded on at least 90% of its ticks",
        run: intentions::bumpy_drive,
    },
    Case {
        name: "flat-rock-full-speed",
        summary: "driven at full speed on flat rock a sphere never leaves the ground or kicks upward",
        run: intentions::flat_rock_full_speed,
    },
    Case {
        name: "rut-conserved",
        summary: "rolling over sand leaves a bounded trail and berms that conserve the sand",
        run: intentions::rut_conserved,
    },
    Case {
        name: "hill-climb",
        summary: "a pushed sphere climbs a ramp to its plateau",
        run: intentions::hill_climb,
    },
    Case {
        name: "crater-conserved",
        summary: "a hard landing craters sand, conserves it and ends grounded",
        run: intentions::crater_conserved,
    },
    Case {
        name: "float-and-sink",
        summary: "wood floats at its density's submerged share; steel sinks to the bed",
        run: intentions::float_and_sink,
    },
    Case {
        name: "wake-scaling",
        summary: "a half-submerged sphere raises a wave more than three times a barely covered one's",
        run: intentions::wake_scaling,
    },
    Case {
        name: "splash-conserved",
        summary: "a plunging sphere opens a deep cavity and conserves water every tick",
        run: intentions::splash_conserved,
    },
    Case {
        name: "pair-collision",
        summary: "bodies meeting in mid-air keep their momentum, gain no energy and part at the restitution",
        run: adversarial::pair_collision,
    },
    Case {
        name: "coincident-centres",
        summary: "bodies spawned at one centre separate without a non-finite value and rest apart",
        run: adversarial::coincident_centres,
    },
    Case {
        name: "dense-cluster",
        summary: "512 mixed-radius bodies pile on sand: bounded overlap, sand kept, bins equal all pairs bit for bit",
        run: adversarial::dense_cluster,
    },
    Case {
        name: "fast-bodies",
        summary: "bodies at the speed limit neither pass through each other nor through a one-cell wall",
        run: adversarial::fast_bodies,
    },
    Case {
        name: "shared-deformation",
        summary: "contested trails, berms and splashes with strokes on the same cells account for every unit",
        run: adversarial::shared_deformation,
    },
    Case {
        name: "edit-under-resting-body",
        summary: "raised ground lifts a resting body onto it; lowered ground lets it fall and rest",
        run: adversarial::edit_under_resting_body,
    },
    Case {
        name: "residency-edge",
        summary: "bodies suspend with their state at missing terrain, block others, resume, and survive the window moving",
        run: adversarial::residency_edge,
    },
    Case {
        name: "ray-queries",
        summary: "rays meet the collision surface where an f64 reference does, miss, leave residency at its edge, and report bodies",
        run: rays::ray_queries,
    },
    Case {
        name: "ray-targeted-commands",
        summary: "edits, spawns, attraction and lift act at a ray's GPU-resolved hit; acknowledged edits are observed; misses change nothing",
        run: rays::ray_targeted_commands,
    },
    Case {
        name: "controls",
        summary: "drive, jump, remove and spawn through runtime receipts: retries apply once, stale handles and a full table are refused",
        run: consumer::controls,
    },
    Case {
        name: "observation-delivery",
        summary: "a consumer drains body events late from a saturated ring and still sees every counter once; replays in hostile orders agree",
        run: consumer::observation_delivery,
    },
    Case {
        name: "body-scaling-bins",
        summary: "64 to 4096 bodies, distributed and in a dense attracted pile, with the bins: body pass time, pairs, residuals, bytes, CPU work",
        run: measure::scaling_bins,
    },
    Case {
        name: "body-scaling-all-pairs",
        summary: "the same workloads with the all-pairs broadphase, for comparison",
        run: measure::scaling_all_pairs,
    },
    Case {
        name: "body-legacy-workloads",
        summary: "the legacy baseline's frozen body-contact scenarios (64 and 256 spheres) replayed tick for tick on the GPU",
        run: measure::legacy_workloads,
    },
    Case {
        name: "body-pacing",
        summary: "the runtime at 60 Hz of wall time with 256 and 4096 bodies: ticks kept and discarded, pressure, latency, sample age",
        run: measure::pacing,
    },
    Case {
        name: "capacity-tick-phases",
        summary: "the capacity profile's idle and body-contact scenarios at radius 32 replayed tick by tick: device time of each phase of a tick and the tiles with an active cell (measurement)",
        run: measure::capacity_tick_phases,
    },
];
