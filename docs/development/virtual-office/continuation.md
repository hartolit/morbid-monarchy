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
  [whiteboard topic](whiteboard/discussions/command-journal.md)); `b7d86bb` refuses damaged
  worlds before opening them for writing (its version-1 fixture was removed again: no save
  compatibility before release, the user's decision); `21674ad` keeps the
  world snapshot's device image between saves; `2932764` autosaves every 60 ticks by default and
  never off, with the `bounded-loss` case. Measurements in the
  [budgets](../../reference/performance-budgets.md#checkpoint-cadence-after-07), reports under
  `target/evidence/journal-*`, drafts and helpers (`cost.py`, `table.py` summarize a cadence
  case's result) under `target/drafts/journal/`.
- **Then the capacity item, at the user's choice (in part):** `6671c7f` dropped the version-1
  compatibility guard (saved worlds before release are test data; the user's three worlds went
  to the trash, not deleted); `10cf62f` lets dry foliage sleep (physics revision 2, `cells`
  `dry-foliage-sleeps`, the user's approval); `63f8ce6` and `d4d2d6c` cache the landscape's
  lighting per chunk vertex (`render` `lighting-cache-matches`, `frame-cost` at radius 32) and
  `bodies` `capacity-tick-phases` times radius 32's tick by phase. `check all` (160 of 160)
  ran on `d4d2d6c`, the capacity profile (5 runs a case, none rejected) and the three scripts in
  the window on `76376d8`'s tree. Then `67edbc2` moves the window in place (`check all` 162 of 162),
  `565366e` adds the allocator's account to bench summaries, and all three profiles and the scripts
  ran in the window on `6e3f2d9`. Measurements in the
  [budgets](../../reference/performance-budgets.md#capacity-after-07), drafts and the recorded
  before/after case results under `target/drafts/capacity/`. Then `7f9d267` lists each body's
  pair candidates once a substep: the body pass at 4,096 dense bodies fell from 5.0–5.1 to 2.3 ms,
  the bodies bit for bit as before (`check all` 162 of 162; [the dense body
  pass](../../reference/performance-budgets.md#the-dense-body-pass); drafts, the per-kernel
  timestamp patch and the run records under `target/drafts/dense/`).
- **Then the swapchain acquisition item, at the user's choice:** `4485dad` moves the workspace to
  wgpu 30.0.1, naga 30.0.1 and egui 0.37.0 (wgpu-hal 30 passes the acquisition no fence outside
  Windows); a mapped range wgpu refuses is now a recorded fault (`transfers`
  `refused-mapped-range`). `check all` 163 of 163 on `4485dad`; the 15 s radius-6 experiment, all
  three profiles, the scripts and the native connected scenario (two of three runs; one crossed
  step 6's rebase margin, a flaw of the check fixed in `b145fc9`) ran in the window on it
  ([budgets](../../reference/performance-budgets.md#the-three-profiles-on-wgpu-30)): no frame
  waits for a swapchain image, the worst radius-6 p99 is 2.03 ms, and the client spends 0.05–0.15
  ms more main-thread CPU per frame. Drafts, the table and A/B helpers (`tables.py`, `ab.sh`) under
  `target/drafts/wgpu30/`, reports under `target/evidence/wgpu30-*`.
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

1. **The 10.09 ms swapchain acquisition waits — done.** They were a 10 ms sleep in the NVIDIA
   driver under the fence wgpu-hal 29.0.4 waited on after each `vkAcquireNextImageKHR`; the user
   chose the upgrade to wgpu 30 (`4485dad`), and since then no frame of any profile waits
   ([budgets](../../reference/performance-budgets.md#swapchain-acquisition-after-07)). The 0.05–0.15
   ms of main-thread CPU it costs per frame is the NVIDIA driver taking longer for the same calls
   (command buffer and render pass begins in the frame's `finish`), not egui and not the newly
   enabled `VK_EXT_image_drm_format_modifier` (a draft without it cost the same); which wgpu-hal 30
   change causes it is open and worth little
   ([budgets](../../reference/performance-budgets.md#the-three-profiles-on-wgpu-30); the profiler,
   counters and drafts under `target/drafts/wgpu30/`). Step 6's rebase check, which a sphere crossed once in
   three native runs, is fixed in `b145fc9`: the sphere rocked in a three-tick cycle the 30-frame
   stillness test missed; each window move is now judged by the tick that carries it and the
   median still sphere after it (20 of 20 headless runs, `check all` 163 of 163, draft origin and
   resume faults failing it; drafts under `target/drafts/rebase-check/`), and with the native
   consumer three of three runs on `1c900d0`. The sampler, the drafts and the earlier clients under
   `target/drafts/acquire/`, runs under `target/evidence/acquire/`.
2. **Capacity limits, the frame and tick costs done:** dry foliage sleeps (`10cf62f`, physics
   revision 2), the landscape's lighting is cached per chunk vertex (`63f8ce6`, `d4d2d6c`) and a
   window move happens in place without waking the window (`67edbc2`, the user's choice). 07's
   protocol on `6e3f2d9`
   ([budgets](../../reference/performance-budgets.md#the-three-profiles-after-the-window-move)):
   frame medians 0.64–0.80 ms at radius 6, 1.64–1.89 ms at radius 16 and 4.99–11.72 ms at radius 32,
   the dense radius-32 runs at 60 ticks/s with no tick discarded and the longest capacity frame 23.8
   ms. The body pass at 4,096 dense bodies then fell from 5.0–5.1 to 2.3 ms with the same results
   (`7f9d267`), and the dense window cases' frame medians from 10.6–11.7 to 7.0–7.3 ms (on
   `920d097`, `target/evidence/dense-bench-capacity/`). What remains of it is the solver's 64
   passes (about 0.93 ms) and the terrain contact (about 1.2 ms, mostly the bodies that reach the
   ground). A draft that over-relaxes the position passes for bodies with two or more corrections
   (factor 1.6) left 3.6 deep pairs in `dense-cluster` against 13 and the dense 4,096 body pass at
   2.79 ms against 3.80, all `bodies` cases passing; it is a physics change awaiting the user's
   decision ([dense-pile discussion](whiteboard/discussions/dense-pile-convergence.md), draft
   under `target/drafts/dense2/`); since `4485dad` (wgpu 30) no frame waits for a
   swapchain image (item 1). Benchmark runs need
   the pointer on a 2,560 × 1,440 display and the screen lock off: KWin opens the window on the
   active display, and a run on `6e3f2d9` had single frames of up to 478 ms that twelve runs without
   the lock did not. No frame rate is promised at radius 32.
3. **Unqualified:** every other adapter, driver, compositor and platform.
4. **After the autosave cadence:** the `loss` and `worlds` scripts passed again on `dfeb530`'s
   code and the frozen `coupled` scenarios ran in the window with `--autosave 60` at the frame
   times of autosave off ([budgets](../../reference/performance-budgets.md#checkpoint-cadence-after-07));
   the scripts last ran on `4485dad`'s tree, and the frozen benchmark protocol still saves never. A
   command journal for a loss below the cadence is parked on the whiteboard with what it needs
   first.

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
