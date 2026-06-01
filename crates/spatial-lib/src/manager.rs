use crate::math::{ChunkKey, ChunkView};

/// Recyclable buffer accumulating the geometric differences between spatial states.
///
/// The application layer must maintain this instance and pass it by mutable
/// reference during updates to eliminate continuous heap allocations.
#[derive(Debug, Clone, Default)]
pub struct ViewUpdate {
    /// Chunks that have fallen out of the active simulation window.
    pub demoted_from_active: Vec<ChunkKey>,
    /// Chunks that have newly entered the active simulation window.
    pub promoted_to_active: Vec<ChunkKey>,
    /// Chunks requested from the disk/generator to buffer the active window.
    pub newly_preloaded: Vec<ChunkKey>,
}

impl ViewUpdate {
    /// Resets the internal vectors while retaining their heap capacity.
    #[inline(always)]
    pub fn clear(&mut self) {
        self.demoted_from_active.clear();
        self.promoted_to_active.clear();
        self.newly_preloaded.clear();
    }
}

/// Orchestrates the shifting boundary windows dictating which chunks are actively
/// simulated versus cached or flushed to disk.
#[derive(Debug, Clone)]
pub struct ChunkManager {
    pub active_view: Option<ChunkView>,
    pub preload_view: Option<ChunkView>,
    pub active_radius_x: u32,
    pub active_radius_y: u32,
    pub preload_ext_radius: u32,
    pub preload_trigger: u32,
}

impl ChunkManager {
    /// Initializes the manager with absolute coordinate boundaries.
    pub fn new(active_rx: u32, active_ry: u32, preload_ext: u32, trigger: u32) -> Self {
        Self {
            active_view: None,
            preload_view: None,
            active_radius_x: active_rx,
            active_radius_y: active_ry,
            preload_ext_radius: preload_ext,
            preload_trigger: trigger,
        }
    }

    /// Evaluates the movement of the focal point against the established boundaries,
    /// populating the provided `ViewUpdate` with the exact chunk lifecycle transitions.
    pub fn update_focus(&mut self, center: ChunkKey, update: &mut ViewUpdate) {
        update.clear();

        let new_active_view = ChunkView::from_rect_xy(
            center,
            self.active_radius_x as i32,
            self.active_radius_y as i32,
        );

        if self.active_view != Some(new_active_view) {
            if let Some(old_active) = self.active_view {
                update
                    .demoted_from_active
                    .extend(old_active.iter().filter(|k| !new_active_view.contains(k)));
            }

            let old_active_ref = self.active_view.as_ref();
            update.promoted_to_active.extend(
                new_active_view
                    .iter()
                    .filter(|k| old_active_ref.map_or(true, |old| !old.contains(k))),
            );

            self.active_view = Some(new_active_view);
        }

        let mut requires_preload = false;
        if let Some(current_preload) = self.preload_view {
            let danger_view = ChunkView::from_rect_xy(
                center,
                (self.active_radius_x + self.preload_trigger) as i32,
                (self.active_radius_y + self.preload_trigger) as i32,
            );

            if !current_preload.contains(&danger_view.min)
                || !current_preload.contains(&danger_view.max)
            {
                requires_preload = true;
            }
        } else {
            requires_preload = true;
        }

        if requires_preload {
            let new_preload_view = ChunkView::from_rect_xy(
                center,
                (self.active_radius_x + self.preload_ext_radius) as i32,
                (self.active_radius_y + self.preload_ext_radius) as i32,
            );

            let old_preload_ref = self.preload_view.as_ref();
            update.newly_preloaded.extend(
                new_preload_view
                    .iter()
                    .filter(|k| old_preload_ref.map_or(true, |old| !old.contains(k))),
            );

            self.preload_view = Some(new_preload_view);
        }
    }
}
