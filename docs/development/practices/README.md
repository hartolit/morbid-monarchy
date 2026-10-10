# Development practices

Read the practice that owns the changed risk, not an arbitrary full-document checklist.
[Workflow](../workflow.md) owns assignment procedure; [architecture](../../architecture.md) owns
project responsibilities. Practices explain how to implement and review those responsibilities.

| Practice | Apply when |
| --- | --- |
| [Implementation](implementation.md) | Changing Rust interfaces, ownership, module/package structure, configuration or lifecycle. |
| [Documentation](documentation.md) | Writing prose, comments, package introductions, examples or handoffs. |
| [Performance](performance.md) | Changing recurring work, allocations, data movement, resource use or performance claims. |
| [GPU programming](gpu-programming.md) | Changing layouts, kernels, dispatches, renderer sharing, readbacks or GPU resource lifetimes. |
| [Simulation correctness](simulation-correctness.md) | Changing material rules, time integration, contacts, randomness or sparse scheduling. |

A shader optimization usually needs the last three plus implementation. A spelling correction does
not. Selected practices do not expand assignment scope or authorize a product change.
