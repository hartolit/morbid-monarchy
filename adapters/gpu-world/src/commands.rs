//! Turning admitted commands into GPU records.
//!
//! Commands arrive in absolute cell coordinates; kernels see window-local ones. The conversion
//! uses 64-bit arithmetic, so a command far outside the window cannot overflow into it: a
//! brush stroke that does not touch the window is not dispatched, and its cells are counted as
//! not resident here; a query outside the window is given a local position the kernel reports
//! as not resident. Strokes that do touch the window have local centres within a few thousand
//! cells of the origin, which fit an `i32`.

use world_model::{
    command::{Command, CommandKind},
    glam::IVec2,
    spatial::CHUNK_SIZE,
};
use world_runtime::{
    ids::CommandId,
    port::{AdmittedCommand, Refusal, ResidentWindow},
};

use crate::{
    layout::{CommandRecord, codes, tool_code},
    plan::EDIT_WORKGROUP_EDGE,
};

/// What the CPU expects back for one command position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    pub id: CommandId,
    pub kind: CommandKind,
    /// The queried cell, in absolute coordinates.
    pub cell: IVec2,
    /// Stroke cells the CPU already knows lie outside the window.
    pub outside_window: u32,
}

/// The dispatch a command needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dispatch {
    /// Nothing to run: a stroke entirely outside the window.
    None,
    /// `apply_edit` over `groups × groups` workgroups.
    Edit {
        groups: u32,
    },
    QueryColumn,
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

/// Encodes command `sequence` of a tick.
pub fn encode(
    sequence: u32,
    admitted: &AdmittedCommand,
    window: &ResidentWindow,
) -> Result<(CommandRecord, Expected, Dispatch), Refusal> {
    let [id_lo, id_hi] = admitted.id.to_words();
    let mut record = CommandRecord {
        id_lo,
        id_hi,
        sequence,
        ..CommandRecord::default()
    };
    let mut expected = Expected {
        id: admitted.id,
        kind: admitted.command.kind(),
        cell: IVec2::ZERO,
        outside_window: 0,
    };
    let ((origin_x, origin_y), (width, height)) = window_frame(window);
    let dispatch = match admitted.command {
        Command::EditMaterial(edit) => {
            record.kind = codes::COMMAND_EDIT;
            record.tool = tool_code(edit.tool());
            record.radius = edit.radius() as u32;
            record.strength = u32::from(edit.strength());
            let x = i64::from(edit.center().x) - origin_x;
            let y = i64::from(edit.center().y) - origin_y;
            let radius = i64::from(edit.radius());
            let touches =
                x + radius >= 0 && y + radius >= 0 && x - radius < width && y - radius < height;
            if touches {
                // Within the window plus one radius on each side: at most 65 × 64 + 64 cells.
                record.center_x = x as i32;
                record.center_y = y as i32;
                Dispatch::Edit {
                    groups: (2 * edit.radius() as u32 + 1).div_ceil(EDIT_WORKGROUP_EDGE),
                }
            } else {
                expected.outside_window = stroke_cell_count(edit.radius());
                Dispatch::None
            }
        }
        Command::QueryColumn { cell } => {
            record.kind = codes::COMMAND_QUERY_COLUMN;
            expected.cell = cell;
            let x = i64::from(cell.x) - origin_x;
            let y = i64::from(cell.y) - origin_y;
            let inside = (0..width).contains(&x) && (0..height).contains(&y);
            (record.cell_x, record.cell_y) = if inside {
                (x as i32, y as i32)
            } else {
                (-1, -1)
            };
            Dispatch::QueryColumn
        }
        other => return Err(Refusal::NotSupported(other.kind())),
    };
    Ok((record, expected, dispatch))
}

#[cfg(test)]
mod tests {
    use world_model::{
        command::{EditTool, MaterialEdit},
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
        let (record, expected, dispatch) =
            encode(4, &edit(580, -250, 3), &window()).expect("encode");
        assert_eq!((record.center_x, record.center_y), (4, 6));
        assert_eq!((record.id_lo, record.id_hi, record.sequence), (2, 1, 4));
        assert_eq!(record.tool, tool_code(EditTool::Lower));
        assert_eq!(dispatch, Dispatch::Edit { groups: 1 });
        assert_eq!(expected.outside_window, 0);
    }

    #[test]
    fn far_strokes_are_not_dispatched_and_far_queries_are_outside() {
        let (_, expected, dispatch) =
            encode(0, &edit(i32::MIN + 100, i32::MAX - 100, 64), &window()).expect("encode");
        assert_eq!(dispatch, Dispatch::None);
        assert_eq!(expected.outside_window, 12_853);
        // A stroke that only grazes the window's corner is dispatched.
        let (_, _, dispatch) = encode(0, &edit(575, -257, 1), &window()).expect("encode");
        assert_eq!(dispatch, Dispatch::Edit { groups: 1 });
        let query = AdmittedCommand {
            id: CommandId::new(1),
            command: Command::QueryColumn {
                cell: IVec2::new(i32::MAX, 0),
            },
        };
        let (record, expected, dispatch) = encode(1, &query, &window()).expect("encode");
        assert_eq!((record.cell_x, record.cell_y), (-1, -1));
        assert_eq!(expected.cell, IVec2::new(i32::MAX, 0));
        assert_eq!(dispatch, Dispatch::QueryColumn);
    }
}
