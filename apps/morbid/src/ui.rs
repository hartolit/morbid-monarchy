//! The overlay: the start screen, the in-world tool bar and panels (the baseline's, plus the
//! session, outcome and pressure panels), and the dialogs for closing and device loss.
//!
//! Panels edit the client's own state ([`Play`], [`Hud`]); anything that changes the world is a
//! returned action the client turns into a command or a session call. Figures shown are what
//! the runtime, the store and the renderer report; nothing is displayed as applied before its
//! outcome arrived.

pub mod compass;

use std::path::PathBuf;

use gpu_world::context::AdapterRecord;
use world_model::config::{
    ACTIVE_RADIUS_CHUNKS, AMBIENT_TEMPERATURE_C, BRUSH_RADIUS_CELLS, BRUSH_STRENGTH,
    ELEVATION_SCALE, MAX_TICKS_PER_FRAME, TICK_RATE_HZ,
};
use world_render::{
    Lighting, Overlay as DebugOverlay,
    lighting::{AMBIENT_RANGE, SUN_AZIMUTH_RANGE, SUN_ELEVATION_RANGE, UNIT_RANGE},
};
use world_runtime::{
    checkpoint::{AUTOSAVE_TICKS, SaveReceipt, SaveState},
    ids::Tick,
    ledger::LedgerSummary,
    port::TransferCounters,
    residency::{RefusedMove, ResidencySummary},
    session::{CloseReport, LossReport},
    store::CheckpointManifest,
};

use crate::{
    catalog::{EntryState, WorldEntry},
    play::{Play, TOOLS, Tool, Verdict},
};

const HEADING_SIZE: f32 = 15.0;
const SPACING: f32 = 16.0;
const ELEVATION_SPEED: f64 = 0.05;
const PANEL_OFFSET_X: f32 = 10.0;
const PANEL_OFFSET_Y: f32 = 40.0;
const STACK_GAP: f32 = 8.0;
const FPS_GOOD: f64 = 32.0;

fn tick_text(tick: Option<Tick>) -> String {
    tick.map_or_else(|| "none".to_owned(), |tick| tick.value().to_string())
}

// ---------------------------------------------------------------------------------------------
// Start screen

/// What the start screen shows and edits.
pub struct StartScreen {
    pub worlds: Vec<WorldEntry>,
    pub worlds_dir: PathBuf,
    pub adapters: Vec<AdapterRecord>,
    pub adapter: Option<String>,
    pub new_name: String,
    pub new_seed: String,
    pub import_path: String,
    pub import_name: String,
    pub import_seed: String,
    pub import_scale: String,
    /// Newest first: what happened (closes, losses, refusals), for the person to read.
    pub messages: Vec<String>,
}

/// What a start-screen control asked for; the client handles it (a driver may submit one).
#[derive(Debug, Clone, PartialEq)]
pub enum StartAction {
    ChooseAdapter(String),
    Open {
        name: String,
        path: PathBuf,
    },
    Create {
        name: String,
        seed: u32,
    },
    Import {
        name: String,
        legacy: PathBuf,
        seed: u32,
        scale: f32,
    },
    Refresh,
    Quit,
}

impl StartScreen {
    pub fn message(&mut self, text: impl Into<String>) {
        self.messages.insert(0, text.into());
        self.messages.truncate(12);
    }
}

pub fn start_screen(ui: &mut egui::Ui, screen: &mut StartScreen) -> Option<StartAction> {
    let mut action = None;
    egui::Panel::top("start_title").show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Morbid Monarchy").strong().size(20.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Quit").clicked() {
                    action = Some(StartAction::Quit);
                }
            });
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.heading("GPU adapter");
            match &screen.adapter {
                Some(id) => {
                    let name = screen
                        .adapters
                        .iter()
                        .find(|record| &record.id == id)
                        .map_or("not presentable here", |record| record.name.as_str());
                    ui.label(format!("Worlds run on {id} ({name})."));
                }
                None => {
                    ui.label("Choose the adapter the world runs on; the choice is remembered.");
                }
            }
            ui.horizontal_wrapped(|ui| {
                for record in &screen.adapters {
                    let selected = screen.adapter.as_deref() == Some(record.id.as_str());
                    let text = format!(
                        "{}  {} ({}, {})",
                        record.id, record.name, record.backend, record.driver
                    );
                    if ui.selectable_label(selected, text).clicked() {
                        action = Some(StartAction::ChooseAdapter(record.id.clone()));
                    }
                }
                if screen.adapters.is_empty() {
                    ui.colored_label(egui::Color32::RED, "No hardware adapter can present to this window.");
                }
            });
            ui.separator();

            ui.heading("Worlds");
            ui.label(format!("In {}", screen.worlds_dir.display()));
            if ui.button("Refresh").clicked() {
                action = Some(StartAction::Refresh);
            }
            egui::Grid::new("worlds").striped(true).show(ui, |ui| {
                for entry in &screen.worlds {
                    ui.label(egui::RichText::new(&entry.name).strong());
                    match &entry.state {
                        EntryState::Ready {
                            seed,
                            imported,
                            tick,
                            chunks,
                            bodies,
                        } => {
                            ui.label(format!(
                                "seed {seed}{}, saved at tick {}, {chunks} chunks, {bodies} spheres",
                                if *imported { ", imported" } else { "" },
                                tick_text(*tick)
                            ));
                            let enabled = screen.adapter.is_some();
                            if ui.add_enabled(enabled, egui::Button::new("Open")).clicked() {
                                action = Some(StartAction::Open {
                                    name: entry.name.clone(),
                                    path: entry.path.clone(),
                                });
                            }
                        }
                        EntryState::Unreadable(reason) => {
                            ui.colored_label(egui::Color32::LIGHT_RED, format!("cannot be opened: {reason}"));
                            ui.label("");
                        }
                    }
                    ui.end_row();
                }
            });
            if screen.worlds.is_empty() {
                ui.label("No saved worlds yet.");
            }
            ui.separator();

            ui.heading("New world");
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut screen.new_name);
                ui.label("Seed");
                ui.add(egui::TextEdit::singleline(&mut screen.new_seed).desired_width(90.0));
                let seed = screen.new_seed.trim().parse::<u32>();
                if ui
                    .add_enabled(seed.is_ok(), egui::Button::new("Create"))
                    .clicked()
                    && let Ok(seed) = seed
                {
                    action = Some(StartAction::Create {
                        name: screen.new_name.clone(),
                        seed,
                    });
                }
            });
            ui.separator();

            ui.heading("Import a baseline save");
            ui.label(
                "Reads a save.redb the Bevy baseline wrote and writes a new world; the original is not changed. \
                 The old format recorded neither its seed nor its elevation scale (the baseline used 69 and 0.5).",
            );
            egui::Grid::new("import").show(ui, |ui| {
                ui.label("Legacy file");
                ui.text_edit_singleline(&mut screen.import_path);
                ui.end_row();
                ui.label("New name");
                ui.text_edit_singleline(&mut screen.import_name);
                ui.end_row();
                ui.label("Seed");
                ui.text_edit_singleline(&mut screen.import_seed);
                ui.end_row();
                ui.label("Elevation scale");
                ui.text_edit_singleline(&mut screen.import_scale);
                ui.end_row();
            });
            let seed = screen.import_seed.trim().parse::<u32>();
            let scale = screen.import_scale.trim().parse::<f32>();
            if ui
                .add_enabled(
                    seed.is_ok() && scale.is_ok() && !screen.import_path.trim().is_empty(),
                    egui::Button::new("Import"),
                )
                .clicked()
                && let (Ok(seed), Ok(scale)) = (seed, scale)
            {
                action = Some(StartAction::Import {
                    name: screen.import_name.clone(),
                    legacy: PathBuf::from(screen.import_path.trim()),
                    seed,
                    scale,
                });
            }
            ui.separator();

            ui.heading("Messages");
            for message in &screen.messages {
                ui.label(message);
            }
        });
    });
    action
}

pub fn close_summary(name: &str, report: &CloseReport) -> String {
    if report.saved {
        format!(
            "{name}: closed and saved; checkpoint {} holds tick {} ({:.1} s).",
            report.latest.id.value(),
            tick_text(report.latest.tick),
            report.waited.as_secs_f64()
        )
    } else {
        format!(
            "{name}: closed WITHOUT saving everything: the latest complete checkpoint is {} at tick {}; \
             {} captured revisions unwritten, timed out {}, last save {:?}. The next open reclaims what \
             this session left.",
            report.latest.id.value(),
            tick_text(report.latest.tick),
            report.unwritten,
            report.timed_out,
            report.save.as_ref().map(|receipt| &receipt.state)
        )
    }
}

// ---------------------------------------------------------------------------------------------
// In-world overlay

/// Which panels show, and the renderer's settings the panels edit.
pub struct Hud {
    pub visible: bool,
    pub statistics: bool,
    pub simulation: bool,
    pub lighting_panel: bool,
    pub session: bool,
    pub outcomes: bool,
    pub lighting: Lighting,
    pub debug: DebugOverlay,
}

impl Default for Hud {
    fn default() -> Self {
        Self {
            visible: true,
            statistics: false,
            simulation: false,
            lighting_panel: false,
            session: true,
            outcomes: false,
            lighting: Lighting::default(),
            debug: DebugOverlay::default(),
        }
    }
}

/// What the overlay reads from the session and the frame, gathered once per frame.
pub struct WorldStatus {
    pub name: String,
    pub epoch: u64,
    pub last_submitted: Option<Tick>,
    pub ledger: LedgerSummary,
    pub counters: TransferCounters,
    pub residency: ResidencySummary,
    pub refused_move: Option<RefusedMove>,
    pub failed_chunks: usize,
    pub latest: CheckpointManifest,
    pub receipt: Option<SaveReceipt>,
    pub autosave: Option<u64>,
    pub previous_unclean: bool,
    pub fps: f64,
    pub frame_ms: f64,
    pub frame_p95_ms: f64,
    pub render: world_render::RenderCounters,
    pub presentations_in_flight: u32,
    pub sample_age_ticks: f64,
    pub discarded_ticks: u64,
    pub pending_commands: usize,
    pub body_capacity: u32,
    pub bodies_tracked: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HudAction {
    Save,
    Close,
    ApplySettings,
    Resize,
    /// A new autosave interval, in ticks.
    SetAutosave(u64),
}

fn save_text(receipt: Option<&SaveReceipt>) -> (String, egui::Color32) {
    match receipt.map(|receipt| (&receipt.state, receipt.reason, receipt.attempts)) {
        None => ("no save yet this session".to_owned(), egui::Color32::GRAY),
        Some((SaveState::Committed { checkpoint, tick }, reason, _)) => (
            format!(
                "{reason:?} save committed: checkpoint {} at tick {}",
                checkpoint.value(),
                tick_text(*tick)
            ),
            egui::Color32::LIGHT_GREEN,
        ),
        Some((SaveState::Failed { failure, retrying }, reason, attempts)) => (
            format!(
                "{reason:?} save FAILED after {attempts} attempts: {failure:?}{}",
                if *retrying {
                    "; the payloads are kept and the save retries"
                } else {
                    "; nothing was published"
                }
            ),
            egui::Color32::LIGHT_RED,
        ),
        Some((state, reason, _)) => (
            format!("{reason:?} save in progress: {state:?}"),
            egui::Color32::YELLOW,
        ),
    }
}

pub fn world_overlay(
    ui: &mut egui::Ui,
    hud: &mut Hud,
    play: &mut Play,
    status: &WorldStatus,
) -> Vec<HudAction> {
    let mut actions = Vec::new();
    if !hud.visible {
        return actions;
    }
    let ctx = ui.ctx().clone();
    egui::Panel::top("toolbar").show(ui, |ui| {
        ui.horizontal_centered(|ui| {
            let separator = |ui: &mut egui::Ui| {
                ui.add_space(SPACING);
                ui.separator();
                ui.add_space(SPACING);
            };
            ui.label(egui::RichText::new("Tools").strong().size(HEADING_SIZE));
            ui.add_space(SPACING);
            if ui
                .checkbox(&mut play.statistics_open, "Statistics")
                .changed()
            {
                hud.statistics = play.statistics_open;
            }
            ui.checkbox(&mut hud.simulation, "Simulation");
            ui.checkbox(&mut hud.lighting_panel, "Lighting");
            ui.checkbox(&mut hud.session, "World");
            ui.checkbox(&mut hud.outcomes, "Outcomes");
            separator(ui);

            ui.label(egui::RichText::new("Elevation Scale:").color(egui::Color32::LIGHT_GRAY));
            if ui
                .add(
                    egui::DragValue::new(&mut play.requested.elevation_scale)
                        .range(ELEVATION_SCALE)
                        .speed(ELEVATION_SPEED),
                )
                .changed()
            {
                actions.push(HudAction::ApplySettings);
            }
            separator(ui);

            ui.label(egui::RichText::new("Active Radius (X/Y):").color(egui::Color32::LIGHT_GRAY));
            ui.add(egui::DragValue::new(&mut play.requested.radius[0]).range(ACTIVE_RADIUS_CHUNKS));
            ui.label("x");
            ui.add(egui::DragValue::new(&mut play.requested.radius[1]).range(ACTIVE_RADIUS_CHUNKS));
            ui.add_space(SPACING);
            if ui.button("Apply Resize").clicked() && play.requested.radius != play.applied.radius {
                actions.push(HudAction::Resize);
            }
            separator(ui);

            ui.label(egui::RichText::new("Brush:").strong());
            egui::ComboBox::from_id_salt("brush_selector")
                .selected_text(play.tool.label())
                .show_ui(ui, |ui| {
                    for tool in TOOLS {
                        if ui
                            .selectable_label(play.tool == tool, tool.label())
                            .clicked()
                        {
                            play.select_tool(tool);
                        }
                    }
                });
            if play.tool != Tool::None {
                ui.label("Radius:");
                ui.add(egui::DragValue::new(&mut play.brush.radius).range(BRUSH_RADIUS_CELLS));
                ui.label("Strength:");
                ui.add(egui::DragValue::new(&mut play.brush.strength).range(BRUSH_STRENGTH));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("FPS: {:.0}", status.fps))
                        .strong()
                        .color(if status.fps > FPS_GOOD {
                            egui::Color32::GREEN
                        } else {
                            egui::Color32::RED
                        }),
                );
            });
        });
    });

    if hud.simulation {
        egui::Window::new("Simulation")
            .anchor(egui::Align2::LEFT_TOP, [PANEL_OFFSET_X, PANEL_OFFSET_Y])
            .resizable(false)
            .collapsible(false)
            .show(&ctx, |ui| {
                let rules = &mut play.requested.rules;
                let mut changed = false;
                changed |= ui.checkbox(&mut rules.liquid, "Run Liquid").changed();
                changed |= ui.checkbox(&mut rules.granular, "Run Granular").changed();
                changed |= ui.checkbox(&mut rules.thermal, "Run Thermal").changed();
                changed |= ui.checkbox(&mut rules.fire, "Run Fire").changed();
                changed |= ui.checkbox(&mut rules.biology, "Run Biology").changed();
                ui.separator();
                egui::Grid::new("simulation_grid").show(ui, |ui| {
                    ui.label("Tick Rate (Hz):");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut play.requested.tick_rate_hz)
                                .range(TICK_RATE_HZ),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("Max Ticks / Frame:");
                    changed |= ui
                        .add(
                            egui::DragValue::new(&mut play.requested.max_ticks_per_frame)
                                .range(MAX_TICKS_PER_FRAME),
                        )
                        .changed();
                    ui.end_row();
                    // A change wakes every resident cell once, so only an actual edit sends it.
                    ui.label("Temperature (°C):");
                    changed |= ui
                        .add(egui::Slider::new(
                            &mut play.requested.ambient_c,
                            AMBIENT_TEMPERATURE_C,
                        ))
                        .changed();
                    ui.end_row();
                });
                if changed {
                    actions.push(HudAction::ApplySettings);
                }
            });
    }

    if hud.lighting_panel {
        egui::Window::new("Lighting")
            .anchor(egui::Align2::LEFT_BOTTOM, [PANEL_OFFSET_X, -PANEL_OFFSET_X])
            .resizable(false)
            .collapsible(false)
            .show(&ctx, |ui| {
                egui::Grid::new("lighting_grid").show(ui, |ui| {
                    let slider = |ui: &mut egui::Ui, label: &str, value: &mut f32, range| {
                        ui.label(label);
                        ui.add(egui::Slider::new(value, range));
                        ui.end_row();
                    };
                    let lighting = &mut hud.lighting;
                    slider(
                        ui,
                        "Sun Azimuth (deg):",
                        &mut lighting.sun_azimuth_degrees,
                        SUN_AZIMUTH_RANGE,
                    );
                    slider(
                        ui,
                        "Sun Elevation (deg):",
                        &mut lighting.sun_elevation_degrees,
                        SUN_ELEVATION_RANGE,
                    );
                    slider(
                        ui,
                        "Sky Brightness:",
                        &mut lighting.ambient_brightness,
                        AMBIENT_RANGE,
                    );
                    slider(
                        ui,
                        "Terrain Shadows:",
                        &mut lighting.terrain_shadow_strength,
                        UNIT_RANGE,
                    );
                    slider(
                        ui,
                        "Ambient Occlusion:",
                        &mut lighting.ambient_occlusion_strength,
                        UNIT_RANGE,
                    );
                });
                ui.separator();
                ui.checkbox(&mut hud.debug.activity, "Debug: activity tiles (F3)");
                ui.checkbox(&mut hud.debug.bodies, "Debug: sphere contact state");
            });
    }

    compass::show(
        &ctx,
        play.observer.yaw,
        play.observer.pitch,
        hud.lighting.sun_azimuth_degrees,
    );

    let below_compass = compass::ANCHOR_OFFSET[1] + compass::FOOTPRINT_HEIGHT + STACK_GAP;
    if play.statistics_open {
        egui::Window::new("World Statistics")
            .anchor(
                egui::Align2::RIGHT_TOP,
                [compass::ANCHOR_OFFSET[0], below_compass],
            )
            .resizable(false)
            .collapsible(false)
            .show(&ctx, |ui| {
                egui::Grid::new("stats_grid")
                    .striped(true)
                    .show(ui, |ui| match play.totals {
                        Some((tick, totals)) => {
                            ui.label(egui::RichText::new("Total Liquid Vol:").strong());
                            ui.label(
                                egui::RichText::new(totals.fluid_volume().to_string())
                                    .color(egui::Color32::CYAN),
                            );
                            ui.end_row();
                            ui.label(egui::RichText::new("Total Elevation:").strong());
                            ui.label(
                                egui::RichText::new(totals.elevation.to_string())
                                    .color(egui::Color32::LIGHT_GRAY),
                            );
                            ui.end_row();
                            ui.label(egui::RichText::new("Loose Material:").strong());
                            ui.label(totals.granular.to_string());
                            ui.end_row();
                            ui.label("Measured at tick:");
                            ui.label(format!("{} ({} cells)", tick.value(), totals.cells));
                            ui.end_row();
                        }
                        None => {
                            ui.label("Measuring on the GPU…");
                            ui.end_row();
                        }
                    });
            });
    }

    if hud.session {
        egui::Window::new("World")
            .anchor(
                egui::Align2::RIGHT_BOTTOM,
                [-PANEL_OFFSET_X, -PANEL_OFFSET_X],
            )
            .resizable(false)
            .collapsible(true)
            .show(&ctx, |ui| world_panel(ui, play, status, &mut actions));
    }

    if hud.outcomes {
        egui::Window::new("Outcomes")
            .anchor(egui::Align2::LEFT_BOTTOM, [PANEL_OFFSET_X, -280.0])
            .default_width(420.0)
            .collapsible(true)
            .show(&ctx, |ui| outcome_panel(ui, play));
    }

    // The cursor's GPU-resolved target, beside the pointer.
    if let (Some(target), Some(pointer)) = (play.cursor, ctx.pointer_hover_pos())
        && play.tool != Tool::None
    {
        let text = match (target.hit.cell(), target.body) {
            (_, Some(body)) => format!("sphere slot {} ({:.1})", body.body.index(), body.distance),
            (Some(cell), None) => format!("({}, {}) tick {}", cell.x, cell.y, target.tick.value()),
            (None, None) => "no surface".to_owned(),
        };
        egui::Area::new(egui::Id::unique("cursor_target"))
            .fixed_pos(pointer + egui::vec2(16.0, 16.0))
            .interactable(false)
            .show(&ctx, |ui| {
                ui.label(
                    egui::RichText::new(text)
                        .small()
                        .background_color(egui::Color32::from_black_alpha(160)),
                );
            });
    }
    actions
}

fn world_panel(
    ui: &mut egui::Ui,
    play: &mut Play,
    status: &WorldStatus,
    actions: &mut Vec<HudAction>,
) {
    ui.label(egui::RichText::new(&status.name).strong());
    egui::Grid::new("world_grid").striped(true).show(ui, |ui| {
        ui.label("Session epoch");
        ui.label(status.epoch.to_string());
        ui.end_row();
        ui.label("Tick submitted / observed");
        ui.label(format!(
            "{} / {}",
            tick_text(status.last_submitted),
            tick_text(status.ledger.last_observed_tick)
        ));
        ui.end_row();
        ui.label("Latest checkpoint");
        let since = match (status.last_submitted, status.latest.tick) {
            (Some(last), Some(saved)) => last.since(saved),
            (Some(last), None) => last.value() + 1,
            (None, _) => 0,
        };
        ui.label(format!(
            "{} at tick {} ({} chunks, {} spheres); {since} ticks since, which a crash would lose",
            status.latest.id.value(),
            tick_text(status.latest.tick),
            status.latest.chunks,
            status.latest.bodies
        ));
        ui.end_row();
        ui.label("Last save");
        let (text, colour) = save_text(status.receipt.as_ref());
        ui.colored_label(colour, text);
        ui.end_row();
        ui.label("Autosave every");
        match status.autosave {
            Some(mut every) => {
                // The interval bounds what a crash or a lost device loses; it is never off.
                let edited = ui
                    .add(
                        egui::DragValue::new(&mut every)
                            .range(AUTOSAVE_TICKS)
                            .suffix(" ticks"),
                    )
                    .on_hover_text(
                        "A crash or a lost device loses at most this and the save in progress.",
                    )
                    .changed();
                if edited {
                    actions.push(HudAction::SetAutosave(every));
                }
            }
            None => {
                ui.label("off (a scripted run saves when its script says)");
            }
        }
        ui.end_row();
        ui.label("Resident chunks");
        let r = &status.residency;
        ui.label(format!(
            "{} (loading {}, uploading {}, evicting {}, failed {})",
            r.resident, r.loading, r.uploading, r.evicting, r.failed
        ));
        ui.end_row();
        ui.label("Writes");
        ui.label(format!(
            "{} committed, {} pending, {} failed; retained {} KiB",
            r.writes_committed,
            r.writes_pending,
            r.write_failures,
            r.retained_bytes / 1024
        ));
        ui.end_row();
        ui.label("Pressure");
        ui.label(match (r.pressure, &status.refused_move) {
            (None, None) => "none".to_owned(),
            (pressure, refused) => format!(
                "{}{}",
                pressure.map_or(String::new(), |p| format!("residency {p:?} ")),
                refused.map_or(String::new(), |m| format!(
                    "window move refused: {:?}",
                    m.refusal
                ))
            ),
        });
        ui.end_row();
        ui.label("Ticks in flight / frames");
        ui.label(format!(
            "high water {}, frames {}; {} discarded ticks",
            status.counters.in_flight_high_water,
            status.presentations_in_flight,
            status.discarded_ticks
        ));
        ui.end_row();
        ui.label("Observation age");
        ui.label(format!(
            "{:.1} ticks (sample), {:.1} ticks max (outcomes)",
            status.sample_age_ticks, status.ledger.observation_age_ticks.max
        ));
        ui.end_row();
        ui.label("Commands outstanding");
        ui.label(format!(
            "{} ({} in the client)",
            status.pending_commands,
            play.outstanding()
        ));
        ui.end_row();
        ui.label("Spheres");
        ui.label(format!(
            "{} tracked of {}",
            status.bodies_tracked, status.body_capacity
        ));
        ui.end_row();
        ui.label("Unbalanced ticks");
        let unbalanced = status.ledger.inconsistent_ticks;
        ui.colored_label(
            if unbalanced == 0 {
                egui::Color32::GRAY
            } else {
                egui::Color32::LIGHT_RED
            },
            unbalanced.to_string(),
        );
        ui.end_row();
        ui.label("Frame");
        ui.label(format!(
            "{:.2} ms (p95 {:.2})",
            status.frame_ms, status.frame_p95_ms
        ));
        ui.end_row();
        ui.label("Renderer");
        ui.label(format!(
            "{} chunk instances, {} sphere slots; bind groups {}, targets {}",
            status.render.chunk_instances,
            status.render.body_instances,
            status.render.world_bind_groups_created,
            status.render.target_sets_created
        ));
        ui.end_row();
    });
    if status.previous_unclean {
        ui.colored_label(
            egui::Color32::YELLOW,
            "The previous session did not close: the world resumed at its latest checkpoint, what \
             came after it was lost, and what it left unpublished was reclaimed.",
        );
    }
    if status.failed_chunks > 0 {
        ui.colored_label(
            egui::Color32::LIGHT_RED,
            format!(
                "{} chunks could not be loaded and stay out of the world (never generated over).",
                status.failed_chunks
            ),
        );
    }
    ui.horizontal(|ui| {
        if ui.button("Save now (F5)").clicked() {
            actions.push(HudAction::Save);
        }
        if ui.button("Close world").clicked() {
            actions.push(HudAction::Close);
        }
    });
}

fn outcome_panel(ui: &mut egui::Ui, play: &Play) {
    let counts = play.counts;
    ui.label(format!(
        "accepted {}, applied {}, rejected {}, refused at admission {}; last latency {} ticks",
        counts.accepted,
        counts.applied,
        counts.rejected,
        counts.refused_at_admission,
        counts.latency_ticks
    ));
    ui.separator();
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .show(ui, |ui| {
            for entry in &play.log {
                let (colour, text) = match &entry.verdict {
                    Verdict::Accepted => (egui::Color32::GRAY, "accepted".to_owned()),
                    Verdict::Applied(text) => (egui::Color32::LIGHT_GREEN, text.clone()),
                    Verdict::Rejected(text) => (egui::Color32::LIGHT_RED, text.clone()),
                };
                ui.colored_label(
                    colour,
                    format!(
                        "{} {}: {text}",
                        entry
                            .tick
                            .map_or("     -".to_owned(), |tick| format!("{:>6}", tick.value())),
                        entry.what
                    ),
                );
            }
        });
    ui.separator();
    ui.label("Sphere events (from the body monitor)");
    egui::ScrollArea::vertical()
        .id_salt("body_events")
        .max_height(120.0)
        .show(ui, |ui| {
            for event in play.body_events.iter().take(24) {
                ui.label(format!("{:>6} {event:?}", event.tick().value()));
            }
        });
}

// ---------------------------------------------------------------------------------------------
// Dialogs

pub enum LossAction {
    Recover,
    Close,
}

pub fn loss_dialog(ui: &mut egui::Ui, name: &str, report: &LossReport) -> Option<LossAction> {
    let mut action = None;
    egui::CentralPanel::default().show(ui, |ui| {
        ui.heading(format!("{name}: the GPU device was lost"));
        ui.label(format!("Reason: {:?}", report.reason));
        ui.label(format!(
            "The latest complete checkpoint is {} at tick {}. The last tick submitted was {}; {} ticks after the checkpoint are lost.",
            report.last_complete.value(),
            tick_text(report.last_complete_tick),
            tick_text(report.last_submitted),
            report.ticks_lost
        ));
        ui.label(format!(
            "{} chunks were resident; changes since their last snapshot are gone. {} captured revisions were written but no complete checkpoint holds them.",
            report.resident_chunks, report.captured_not_published
        ));
        if let Some(every) = report.autosave_ticks {
            ui.label(format!(
                "Autosave ran every {every} ticks: a loss takes at most that and the save in \
                 progress, unless saves were failing."
            ));
        }
        if report.finished_captured_save {
            ui.label("A save that had everything on the CPU was finished after the loss.");
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Recover from the checkpoint").clicked() {
                action = Some(LossAction::Recover);
            }
            if ui.button("Back to worlds").clicked() {
                action = Some(LossAction::Close);
            }
        });
    });
    action
}

pub fn notice(ui: &mut egui::Ui, text: &str) {
    egui::CentralPanel::default().show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 2.0 - 20.0);
            ui.heading(text);
        });
    });
}
