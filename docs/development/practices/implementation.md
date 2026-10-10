# Implementation practice

## Work on the design that remains

State the intended rule, owner and supported operation before choosing a mechanism. Search source,
callers, configuration and tests for equivalent logic. Implement the simplest complete design,
migrate its consumers, and remove competing paths. Do not optimize for the smallest diff, a new
abstraction, or completion of a file list.

A concept needs one semantic owner, canonical representation and normal operation path. Derived
render data, immutable snapshots and independent test references have genuinely different roles;
name and test them rather than treating all duplication as forbidden. Two writable live worlds
are not such an exception.

## Compose boundaries that earn their cost

A crate must enforce real semantic ownership, a durable contract, a consumer-owned adapter port,
heavy/platform dependency isolation, or an actual independently usable consumer. Otherwise use a
module. A material, shader pass or UI panel does not automatically deserve a package. Remove
ownerless `common`/`utils` collections and forwarding-only layers.

High-level policy owns the required interface. Concrete adapters implement it and are selected by
the composition root. An adopted adapter is not complete until all applicable production callers
use it and local bypasses are removed. Temporary migration paths need a named purpose and removal
stage; no speculative fallback survives cutover.

Data layout is not dictated by crate boundaries or ECS. Store CPU orchestration data in ECS where
that helps its access pattern; store GPU arrays in the GPU owner. Choose array-of-structures,
structure-of-arrays or packed words from actual access and measured cost. A preallocated `Vec`
is a valid pool; using it does not imply allocating per tick.

## Keep interfaces narrow and meaningful

Use private fields, validated constructors, enums and typed IDs. Separate raw input, validated
configuration, live resources and observations. A caller must not need a later `validate()` to
make an internal value usable. Failure types distinguish invalid input, unsupported capability,
capacity pressure, stale identity, corrupt data, failed I/O and lost device.

Functions are the first abstraction. Use closures for local variation, enums for closed choices,
and consumer-owned traits for genuinely replaceable boundaries. Dynamic dispatch belongs at a
batch or service boundary, not inside a cell/body loop without evidence. Do not make internals
public solely to simplify tests.

A GPU buffer view can expose wgpu types because the mechanism is the subject of that API. A saved
chunk, gameplay command or residency decision cannot leak a buffer handle. This distinction is
more useful than wrapping every library type in a home-grown equivalent.

## Rust quality and lifecycle

Give every task, queue, pool, submission, mapped range, snapshot and store one lifecycle owner.
Prefer immutable/unique ownership, then bounded messages, then a lock around one coherent invariant.
Do not default to `Arc<Mutex<_>>`, spin locks or lock-free algorithms. Share immutable GPU context
handles where required; do not clone them inside hot loops merely for convenience.

A task handle being dropped is not evidence that work was cancelled or joined. Stop/drain paths
must be explicit, tested and bounded. Do not hold a lock across an external callback, GPU wait or
I/O operation. CPU atomics need a documented publication/ordering argument when they synchronize
other memory; independent statistics can use relaxed ordering. CPU atomics do not establish GPU
execution order.

Product code must not panic on user input, adapter limits, missing assets, corrupt saves or a
recoverable device/surface failure. `unwrap`/`expect` require a local proven invariant and a reason
why a typed failure is not useful. Tests may use them for setup and assertions. Avoid blanket lint
exceptions, swallowed `Result`s, unbounded retries and detached shutdown work.

Safe Rust is the default. Do not introduce `unsafe` to bypass profiling or API design. Any required
unsafe boundary needs a small implementation, a specific safety proof, tests of its assumptions,
and review of aliasing, alignment, initialization and lifetime. A `Pod` derive alone does not prove
Rust/WGSL layout compatibility; see the GPU practice.

## Configuration, units and bounds

Semantic invariants, hardware limits, supported product limits, defaults and example settings are
different. Name the source of a bound. Validate the combined effect before consuming resources,
including temporary old/new allocations during resize and all queues sharing a budget.

Parse human-authored TOML once into validated owner-specific plans. Preserve defaults in one owner.
Do not invent configuration switches for every literal or make correctness optional. Material
scales, tick rates and solver thresholds carry units and valid ranges. Example capacities are not
universal physical constants.

Reject unknown settings and non-finite numeric inputs. Use checked size/index arithmetic. A requested
radius, body count or feature that cannot be supported is refused with a reason; it is not silently
clamped while reports retain the requested value.

## Hot paths and structure

Recurring work must follow the [performance practice](performance.md). Reuse storage after warm-up;
move formatting, diagnostics and asset work out of tight loops. Do not equate fewer Rust lines,
more iterators, `#[inline(always)]`, `const fn` or SoA with an established speedup. A `const fn`
permits compile-time evaluation; it does not guarantee every call is precomputed.

Organize modules by cohesive algorithms, state machines or interfaces. A large solver may require
splitting contact generation, response and deformation, not arbitrary `part1` files. Review a
production Rust/WGSL module around 700 implementation lines or when comprehension already suffers;
this is a review trigger, not a mandate to hide logic behind includes or delete explanation.
Tests may be substantial; give their fixtures and scenarios their own meaningful structure.

Public package READMEs and library rustdoc explain the supported operation. Comments record why a
layout, order, threshold or capacity rule is necessary. Do not narrate syntax or store changelogs
inside code.

## Adoption and compatibility

Migrate producers, consumers, construction, configuration, errors, tests, assets and docs in the
same responsibility. Remove obsolete aliases, wrappers, imports and fallback branches. Preserve
supported on-disk data through a tested import/migration or an explicit non-destructive refusal.
Internal development history does not create perpetual API compatibility.

The supplied saved-world format has no adequate version envelope. Do not open a user's only save
as a migration experiment. Preserve original bytes and validate the complete replacement before
publishing it; the world-format contract owns this lifecycle.

## Enforce the rules

Add repository checks for dependency direction, forbidden Bevy engine paths after cutover,
reference-code isolation, shader assembly/layout validation, lock/toolchain pins and documentation
links. Check selected default/feature combinations rather than assuming `--all-features` represents
every supported composition. Source-shape checks cannot prove physical correctness or resource
lifetime; execute those through production boundaries.

A change is done when one coherent implementation remains and the promised supported operation
works, including failure. A green compiler is necessary, not sufficient.
