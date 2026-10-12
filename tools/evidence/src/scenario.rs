//! Frozen measurement scenarios: schema, validation against the profile definitions, and
//! identity.
//!
//! A scenario file fixes everything a run depends on: seed, active-world size, simulation
//! settings, presentation, camera, bodies and ordered actions. Physical actions are scheduled
//! by logical simulation tick rather than by frame, so what a run simulates does not depend on
//! how fast it renders. The measurement window is counted in ticks too: every implementation
//! measures the same simulated content, and renders however many frames it can in that time.
//!
//! Files live at `tools/evidence/scenarios/<profile>/<case>.toml`. Validation rejects a file
//! whose world size, resolution, tick rate or body count differs from its profile, so a case
//! cannot quietly run an easier world.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use world_model::{
    body::{self, BodyHandle},
    command::{
        AmbientTemperature, BodySpawn, Command, CommandError, Direction, EditTool, MaterialEdit,
    },
    config,
    glam::IVec2,
};

use crate::identity::sha256_hex;

pub const SCENARIO_SCHEMA: u32 = 1;

/// Measurement cases every profile defines (performance budgets, `Profiles`).
pub const CASES: [&str; 6] = [
    "idle",
    "flow",
    "body-contact",
    "mixed",
    "streaming",
    "coupled",
];

/// Frozen profile parameters from `docs/reference/performance-budgets.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileSpec {
    pub name: &'static str,
    pub active_radius: u32,
    /// Bodies in every case that has bodies.
    pub bodies: u32,
    pub resolution: [u32; 2],
    pub tick_rate_hz: u32,
}

pub const PROFILES: [ProfileSpec; 3] = [
    ProfileSpec {
        name: "qualification",
        active_radius: 6,
        bodies: 64,
        resolution: [1920, 1080],
        tick_rate_hz: 60,
    },
    ProfileSpec {
        name: "scale",
        active_radius: 16,
        bodies: 256,
        resolution: [1920, 1080],
        tick_rate_hz: 60,
    },
    ProfileSpec {
        name: "capacity",
        active_radius: 32,
        bodies: 4096,
        resolution: [1920, 1080],
        tick_rate_hz: 60,
    },
];

pub fn profile(name: &str) -> Option<ProfileSpec> {
    PROFILES.into_iter().find(|profile| profile.name == name)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub schema: u32,
    pub profile: String,
    pub case: String,
    pub description: String,
    pub world: World,
    pub presentation: Presentation,
    pub camera: Camera,
    pub run: RunPlan,
    #[serde(default)]
    pub bodies: Vec<BodyGroup>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct World {
    pub seed: u32,
    /// Chunks on each side of the focus chunk, per axis.
    pub active_radius_chunks: [u32; 2],
    pub tick_rate_hz: f64,
    pub max_ticks_per_frame: u32,
    pub ambient_temperature_c: f32,
    /// World units per elevation unit.
    pub elevation_scale: f32,
    /// Enabled rule families; the others are switched off.
    pub families: Vec<Family>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Liquid,
    Granular,
    Thermal,
    Fire,
    Biology,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Presentation {
    /// Requested window size in physical pixels.
    pub resolution: [u32; 2],
    pub present_mode: PresentMode,
    /// Whether the developer overlay (tool bar, panels, compass) is drawn.
    pub overlay: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresentMode {
    /// Uncapped presentation, so frame time reflects work rather than the display refresh.
    NoVsync,
    Vsync,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    /// Cell coordinates under the observer at tick 0; the residency focus follows them.
    pub start_cell: [i32; 2],
    /// Observer height in world units.
    pub height: f32,
    pub yaw_degrees: f32,
    pub pitch_degrees: f32,
    /// Optional closed loop of cells the observer travels along, starting at `path[0]`.
    #[serde(default)]
    pub path: Vec<[i32; 2]>,
    /// Travel speed along `path`, in cells per simulation tick.
    #[serde(default)]
    pub cells_per_tick: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunPlan {
    /// Independent repetitions of the case.
    pub runs: u32,
    /// Simulation ticks run after the active window is resident and before measurement starts.
    pub warmup_ticks: u64,
    /// Simulation ticks in the measured window (60 ticks are one simulated second).
    pub measured_ticks: u64,
    /// Longest wait for the initial active window to become resident.
    pub residency_timeout_seconds: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BodyGroup {
    pub preset: BodyPreset,
    pub count: u32,
    /// Cell of the first body; the rest fill rows of `columns` toward +x then +y.
    pub origin_cell: [i32; 2],
    pub columns: u32,
    pub spacing_cells: f32,
    /// Spawn height above the column's solid top, in world units (the tool uses 10).
    pub drop_height: f32,
    pub at_tick: u64,
}

impl BodyGroup {
    /// The group's spawns in spawn order: rows of `columns` bodies toward +x, then +y.
    pub fn spawns(&self) -> Result<Vec<BodySpawn>, CommandError> {
        let spacing = self.spacing_cells as i32;
        (0..self.count as i32)
            .map(|index| {
                let columns = self.columns as i32;
                let column = IVec2::from(self.origin_cell)
                    + IVec2::new(index % columns, index / columns) * spacing;
                BodySpawn::new(self.preset.model(), column, self.drop_height)
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BodyPreset {
    Metal,
    Wood,
}

impl BodyPreset {
    pub fn model(self) -> body::BodyPreset {
        match self {
            Self::Metal => body::BodyPreset::Metal,
            Self::Wood => body::BodyPreset::Wood,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields, tag = "kind", rename_all = "kebab-case")]
pub enum Action {
    /// Applies a brush once per tick in `[from_tick, to_tick]` at `every_ticks` intervals.
    Brush {
        tool: BrushTool,
        cell: [i32; 2],
        radius: i32,
        strength: u8,
        from_tick: u64,
        to_tick: u64,
        every_ticks: u64,
    },
    /// Accelerates every body toward the target, alternating targets when `alternate_cell` is
    /// set.
    Attract {
        cell: [i32; 2],
        #[serde(default)]
        alternate_cell: Option<[i32; 2]>,
        #[serde(default)]
        alternate_every_ticks: u64,
        from_tick: u64,
        to_tick: u64,
    },
    /// Lifts bodies near the target.
    Lift {
        cell: [i32; 2],
        from_tick: u64,
        to_tick: u64,
    },
    /// Sets the ambient temperature at `at_tick`, as the climate slider does. The baseline
    /// wakes the whole window on every change.
    Climate {
        ambient_temperature_c: f32,
        at_tick: u64,
    },
    /// Possesses the body nearest the observer at `from_tick` and drives it along `direction`
    /// (cells), jumping every `jump_every_ticks` when grounded (0 disables jumping).
    Drive {
        direction: [f32; 2],
        from_tick: u64,
        to_tick: u64,
        #[serde(default)]
        jump_every_ticks: u64,
    },
}

impl Action {
    /// Whether a drive action controls a body at `tick`; the caller chooses that body when the
    /// drive starts.
    pub fn drives_at(&self, tick: u64) -> bool {
        matches!(*self, Self::Drive { from_tick, to_tick, .. } if (from_tick..=to_tick).contains(&tick))
    }

    /// The commands this action issues at timeline tick `tick`, in order. `driven` is the body
    /// a drive action controls; without one, a drive issues nothing.
    pub fn commands_at(
        &self,
        tick: u64,
        driven: Option<BodyHandle>,
    ) -> Result<Vec<Command>, CommandError> {
        let active = |from: u64, to: u64| (from..=to).contains(&tick);
        let mut commands = Vec::new();
        match *self {
            Self::Brush {
                tool,
                cell,
                radius,
                strength,
                from_tick,
                to_tick,
                every_ticks,
            } => {
                if active(from_tick, to_tick)
                    && every_ticks > 0
                    && (tick - from_tick).is_multiple_of(every_ticks)
                {
                    let edit = MaterialEdit::new(tool.edit_tool(), cell.into(), radius, strength)?;
                    commands.push(Command::EditMaterial(edit));
                }
            }
            Self::Attract {
                cell,
                alternate_cell,
                alternate_every_ticks,
                from_tick,
                to_tick,
            } => {
                if active(from_tick, to_tick) {
                    let alternate = alternate_every_ticks > 0
                        && ((tick - from_tick) / alternate_every_ticks) % 2 == 1;
                    let target = match (alternate, alternate_cell) {
                        (true, Some(alternate)) => alternate,
                        _ => cell,
                    };
                    commands.push(Command::AttractBodies {
                        target: target.into(),
                    });
                }
            }
            Self::Lift {
                cell,
                from_tick,
                to_tick,
            } => {
                if active(from_tick, to_tick) {
                    commands.push(Command::LiftBodies {
                        target: cell.into(),
                    });
                }
            }
            Self::Climate {
                ambient_temperature_c,
                at_tick,
            } => {
                if tick == at_tick {
                    commands.push(Command::SetAmbientTemperature(AmbientTemperature::celsius(
                        ambient_temperature_c,
                    )?));
                }
            }
            Self::Drive {
                direction,
                from_tick,
                to_tick,
                jump_every_ticks,
            } => {
                if let Some(body) = driven.filter(|_| active(from_tick, to_tick)) {
                    let direction = Direction::new(direction[0], direction[1])?;
                    commands.push(Command::DriveBody { body, direction });
                    if jump_every_ticks > 0 && (tick - from_tick).is_multiple_of(jump_every_ticks) {
                        commands.push(Command::JumpBody { body });
                    }
                }
            }
        }
        Ok(commands)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BrushTool {
    Water,
    Lava,
    Oil,
    Ice,
    Fire,
    Sand,
    Raise,
    Lower,
}

impl BrushTool {
    pub fn edit_tool(self) -> EditTool {
        match self {
            Self::Water => EditTool::Water,
            Self::Lava => EditTool::Lava,
            Self::Oil => EditTool::Oil,
            Self::Ice => EditTool::Ice,
            Self::Fire => EditTool::Fire,
            Self::Sand => EditTool::Sand,
            Self::Raise => EditTool::Raise,
            Self::Lower => EditTool::Lower,
        }
    }
}

/// A validated scenario with its file identity.
#[derive(Debug, Clone)]
pub struct LoadedScenario {
    pub path: PathBuf,
    pub sha256: String,
    pub scenario: Scenario,
}

pub fn scenario_path(scenarios_dir: &Path, profile: &str, case: &str) -> PathBuf {
    scenarios_dir.join(profile).join(format!("{case}.toml"))
}

pub fn load(path: &Path) -> Result<LoadedScenario, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| format!("{} is not UTF-8", path.display()))?;
    let scenario: Scenario =
        toml::from_str(text).map_err(|error| format!("parse {}: {error}", path.display()))?;
    let expected_case = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let expected_profile = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    validate(&scenario, expected_profile, expected_case)
        .map_err(|problems| format!("{}: {}", path.display(), problems.join("; ")))?;
    Ok(LoadedScenario {
        path: path.to_path_buf(),
        sha256: sha256_hex(&bytes),
        scenario,
    })
}

/// Checks a scenario against its file location, its profile and the supported ranges.
pub fn validate(
    scenario: &Scenario,
    expected_profile: &str,
    expected_case: &str,
) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    let mut check = |ok: bool, message: String| {
        if !ok {
            problems.push(message);
        }
    };

    check(
        scenario.schema == SCENARIO_SCHEMA,
        format!("schema must be {SCENARIO_SCHEMA}"),
    );
    check(
        scenario.profile == expected_profile,
        format!(
            "profile `{}` does not match directory `{expected_profile}`",
            scenario.profile
        ),
    );
    check(
        scenario.case == expected_case,
        format!(
            "case `{}` does not match file `{expected_case}`",
            scenario.case
        ),
    );
    check(
        CASES.contains(&scenario.case.as_str()),
        format!("unknown case `{}`", scenario.case),
    );
    check(
        !scenario.description.trim().is_empty(),
        "description is empty".to_owned(),
    );

    let spec = profile(&scenario.profile);
    check(
        spec.is_some(),
        format!("unknown profile `{}`", scenario.profile),
    );

    let world = &scenario.world;
    if let Some(spec) = spec {
        check(
            world.active_radius_chunks == [spec.active_radius; 2],
            format!(
                "active radius must be {} for {}",
                spec.active_radius, spec.name
            ),
        );
        check(
            scenario.presentation.resolution == spec.resolution,
            format!("resolution must be {:?} for {}", spec.resolution, spec.name),
        );
        check(
            world.tick_rate_hz == f64::from(spec.tick_rate_hz),
            format!(
                "tick rate must be {} Hz for {}",
                spec.tick_rate_hz, spec.name
            ),
        );
        let bodies: u32 = scenario.bodies.iter().map(|group| group.count).sum();
        check(
            bodies == 0 || bodies == spec.bodies,
            format!(
                "cases with bodies must spawn exactly {} for {}, found {bodies}",
                spec.bodies, spec.name
            ),
        );
    }
    check(
        world
            .active_radius_chunks
            .iter()
            .all(|radius| config::ACTIVE_RADIUS_CHUNKS.contains(radius)),
        format!(
            "active radius must be within {:?} chunks",
            config::ACTIVE_RADIUS_CHUNKS
        ),
    );
    check(
        config::TICK_RATE_HZ.contains(&world.tick_rate_hz),
        format!("tick rate must be within {:?} Hz", config::TICK_RATE_HZ),
    );
    check(
        config::MAX_TICKS_PER_FRAME.contains(&world.max_ticks_per_frame),
        format!(
            "max ticks per frame must be within {:?}",
            config::MAX_TICKS_PER_FRAME
        ),
    );
    if let Err(error) = AmbientTemperature::celsius(world.ambient_temperature_c) {
        check(false, format!("world: {error}"));
    }
    check(
        config::ELEVATION_SCALE.contains(&world.elevation_scale),
        format!(
            "elevation scale must be within {:?}",
            config::ELEVATION_SCALE
        ),
    );
    let mut families = world.families.clone();
    families.sort();
    families.dedup();
    check(
        families.len() == world.families.len(),
        "families are listed twice".to_owned(),
    );

    let camera = &scenario.camera;
    check(
        camera.height.is_finite() && camera.height > 0.0,
        "camera height must be positive".to_owned(),
    );
    check(
        camera.pitch_degrees.is_finite() && (-89.0..=89.0).contains(&camera.pitch_degrees),
        "camera pitch must be within ±89°".to_owned(),
    );
    check(
        camera.yaw_degrees.is_finite(),
        "camera yaw must be finite".to_owned(),
    );
    if camera.path.is_empty() {
        check(
            camera.cells_per_tick == 0.0,
            "cells_per_tick needs a path".to_owned(),
        );
    } else {
        check(
            camera.path.len() >= 2,
            "a camera path needs at least two cells".to_owned(),
        );
        check(
            camera.path[0] == camera.start_cell,
            "a camera path starts at start_cell".to_owned(),
        );
        check(
            camera.cells_per_tick.is_finite() && camera.cells_per_tick > 0.0,
            "a camera path needs a positive cells_per_tick".to_owned(),
        );
    }

    let run = &scenario.run;
    check((1..=20).contains(&run.runs), "runs must be 1–20".to_owned());
    check(
        run.measured_ticks >= 60,
        "measured_ticks must be at least 60".to_owned(),
    );
    check(
        run.residency_timeout_seconds.is_finite() && run.residency_timeout_seconds > 0.0,
        "residency timeout must be positive".to_owned(),
    );

    for (index, group) in scenario.bodies.iter().enumerate() {
        check(
            group.count > 0 && group.columns > 0,
            format!("body group {index} needs count and columns"),
        );
        check(
            group.spacing_cells.is_finite()
                && group.spacing_cells >= 1.0
                && group.spacing_cells.fract() == 0.0,
            format!("body group {index} spacing must be a positive whole number of cells"),
        );
        if group.columns > 0
            && let Err(error) = group.spawns()
        {
            check(false, format!("body group {index}: {error}"));
        }
    }

    for (index, action) in scenario.actions.iter().enumerate() {
        let (from, to) = match action {
            Action::Brush {
                every_ticks,
                from_tick,
                to_tick,
                ..
            } => {
                check(
                    *every_ticks >= 1,
                    format!("action {index}: every_ticks must be at least 1"),
                );
                (*from_tick, *to_tick)
            }
            Action::Attract {
                alternate_cell,
                alternate_every_ticks,
                from_tick,
                to_tick,
                ..
            } => {
                check(
                    alternate_cell.is_some() == (*alternate_every_ticks > 0),
                    format!("action {index}: alternate_cell and alternate_every_ticks go together"),
                );
                (*from_tick, *to_tick)
            }
            Action::Lift {
                from_tick, to_tick, ..
            } => (*from_tick, *to_tick),
            Action::Climate { at_tick, .. } => (*at_tick, *at_tick),
            Action::Drive {
                from_tick, to_tick, ..
            } => {
                check(
                    !scenario.bodies.is_empty(),
                    format!("action {index}: drive needs bodies"),
                );
                (*from_tick, *to_tick)
            }
        };
        check(
            from <= to,
            format!("action {index}: from_tick is after to_tick"),
        );
        // The commands the action issues when it starts carry every value it can hold; an
        // unsupported value cannot become a command.
        if let Err(error) = action.commands_at(from, Some(BodyHandle::new(0, 0))) {
            check(false, format!("action {index}: {error}"));
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
schema = 1
profile = "qualification"
case = "flow"
description = "water and lava poured onto the shore"

[world]
seed = 69
active_radius_chunks = [6, 6]
tick_rate_hz = 60.0
max_ticks_per_frame = 4
ambient_temperature_c = 12.0
elevation_scale = 0.5
families = ["liquid", "granular", "thermal", "fire", "biology"]

[presentation]
resolution = [1920, 1080]
present_mode = "no-vsync"
overlay = true

[camera]
start_cell = [128, 320]
height = 150.0
yaw_degrees = 0.0
pitch_degrees = -50.0

[run]
runs = 5
warmup_ticks = 300
measured_ticks = 1800
residency_timeout_seconds = 120.0

[[actions]]
kind = "brush"
tool = "water"
cell = [220, 300]
radius = 6
strength = 20
from_tick = 0
to_tick = 100000
every_ticks = 1
"#;

    fn parse(text: &str) -> Scenario {
        toml::from_str(text).expect("scenario parses")
    }

    #[test]
    fn valid_scenario_passes() {
        assert_eq!(validate(&parse(VALID), "qualification", "flow"), Ok(()));
    }

    #[test]
    fn profile_parameters_cannot_be_weakened() {
        let smaller = parse(&VALID.replace("[6, 6]", "[4, 4]"));
        assert!(validate(&smaller, "qualification", "flow").is_err());
        let lower_resolution = parse(&VALID.replace("[1920, 1080]", "[1280, 720]"));
        assert!(validate(&lower_resolution, "qualification", "flow").is_err());
        let fewer_bodies = parse(&format!(
            "{VALID}\n[[bodies]]\npreset = \"metal\"\ncount = 32\norigin_cell = [0, 0]\ncolumns = 8\nspacing_cells = 10.0\ndrop_height = 10.0\nat_tick = 0\n"
        ));
        assert!(validate(&fewer_bodies, "qualification", "flow").is_err());
    }

    #[test]
    fn location_and_ranges_are_checked() {
        assert!(validate(&parse(VALID), "scale", "flow").is_err());
        assert!(validate(&parse(VALID), "qualification", "idle").is_err());
        assert!(
            validate(
                &parse(&VALID.replace("radius = 6", "radius = 65")),
                "qualification",
                "flow"
            )
            .is_err()
        );
        assert!(
            validate(
                &parse(&VALID.replace("elevation_scale = 0.5", "elevation_scale = 9.0")),
                "qualification",
                "flow"
            )
            .is_err()
        );
        assert!(
            toml::from_str::<Scenario>(
                &VALID.replace("overlay = true", "overlay = true\nfps_cap = 30")
            )
            .is_err()
        );
    }
    #[test]
    fn unsupported_values_are_refused_by_the_command_they_would_become() {
        let strength_zero = parse(&VALID.replace("strength = 20", "strength = 0"));
        let problems = validate(&strength_zero, "qualification", "flow").unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("brush strength 0")),
            "{problems:?}"
        );
        let fractional = parse(&format!(
            "{VALID}\n[[bodies]]\npreset = \"metal\"\ncount = 64\norigin_cell = [0, 0]\ncolumns = 8\nspacing_cells = 9.5\ndrop_height = 10.0\nat_tick = 0\n"
        ));
        assert!(validate(&fractional, "qualification", "flow").is_err());
    }

    #[test]
    fn actions_become_commands_on_their_ticks_in_order() {
        let scenario = parse(VALID);
        let brush = &scenario.actions[0];
        let commands = brush.commands_at(0, None).expect("valid");
        assert!(matches!(
            commands.as_slice(),
            [Command::EditMaterial(edit)]
                if edit.tool() == EditTool::Water
                    && edit.center() == IVec2::new(220, 300)
                    && edit.radius() == 6
                    && edit.strength() == 20
        ));
        assert!(brush.commands_at(100_001, None).expect("valid").is_empty());

        let drive = Action::Drive {
            direction: [-1.0, 1.0],
            from_tick: 60,
            to_tick: 1_000,
            jump_every_ticks: 300,
        };
        let body = BodyHandle::new(3, 0);
        assert!(!drive.drives_at(59) && drive.drives_at(60));
        assert!(drive.commands_at(60, None).expect("valid").is_empty());
        assert!(matches!(
            drive.commands_at(60, Some(body)).expect("valid").as_slice(),
            [Command::DriveBody { .. }, Command::JumpBody { body: jumped }] if *jumped == body
        ));
        assert_eq!(drive.commands_at(61, Some(body)).expect("valid").len(), 1);

        let group = BodyGroup {
            preset: BodyPreset::Wood,
            count: 3,
            origin_cell: [10, -4],
            columns: 2,
            spacing_cells: 9.0,
            drop_height: 10.0,
            at_tick: 0,
        };
        let columns: Vec<IVec2> = group
            .spawns()
            .expect("valid")
            .iter()
            .map(|spawn| spawn.column())
            .collect();
        assert_eq!(
            columns,
            [IVec2::new(10, -4), IVec2::new(19, -4), IVec2::new(10, 5)]
        );
    }
}
