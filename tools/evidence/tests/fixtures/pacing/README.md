# Recorded frame times for the pacing rule

Real benchmark runs that decide whether `measurement::presentation_pacing` can tell a throttled
window from a saturated one. Each file holds one run's measured frames, in order: the `frame_ms`
column of the rows whose phase is `measured`, copied verbatim from that run's `samples.csv`, one
value per line. That is the whole input of the rule. Treat these files as immutable; a new
recording is a new fixture.

| File | Run | Recorded | Verdict the rule must give |
| --- | --- | --- | --- |
| `capacity-coupled-run-1.txt` | `cargo evidence bench --profile capacity --runs 5` on `eb9d8fb` (clean), case `coupled`, attempt `run-1` | 2026-10-21, native client | Not paced: 8 frames within 0.15 ms of 16.67 ms, none next to another, 0 beside |
| `capacity-coupled-run-6.txt` | the same invocation, attempt `run-6` | 2026-10-21, native client | Not paced: 23 such frames, none next to another, 2 beside |
| `capacity-coupled-run-7.txt` | the same invocation, attempt `run-7` | 2026-10-21, native client | Not paced: 46 such frames, none next to another, 9 beside |
| `baseline-body-contact-run-3.txt` | `cargo evidence bench --profile qualification` on `79e7ead` (clean), case `body-contact`, attempt `run-3` | 2026-10-10, Bevy baseline | Paced at 16.67 ms: 131 frames on it, 128 of them in stretches, longest stretch 43 |
| `baseline-flow-run-4.txt` | the same invocation, case `flow`, attempt `run-4` | 2026-10-10, Bevy baseline | Paced at 16.67 ms: 58 frames on it in one stretch, 1.5% of the run |

The capacity attempts were rejected by the rule as it stood at `eb9d8fb`. With the device
saturated, their frames alternate between a long frame that runs no tick and a short catch-up frame,
usually of three ticks, that ends near the 60 Hz tick cadence, never two short ones in a row. In the
baseline runs KWin throttled the Bevy application's window to 60 Hz, the diagnosis `79e7ead`
recorded when it introduced the rule: frames held at 16.67 ms while the main thread was mostly
idle. Of the 14 throttled baseline runs, these two give the rule the least evidence: body-contact
run 3 has the shortest unbroken stretch, and flow run 4 has the smallest share of frames on the
interval. The
[regression policy](../../../../../docs/reference/performance-budgets.md#regression-and-improvement-policy)
records the decision these runs support.

## Provenance

The source runs are kept on the nominated machine only, under
`target/evidence/07-bench-capacity/coupled/` and
`target/evidence/rejected-baseline-qualification-79e7ead/` (the second report was written to
`target/evidence/baseline-qualification` and renamed when the baseline was measured again).

| File | SHA-256 of the source `samples.csv` | SHA-256 of the fixture |
| --- | --- | --- |
| `capacity-coupled-run-1.txt` | `1eadbb9447dc5d218d0bd8b0d3eb1f42907d8776dab8a2e0eb8b3ba4b3f51bf5` | `54ae111282a50f30915a4dafe91b6257f6fe45a8a6650b6f38215895dddd16f3` |
| `capacity-coupled-run-6.txt` | `828f835a4ebba77e47f8657f49472a9876f6eda0d9d262445f42cb11f9b06761` | `6cfe31b08ee7e8a7b62579d237da002ada5d97b129687c967fea9bd435bd693e` |
| `capacity-coupled-run-7.txt` | `7ee245c15f27ac29dd36caf53bc09cb78ecd51021bc666c3af0955e9ce9f36c2` | `759c65dd04959cbe00a2f8ce90180b656494f84e91dd0176e35e4f0e7ab6d322` |
| `baseline-body-contact-run-3.txt` | `e11f2ccb08150a3529f244ec0bbc8c4c755dbd96d67d68c01ed5392647e9e172` | `45495fa328bb81dc5ef27be8e16390294a02d81968f371a34a594007328da2cb` |
| `baseline-flow-run-4.txt` | `646054692478c4c86970e20cba5a0aa51dea75e04b535a0da82d141b7b4954df` | `fe302f71af87612215faa394dae4515560309018fbcb18c0f20eff2e7fe3a734` |
