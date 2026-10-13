//! Checkpoint B: the packed cell and record layouts executed on the device, and the
//! deliberately corrupted specimens the probes must detect.

use super::*;

/// Cells that put every field at its maximum alone, all ones, all zeros, then arbitrary words.
pub(super) fn boundary_cells() -> Vec<WorldCell> {
    let mut cells = pattern(7);
    for (index, field) in WorldCell::LAYOUT.iter().enumerate() {
        cells[index] = WorldCell(field.max() << field.shift);
    }
    cells[11] = WorldCell(u64::MAX);
    cells[12] = WorldCell(0);
    cells
}

pub(super) fn cell_fields(cell: WorldCell) -> [u32; 11] {
    [
        u32::from(cell.terrain_mat().0),
        u32::from(cell.surface_mat().0),
        u32::from(cell.granular_mat().0),
        u32::from(cell.fluid_mat().0),
        u32::from(cell.variants()),
        u32::from(cell.elevation()),
        u32::from(cell.fluid_vol()),
        u32::from(cell.granular_vol()),
        u32::from(cell.surface_state()),
        u32::from(cell.terrain_state()),
        u32::from(cell.compass()),
    ]
}

pub(super) fn cell_setters(cell: WorldCell) -> [WorldCell; 9] {
    use world_model::material::TerrainMat;
    let mut out = [cell; 9];
    out[0].set_terrain_mat(TerrainMat(15));
    out[1].set_surface_mat(SurfaceMat(15));
    out[2].set_granular_mat(GranularMat(15));
    out[3].set_fluid_mat(FluidMat(7));
    out[4].set_elevation(0);
    out[5].set_fluid_vol(WorldCell::MAX_FLUID_VOL);
    out[6].set_granular_vol(0);
    out[7].set_surface_state(WorldCell::MAX_SURFACE_STATE);
    out[8].set_compass(0);
    out
}

/// Runs the cell probe over `count` cells of `slot` and returns the number of mismatches.
pub(super) fn probe_cells(
    driver: &mut Driver,
    declarations: SourceModule,
    slot: SlotRef,
    cells: &[WorldCell],
    count: u32,
    check: &mut Check,
    report: bool,
) -> Result<usize, String> {
    let assembly = probe::cell_probe(declarations);
    let params = [slot.index() * CHUNK_CELL_COUNT as u32, count, 0, 0];
    let groups = count.div_ceil(64);
    let outputs = probe::run(
        &mut driver.world,
        &assembly,
        "probe_cells_kernel",
        &[
            Binding::WorldCells,
            Binding::Output(groups * 64 * CELL_PROBE_WORDS),
            Binding::Output(groups),
            Binding::Uniform(&params),
        ],
        [groups, 1, 1],
    )?;
    let (fields, sums) = (&outputs[0], &outputs[1]);
    let mut mismatches = 0;
    for (index, cell) in cells.iter().take(count as usize).enumerate() {
        let base = index * CELL_PROBE_WORDS as usize;
        let expected = cell_fields(*cell);
        if fields[base..base + 11] != expected {
            mismatches += 1;
            if report && mismatches <= 5 {
                check.fail(format!(
                    "cell {index} {:#018x}: fields {:?}, expected {expected:?}",
                    cell.0,
                    &fields[base..base + 11]
                ));
            }
        }
        for (setter, wanted) in cell_setters(*cell).iter().enumerate() {
            let words = [
                fields[base + 11 + setter * 2],
                fields[base + 12 + setter * 2],
            ];
            if WorldCell::from_words(words) != *wanted {
                mismatches += 1;
                if report && mismatches <= 5 {
                    check.fail(format!(
                        "cell {index} setter {setter}: {words:x?}, expected {:x?}",
                        wanted.to_words()
                    ));
                }
            }
        }
    }
    let untouched = fields[count as usize * CELL_PROBE_WORDS as usize..]
        .iter()
        .all(|word| *word == SENTINEL);
    if report {
        check.that(untouched, || {
            format!("inactive invocations wrote past cell {count}")
        });
    }
    for (group, sum) in sums.iter().enumerate() {
        let expected: u32 = cells
            .iter()
            .take(count as usize)
            .skip(group * 64)
            .take(64)
            .map(|cell| u32::from(cell.elevation()))
            .sum();
        if *sum != expected {
            mismatches += 1;
            if report {
                check.fail(format!(
                    "workgroup {group} elevation sum {sum}, expected {expected}"
                ));
            }
        }
    }
    Ok(mismatches)
}

pub(super) fn cell_layout_probe(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let cells = boundary_cells();
    let slot = driver.upload(ChunkKey::new(-1, 0, 0), &cells)?;
    for count in [1, 13, 63, 64, 65, CHUNK_CELL_COUNT as u32] {
        let mismatches = probe_cells(
            &mut driver,
            generated_declarations(),
            slot,
            &cells,
            count,
            check,
            true,
        )?;
        check.equal(&format!("mismatches over {count} cells"), mismatches, 0);
    }
    Ok(())
}

/// Runs the probe for `layout` with boundary samples and returns the mismatching words.
pub(super) fn probe_record(
    driver: &mut Driver,
    declarations: SourceModule,
    layout: &layout::RecordLayout,
) -> Result<Vec<String>, String> {
    let samples = 4usize;
    let input = probe::record_samples(layout, samples);
    let assembly = probe::record_probe(declarations, layout);
    let outputs = probe::run(
        &mut driver.world,
        &assembly,
        "probe_record",
        &[
            Binding::Storage(&input),
            Binding::Output(input.len() as u32),
        ],
        [samples as u32, 1, 1],
    )?;
    let fields = layout.fields.len();
    Ok(input
        .iter()
        .zip(&outputs[0])
        .enumerate()
        .filter(|(_, (written, read))| written != read)
        .map(|(index, (written, read))| {
            format!(
                "{}.{} sample {}: read {read:#010x}, wrote {written:#010x}",
                layout.name,
                layout.fields[index % fields].name,
                index / fields
            )
        })
        .collect())
}

pub(super) fn record_layout_probe(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    for record in RECORDS {
        for mismatch in probe_record(&mut driver, generated_declarations(), &record)? {
            check.fail(mismatch);
        }
    }
    check.metric("records", RECORDS.len() as u64);
    Ok(())
}

pub(super) fn layout_specimens_detected(adapter: &str, check: &mut Check) -> Result<(), String> {
    let mut driver = Driver::open(config(adapter, 1))?;
    let cells = boundary_cells();
    let slot = driver.upload(ChunkKey::new(0, 0, 0), &cells)?;

    // Specimen 1: elevation declared one bit too low, overlapping the variants.
    let mut shifted = WorldCell::LAYOUT;
    shifted[5].shift = 19;
    let text =
        layout::wgsl_declarations_for(&shifted).map_err(|error| format!("specimen: {error}"))?;
    let corrupted = SourceModule {
        path: "generated/layout.wgsl",
        text: text.into(),
    };
    let mismatches = probe_cells(&mut driver, corrupted, slot, &cells, 64, check, false)?;
    check.that(mismatches > 0, || {
        "a shifted elevation field was not detected".to_owned()
    });
    check.metric("shifted_field_mismatches", mismatches as u64);

    // Specimen 2: two fields of the result record declared in swapped order.
    let declarations = layout::wgsl_declarations();
    let original = "    changed: atomic<u32>,\n    not_resident: atomic<u32>,\n";
    let swapped = "    not_resident: atomic<u32>,\n    changed: atomic<u32>,\n";
    check.that(declarations.contains(original), || {
        "specimen anchor not found".to_owned()
    });
    let corrupted = SourceModule {
        path: "generated/layout.wgsl",
        text: declarations.replace(original, swapped).into(),
    };
    let mismatches = probe_record(&mut driver, corrupted, &layout::ResultRecord::LAYOUT)?;
    check.that(!mismatches.is_empty(), || {
        "swapped record fields were not detected".to_owned()
    });
    check.metric("swapped_field_mismatches", mismatches.len() as u64);

    // The same probes over the true declarations find nothing.
    let clean = probe_cells(
        &mut driver,
        generated_declarations(),
        slot,
        &cells,
        64,
        check,
        true,
    )?;
    check.equal("true declarations", clean, 0);
    Ok(())
}
