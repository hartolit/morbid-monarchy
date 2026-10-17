//! The `cells` GPU suite: the material phases, their accounting and their sparse scheduling,
//! executed on the selected adapter and compared with the CPU [`reference`](mod@reference).
//!
//! Most cases drive a GPU world and the reference side by side ([`twin`]): every tick's
//! accounting and activity counts must equal the reference's, and captured cells must equal its
//! cells. Probes run the WGSL rule functions against world-model directly.

pub mod adversarial;
pub mod clock;
pub mod intentions;
pub mod measure;
pub mod probes;
pub mod reference;
pub mod scale;
pub mod twin;

use super::Case;

pub const CASES: &[Case] = &[
    Case {
        name: "column-rules-match",
        summary: "the WGSL column rules equal world-model's on boundary and arbitrary columns",
        run: probes::column_rules_match,
    },
    Case {
        name: "rule-hashes-match",
        summary: "keyed hashes, draws and shuffles equal world-model's for extreme positions and ticks",
        run: probes::rule_hashes_match,
    },
    Case {
        name: "material-tables-match",
        summary: "generated material tables equal world-model's properties for every code",
        run: probes::material_tables_match,
    },
    Case {
        name: "dam-break",
        summary: "a water column spreads, conserves, sleeps and rests level, sparse and dense",
        run: intentions::dam_break,
    },
    Case {
        name: "walled-basin",
        summary: "a full column fills a walled basin level and rests",
        run: intentions::walled_basin,
    },
    Case {
        name: "granular-pile",
        summary: "a sand pile slides, conserves its solid volume and rests",
        run: intentions::granular_pile,
    },
    Case {
        name: "chaotic-terrain",
        summary: "seeded chaotic terrain balances its accounting every tick for 600 ticks, sparse and dense",
        run: intentions::chaotic_terrain,
    },
    Case {
        name: "oil-patch",
        summary: "oil poured into a lake stays a patch and mixing deletes no volume",
        run: intentions::oil_patch,
    },
    Case {
        name: "freeze-and-thaw",
        summary: "a pond freezes from the shore at -10 C and thaws at 20 C, woken by the climate command",
        run: intentions::freeze_and_thaw,
    },
    Case {
        name: "ice-floor",
        summary: "ice is a floor water cannot enter",
        run: intentions::ice_floor,
    },
    Case {
        name: "magma-quench-and-boil",
        summary: "magma beside water turns to stone and boils the water in one tick",
        run: intentions::magma_quench_and_boil,
    },
    Case {
        name: "oil-burns-out",
        summary: "burning oil consumes exactly its fuel and the fire goes out",
        run: intentions::oil_burns_out,
    },
    Case {
        name: "fixed-clock",
        summary: "frame time becomes whole ticks and catch-up is capped, through the GPU world",
        run: clock::fixed_clock,
    },
    Case {
        name: "presentation-rates",
        summary: "tick-scheduled inputs at 30, 60, 144 Hz and with stalls give one world; stalls report discarded time",
        run: clock::presentation_rates,
    },
    Case {
        name: "unmatched-transfer-detected",
        summary: "a deliberately unmatched liquid transfer is caught by the accounting and the reference",
        run: adversarial::unmatched_transfer_detected,
    },
    Case {
        name: "donor-conflicts",
        summary: "competing donors: the fullest (steepest) wins exactly, the others keep their material",
        run: adversarial::donor_conflicts,
    },
    Case {
        name: "full-and-frozen-receivers",
        summary: "full, frozen and mixed receivers and strokes into them lose nothing and add what fits",
        run: adversarial::full_and_frozen_receivers,
    },
    Case {
        name: "ceiling-quench",
        summary: "magma and coolant at the elevation ceiling: no loss, liquid remainder, the region sleeps",
        run: adversarial::ceiling_quench,
    },
    Case {
        name: "seam-flow",
        summary: "flow crosses a four-chunk junction at negative keys and tile seams, sparse and dense",
        run: adversarial::seam_flow,
    },
    Case {
        name: "sleeping-receivers",
        summary: "a change two cells away wakes a sleeping donor across a tile seam; sleeping tiles receive",
        run: adversarial::sleeping_receivers,
    },
    Case {
        name: "zero-active-tiles",
        summary: "an asleep world does no cell work tick after tick; one edit wakes only its reach",
        run: adversarial::zero_active_tiles,
    },
    Case {
        name: "climate-and-rule-reactivation",
        summary: "climate and rule changes wake a sleeping world once and it resumes",
        run: adversarial::climate_and_rule_reactivation,
    },
    Case {
        name: "chunk-activation-and-retirement",
        summary: "a streamed chunk wakes with its boundary dependants; a retired one wakes its border",
        run: adversarial::chunk_activation_and_retirement,
    },
    Case {
        name: "rebased-window",
        summary: "the same absolute chunks in differently placed windows evolve identically",
        run: adversarial::rebased_window,
    },
    Case {
        name: "tick-rollover",
        summary: "the rules follow the reference across the low tick word's rollover",
        run: adversarial::tick_rollover,
    },
    Case {
        name: "keyed-growth-and-fire",
        summary: "keyed growth and fire equal the reference, reproduce by seed and differ across seeds",
        run: scale::keyed_growth_and_fire,
    },
    Case {
        name: "dense-sparse-agreement",
        summary: "dense and sparse execution agree tick for tick with every rule on",
        run: scale::dense_sparse_agreement,
    },
    Case {
        name: "long-settlement",
        summary: "nine chunks of chaotic terrain settle completely against the reference",
        run: scale::long_settlement,
    },
    Case {
        name: "extreme-capacity",
        summary: "totals beyond 32 bits are exact; strokes at every maximum add or remove what fits",
        run: scale::extreme_capacity,
    },
    Case {
        name: "full-activity-dispatch-split",
        summary: "69,696 woken tiles split across dispatch dimensions, sparse and dense agree",
        run: scale::full_activity_dispatch_split,
    },
    Case {
        name: "activity-measurement",
        summary: "idle, mixed and full activity at radius 6, sparse and dense: passes, bytes, encode time",
        run: measure::activity_measurement,
    },
];
