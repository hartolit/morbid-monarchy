# 0002 — Native composition and meaningful boundaries

**Date:** 2026-10-10. **State:** the native application and standalone `bevy_ecs` are the
user-requested direction, adopted in prompt 00. The concrete package map is still a proposal,
which 01 checks against actual consumers.

## Decision

Retain `bevy_ecs` for CPU orchestration but replace the Bevy engine application/rendering path with
a native application, shared wgpu execution context and renderer. Separate semantic contracts,
runtime policy, GPU execution, persistence mechanism, rendering and application composition where
those boundaries enforce actual ownership or dependency isolation.

Use private modules for material families, body algorithms and pass internals. Do not retain the
old “one crate per distinct feature” rule. Do not make a platform-neutral wrapper around every GPU
handle. GPU execution and rendering can intentionally share GPU-specific types while runtime
intent/persistence contracts remain independent of them.

## Alternatives

Bevy can host GPU compute and a GPU-authoritative simulation; removing it is not a prerequisite
for residency. Retaining its rendering could reduce replacement work. The requested native target
instead favors direct resource control and a smaller application-specific composition. Preserve
useful shaders/math and feature coverage rather than reimplementing everything without inspection.

A single monolithic GPU engine would avoid interface work but combine physical policy, storage,
windowing and rendering. Conversely, a crate per kernel adds API and build friction without useful
independence. The proposed middle boundary map is checked against actual consumers in 01.

## Transition and reversal trigger

Keep the old app only as a temporary baseline until native cutover in 06. Headless GPU tests use
the actual new physical implementation. Remove legacy production flags, engine imports and duplicate
paths after cutover; Git/artifacts retain the baseline. A small CPU reference remains a development
consumer, not an app.

If a proposed package has no actual boundary/consumer, use a module before creating it. A dependency
cycle, duplicate wgpu device family or interface forcing world copies is evidence that the chosen
boundary needs correction.
