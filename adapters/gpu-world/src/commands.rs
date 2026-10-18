//! Turning admitted commands into GPU records.
//!
//! Commands arrive in absolute cell coordinates; kernels see window-local ones. The conversion
//! uses 64-bit arithmetic, so a command far outside the window cannot overflow into it: a
//! brush stroke that does not touch the window is not dispatched, and its cells are counted as
//! not resident here; a query outside the window is given a local position the kernel reports
//! as not resident. Strokes that do touch the window have local centres within a few thousand
//! cells of the origin, which fit an `i32`.
//!
//! Configuration commands (ambient temperature, simulation rules) change the tick's parameters on
//! the CPU and dispatch nothing; their record only carries the identity their result echoes.
//! A totals command dispatches the reduction over every window chunk.

use world_model::{
    body::BodyHandle,
    command::{Command, CommandKind, Ray, Target},
    glam::{DVec3, IVec2},
    spatial::CHUNK_SIZE,
};
use world_runtime::{
    ids::CommandId,
    port::{AdmittedCommand, ResidentWindow},
};

use crate::{
    bodies::{BodyRefusal, BodyTable},
    layout::{CommandRecord, activity::TILES_PER_CHUNK, codes, tool_code},
    plan::EDIT_WORKGROUP_EDGE,
};

/// What the CPU expects back for one command position, and what it needs to turn the
/// window-local result into absolute values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Expected {
    pub id: CommandId,
    pub kind: CommandKind,
    /// The queried cell or the column target, in absolute coordinates.
    pub cell: IVec2,
    /// Stroke cells the CPU already knows lie outside the window.
    pub outside_window: u32,
    /// The body a body command names, or the slot a spawn took.
    pub body: Option<BodyHandle>,
    /// The ray of a ray-targeted command or query, in absolute render space.
    pub ray: Option<Ray>,
    /// The window's first cell when the tick was encoded.
    pub origin: (i64, i64),
    /// The world's body capacity, for a spawn refused because every slot is taken.
    pub body_capacity: u32,
}

impl Expected {
    /// An absolute cell from a window-local one.
    pub fn absolute_cell(&self, local: IVec2) -> IVec2 {
        IVec2::new(
            (self.origin.0 + i64::from(local.x)) as i32,
            (self.origin.1 + i64::from(local.y)) as i32,
        )
    }

    /// An absolute render-space point from a window-local one.
    pub fn absolute_point(&self, local: [f32; 3]) -> DVec3 {
        DVec3::new(
            self.origin.0 as f64 + f64::from(local[0]),
            f64::from(local[1]),
            -(self.origin.1 as f64) + f64::from(local[2]),
        )
    }
}

/// The dispatches a command needs. `ray` commands first run `resolve_target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch {
    /// Nothing to run: a stroke entirely outside the window, a configuration change, or a
    /// command the CPU answered.
    None,
    /// `apply_edit` over `groups × groups` workgroups.
    Edit {
        groups: u32,
        ray: bool,
    },
    QueryColumn,
    /// `measure_totals` over `(TILES_PER_CHUNK, chunks)` workgroups.
    Totals {
        chunks: u32,
    },
    QueryRay,
    Spawn {
        ray: bool,
    },
    Remove,
    Drive,
    Jump,
    /// `attract_bodies` or `lift_bodies` over every body slot in use.
    Pull {
        attract: bool,
        ray: bool,
    },
}

impl Dispatch {
    /// Workgroups of a totals reduction over a window of `chunks` chunks.
    pub fn totals_groups(chunks: u32) -> [u32; 3] {
        [TILES_PER_CHUNK, chunks, 1]
    }

    /// Whether the command resolves a ray before its own kernel.
    pub fn resolves_ray(&self) -> bool {
        matches!(
            self,
            Self::Edit { ray: true, .. }
                | Self::Spawn { ray: true }
                | Self::Pull { ray: true, .. }
                | Self::QueryRay
        )
    }
}

/// Cells within `radius` of a centre (inclusive), as a brush stroke touches them.
pub fn stroke_cell_count(radius: i32) -> u32 {
    let radius = i64::from(radius.max(0));
    let mut count = 0i64;
    for dy in -radius..=radius {
        let reach = radius * radius - dy * dy;
        count += 2 * integer_sqrt(reach) + 1;
    }
    count as u32
}

fn integer_sqrt(value: i64) -> i64 {
    let mut root = (value as f64).sqrt() as i64;
    while root * root > value {
        root -= 1;
    }
    while (root + 1) * (root + 1) <= value {
        root += 1;
    }
    root
}

/// The window's origin cell and its extent in cells.
fn window_frame(window: &ResidentWindow) -> ((i64, i64), (i64, i64)) {
    let min = window.min_key();
    let size = CHUNK_SIZE as i64;
    (
        (i64::from(min.key.x) * size, i64::from(min.key.y) * size),
        (
            i64::from(window.width()) * size,
            i64::from(window.height()) * size,
        ),
    )
}

/// A column target as window-local cell coordinates, or `(-1, -1)` (outside, which kernels
/// report as not resident) when it lies outside the window.
fn local_cell(cell: IVec2, origin: (i64, i64), extent: (i64, i64)) -> (i32, i32) {
    let x = i64::from(cell.x) - origin.0;
    let y = i64::from(cell.y) - origin.1;
    if (0..extent.0).contains(&x) && (0..extent.1).contains(&y) {
        (x as i32, y as i32)
    } else {
        (-1, -1)
    }
}

/// Writes a target into the record: a window-local column, or a window-local ray.
fn write_target(
    record: &mut CommandRecord,
    target: Target,
    origin: (i64, i64),
    extent: (i64, i64),
) {
    match target {
        Target::Column(cell) => {
            record.target_kind = codes::TARGET_COLUMN;
            (record.cell_x, record.cell_y) = local_cell(cell, origin, extent);
        }
        Target::Ray(ray) => write_ray(record, &ray, origin),
    }
}

fn write_ray(record: &mut CommandRecord, ray: &Ray, origin: (i64, i64)) {
    record.target_kind = codes::TARGET_RAY;
    let start = ray.origin();
    record.origin_x = (start.x - origin.0 as f64) as f32;
    record.origin_y = start.y as f32;
    record.origin_z = (start.z + origin.1 as f64) as f32;
    let direction = ray.direction();
    record.direction_x = direction.x;
    record.direction_y = direction.y;
    record.direction_z = direction.z;
    record.ray_length = ray.max_distance();
}

/// Answers a command on the CPU with `status`; no kernel runs for it.
fn refuse(record: &mut CommandRecord, status: u32) -> Dispatch {
    record.kind = codes::COMMAND_REFUSED;
    record.tool = status;
    Dispatch::None
}

fn refusal_status(refusal: BodyRefusal) -> u32 {
    match refusal {
        BodyRefusal::Stale => codes::RESULT_STALE_BODY,
        BodyRefusal::Capacity => codes::RESULT_BODY_CAPACITY,
    }
}

/// Encodes command `sequence` of a tick. Body commands take or check their slot in `bodies`:
/// a spawn takes the lowest free slot, a removal frees its slot at once, and a command naming
/// a handle that is not live is answered here without a dispatch.
pub fn encode(
    sequence: u32,
    admitted: &AdmittedCommand,
    window: &ResidentWindow,
    bodies: &mut BodyTable,
) -> (CommandRecord, Expected, Dispatch) {
    let [id_lo, id_hi] = admitted.id.to_words();
    let mut record = CommandRecord {
        id_lo,
        id_hi,
        sequence,
        ..CommandRecord::default()
    };
    let ((origin_x, origin_y), (width, height)) = window_frame(window);
    let origin = (origin_x, origin_y);
    let extent = (width, height);
    let mut expected = Expected {
        id: admitted.id,
        kind: admitted.command.kind(),
        cell: IVec2::ZERO,
        outside_window: 0,
        body: admitted.command.body(),
        ray: admitted.command.ray(),
        origin,
        body_capacity: bodies.capacity(),
    };
    let body_words = |record: &mut CommandRecord, body: BodyHandle| {
        record.body_index = body.index();
        record.body_generation = body.generation();
    };
    let dispatch = match admitted.command {
        Command::EditMaterial(edit) => {
            record.kind = codes::COMMAND_EDIT;
            record.tool = tool_code(edit.tool());
            record.radius = edit.radius() as u32;
            record.strength = u32::from(edit.strength());
            let groups = (2 * edit.radius() as u32 + 1).div_ceil(EDIT_WORKGROUP_EDGE);
            match edit.target() {
                Target::Ray(ray) => {
                    write_ray(&mut record, &ray, origin);
                    Dispatch::Edit { groups, ray: true }
                }
                Target::Column(center) => {
                    expected.cell = center;
                    let x = i64::from(center.x) - origin_x;
                    let y = i64::from(center.y) - origin_y;
                    let radius = i64::from(edit.radius());
                    let touches = x + radius >= 0
                        && y + radius >= 0
                        && x - radius < width
                        && y - radius < height;
                    if touches {
                        // Within the window plus one radius on each side: at most 65 × 64 + 64
                        // cells.
                        record.center_x = x as i32;
                        record.center_y = y as i32;
                        Dispatch::Edit { groups, ray: false }
                    } else {
                        expected.outside_window = stroke_cell_count(edit.radius());
                        Dispatch::None
                    }
                }
            }
        }
        Command::QueryColumn { cell } => {
            record.kind = codes::COMMAND_QUERY_COLUMN;
            expected.cell = cell;
            (record.cell_x, record.cell_y) = local_cell(cell, origin, extent);
            Dispatch::QueryColumn
        }
        Command::SetAmbientTemperature(_)
        | Command::SetSimulationRules(_)
        | Command::SetElevationScale(_) => {
            record.kind = codes::COMMAND_CONFIGURE;
            Dispatch::None
        }
        Command::MeasureTotals => {
            record.kind = codes::COMMAND_TOTALS;
            Dispatch::Totals {
                chunks: window.chunk_count(),
            }
        }
        Command::QueryRay { ray } => {
            record.kind = codes::COMMAND_QUERY_RAY;
            write_ray(&mut record, &ray, origin);
            Dispatch::QueryRay
        }
        Command::SpawnBody(spawn) => {
            record.kind = codes::COMMAND_SPAWN;
            let sphere = spawn.sphere();
            record.a = sphere.mass();
            record.b = sphere.radius();
            record.c = sphere.relative_density();
            record.d = spawn.drop_height();
            if let Target::Column(cell) = spawn.target() {
                expected.cell = cell;
            }
            write_target(&mut record, spawn.target(), origin, extent);
            match bodies.allocate() {
                Ok(body) => {
                    body_words(&mut record, body);
                    expected.body = Some(body);
                    Dispatch::Spawn {
                        ray: matches!(spawn.target(), Target::Ray(_)),
                    }
                }
                Err(refusal) => refuse(&mut record, refusal_status(refusal)),
            }
        }
        Command::RemoveBody { body } => {
            record.kind = codes::COMMAND_REMOVE;
            body_words(&mut record, body);
            match bodies.release(body) {
                Ok(()) => Dispatch::Remove,
                Err(refusal) => refuse(&mut record, refusal_status(refusal)),
            }
        }
        Command::DriveBody { body, drive } => {
            record.kind = codes::COMMAND_DRIVE;
            body_words(&mut record, body);
            let (x, z) = drive.direction().render_xz();
            record.a = x;
            record.b = z;
            record.c = drive.strength().acceleration();
            record.d = drive.strength().max_speed();
            if bodies.is_live(body) {
                Dispatch::Drive
            } else {
                refuse(&mut record, codes::RESULT_STALE_BODY)
            }
        }
        Command::JumpBody { body } => {
            record.kind = codes::COMMAND_JUMP;
            body_words(&mut record, body);
            if bodies.is_live(body) {
                Dispatch::Jump
            } else {
                refuse(&mut record, codes::RESULT_STALE_BODY)
            }
        }
        Command::AttractBodies { target } | Command::LiftBodies { target } => {
            let attract = matches!(admitted.command, Command::AttractBodies { .. });
            record.kind = if attract {
                codes::COMMAND_ATTRACT
            } else {
                codes::COMMAND_LIFT
            };
            if let Target::Column(cell) = target {
                expected.cell = cell;
            }
            write_target(&mut record, target, origin, extent);
            Dispatch::Pull {
                attract,
                ray: matches!(target, Target::Ray(_)),
            }
        }
    };
    (record, expected, dispatch)
}

#[cfg(test)]
mod tests {
    use world_model::{
        body::BodyPreset,
        command::{BodySpawn, Direction, Drive, EditTool, MaterialEdit},
        glam::Vec3,
        spatial::ChunkKey,
    };

    use super::*;

    fn window() -> ResidentWindow {
        ResidentWindow::new(ChunkKey::new(10, -3, 0), 1, 1).expect("window")
    }

    fn edit(x: i32, y: i32, radius: i32) -> AdmittedCommand {
        AdmittedCommand {
            id: CommandId::new(0x1_0000_0002),
            command: Command::EditMaterial(
                MaterialEdit::new(EditTool::Lower, IVec2::new(x, y), radius, 9).expect("valid"),
            ),
        }
    }

    #[test]
    fn stroke_counts_match_the_brush_footprint() {
        assert_eq!(stroke_cell_count(0), 1);
        assert_eq!(stroke_cell_count(1), 5);
        assert_eq!(stroke_cell_count(2), 13);
        assert_eq!(stroke_cell_count(64), 12_853);
    }

    #[test]
    fn coordinates_become_window_local() {
        // Window chunks 9..=11 by -4..=-2: origin cell (576, -256).
        let mut bodies = BodyTable::new(4);
        let (record, expected, dispatch) = encode(4, &edit(580, -250, 3), &window(), &mut bodies);
        assert_eq!((record.center_x, record.center_y), (4, 6));
        assert_eq!((record.id_lo, record.id_hi, record.sequence), (2, 1, 4));
        assert_eq!(record.tool, tool_code(EditTool::Lower));
        assert_eq!(
            dispatch,
            Dispatch::Edit {
                groups: 1,
                ray: false
            }
        );
        assert_eq!(expected.outside_window, 0);
        assert_eq!(
            expected.absolute_cell(IVec2::new(4, 6)),
            IVec2::new(580, -250)
        );
    }

    #[test]
    fn far_strokes_are_not_dispatched_and_far_queries_are_outside() {
        let mut bodies = BodyTable::new(4);
        let (_, expected, dispatch) = encode(
            0,
            &edit(i32::MIN + 100, i32::MAX - 100, 64),
            &window(),
            &mut bodies,
        );
        assert_eq!(dispatch, Dispatch::None);
        assert_eq!(expected.outside_window, 12_853);
        // A stroke that only grazes the window's corner is dispatched.
        let (_, _, dispatch) = encode(0, &edit(575, -257, 1), &window(), &mut bodies);
        assert_eq!(
            dispatch,
            Dispatch::Edit {
                groups: 1,
                ray: false
            }
        );
        let query = AdmittedCommand {
            id: CommandId::new(1),
            command: Command::QueryColumn {
                cell: IVec2::new(i32::MAX, 0),
            },
        };
        let (record, expected, dispatch) = encode(1, &query, &window(), &mut bodies);
        assert_eq!((record.cell_x, record.cell_y), (-1, -1));
        assert_eq!(expected.cell, IVec2::new(i32::MAX, 0));
        assert_eq!(dispatch, Dispatch::QueryColumn);
    }

    #[test]
    fn body_commands_take_and_check_slots_and_rays_become_window_local() {
        let mut bodies = BodyTable::new(1);
        let admitted = |id: u64, command: Command| AdmittedCommand {
            id: CommandId::new(id),
            command,
        };
        let spawn = Command::SpawnBody(
            BodySpawn::new(BodyPreset::Metal, IVec2::new(580, -250), 10.0).expect("valid"),
        );
        let (record, expected, dispatch) = encode(0, &admitted(1, spawn), &window(), &mut bodies);
        assert_eq!(dispatch, Dispatch::Spawn { ray: false });
        let body = expected.body.expect("a slot");
        assert_eq!((record.cell_x, record.cell_y), (4, 6));
        assert_eq!(
            (record.a, record.b, record.c, record.d),
            (4.0, 4.0, 7.8, 10.0)
        );
        // The table is full: a second spawn is answered on the CPU.
        let (record, _, dispatch) = encode(1, &admitted(2, spawn), &window(), &mut bodies);
        assert_eq!(dispatch, Dispatch::None);
        assert_eq!(
            (record.kind, record.tool),
            (codes::COMMAND_REFUSED, codes::RESULT_BODY_CAPACITY)
        );
        let drive = Command::DriveBody {
            body,
            drive: Drive::controls(Direction::new(0.0, 1.0).expect("unit")),
        };
        let (record, _, dispatch) = encode(2, &admitted(3, drive), &window(), &mut bodies);
        assert_eq!(dispatch, Dispatch::Drive);
        assert_eq!((record.a, record.b), (0.0, -1.0), "north is render -z");
        let remove = Command::RemoveBody { body };
        assert_eq!(
            encode(3, &admitted(4, remove), &window(), &mut bodies).2,
            Dispatch::Remove
        );
        // Every later command naming the removed handle is stale at once.
        let (record, _, dispatch) = encode(4, &admitted(5, drive), &window(), &mut bodies);
        assert_eq!(dispatch, Dispatch::None);
        assert_eq!(record.tool, codes::RESULT_STALE_BODY);
        let ray = Ray::new(DVec3::new(600.5, 30.0, 240.0), Vec3::NEG_Y, 100.0).expect("valid");
        let (record, expected, dispatch) = encode(
            5,
            &admitted(6, Command::QueryRay { ray }),
            &window(),
            &mut bodies,
        );
        assert_eq!(dispatch, Dispatch::QueryRay);
        // Origin cell (576, -256): x 600.5 - 576, z 240 - 256.
        assert_eq!(
            (record.origin_x, record.origin_y, record.origin_z),
            (24.5, 30.0, -16.0)
        );
        assert_eq!(
            expected.absolute_point([24.5, 30.0, -16.0]),
            DVec3::new(600.5, 30.0, 240.0)
        );
    }
}
