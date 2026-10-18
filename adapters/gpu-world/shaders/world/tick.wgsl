// begin_tick: initializes the result record of every command position the tick uses.
//
// Input: the tick's command records and parameters. Output: results[0..command_count], one
// invocation per record, so each record has one writer. Runs before any command kernel of the
// tick; wgpu orders the following dispatches after it because they use the same read-write
// storage. Positions past command_count are not touched and are never read back. A totals
// command's record is zeroed here, before its reduction adds to it.

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
    atomicStore(&results[index].count, 0u);
    results[index].v0 = 0u;
    results[index].v1 = 0u;
    results[index].v2 = 0u;
    results[index].v3 = 0u;
    results[index].v4 = BODY_NONE;
    results[index].v5 = 0u;
    results[index].v6 = 0u;
    results[index].v7 = 0u;
    results[index].v8 = 0u;
    results[index].v9 = 0u;
    var status = RESULT_PENDING;
    switch command.kind {
        case COMMAND_EDIT: {
            status = RESULT_EDITED;
        }
        case COMMAND_CONFIGURE: {
            // Applied by the CPU to this tick's parameters; nothing runs on the device.
            status = RESULT_CONFIGURED;
        }
        case COMMAND_TOTALS: {
            status = RESULT_TOTALS;
            let base = DIAG_WORDS + index * TOTALS_WORDS;
            for (var word = 0u; word < TOTALS_WORDS; word++) {
                atomicStore(&diagnostics[base + word], 0u);
            }
        }
        // The body commands start from their success status; their kernels replace it when
        // the target is missing or the body stale.
        case COMMAND_SPAWN: {
            status = RESULT_BODY_SPAWNED;
        }
        case COMMAND_REMOVE: {
            status = RESULT_BODY_REMOVED;
        }
        case COMMAND_DRIVE: {
            status = RESULT_DRIVEN;
        }
        case COMMAND_JUMP: {
            status = RESULT_JUMPED;
        }
        case COMMAND_ATTRACT: {
            status = RESULT_ATTRACTED;
        }
        case COMMAND_LIFT: {
            status = RESULT_LIFTED;
        }
        case COMMAND_QUERY_RAY: {
            status = RESULT_RAY;
        }
        // Answered on the CPU (a stale handle, a full body table): the status travels in
        // `tool`.
        case COMMAND_REFUSED: {
            status = command.tool;
        }
        default: {}
    }
    results[index].status = status;
}
