# monarchy-evidence

The repository's evidence runner, invoked as `cargo evidence` (an alias for
`cargo run --locked --release -p monarchy-evidence --`). Every command writes a `report.json` and
the logs of its nested commands to `target/evidence/<time>-<command>/`, or to `--output DIR`. It
exits 0 only when every required item passed. Unavailable hardware, a missing tool or an empty
test selection is never reported as success.

| Command | What it establishes |
| --- | --- |
| `check docs` | Local Markdown links and anchors, the owner index, sprint structure, retired rule files, a README for every package and its name in the architecture. |
| `check static` | rustfmt; compilation of the declared feature combinations; Clippy against [`lint-baseline.toml`](lint-baseline.toml); rustdoc with warnings denied; naga validation of every assembled `gpu-world` shader; `cargo deny` with [`deny.toml`](../../deny.toml) and `cargo machete` at their required versions; dependency direction (each package's own resolved closure); toolchain/lock/alias pins; the CI workflow's lanes and checker versions. |
| `test cpu` | Builds and runs every test executable, then checks results against [`coverage.toml`](coverage.toml). |
| `inspect-adapter` | Lists every adapter on every backend with the stable `--adapter` identifier `gpu-world` assigns, limits and features. |
| `test gpu --suite NAME --adapter ID` | `transfers` (02), `cells` (03), `bodies` (04), `streaming` (05) and `render` (06) run every case of the suite in its own `gpu-suite` process on the named adapter and check the cases against the registry. |
| `run --scenario NAME --adapter ID [--consumer native]` | `resident-world` runs in its own `gpu-suite` process through the runtime's session, the live world and a temporary store, one item per step; steps 1–3 exist since 04 and 4–5 since 05. Step 6 runs 05's part and stays failed until 07 completes it. `--consumer native` shows every session in the native client's window (`morbid_app::viewer`). |
| `session --store PATH --adapter ID` | Opens a saved world (`world-store`) headless in its own `gpu-suite` process through `world_runtime::session::WorldSession` and the live world, runs it (`--ticks`, a new window with `--center`/`--radius`, `--edit TOOL,X,Y`, `--autosave`), and closes it in order; one item each for opening, running and closing, which passes only when the close saved the last completed tick. Running it again reopens what the close saved. |
| `bench --profile NAME --adapter ID` | Repeated release measurements of the frozen [scenarios](scenarios/) in the native client's window, one sample per presented frame. |
| `check all --adapter ID` | Every lane above, without skipping. |

The command contracts are owned by [workflow](../../docs/development/workflow.md), and what each
lane proves is owned by [verification evidence](../../docs/development/verification-evidence.md).

## Files

- `coverage.toml` gives every test in a covered package a stable behavior ID, an expectation and a
  destination. Renaming a test changes its `test` field, not its ID. The 28 baseline tests are
  counted and must all remain. GPU suite cases are registered with `target = "gpu:<suite>"`;
  `test gpu` requires every defined case to be registered and every registered case to exist.
- `lint-baseline.toml` listed the legacy warnings, each with an owner and a removal stage; it is
  empty since 06 removed the Bevy baseline. A new warning fails the lane.
- `scenarios/<profile>/<case>.toml` are the frozen measurement workloads. Validation rejects a file
  whose world size, resolution, tick rate or body count differs from its profile.
- `baselines/legacy-visual/` holds the visual reference: frames the removed baseline driver saved
  from the frozen qualification scenarios, as JPEG copies with the PNG hashes.
- `baselines/legacy-6e108d6/` holds the measured legacy baseline: each bench `report.json` with
  GPU process arguments removed and paths made relative, plus the SHA-256 of every raw file the
  run left in `target/evidence/`. The raw samples stay on the measuring machine. The conclusions
  are in [performance budgets](../../docs/reference/performance-budgets.md#measured-legacy-baseline-01).
- `tests/fixtures/pacing/` holds the measured frame times of five recorded benchmark runs that
  the presentation-pacing rule (`measurement::presentation_pacing`) must reject as throttled or
  accept as paced by work; its README gives their provenance.

## Native client driver

`src/bin/native-client.rs` runs the native client (`morbid-app`) in its real window under a
script (`morbid_app::driver::Driver`), with the same composition, event handling, renderer and
overlay a person uses; `src/native.rs` holds the scripts and `src/native/bench.rs` the
measurement driver. `capture` saves frames; `script features|loss|worlds` exercise the feature
inventory, device loss and recovery, and the start screen, writing `report.json`; `bench` runs
one frozen scenario on a new world made to it: the scenario's commands go through the client's
command path for their exact ticks, the observer follows the camera path by tick, and every
presented frame is sampled. `cargo evidence bench` builds it and gives each run a new
directory, so no person's worlds are opened. The commands are in the
[workflow](../../docs/development/workflow.md#scripted-runs-of-the-native-window--06).

## GPU suites

`src/suites/` holds the GPU suites and the connected scenarios (`scenarios.rs`);
`src/bin/gpu-suite.rs` runs one case (or one scenario, or one headless session from
`suites/session.rs`) per process, so a lost device, crash or
hang affects one case, which is killed after three minutes (a scenario after five). Cases drive
`gpu_world::world::GpuWorld` and `world_runtime::runtime::Runtime` through their public
interfaces, as the application will. Layout probes (`suites/probe.rs`) assemble small kernels
from `gpu-world`'s exported declarations and run them on the session's device through its
timeline. The binary installs a counting allocator so steady-state cases can report
allocations. Each case writes a JSON result with failures, notes and metrics; the runner keeps
them in `gpu-<suite>/` of its run directory and the metrics in `report.json`. The `streaming`
cases (`suites/streaming/`) create real redb stores in temporary directories (the cases that
measure storage, `checkpoint-cadence-*` and `bounded-loss`, under `target/streaming-scratch/`,
on the workspace's disk rather than a `tmpfs` where a sync costs nothing), inject I/O failures
through `world_store::faults`, and re-run the binary as a child process to interrupt a live
session at a store barrier; none opens `world_data/`.

## Adding a lane

Register a GPU suite in `suites.rs` only when it executes real kernels, and a scenario only when it
runs through the production runtime. Add its tests under `tests/`, update the command table in
the workflow, and make it fail or report unavailable when it cannot execute.
