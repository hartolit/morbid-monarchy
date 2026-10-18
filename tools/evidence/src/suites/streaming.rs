//! The `streaming` GPU suite: residency, snapshots, checkpoints, storage failures and recovery,
//! executed on the selected adapter through `gpu-world`, `world-runtime` and `world-store` with
//! real temporary stores.

use super::Case;

mod checkpoints;
mod mechanism;
mod residency;
mod world;

pub const CASES: &[Case] = &[
    Case {
        name: "snapshot-change-flags",
        summary: "edits, material flow and body deformation mark exactly the chunks they change; snapshots clear the mark and stream only changed chunks",
        run: mechanism::change_flags,
    },
    Case {
        name: "newest-retained-wins",
        summary: "a chunk that returns before its delayed save finishes is the newest state; a newer save finishing after an older one wins; an older publication is refused; the world reopens with it",
        run: residency::newest_retained_wins,
    },
    Case {
        name: "reordered-and-stale-loads",
        summary: "loads answered out of order or after their key left are never applied: every resident chunk holds its own key's saved or generated content",
        run: residency::reordered_and_stale_loads,
    },
    Case {
        name: "missing-neighbours-closed",
        summary: "an unreadable neighbour is a closed boundary at a negative-key corner: water spreads into the resident chunks, none is lost and none enters it",
        run: residency::missing_neighbours_are_closed,
    },
    Case {
        name: "refused-rows-stay-out",
        summary: "corrupt, truncated and future-version rows keep their chunks out, are never generated over, survive byte for byte and are not named by later checkpoints",
        run: residency::refused_rows_stay_out,
    },
    Case {
        name: "resident-only-autosave",
        summary: "changes to chunks that never leave the window are saved by autosave alone and reopen after a session that was never closed",
        run: checkpoints::resident_only_autosave,
    },
    Case {
        name: "periodic-saves",
        summary: "periodic saves follow one another with increasing identities and ticks, each complete, and the world reopens at the latest",
        run: checkpoints::periodic_saves,
    },
    Case {
        name: "snapshot-resize-overlap",
        summary: "a world snapshot drains while the window grows within the budget; a growth beyond it is refused before any eviction and the world stays intact",
        run: checkpoints::snapshot_resize_overlap,
    },
    Case {
        name: "failed-commits-retained",
        summary: "I/O errors under redb fail eviction writes and a publication; payloads stay retained, the failure is reported, retries succeed and the reopened world holds every change",
        run: checkpoints::failed_commits_retained,
    },
    Case {
        name: "retention-budget",
        summary: "with writes failing, retention stops at its budget and the window waits visibly; afterwards it moves and every change is saved",
        run: checkpoints::retention_budget,
    },
    Case {
        name: "delayed-mapping",
        summary: "map deliveries held during eviction and world snapshots: nothing is lost or taken early, and everything completes when they resume",
        run: checkpoints::delayed_mapping,
    },
    Case {
        name: "body-suspension-resume",
        summary: "a body whose region leaves the window is suspended, saved suspended with its handle, reopened once, and resumed when its terrain returns",
        run: checkpoints::body_suspension_resume,
    },
    Case {
        name: "snapshot-restore-bodies",
        summary: "a world snapshot's bodies restored into a new session continue bit for bit, with the same handles and none twice",
        run: mechanism::restore_bodies,
    },
];
