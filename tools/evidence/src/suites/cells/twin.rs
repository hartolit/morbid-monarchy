//! A GPU world and the CPU reference driven tick for tick through the same inputs.
//!
//! Every tick's diagnostics (the accounting and the activity counts) are compared with the
//! reference's; cells are compared by capturing every resident chunk, a labelled diagnostic
//! transfer of small fixtures. A capture runs one more tick, so the reference steps once more
//! too: both compare the cells at the end of the same tick.

use gpu_world::world::{Execution, GpuWorld, GpuWorldConfig};
use world_model::{
    cell::WorldCell,
    command::{AmbientTemperature, Command},
    glam::IVec2,
    rules::SimulationRules,
    spatial::{CHUNK_CELL_COUNT, ChunkKey},
};
use world_runtime::{
    ids::{Tick, WorldId},
    port::{
        CellReport, CommandResult, Outcome, PhysicalWorld, ResidentWindow, SlotRef, TickReport,
    },
};

use super::{
    super::{Check, driver::Driver},
    reference::{ReferenceWorld, TickDiagnostics},
};

/// How a twin is opened.
#[derive(Debug, Clone, Copy)]
pub struct Setup {
    pub center: ChunkKey,
    pub radius_x: u32,
    pub radius_y: u32,
    pub rules: SimulationRules,
    pub ambient_c: f32,
    pub seed: u64,
    pub execution: Execution,
    pub first_tick: u64,
    pub fault_injection: bool,
    pub timestamps: bool,
}

impl Setup {
    pub fn new(center: ChunkKey, radius: u32) -> Self {
        Self {
            center,
            radius_x: radius,
            radius_y: radius,
            rules: SimulationRules::ALL,
            ambient_c: world_model::config::DEFAULT_AMBIENT_TEMPERATURE_C,
            seed: 0x03C0_FFEE,
            execution: Execution::Sparse,
            first_tick: 0,
            fault_injection: false,
            timestamps: false,
        }
    }

    pub fn rules(mut self, rules: SimulationRules) -> Self {
        self.rules = rules;
        self
    }

    pub fn ambient(mut self, celsius: f32) -> Self {
        self.ambient_c = celsius;
        self
    }

    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn dense(mut self) -> Self {
        self.execution = Execution::Dense;
        self
    }

    pub fn first_tick(mut self, tick: u64) -> Self {
        self.first_tick = tick;
        self
    }

    pub fn faults(mut self) -> Self {
        self.fault_injection = true;
        self
    }

    pub fn window(&self) -> ResidentWindow {
        ResidentWindow::new(self.center, self.radius_x, self.radius_y).expect("supported window")
    }

    pub fn config(&self, adapter: &str) -> GpuWorldConfig {
        let mut config = GpuWorldConfig::new(WorldId::new(0x0300), adapter, self.window());
        config.allow_software = super::super::allow_software();
        config.rules = self.rules;
        config.ambient_temperature =
            AmbientTemperature::celsius(self.ambient_c).expect("supported temperature");
        config.rule_seed = self.seed;
        config.execution = self.execution;
        config.first_tick = Tick::new(self.first_tick);
        config.fault_injection = self.fault_injection;
        config.timestamps = self.timestamps;
        config
    }

    fn reference(&self) -> ReferenceWorld {
        let min = self.window().min_key();
        ReferenceWorld::new(
            IVec2::new(min.key.x, min.key.y),
            self.window().width(),
            self.window().height(),
            self.first_tick,
            self.seed,
            self.rules,
            self.ambient_c,
        )
    }
}

/// Differences between a GPU tick report and the reference's diagnostics, by field.
pub fn report_differences(
    gpu: &CellReport,
    reference: &TickDiagnostics,
    dense: bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    let pairs: [(&str, u64, u64); 16] = [
        ("fluid_in", gpu.fluid_in, reference.fluid_in),
        ("fluid_out", gpu.fluid_out, reference.fluid_out),
        ("solid_in", gpu.solid_in, reference.solid_in),
        ("solid_out", gpu.solid_out, reference.solid_out),
        ("quench", gpu.quench, reference.quench),
        ("boil", gpu.boil, reference.boil),
        ("burn", gpu.burn, reference.burn),
        ("uptake", gpu.uptake, reference.uptake),
        ("erosion", gpu.erosion, reference.erosion),
        ("tool_fluid_in", gpu.tool_fluid_in, reference.tool_fluid_in),
        ("tool_solid_in", gpu.tool_solid_in, reference.tool_solid_in),
        (
            "tool_solid_out",
            gpu.tool_solid_out,
            reference.tool_solid_out,
        ),
        (
            "awake_cells",
            u64::from(gpu.awake_cells),
            reference.awake_cells,
        ),
        (
            "active_cells",
            u64::from(gpu.active_cells),
            reference.active_cells,
        ),
        (
            "changed_cells",
            u64::from(gpu.changed_cells),
            reference.changed_cells,
        ),
        (
            "receive_mismatches",
            u64::from(gpu.receive_mismatches),
            reference.receive_mismatches,
        ),
    ];
    for (name, gpu, reference) in pairs {
        if gpu != reference {
            problems.push(format!("{name}: gpu {gpu}, reference {reference}"));
        }
    }
    if !dense && u64::from(gpu.process_tiles) != reference.process_tiles {
        problems.push(format!(
            "process_tiles: gpu {}, reference {}",
            gpu.process_tiles, reference.process_tiles
        ));
    }
    if gpu.stale_flow != 0 {
        problems.push(format!("stale_flow: {}", gpu.stale_flow));
    }
    problems
}

pub struct Twin {
    pub setup: Setup,
    pub driver: Driver,
    pub reference: ReferenceWorld,
    pub slots: Vec<(ChunkKey, SlotRef)>,
    /// The latest tick's report from the GPU.
    pub last: Option<TickReport>,
    /// Ticks whose report differed from the reference, and the first differences seen.
    pub report_mismatches: u64,
    pub first_mismatch: Option<String>,
    /// Ticks whose own accounting did not balance.
    pub inconsistent_ticks: u64,
    /// Work the compared ticks did: ticks, and the sums of awake, changed and processed tiles.
    pub totals: WorkTotals,
}

/// Sums over the ticks a twin compared, so a case shows it exercised the rules.
#[derive(Debug, Clone, Copy, Default)]
pub struct WorkTotals {
    pub ticks: u64,
    pub awake_cells: u64,
    pub changed_cells: u64,
    pub process_tiles: u64,
    pub moved_fluid: u64,
    pub moved_solid: u64,
}

impl Twin {
    /// Opens the GPU world and the reference and activates `chunks` (at most the upload pool's
    /// size per tick; more activate over several ticks, mirrored in the reference).
    pub fn open(
        adapter: &str,
        setup: Setup,
        chunks: &[(ChunkKey, Vec<WorldCell>)],
    ) -> Result<Self, String> {
        let driver = Driver::open(setup.config(adapter))?;
        let mut twin = Self {
            setup,
            driver,
            reference: setup.reference(),
            slots: Vec::new(),
            last: None,
            report_mismatches: 0,
            first_mismatch: None,
            inconsistent_ticks: 0,
            totals: WorkTotals::default(),
        };
        twin.driver.next_tick = Tick::new(setup.first_tick);
        twin.activate(chunks)?;
        Ok(twin)
    }

    /// Activates chunks in batches of the upload pool's size, one tick per batch.
    pub fn activate(&mut self, chunks: &[(ChunkKey, Vec<WorldCell>)]) -> Result<(), String> {
        let batch = self.driver.world.capabilities().chunk_upload_slots as usize;
        for group in chunks.chunks(batch) {
            for (key, cells) in group {
                let slot = self.driver.stage_upload(*key, cells)?;
                self.reference
                    .activate(IVec2::new(key.key.x, key.key.y), cells);
                self.slots.push((*key, slot));
            }
            self.step(&[])?;
        }
        Ok(())
    }

    /// Retires a chunk in the next tick (which this does not run).
    pub fn retire(&mut self, key: ChunkKey) -> Result<(), String> {
        let position = self
            .slots
            .iter()
            .position(|(resident, _)| *resident == key)
            .ok_or_else(|| format!("{key:?} is not resident"))?;
        let (_, slot) = self.slots.remove(position);
        self.driver
            .world
            .retire_slot(slot)
            .map_err(|refusal| format!("retire {key:?}: {refusal:?}"))?;
        self.reference.retire(IVec2::new(key.key.x, key.key.y));
        Ok(())
    }

    /// Runs one tick of `commands` on both and compares the tick's report. Returns the GPU's
    /// command results in order.
    pub fn step(&mut self, commands: &[Command]) -> Result<Vec<CommandResult>, String> {
        let mut expected = Vec::with_capacity(commands.len());
        for command in commands {
            expected.push(self.apply_to_reference(command));
        }
        let tick = self.driver.next_tick;
        self.driver.outcomes.clear();
        let ids = self
            .driver
            .submit(commands)
            .map_err(|refusal| format!("submit: {refusal:?}"))?;
        self.driver.settle()?;
        self.reference.step();
        let report = self
            .driver
            .outcomes
            .iter()
            .find_map(|outcome| match outcome {
                Outcome::TickCompleted(report) if report.tick == tick => Some(*report),
                _ => None,
            })
            .ok_or_else(|| format!("no report for {tick:?}"))?;
        self.last = Some(report);
        let work = &mut self.totals;
        work.ticks += 1;
        work.awake_cells += u64::from(report.cells.awake_cells);
        work.changed_cells += u64::from(report.cells.changed_cells);
        work.process_tiles += u64::from(report.cells.process_tiles);
        work.moved_fluid += report.cells.fluid_out;
        work.moved_solid += report.cells.solid_out;
        if !report.cells.consistent() {
            self.inconsistent_ticks += 1;
        }
        let differences = report_differences(
            &report.cells,
            &self.reference.last(),
            self.setup.execution == Execution::Dense,
        );
        if !differences.is_empty() {
            self.report_mismatches += 1;
            if self.first_mismatch.is_none() {
                self.first_mismatch =
                    Some(format!("tick {}: {}", tick.value(), differences.join("; ")));
            }
        }
        let mut results = Vec::with_capacity(ids.len());
        for (id, expected) in ids.iter().zip(expected) {
            let result = self
                .driver
                .result(*id)
                .ok_or_else(|| format!("no outcome for {id:?}"))?;
            if let Some(expected) = expected
                && expected != result
            {
                self.report_mismatches += 1;
                if self.first_mismatch.is_none() {
                    self.first_mismatch = Some(format!(
                        "tick {}: result {result:?}, reference {expected:?}",
                        tick.value()
                    ));
                }
            }
            results.push(result);
        }
        Ok(results)
    }

    /// Applies a command to the reference; returns the result the GPU must report when the
    /// reference can say it.
    fn apply_to_reference(&mut self, command: &Command) -> Option<CommandResult> {
        match *command {
            Command::EditMaterial(edit) => {
                let inside = self.stroke_inside(edit.center(), edit.radius());
                let changed =
                    self.reference
                        .edit(edit.tool(), edit.center(), edit.radius(), edit.strength());
                inside.then_some(CommandResult::Edited {
                    changed,
                    not_resident: 0,
                })
            }
            Command::SetAmbientTemperature(temperature) => {
                self.reference.set_ambient(temperature.degrees());
                Some(CommandResult::Configured)
            }
            Command::SetSimulationRules(rules) => {
                self.reference.set_rules(rules);
                Some(CommandResult::Configured)
            }
            Command::MeasureTotals => Some(CommandResult::Totals(self.reference.totals())),
            Command::QueryColumn { cell } => Some(match self.reference.cell(cell) {
                Some(column) => CommandResult::Column { cell, column },
                None => CommandResult::ColumnNotResident { cell },
            }),
            _ => None,
        }
    }

    /// Whether every cell of a stroke is resident (otherwise its not-resident count is the
    /// GPU's to report, checked by the transfers suite).
    fn stroke_inside(&self, center: IVec2, radius: i32) -> bool {
        (-radius..=radius).all(|dy| {
            (-radius..=radius).all(|dx| {
                dx * dx + dy * dy > radius * radius
                    || self.reference.cell(center + IVec2::new(dx, dy)).is_some()
            })
        })
    }

    /// Runs empty ticks.
    pub fn run(&mut self, ticks: u32) -> Result<(), String> {
        for _ in 0..ticks {
            self.step(&[])?;
        }
        Ok(())
    }

    /// Runs empty ticks until a tick has no awake cell; returns the ticks run, or `None` after
    /// `limit` ticks.
    pub fn run_until_asleep(&mut self, limit: u32) -> Result<Option<u32>, String> {
        for tick in 1..=limit {
            self.step(&[])?;
            if self.last.is_some_and(|last| last.cells.awake_cells == 0) {
                return Ok(Some(tick));
            }
        }
        Ok(None)
    }

    /// Captures every resident chunk at the end of one more (empty) tick, compares it with the
    /// reference after the same tick, reports differences to `check` under `label`, and returns
    /// the captured cells.
    pub fn compare_cells(
        &mut self,
        check: &mut Check,
        label: &str,
    ) -> Result<Vec<(ChunkKey, Vec<WorldCell>)>, String> {
        let captured = self.capture_all()?;
        let mut differing = 0;
        let mut example = None;
        for (key, cells) in &captured {
            let reference = self.reference.chunk(IVec2::new(key.key.x, key.key.y));
            for (index, (gpu, cpu)) in cells.iter().zip(&reference).enumerate() {
                if gpu != cpu {
                    differing += 1;
                    example.get_or_insert(format!(
                        "{key:?} cell {index}: gpu {:#018x}, reference {:#018x}",
                        gpu.0, cpu.0
                    ));
                }
            }
        }
        check.that(differing == 0, || {
            format!(
                "{label}: {differing} cells differ from the reference at tick {}; {}",
                self.reference.tick() - 1,
                example.clone().unwrap_or_default()
            )
        });
        Ok(captured)
    }

    /// Every resident chunk's cells at the end of one more (empty) tick, which both run.
    pub fn capture_all(&mut self) -> Result<Vec<(ChunkKey, Vec<WorldCell>)>, String> {
        let mut tickets = Vec::new();
        for (key, slot) in &self.slots {
            let ticket = self
                .driver
                .world
                .capture_chunk(*slot)
                .map_err(|refusal| format!("capture {key:?}: {refusal:?}"))?;
            tickets.push((*key, ticket));
        }
        self.step(&[])?;
        let mut captured = Vec::with_capacity(tickets.len());
        for (key, ticket) in tickets {
            self.driver.poll_until("capture", |driver| {
                driver.outcomes.iter().any(|outcome| {
                    matches!(outcome, Outcome::CaptureReady { ticket: ready, .. } if *ready == ticket)
                })
            })?;
            let mut cells = vec![WorldCell::default(); CHUNK_CELL_COUNT];
            self.driver
                .world
                .take_capture(ticket, &mut cells)
                .map_err(|refusal| format!("take capture: {refusal:?}"))?;
            captured.push((key, cells));
        }
        Ok(captured)
    }

    /// Reports the tick comparisons and the accounting to `check`.
    pub fn finish(&self, check: &mut Check, label: &str) {
        check.that(self.report_mismatches == 0, || {
            format!(
                "{label}: {} ticks differ from the reference; first: {}",
                self.report_mismatches,
                self.first_mismatch.clone().unwrap_or_default()
            )
        });
        check.equal(
            &format!("{label}: inconsistent ticks"),
            self.inconsistent_ticks,
            0,
        );
        let work = self.totals;
        check.metric(&format!("{label}_ticks"), work.ticks);
        check.metric(&format!("{label}_awake_cells"), work.awake_cells);
        check.metric(&format!("{label}_changed_cells"), work.changed_cells);
        check.metric(&format!("{label}_process_tiles"), work.process_tiles);
        check.metric(&format!("{label}_moved_fluid"), work.moved_fluid);
        check.metric(&format!("{label}_moved_solid"), work.moved_solid);
    }

    pub fn world(&self) -> &GpuWorld {
        &self.driver.world
    }
}

/// A chunk of `cell` everywhere.
pub fn uniform_chunk(cell: WorldCell) -> Vec<WorldCell> {
    vec![cell; CHUNK_CELL_COUNT]
}

/// Index of in-chunk cell `(x, y)`.
pub fn at(x: usize, y: usize) -> usize {
    y * world_model::spatial::CHUNK_SIZE + x
}
