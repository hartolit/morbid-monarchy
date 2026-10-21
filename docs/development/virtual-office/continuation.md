# Continuation note

**Written 2026-10-21 at the end of the GPU transition, for the next agent; updated the same day
after three open items were done.** No sprint is active and no successor is authorized: the next
assignment is the user's to choose. This note says where work stopped, what is open and how to work
on this machine. It is temporary; replace or remove it when the next assignment starts, and move
anything lasting into its maintained owner.

## Where things stand

- **Branch `experimental`, clean tree, nothing pushed.** The transition's commits end with
  `d9bffb6` (resident-world step 6), `eb9d8fb` (the code every 07 lane and benchmark ran on),
  `b49b05c` (07's documents) and `981b0ea` (sprint removed).
- **After the transition, at the user's choice:** `c4271ad` stopped the benchmark's pacing rule
  from rejecting saturated runs whose isolated catch-up frames end on the 60 Hz tick cadence. Its
  spike test now counts only frames in stretches of at least two; the decision and its evidence are
  in the [regression policy](../../reference/performance-budgets.md#regression-and-improvement-policy),
  the recorded runs it is tested on in `tools/evidence/tests/fixtures/pacing/`.
- **Then the client's loop without vsync:** `2c63b64` runs a frame only when one has work and
  otherwise waits 250 µs at a time (`GpuWorld::poll_admission`, `render`
  `presentation-admission`). `check all` (153 of 153), the scripts (`features` 3×41, `loss`,
  `worlds`) and all three benchmark profiles (5 runs each, none rejected) ran on it; the comparison
  with 07 is in the [budgets](../../reference/performance-budgets.md#client-loop-without-vsync-after-07)
  and its reports under `target/evidence/loop-*`.
- **Then the command-journal item, decided against a journal:** `4f42879` and `2e1a938` measure
  replay from a checkpoint and the cost of checkpoint cadence (`streaming` `replay-reproduction`,
  `checkpoint-cadence-*`); `0ec68c5` records the decision
  ([ADR 0004](../../decisions/0004-bounded-loss-by-checkpoint-cadence.md), the
  [whiteboard topic](whiteboard/discussions/command-journal.md)); `b7d86bb` tests version-1
  worlds against a committed fixture and refuses damaged ones unchanged; `21674ad` keeps the
  world snapshot's device image between saves; `2932764` autosaves every 60 ticks by default and
  never off, with the `bounded-loss` case. Measurements in the
  [budgets](../../reference/performance-budgets.md#checkpoint-cadence-after-07), reports under
  `target/evidence/journal-*`, drafts and helpers (`cost.py`, `table.py` summarize a cadence
  case's result) under `target/drafts/journal/`.
- **The GPU transition (00–07) is complete and qualified** on the nominated desktop (RTX 5070 Ti,
  `vulkan-10de-2c05`, NVIDIA 615.78.08, Linux 7.2.11, KWin Wayland). The final acceptance of
  A01–A15, the feature inventory, the baseline's dispositions and the commands are in the
  [qualification](../qualification.md); the measurements in the
  [performance budgets](../../reference/performance-budgets.md#integrated-qualification-07); what
  exists now in [status](../../product/status.md).
- **The sprint's prompts, handoff, acceptance matrix and source audit are only in Git:**
  `git log -- docs/development/virtual-office/gpu-transition`, and `git show b49b05c:PATH` for the
  last version of a file.

## Open items, in a sensible order

None of these is authorized work; propose them to the user. The first comes from 07's
findings ([status](../../product/status.md#known-gaps) lists the gaps).

1. **The 10.09 ms swapchain acquisition waits** without vsync (up to 22% of presented frames in
   07, at most 4.2% since `2c63b64`) set the worst p99 near 10.7 ms. Measured on KWin Wayland
   (mailbox) and Xwayland (immediate), absent with FIFO, unchanged by one more swapchain image;
   blaming the NVIDIA driver is an inference. Next evidence would be another compositor or driver.
2. **Capacity limits:** 4,096 dense bodies at radius 32 take about 8 ms of device time per tick,
   and the simulation falls to 51–53 ticks/s with counted discarded ticks. The renderer at radius
   32 makes frames 18–27 ms, and since `2c63b64` one frame of a dense run takes 40–56 ms (its
   cause is an inference in the budgets). No frame rate is promised there; see the
   [dense-pile discussion](whiteboard/discussions/dense-pile-convergence.md) before touching the
   body solver.
3. **Unqualified:** every other adapter, driver, compositor and platform.
4. **After the autosave cadence:** the `loss` and `worlds` scripts passed again on `dfeb530`'s
   code and the frozen `coupled` scenarios ran in the window with `--autosave 60` at the frame
   times of autosave off ([budgets](../../reference/performance-budgets.md#checkpoint-cadence-after-07));
   `features` last ran on `2c63b64`, and the frozen benchmark protocol still saves never. A
   command journal for a loss below the cadence is parked on the whiteboard with what it needs
   first; a future change of the format, layout, physics or generator still needs a migration of
   version-1 worlds, which the `world-store` fixture test enforces.

The [roadmap](../../product/roadmap.md#after-qualification) says how to choose what comes after
the transition.

## How to work here

- **Start** with [AGENTS.md](../../../AGENTS.md), then vision, architecture, status and
  [workflow](../workflow.md). Commit locally by responsibility with the checks in the message;
  no push, squash, rebase, hard reset or destructive cleanup.
- **Builds:** the disk is nearly full and debug builds take about 80 GiB: build with
  `CARGO_BUILD_JOBS=4` and never run `cargo clean`.
- **Evidence lanes** (`cargo evidence ...`) hash the whole tree and fail `identity.stable` if a
  tracked file is edited or staged while they run. Draft under the git-ignored `target/` instead
  (the `/tmp` scratchpad is wiped by reboots). The full gate is
  `cargo evidence check all --adapter vulkan-10de-2c05`, about 3–5 minutes.
- **Anything that opens a window** (`--consumer native`, the `native-client` scripts,
  `cargo evidence bench`) appears on the user's desktop. Check first that the desktop is free
  (`nvidia-smi`, `top`; `pgrep -f` can match your own command line), and when the user is busy,
  hold all such runs until they say go. A benchmark window that opens on the 1080p screen is
  given 1920×1052 and rejected; the runner retries twice per case. The person's real mouse over
  the window can disturb scripted input.
- **Closing a client window like a person's click** (an orderly close that saves): load a
  transient KWin script calling `closeWindow()` on the window whose `pid` matches, through
  `qdbus org.kde.KWin /Scripting org.kde.kwin.Scripting.loadScript FILE NAME`, `.start`, then
  `.unloadScript NAME`, with the pid of the real process.
- **Machine-local material** (not tracked; `cargo clean` would delete it): every 07 report under
  `target/evidence/07*`, the baseline's raw samples under `target/evidence/baseline-*`, and 07's
  helper scripts under `target/drafts/07/` (`analyze.py` and `tables.py` summarize a benchmark
  report against the baseline, `packaged.sh` repeats the packaged fresh start, `diag.sh` and
  `summ.py` run and summarize one diagnostic benchmark run); the pacing rule's analysis under
  `target/drafts/pacing/` (`margins.py` evaluates rule variants over every kept benchmark run);
  the loop's under `target/drafts/loop/` (`ab2.sh` runs one short presentation run with a given
  driver binary, `compare.py` sets a profile's report beside 07's, and `native-client-before` is
  `8eb3179`'s driver, built in its own checkout; never build another checkout into this `target/`).
- `world_data/save.redb` at the repository root is a generated save the user called unimportant;
  experiments use temporary stores.
