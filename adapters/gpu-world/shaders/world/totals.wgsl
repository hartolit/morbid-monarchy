// measure_totals: the exact sums of a MeasureTotals command over every resident cell, as the
// cells stand after the tick's earlier commands.
//
// Dispatch: (TILES_PER_CHUNK, window chunks, 1) workgroups of one tile each. A workgroup sums its
// 64 cells in workgroup memory (at most 64 x 4,095 per sum, well inside 32 bits), then adds each
// sum to the command's 64-bit totals record, carrying wraps of the low word. A diagnostic
// reduction: it reads every resident cell and returns a handful of integers.

var<workgroup> totals_sums: array<atomic<u32>, 11>;

@compute @workgroup_size(8, 8, 1)
fn measure_totals(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let index = dispatch_params.command_index;
    if (commands[index].kind != COMMAND_TOTALS) {
        return;
    }
    let chunk = wg.y;
    let slot = window_slots[chunk];
    if (slot == SLOT_NONE) {
        return;
    }
    if (li < 11u) {
        atomicStore(&totals_sums[li], 0u);
    }
    workgroupBarrier();
    let within_chunk = vec2<u32>(wg.x % TILES_PER_CHUNK_EDGE, wg.x / TILES_PER_CHUNK_EDGE) * TILE_EDGE + lid.xy;
    let cell = cells[slot * CHUNK_CELLS + within_chunk.y * CHUNK_SIZE + within_chunk.x];
    atomicAdd(&totals_sums[cell_fluid_material(cell)], cell_fluid_volume(cell));
    atomicAdd(&totals_sums[8], cell_elevation(cell));
    atomicAdd(&totals_sums[9], cell_granular_volume(cell));
    atomicAdd(&totals_sums[10], 1u);
    workgroupBarrier();
    if (li < 11u) {
        add64(DIAG_WORDS + index * TOTALS_WORDS + 2u * li, atomicLoad(&totals_sums[li]));
    }
}
