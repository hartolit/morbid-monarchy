//! Validated intent for the physical world.
//!
//! A command says what a user, a tool or a scenario asks for, in absolute coordinates and
//! validated values; it does not say how the live world carries it out. The owner of the live
//! world applies commands at a tick boundary: the CPU baseline, or the GPU world since prompts
//! 02–04. Values outside the supported ranges in [`crate::config`] and [`crate::body`] cannot
//! be constructed, so a command that exists is one the world must either apply or refuse for a
//! capacity or identity reason.
//!
//! Targets are a column or a ray. A ray is resolved by the live world against its own surface
//! at the tick the command applies, never against a CPU copy of the terrain; the command's
//! outcome reports where it hit.

use std::fmt;

use glam::{DVec3, IVec2, Vec2, Vec3};

use crate::{
    body::{BodyHandle, BodyPreset, DriveStrength, SphereDescription},
    config,
    material::FluidMat,
    rules::SimulationRules,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTool {
    Water,
    Lava,
    Oil,
    Ice,
    Fire,
    Sand,
    Raise,
    Lower,
}

impl EditTool {
    /// Every tool, in a fixed order.
    pub const ALL: [Self; 8] = [
        Self::Water,
        Self::Lava,
        Self::Oil,
        Self::Ice,
        Self::Fire,
        Self::Sand,
        Self::Raise,
        Self::Lower,
    ];

    /// The liquid (or ice) a painting tool places with `WorldCell::inject_fluid`; `None` for
    /// the tools that change fire, loose material or bedrock instead.
    pub const fn fluid(self) -> Option<FluidMat> {
        match self {
            Self::Water => Some(FluidMat::FLUID_WATER),
            Self::Lava => Some(FluidMat::FLUID_MAGMA),
            Self::Oil => Some(FluidMat::FLUID_OIL),
            Self::Ice => Some(FluidMat::FLUID_ICE),
            Self::Fire | Self::Sand | Self::Raise | Self::Lower => None,
        }
    }
}

/// A ray in absolute render space (y up, cell `y` is render `-z`), as a pointer or a camera
/// casts it. The origin is `f64` so a ray far from the world origin keeps its precision; the
/// live world converts it to its window-local frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    origin: DVec3,
    direction: Vec3,
    max_distance: f32,
}

impl Ray {
    /// `direction` is normalized here; `max_distance` must lie in `(0, RAY_MAX_DISTANCE]`.
    pub fn new(origin: DVec3, direction: Vec3, max_distance: f32) -> Result<Self, CommandError> {
        if !origin.is_finite() {
            return Err(CommandError::RayOrigin);
        }
        let length = direction.length();
        if !(length.is_finite() && length > 0.0) {
            return Err(CommandError::Direction);
        }
        if !(max_distance.is_finite()
            && max_distance > 0.0
            && max_distance <= config::RAY_MAX_DISTANCE)
        {
            return Err(CommandError::RayDistance(max_distance));
        }
        Ok(Self {
            origin,
            direction: direction / length,
            max_distance,
        })
    }

    pub fn origin(&self) -> DVec3 {
        self.origin
    }

    /// A unit vector.
    pub fn direction(&self) -> Vec3 {
        self.direction
    }

    pub fn max_distance(&self) -> f32 {
        self.max_distance
    }
}

/// Where a command acts: the top of a column (at its cell vertex), or wherever a ray first
/// meets the physical surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Target {
    Column(IVec2),
    Ray(Ray),
}

impl Target {
    pub fn column(&self) -> Option<IVec2> {
        match self {
            Self::Column(cell) => Some(*cell),
            Self::Ray(_) => None,
        }
    }
}

/// One brush stroke centred on a cell, or on the cell a ray hits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MaterialEdit {
    tool: EditTool,
    target: Target,
    radius: i32,
    strength: u8,
}

impl MaterialEdit {
    pub fn new(
        tool: EditTool,
        center: IVec2,
        radius: i32,
        strength: u8,
    ) -> Result<Self, CommandError> {
        Self::at(tool, Target::Column(center), radius, strength)
    }

    /// A stroke at `target`; a ray target strokes the quad cell its hit lies in, as the brush
    /// tool does.
    pub fn at(
        tool: EditTool,
        target: Target,
        radius: i32,
        strength: u8,
    ) -> Result<Self, CommandError> {
        if !config::BRUSH_RADIUS_CELLS.contains(&radius) {
            return Err(CommandError::BrushRadius(radius));
        }
        if !config::BRUSH_STRENGTH.contains(&strength) {
            return Err(CommandError::BrushStrength(strength));
        }
        Ok(Self {
            tool,
            target,
            radius,
            strength,
        })
    }

    pub fn tool(&self) -> EditTool {
        self.tool
    }

    pub fn target(&self) -> Target {
        self.target
    }

    /// The centre cell of a column-targeted stroke.
    pub fn center(&self) -> Option<IVec2> {
        self.target.column()
    }

    /// Cells within this distance of the centre (inclusive) are edited.
    pub fn radius(&self) -> i32 {
        self.radius
    }

    /// Units added or removed per cell.
    pub fn strength(&self) -> u8 {
        self.strength
    }
}

/// A sphere dropped onto a column, or onto the point a ray hits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodySpawn {
    sphere: SphereDescription,
    target: Target,
    drop_height: f32,
}

impl BodySpawn {
    /// A preset dropped onto `column`'s vertex. `drop_height` is in world units above the
    /// column's bedrock and loose material.
    pub fn new(preset: BodyPreset, column: IVec2, drop_height: f32) -> Result<Self, CommandError> {
        Self::with(preset.description(), Target::Column(column), drop_height)
    }

    pub fn with(
        sphere: SphereDescription,
        target: Target,
        drop_height: f32,
    ) -> Result<Self, CommandError> {
        if !(drop_height.is_finite() && drop_height >= 0.0) {
            return Err(CommandError::DropHeight(drop_height));
        }
        Ok(Self {
            sphere,
            target,
            drop_height,
        })
    }

    pub fn sphere(&self) -> SphereDescription {
        self.sphere
    }

    pub fn target(&self) -> Target {
        self.target
    }

    /// The column of a column-targeted spawn.
    pub fn column(&self) -> Option<IVec2> {
        self.target.column()
    }

    pub fn drop_height(&self) -> f32 {
        self.drop_height
    }
}

/// A unit horizontal direction in cell axes (`+x` east, `+y` north, which is render `-z`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Direction(Vec2);

impl Direction {
    pub fn new(x: f32, y: f32) -> Result<Self, CommandError> {
        let vector = Vec2::new(x, y);
        let length = vector.length();
        if !(length.is_finite() && length > 0.0) {
            return Err(CommandError::Direction);
        }
        Ok(Self(vector / length))
    }

    pub fn cells(self) -> Vec2 {
        self.0
    }

    /// The same direction as a render-space `(x, z)` pair.
    pub fn render_xz(self) -> (f32, f32) {
        (self.0.x, -self.0.y)
    }
}

/// One tick of drive along a direction, with the controls' strength or a weaker one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Drive {
    direction: Direction,
    strength: DriveStrength,
}

impl Drive {
    /// The controls' drive.
    pub fn controls(direction: Direction) -> Self {
        Self {
            direction,
            strength: DriveStrength::CONTROLS,
        }
    }

    pub fn with(direction: Direction, strength: DriveStrength) -> Self {
        Self {
            direction,
            strength,
        }
    }

    pub fn direction(&self) -> Direction {
        self.direction
    }

    pub fn strength(&self) -> DriveStrength {
        self.strength
    }
}

/// Ambient air temperature within the supported range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmbientTemperature(f32);

impl AmbientTemperature {
    pub fn celsius(degrees: f32) -> Result<Self, CommandError> {
        if !(degrees.is_finite() && config::AMBIENT_TEMPERATURE_C.contains(&degrees)) {
            return Err(CommandError::Temperature(degrees));
        }
        Ok(Self(degrees))
    }

    pub fn degrees(self) -> f32 {
        self.0
    }
}

/// World units per elevation unit, within [`config::ELEVATION_SCALE`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ElevationScale(f32);

impl ElevationScale {
    pub const DEFAULT: Self = Self(config::DEFAULT_ELEVATION_SCALE);

    pub fn new(scale: f32) -> Result<Self, CommandError> {
        if !(scale.is_finite() && config::ELEVATION_SCALE.contains(&scale)) {
            return Err(CommandError::ElevationScale(scale));
        }
        Ok(Self(scale))
    }

    pub fn value(self) -> f32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    EditMaterial(MaterialEdit),
    SpawnBody(BodySpawn),
    /// Removes a body. Its final interaction counters come back in the outcome.
    RemoveBody {
        body: BodyHandle,
    },
    /// Accelerates the body along the drive direction for one tick, up to the drive speed.
    DriveBody {
        body: BodyHandle,
        drive: Drive,
    },
    /// Jumps the body along its ground normal if it is grounded; otherwise nothing happens.
    JumpBody {
        body: BodyHandle,
    },
    /// Accelerates every body toward `target` (a column's top or a ray's hit) for one tick.
    AttractBodies {
        target: Target,
    },
    /// Lifts bodies near `target`'s column for one tick.
    LiftBodies {
        target: Target,
    },
    /// Sets the air temperature from this tick on. A change wakes every resident cell once.
    SetAmbientTemperature(AmbientTemperature),
    /// Sets which rule families run from this tick on. A change wakes every resident cell once.
    SetSimulationRules(SimulationRules),
    /// Sets the elevation scale from this tick on: physical configuration that bodies collide
    /// with and the renderer draws.
    SetElevationScale(ElevationScale),
    /// Reads one column as it stands after the tick's earlier commands. An observation, not a
    /// change: the answer is a copy at a completed tick, never a writable value.
    QueryColumn {
        cell: IVec2,
    },
    /// Casts a ray against the physical surface and the bodies as they stand after the tick's
    /// earlier commands.
    QueryRay {
        ray: Ray,
    },
    /// Sums the material of every resident cell as it stands after the tick's earlier commands:
    /// a diagnostic reduction whose answer is a handful of integers, never the cells.
    MeasureTotals,
}

/// The family of a [`Command`], without its payload. The owner of the live world reports which
/// kinds it executes; a kind it does not execute is refused at admission, never dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommandKind {
    EditMaterial,
    SpawnBody,
    RemoveBody,
    DriveBody,
    JumpBody,
    AttractBodies,
    LiftBodies,
    SetAmbientTemperature,
    SetSimulationRules,
    SetElevationScale,
    QueryColumn,
    QueryRay,
    MeasureTotals,
}

impl Command {
    pub const fn kind(&self) -> CommandKind {
        match self {
            Self::EditMaterial(_) => CommandKind::EditMaterial,
            Self::SpawnBody(_) => CommandKind::SpawnBody,
            Self::RemoveBody { .. } => CommandKind::RemoveBody,
            Self::DriveBody { .. } => CommandKind::DriveBody,
            Self::JumpBody { .. } => CommandKind::JumpBody,
            Self::AttractBodies { .. } => CommandKind::AttractBodies,
            Self::LiftBodies { .. } => CommandKind::LiftBodies,
            Self::SetAmbientTemperature(_) => CommandKind::SetAmbientTemperature,
            Self::SetSimulationRules(_) => CommandKind::SetSimulationRules,
            Self::SetElevationScale(_) => CommandKind::SetElevationScale,
            Self::QueryColumn { .. } => CommandKind::QueryColumn,
            Self::QueryRay { .. } => CommandKind::QueryRay,
            Self::MeasureTotals => CommandKind::MeasureTotals,
        }
    }

    /// The body a command addresses, if it names one.
    pub const fn body(&self) -> Option<BodyHandle> {
        match self {
            Self::RemoveBody { body } | Self::DriveBody { body, .. } | Self::JumpBody { body } => {
                Some(*body)
            }
            _ => None,
        }
    }

    /// The ray the command's target or query casts, if any.
    pub const fn ray(&self) -> Option<Ray> {
        let target = match self {
            Self::EditMaterial(edit) => edit.target,
            Self::SpawnBody(spawn) => spawn.target,
            Self::AttractBodies { target } | Self::LiftBodies { target } => *target,
            Self::QueryRay { ray } => return Some(*ray),
            _ => return None,
        };
        match target {
            Target::Ray(ray) => Some(ray),
            Target::Column(_) => None,
        }
    }
}

/// Why a command could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandError {
    BrushRadius(i32),
    BrushStrength(u8),
    DropHeight(f32),
    Direction,
    Temperature(f32),
    ElevationScale(f32),
    RayOrigin,
    RayDistance(f32),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BrushRadius(radius) => write!(
                f,
                "brush radius {radius} is outside {:?} cells",
                config::BRUSH_RADIUS_CELLS
            ),
            Self::BrushStrength(strength) => write!(
                f,
                "brush strength {strength} is outside {:?}",
                config::BRUSH_STRENGTH
            ),
            Self::DropHeight(height) => {
                write!(
                    f,
                    "drop height {height} is not a finite non-negative distance"
                )
            }
            Self::Direction => write!(f, "a direction must be finite and nonzero"),
            Self::Temperature(degrees) => write!(
                f,
                "temperature {degrees} °C is outside {:?} °C",
                config::AMBIENT_TEMPERATURE_C
            ),
            Self::ElevationScale(scale) => write!(
                f,
                "elevation scale {scale} is outside {:?}",
                config::ELEVATION_SCALE
            ),
            Self::RayOrigin => write!(f, "a ray origin must be finite"),
            Self::RayDistance(distance) => write!(
                f,
                "ray length {distance} is outside (0, {}] world units",
                config::RAY_MAX_DISTANCE
            ),
        }
    }
}

impl std::error::Error for CommandError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_range_values_cannot_become_commands() {
        let cell = IVec2::new(3, -4);
        assert!(MaterialEdit::new(EditTool::Water, cell, 64, 255).is_ok());
        assert_eq!(
            MaterialEdit::new(EditTool::Water, cell, 65, 1),
            Err(CommandError::BrushRadius(65))
        );
        assert_eq!(
            MaterialEdit::new(EditTool::Water, cell, -1, 1),
            Err(CommandError::BrushRadius(-1))
        );
        assert_eq!(
            MaterialEdit::new(EditTool::Sand, cell, 5, 0),
            Err(CommandError::BrushStrength(0))
        );
        assert!(BodySpawn::new(BodyPreset::Wood, cell, f32::NAN).is_err());
        assert!(BodySpawn::new(BodyPreset::Wood, cell, -1.0).is_err());
        assert!(Direction::new(0.0, 0.0).is_err());
        assert!(AmbientTemperature::celsius(41.0).is_err());
        assert!(AmbientTemperature::celsius(f32::INFINITY).is_err());
        assert!(ElevationScale::new(0.005).is_err());
        assert!(ElevationScale::new(5.0).is_ok());
        assert!(Ray::new(DVec3::ZERO, Vec3::ZERO, 10.0).is_err());
        assert!(Ray::new(DVec3::ZERO, Vec3::NEG_Y, 0.0).is_err());
        assert!(Ray::new(DVec3::ZERO, Vec3::NEG_Y, 1000.5).is_err());
        assert!(Ray::new(DVec3::new(f64::NAN, 0.0, 0.0), Vec3::NEG_Y, 10.0).is_err());
        let ray = Ray::new(
            DVec3::new(1e7, 50.0, -1e7),
            Vec3::new(0.0, -3.0, 4.0),
            1000.0,
        )
        .expect("valid");
        assert!((ray.direction().length() - 1.0).abs() < 1e-6);
        assert_eq!(ray.origin().x, 1e7, "the origin keeps its precision");
    }

    #[test]
    fn painting_tools_name_their_liquid_and_commands_their_kind() {
        let painted: Vec<(EditTool, FluidMat)> = EditTool::ALL
            .iter()
            .filter_map(|tool| tool.fluid().map(|fluid| (*tool, fluid)))
            .collect();
        assert_eq!(
            painted,
            [
                (EditTool::Water, FluidMat::FLUID_WATER),
                (EditTool::Lava, FluidMat::FLUID_MAGMA),
                (EditTool::Oil, FluidMat::FLUID_OIL),
                (EditTool::Ice, FluidMat::FLUID_ICE),
            ]
        );
        let query = Command::QueryColumn {
            cell: IVec2::new(1, 2),
        };
        assert_eq!(query.kind(), CommandKind::QueryColumn);
        let edit = Command::EditMaterial(
            MaterialEdit::new(EditTool::Fire, IVec2::ZERO, 0, 1).expect("valid"),
        );
        assert_eq!(edit.kind(), CommandKind::EditMaterial);
        assert_eq!(edit.ray(), None);
        let ray = Ray::new(DVec3::Y * 10.0, Vec3::NEG_Y, 20.0).expect("valid");
        let aimed = Command::EditMaterial(
            MaterialEdit::at(EditTool::Sand, Target::Ray(ray), 3, 2).expect("valid"),
        );
        assert_eq!(aimed.ray(), Some(ray));
        let body = BodyHandle::new(3, 1);
        assert_eq!(Command::RemoveBody { body }.body(), Some(body));
        assert_eq!(Command::MeasureTotals.body(), None);
    }

    #[test]
    fn directions_are_normalized_and_map_y_to_render_minus_z() {
        let direction = Direction::new(-3.0, 4.0).expect("nonzero");
        assert!((direction.cells().length() - 1.0).abs() < 1e-6);
        let (x, z) = direction.render_xz();
        assert!((x + 0.6).abs() < 1e-6 && (z + 0.8).abs() < 1e-6);
        let drive = Drive::controls(direction);
        assert_eq!(drive.strength(), DriveStrength::CONTROLS);
    }
}
