//! Validated intent for the physical world.
//!
//! A command says what a user, a tool or a scenario asks for, in absolute cell coordinates and
//! validated values; it does not say how the live world carries it out. The owner of the live
//! world applies commands at a tick boundary: the CPU baseline today, the GPU world from prompts
//! 02–04. Values outside the supported ranges in [`crate::config`] cannot be constructed, so a
//! command that exists is one the world must either apply or refuse for a capacity reason.

use std::fmt;

use glam::{IVec2, Vec2};

use crate::{
    body::{BodyHandle, BodyPreset},
    config,
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

/// One brush stroke centred on a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialEdit {
    tool: EditTool,
    center: IVec2,
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
        if !config::BRUSH_RADIUS_CELLS.contains(&radius) {
            return Err(CommandError::BrushRadius(radius));
        }
        if !config::BRUSH_STRENGTH.contains(&strength) {
            return Err(CommandError::BrushStrength(strength));
        }
        Ok(Self {
            tool,
            center,
            radius,
            strength,
        })
    }

    pub fn tool(&self) -> EditTool {
        self.tool
    }

    pub fn center(&self) -> IVec2 {
        self.center
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

/// A sphere dropped onto a column.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodySpawn {
    preset: BodyPreset,
    column: IVec2,
    drop_height: f32,
}

impl BodySpawn {
    /// `drop_height` is in world units above the column's bedrock and loose material.
    pub fn new(preset: BodyPreset, column: IVec2, drop_height: f32) -> Result<Self, CommandError> {
        if !(drop_height.is_finite() && drop_height >= 0.0) {
            return Err(CommandError::DropHeight(drop_height));
        }
        Ok(Self {
            preset,
            column,
            drop_height,
        })
    }

    pub fn preset(&self) -> BodyPreset {
        self.preset
    }

    pub fn column(&self) -> IVec2 {
        self.column
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Command {
    EditMaterial(MaterialEdit),
    SpawnBody(BodySpawn),
    /// Accelerates the body along `direction` for one tick, up to the drive speed.
    DriveBody {
        body: BodyHandle,
        direction: Direction,
    },
    /// Jumps the body along its ground normal if it is grounded; otherwise nothing happens.
    JumpBody {
        body: BodyHandle,
    },
    /// Accelerates every body toward the top of `target`'s column for one tick.
    AttractBodies {
        target: IVec2,
    },
    /// Lifts bodies near `target`'s column for one tick.
    LiftBodies {
        target: IVec2,
    },
    SetAmbientTemperature(AmbientTemperature),
}

/// Why a command could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CommandError {
    BrushRadius(i32),
    BrushStrength(u8),
    DropHeight(f32),
    Direction,
    Temperature(f32),
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
    }

    #[test]
    fn directions_are_normalized_and_map_y_to_render_minus_z() {
        let direction = Direction::new(-3.0, 4.0).expect("nonzero");
        assert!((direction.cells().length() - 1.0).abs() < 1e-6);
        let (x, z) = direction.render_xz();
        assert!((x + 0.6).abs() < 1e-6 && (z + 0.8).abs() < 1e-6);
    }
}
