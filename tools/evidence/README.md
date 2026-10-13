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
| `test gpu --suite NAME --adapter ID` | `transfers` runs every case of the suite in its own `gpu-suite` process on the named adapter and checks the cases against the registry. Other suites fail explicitly until their stage registers them. |
| `run --scenario NAME --adapter ID` | Fails explicitly until `resident-world` exists. |
| `bench --profile NAME --adapter ID` | Repeated release measurements of the frozen [scenarios](scenarios/). |
| `check all --adapter ID` | Every lane above, without skipping. |

The command contracts are owned by [workflow](../../docs/development/workflow.md), and what each
lane proves is owned by [verification evidence](../../docs/development/verification-evidence.md).

## Files

- `coverage.toml` gives every test in a covered package a stable behavior ID, an expectation and a
  destination. Renaming a test changes its `test` field, not its ID. The 28 baseline tests are
  counted and must all remain. GPU suite cases are registered with `target = "gpu:<suite>"`;
  `test gpu` requires every defined case to be registered and every registered case to exist.
- `lint-baseline.toml` lists the legacy warnings that remain, each with an owner and a removal
  stage. A new warning fails the lane. A fixed warning whose exception is still listed also fails
  it.
- `scenarios/<profile>/<case>.toml` are the frozen measurement workloads. Validation rejects a file
  whose world size, resolution, tick rate or body count differs from its profile.
- `baselines/legacy-visual/` holds the visual reference: frames the driver saved with
  `--screenshot` from the frozen qualification scenarios, as JPEG copies with the PNG hashes.
- `baselines/legacy-6e108d6/` holds the measured legacy baseline: each bench `report.json` with
  GPU process arguments removed and paths made relative, plus the SHA-256 of every raw file the
  run left in `target/evidence/`. The raw samples stay on the measuring machine. The conclusions
  are in [performance budgets](../../docs/reference/performance-budgets.md#measured-legacy-baseline-01).

## Baseline driver

`src/bin/legacy-baseline.rs` (feature `legacy-baseline`) drives the Bevy baseline application
through `morbid_app::build_app` in scripted input mode. For every simulation tick, the scenario
turns its actions into `world_model` commands; the driver executes them through the same tool
functions the mouse uses, writes per-frame samples, and summarizes the driven body's
`BodyObservation`s. `bench` builds
it on demand and gives each run a new directory, so the user's `world_data/` is never opened. The
driver, and the runner's dependency on the old application, go away with the Bevy application in
prompt 06. Default builds of the runner do not compile Bevy; `check static` enforces that.

## GPU suites

`src/suites/` holds the GPU suites; `src/bin/gpu-suite.rs` runs one case per process, so a lost
device, crash or hang affects one case, which is killed after three minutes. Cases drive
`gpu_world::world::GpuWorld` and `world_runtime::runtime::Runtime` through their public
interfaces, as the application will. Layout probes (`suites/probe.rs`) assemble small kernels
from `gpu-world`'s exported declarations and run them on the session's device through its
timeline. The binary installs a counting allocator so steady-state cases can report
allocations. Each case writes a JSON result with failures, notes and metrics; the runner keeps
them in `gpu-<suite>/` of its run directory and the metrics in `report.json`.

## Adding a lane

Register a GPU suite in `suites.rs` only when it executes real kernels, and a scenario only when it
runs through the production runtime. Add its tests under `tests/`, update the command table in
the workflow, and make it fail or report unavailable when it cannot execute.
