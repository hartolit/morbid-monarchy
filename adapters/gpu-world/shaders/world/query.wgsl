// query_column: copies one column into the command's result record.
//
// A single invocation; it reads the cell after every earlier command of the tick (dispatch
// order, with wgpu's barriers between dispatches) and writes only its own result record.

@compute @workgroup_size(1, 1, 1)
fn query_column() {
    let index = dispatch_params.command_index;
    let command = commands[index];
    if (command.kind != COMMAND_QUERY_COLUMN) {
        return;
    }
    let local = vec2<i32>(command.cell_x, command.cell_y);
    let slot = resident_slot(local);
    if (slot == SLOT_NONE) {
        results[index].status = RESULT_COLUMN_NOT_RESIDENT;
        return;
    }
    let cell = cells[cell_index(slot, local)];
    results[index].word0 = cell.x;
    results[index].word1 = cell.y;
    results[index].slot_index = slot;
    results[index].slot_generation = slot_records[slot].generation;
    results[index].status = RESULT_COLUMN;
}
