// begin_tick: initializes the result record of every command position the tick uses.
//
// Input: the tick's command records and parameters. Output: results[0..command_count], one
// invocation per record, so each record has one writer. Runs before any command kernel of the
// tick; wgpu orders the following dispatches after it because they use the same read-write
// storage. Positions past command_count are not touched and are never read back.

@compute @workgroup_size(64, 1, 1)
fn begin_tick(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= tick.command_count) {
        return;
    }
    let command = commands[index];
    results[index].id_lo = command.id_lo;
    results[index].id_hi = command.id_hi;
    results[index].sequence = command.sequence;
    results[index].tick_lo = tick.tick_lo;
    results[index].tick_hi = tick.tick_hi;
    atomicStore(&results[index].changed, 0u);
    atomicStore(&results[index].not_resident, 0u);
    results[index].word0 = 0u;
    results[index].word1 = 0u;
    results[index].slot_index = SLOT_NONE;
    results[index].slot_generation = 0u;
    results[index].status = select(RESULT_PENDING, RESULT_EDITED, command.kind == COMMAND_EDIT);
}
