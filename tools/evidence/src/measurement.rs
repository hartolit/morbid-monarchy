//! Per-frame samples shared by the native bench driver (writer) and the bench runner (reader),
//! and the statistics computed from them.
//!
//! Counter columns hold the work done during that frame (deltas), not running totals. Optional
//! columns are sampled periodically and empty otherwise, because reading them has a cost.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    Warmup,
    Measured,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameSample {
    /// Frames since the timeline started (the initial window became resident).
    pub frame: u64,
    pub measured: bool,
    /// Wall time from this frame's start to the next frame's start.
    pub frame_ms: f64,
    /// The client's frame on the CPU, from the frame's start to the end of its submission.
    pub main_cpu_us: f64,
    /// Simulation ticks completed during the frame, and the logical tick after it.
    pub ticks: u64,
    pub tick: u64,
    pub simulation_us: f64,
    pub discarded_simulation_us: f64,
    pub full_copy_bytes: u64,
    pub wake_clear_bytes: u64,
    pub body_us: f64,
    pub body_substeps: u64,
    pub pair_tests: u64,
    pub pair_contacts: u64,
    pub bodies: u64,
    pub render_sync_us: f64,
    pub grid_uploads: u64,
    pub grid_upload_bytes: u64,
    pub material_writes: u64,
    pub active_chunks: u64,
    pub cached_chunks: u64,
    pub pending_loads: u64,
    pub save_queue_chunks: u64,
    pub load_tasks: u64,
    pub save_tasks: u64,
    pub load_requests: u64,
    pub loads_completed: u64,
    pub loads_generated: u64,
    pub loads_from_store: u64,
    pub unload_events: u64,
    pub save_batches: u64,
    pub chunks_written: u64,
    pub bytes_encoded: u64,
    pub write_failures: u64,
    pub retained_bytes: u64,
    pub rss_bytes: Option<u64>,
    pub awake_cells: Option<u64>,
}

pub const CSV_HEADER: &str = "frame,phase,frame_ms,main_cpu_us,ticks,tick,simulation_us,discarded_simulation_us,full_copy_bytes,wake_clear_bytes,body_us,body_substeps,pair_tests,pair_contacts,bodies,render_sync_us,grid_uploads,grid_upload_bytes,material_writes,active_chunks,cached_chunks,pending_loads,save_queue_chunks,load_tasks,save_tasks,load_requests,loads_completed,loads_generated,loads_from_store,unload_events,save_batches,chunks_written,bytes_encoded,write_failures,retained_bytes,rss_bytes,awake_cells";

impl FrameSample {
    pub fn to_csv(&self) -> String {
        let optional = |value: Option<u64>| value.map(|v| v.to_string()).unwrap_or_default();
        format!(
            "{},{},{:.4},{:.1},{},{},{:.1},{:.1},{},{},{:.1},{},{},{},{},{:.1},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            self.frame,
            if self.measured { "measured" } else { "warmup" },
            self.frame_ms,
            self.main_cpu_us,
            self.ticks,
            self.tick,
            self.simulation_us,
            self.discarded_simulation_us,
            self.full_copy_bytes,
            self.wake_clear_bytes,
            self.body_us,
            self.body_substeps,
            self.pair_tests,
            self.pair_contacts,
            self.bodies,
            self.render_sync_us,
            self.grid_uploads,
            self.grid_upload_bytes,
            self.material_writes,
            self.active_chunks,
            self.cached_chunks,
            self.pending_loads,
            self.save_queue_chunks,
            self.load_tasks,
            self.save_tasks,
            self.load_requests,
            self.loads_completed,
            self.loads_generated,
            self.loads_from_store,
            self.unload_events,
            self.save_batches,
            self.chunks_written,
            self.bytes_encoded,
            self.write_failures,
            self.retained_bytes,
            optional(self.rss_bytes),
            optional(self.awake_cells),
        )
    }

    pub fn from_csv(line: &str) -> Result<Self, String> {
        let fields: Vec<&str> = line.split(',').collect();
        let expected = CSV_HEADER.split(',').count();
        if fields.len() != expected {
            return Err(format!("{} fields, expected {expected}", fields.len()));
        }
        let mut index = 0;
        let mut next = || {
            let field = fields[index];
            index += 1;
            field
        };
        let int = |text: &str| text.parse::<u64>().map_err(|e| format!("`{text}`: {e}"));
        let float = |text: &str| text.parse::<f64>().map_err(|e| format!("`{text}`: {e}"));
        let optional = |text: &str| {
            if text.is_empty() {
                Ok(None)
            } else {
                int(text).map(Some)
            }
        };
        Ok(Self {
            frame: int(next())?,
            measured: match next() {
                "measured" => true,
                "warmup" => false,
                other => return Err(format!("unknown phase `{other}`")),
            },
            frame_ms: float(next())?,
            main_cpu_us: float(next())?,
            ticks: int(next())?,
            tick: int(next())?,
            simulation_us: float(next())?,
            discarded_simulation_us: float(next())?,
            full_copy_bytes: int(next())?,
            wake_clear_bytes: int(next())?,
            body_us: float(next())?,
            body_substeps: int(next())?,
            pair_tests: int(next())?,
            pair_contacts: int(next())?,
            bodies: int(next())?,
            render_sync_us: float(next())?,
            grid_uploads: int(next())?,
            grid_upload_bytes: int(next())?,
            material_writes: int(next())?,
            active_chunks: int(next())?,
            cached_chunks: int(next())?,
            pending_loads: int(next())?,
            save_queue_chunks: int(next())?,
            load_tasks: int(next())?,
            save_tasks: int(next())?,
            load_requests: int(next())?,
            loads_completed: int(next())?,
            loads_generated: int(next())?,
            loads_from_store: int(next())?,
            unload_events: int(next())?,
            save_batches: int(next())?,
            chunks_written: int(next())?,
            bytes_encoded: int(next())?,
            write_failures: int(next())?,
            retained_bytes: int(next())?,
            rss_bytes: optional(next())?,
            awake_cells: optional(next())?,
        })
    }
}

/// Order statistics of one quantity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Distribution {
    pub count: usize,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
}

/// Nearest-rank percentile of sorted values: the smallest value with at least `percent`% of
/// samples at or below it. Empty input yields zero.
pub fn percentile(sorted: &[f64], percent: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = ((percent / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn distribution(values: impl IntoIterator<Item = f64>) -> Distribution {
    let mut sorted: Vec<f64> = values.into_iter().filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return Distribution::default();
    }
    sorted.sort_by(f64::total_cmp);
    Distribution {
        count: sorted.len(),
        mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
        median: percentile(&sorted, 50.0),
        p95: percentile(&sorted, 95.0),
        p99: percentile(&sorted, 99.0),
        min: sorted[0],
        max: sorted[sorted.len() - 1],
    }
}

/// Display refresh intervals a compositor may pace a window to, in milliseconds.
const REFRESH_INTERVALS_MS: [f64; 5] = [
    1000.0 / 60.0,
    1000.0 / 120.0,
    1000.0 / 144.0,
    1000.0 / 165.0,
    1000.0 / 240.0,
];
/// Half-width of the band around a refresh interval, in milliseconds.
const PACING_BAND_MS: f64 = 0.15;
/// Unbroken frames on one interval that cannot be coincidence.
const PACED_STRETCH_FRAMES: usize = 30;
/// Consecutive frames on one interval that count toward a spike. A throttle holds the window on
/// the interval from one frame to the next; a single frame there is coincidence.
const SPIKE_STRETCH_FRAMES: usize = 2;
/// A spike's frames must make up this share of all frames and exceed the neighbourhood this
/// many times.
const PACED_SPIKE_SHARE: f64 = 0.01;
const PACED_SPIKE_RATIO: usize = 4;

/// Evidence that presentation pacing (an unfocused-window throttle or the compositor), not the
/// application's work, set the frame times.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pacing {
    pub interval_ms: f64,
    /// Frames within ±0.15 ms of the interval.
    pub frames_on_interval: usize,
    /// Those of them in stretches of at least two consecutive frames on the interval.
    pub frames_in_stretches: usize,
    /// Frames 0.45-0.75 ms either side of it: twice the band's width, so an unpaced, smooth
    /// distribution puts about twice as many frames here as on the interval.
    pub frames_beside_interval: usize,
    pub longest_stretch: usize,
}

/// Detects presentation pacing in measured frames. Uncapped presentation produces a smooth
/// frame-time distribution; a compositor that throttles a covered or unfocused window holds
/// consecutive frames on one refresh interval, in long stretches or in shorter ones that add up
/// to a spike there. Isolated frames that happen to match an interval are expected and do not
/// count: when the device is saturated, the client alternates long frames that run no tick with
/// short catch-up frames that end near the 60 Hz tick cadence, one at a time.
pub fn presentation_pacing(samples: &[FrameSample]) -> Option<Pacing> {
    let frames: Vec<f64> = samples
        .iter()
        .filter(|sample| sample.measured)
        .map(|sample| sample.frame_ms)
        .collect();
    REFRESH_INTERVALS_MS
        .iter()
        .map(|&interval| {
            let distance = |frame: f64| (frame - interval).abs();
            let on_interval = |frame: f64| distance(frame) < PACING_BAND_MS;
            let stretches: Vec<usize> = frames
                .chunk_by(|&a, &b| on_interval(a) == on_interval(b))
                .filter(|run| on_interval(run[0]))
                .map(<[f64]>::len)
                .collect();
            Pacing {
                interval_ms: interval,
                frames_on_interval: stretches.iter().sum(),
                frames_in_stretches: stretches
                    .iter()
                    .filter(|&&stretch| stretch >= SPIKE_STRETCH_FRAMES)
                    .sum(),
                frames_beside_interval: frames
                    .iter()
                    .filter(|&&frame| {
                        (3.0 * PACING_BAND_MS..5.0 * PACING_BAND_MS).contains(&distance(frame))
                    })
                    .count(),
                longest_stretch: stretches.iter().copied().max().unwrap_or(0),
            }
        })
        .find(|pacing| {
            let spike = pacing.frames_in_stretches as f64
                >= PACED_SPIKE_SHARE * frames.len() as f64
                && pacing.frames_in_stretches > PACED_SPIKE_RATIO * pacing.frames_beside_interval;
            pacing.longest_stretch >= PACED_STRETCH_FRAMES || spike
        })
}

/// Measured-window summary of one run.
#[derive(Debug, Clone, Serialize)]
pub struct RunStatistics {
    pub measured_frames: usize,
    pub measured_seconds: f64,
    pub frame_ms: Distribution,
    pub frames_over_16_67_ms: usize,
    /// Set when the compositor, not the work, paced the frames; see [`presentation_pacing`].
    pub presentation_pacing: Option<Pacing>,
    pub frames_over_25_ms: usize,
    pub frames_without_tick: usize,
    pub frames_with_several_ticks: usize,
    pub ticks: u64,
    pub ticks_per_second: f64,
    pub discarded_simulation_ms: f64,
    pub main_cpu_ms: Distribution,
    pub simulation_ms: Distribution,
    pub body_ms: Distribution,
    pub render_sync_ms: Distribution,
    pub full_copy_bytes_per_second: f64,
    pub wake_clear_bytes_per_second: f64,
    pub grid_upload_bytes_per_second: f64,
    pub grid_upload_frames: u64,
    pub material_writes_per_frame: f64,
    pub max_bodies: u64,
    pub max_pair_tests_per_frame: u64,
    pub pair_contacts: u64,
    pub retained_bytes_max: u64,
    pub rss_bytes_max: Option<u64>,
    pub awake_cells: Distribution,
    pub load_requests: u64,
    pub loads_generated: u64,
    pub loads_from_store: u64,
    pub unload_events: u64,
    pub chunks_written: u64,
    pub bytes_encoded: u64,
    pub write_failures: u64,
}

pub fn run_statistics(samples: &[FrameSample]) -> RunStatistics {
    let measured: Vec<&FrameSample> = samples.iter().filter(|sample| sample.measured).collect();
    let seconds: f64 = measured.iter().map(|sample| sample.frame_ms).sum::<f64>() / 1000.0;
    let per_second = |total: u64| {
        if seconds > 0.0 {
            total as f64 / seconds
        } else {
            0.0
        }
    };
    let sum =
        |field: fn(&FrameSample) -> u64| measured.iter().map(|sample| field(sample)).sum::<u64>();
    let ms = |field: fn(&FrameSample) -> f64| {
        distribution(measured.iter().map(|sample| field(sample) / 1000.0))
    };
    let ticks = sum(|s| s.ticks);
    RunStatistics {
        measured_frames: measured.len(),
        measured_seconds: seconds,
        frame_ms: distribution(measured.iter().map(|sample| sample.frame_ms)),
        presentation_pacing: presentation_pacing(samples),
        frames_over_16_67_ms: measured
            .iter()
            .filter(|sample| sample.frame_ms > 1000.0 / 60.0)
            .count(),
        frames_over_25_ms: measured
            .iter()
            .filter(|sample| sample.frame_ms > 25.0)
            .count(),
        frames_without_tick: measured.iter().filter(|sample| sample.ticks == 0).count(),
        frames_with_several_ticks: measured.iter().filter(|sample| sample.ticks > 1).count(),
        ticks,
        ticks_per_second: per_second(ticks),
        discarded_simulation_ms: measured
            .iter()
            .map(|sample| sample.discarded_simulation_us)
            .sum::<f64>()
            / 1000.0,
        main_cpu_ms: ms(|s| s.main_cpu_us),
        simulation_ms: ms(|s| s.simulation_us),
        body_ms: ms(|s| s.body_us),
        render_sync_ms: ms(|s| s.render_sync_us),
        full_copy_bytes_per_second: per_second(sum(|s| s.full_copy_bytes)),
        wake_clear_bytes_per_second: per_second(sum(|s| s.wake_clear_bytes)),
        grid_upload_bytes_per_second: per_second(sum(|s| s.grid_upload_bytes)),
        grid_upload_frames: sum(|s| s.grid_uploads),
        material_writes_per_frame: if measured.is_empty() {
            0.0
        } else {
            sum(|s| s.material_writes) as f64 / measured.len() as f64
        },
        max_bodies: measured
            .iter()
            .map(|sample| sample.bodies)
            .max()
            .unwrap_or(0),
        max_pair_tests_per_frame: measured
            .iter()
            .map(|sample| sample.pair_tests)
            .max()
            .unwrap_or(0),
        pair_contacts: sum(|s| s.pair_contacts),
        retained_bytes_max: measured
            .iter()
            .map(|sample| sample.retained_bytes)
            .max()
            .unwrap_or(0),
        rss_bytes_max: measured.iter().filter_map(|sample| sample.rss_bytes).max(),
        awake_cells: distribution(
            measured
                .iter()
                .filter_map(|sample| sample.awake_cells.map(|v| v as f64)),
        ),
        load_requests: sum(|s| s.load_requests),
        loads_generated: sum(|s| s.loads_generated),
        loads_from_store: sum(|s| s.loads_from_store),
        unload_events: sum(|s| s.unload_events),
        chunks_written: sum(|s| s.chunks_written),
        bytes_encoded: sum(|s| s.bytes_encoded),
        write_failures: sum(|s| s.write_failures),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_round_trips_including_empty_optional_columns() {
        let sample = FrameSample {
            frame: 7,
            measured: true,
            frame_ms: 12.5,
            ticks: 2,
            rss_bytes: Some(42),
            awake_cells: None,
            ..FrameSample::default()
        };
        let parsed = FrameSample::from_csv(&sample.to_csv()).expect("round trip");
        assert_eq!(parsed, sample);
        assert_eq!(
            sample.to_csv().split(',').count(),
            CSV_HEADER.split(',').count()
        );
        assert!(FrameSample::from_csv("1,2,3").is_err());
    }

    #[test]
    fn nearest_rank_percentiles() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&values, 50.0), 50.0);
        assert_eq!(percentile(&values, 95.0), 95.0);
        assert_eq!(percentile(&values, 99.0), 99.0);
        assert_eq!(percentile(&[3.0], 99.0), 3.0);
        assert_eq!(percentile(&[], 50.0), 0.0);
        let spread = distribution([5.0, 1.0, 3.0, f64::NAN]);
        assert_eq!(
            (spread.count, spread.min, spread.median, spread.max),
            (3, 1.0, 3.0, 5.0)
        );
    }

    const TICK_MS: f64 = 1000.0 / 60.0;

    fn measured_frames(times: &[f64]) -> Vec<FrameSample> {
        times
            .iter()
            .map(|&frame_ms| FrameSample {
                measured: true,
                frame_ms,
                ..FrameSample::default()
            })
            .collect()
    }

    #[test]
    fn compositor_pacing_is_told_apart_from_coincidental_frame_times() {
        // A smooth uncapped run: frame times spread over 2-10 ms, some near 4.17 by chance.
        let smooth: Vec<f64> = (0..2000)
            .map(|i| 2.0 + (i as f64 * 0.6180339).fract() * 8.0)
            .collect();
        assert_eq!(presentation_pacing(&measured_frames(&smooth)), None);
        // The same run with 100 consecutive frames held to 60 Hz.
        let mut throttled = smooth.clone();
        throttled.splice(500..600, std::iter::repeat_n(16.667, 100));
        let pacing = presentation_pacing(&measured_frames(&throttled)).expect("pacing detected");
        assert_eq!(pacing.longest_stretch, 100);
        assert!((pacing.interval_ms - 1000.0 / 60.0).abs() < 1e-9);
    }

    /// Frames within the pacing band of the 60 Hz interval, and 0.45-0.75 ms beside it, counted
    /// here independently of the rule.
    fn on_and_beside_tick(frames: &[FrameSample]) -> (usize, usize) {
        let near = |range: std::ops::Range<f64>| {
            frames
                .iter()
                .filter(|sample| range.contains(&(sample.frame_ms - TICK_MS).abs()))
                .count()
        };
        (near(0.0..0.15), near(0.45..0.75))
    }

    #[test]
    fn a_throttle_broken_into_short_stretches_is_still_paced() {
        // Twenty throttled stretches of ten frames between single long frames, the last at the
        // end of the run: no stretch reaches 30 frames, but together they are a spike.
        let mut times: Vec<f64> = (0..600)
            .map(|i| 2.0 + (i as f64 * 0.6180339).fract() * 8.0)
            .collect();
        for stretch in 0..20 {
            times.extend(std::iter::repeat_n(TICK_MS, 10));
            if stretch < 19 {
                times.push(25.0);
            }
        }
        let pacing = presentation_pacing(&measured_frames(&times)).expect("pacing detected");
        assert_eq!(
            (
                pacing.frames_on_interval,
                pacing.frames_in_stretches,
                pacing.longest_stretch
            ),
            (200, 200, 10)
        );
        assert!((pacing.interval_ms - TICK_MS).abs() < 1e-9);
    }

    #[test]
    fn isolated_frames_on_the_tick_cadence_are_not_pacing() {
        // A saturated device: long frames that run no tick alternate with catch-up frames that
        // end exactly on the tick cadence, with nothing beside it. Every second frame is on the
        // 60 Hz interval, never two in a row.
        let times: Vec<f64> = (0..400)
            .map(|i| {
                if i % 2 == 0 {
                    30.0 + (i as f64 * 0.6180339).fract() * 10.0
                } else {
                    TICK_MS
                }
            })
            .collect();
        let frames = measured_frames(&times);
        assert_eq!(on_and_beside_tick(&frames), (200, 0));
        assert_eq!(presentation_pacing(&frames), None);
    }

    /// The measured frame times of a recorded run (`tests/fixtures/pacing`).
    fn recorded_run(name: &str) -> Vec<FrameSample> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/pacing")
            .join(name);
        let text = std::fs::read_to_string(&path).expect("fixture");
        let times: Vec<f64> = text
            .lines()
            .map(|line| line.parse().expect("frame time"))
            .collect();
        measured_frames(&times)
    }

    #[test]
    fn recorded_saturated_capacity_runs_are_not_paced() {
        // Rejected as paced by the rule before frames had to be consecutive: each has at least
        // 1% of its frames on 16.67 ms and more than four times as many as beside it.
        for (name, on, beside) in [
            ("capacity-coupled-run-1.txt", 8, 0),
            ("capacity-coupled-run-6.txt", 23, 2),
            ("capacity-coupled-run-7.txt", 46, 9),
        ] {
            let frames = recorded_run(name);
            assert_eq!(on_and_beside_tick(&frames), (on, beside), "{name}");
            assert_eq!(presentation_pacing(&frames), None, "{name}");
        }
    }

    #[test]
    fn recorded_throttled_baseline_runs_stay_paced() {
        for (name, on, in_stretches, beside, longest) in [
            ("baseline-body-contact-run-3.txt", 131, 128, 9, 43),
            ("baseline-flow-run-4.txt", 58, 58, 0, 58),
        ] {
            let frames = recorded_run(name);
            let pacing = presentation_pacing(&frames).expect("pacing detected");
            assert!((pacing.interval_ms - TICK_MS).abs() < 1e-9, "{name}");
            assert_eq!(
                (
                    pacing.frames_on_interval,
                    pacing.frames_in_stretches,
                    pacing.frames_beside_interval,
                    pacing.longest_stretch
                ),
                (on, in_stretches, beside, longest),
                "{name}"
            );
            // The spike alone rejects them too, as it would if the stretches had been broken.
            assert!(
                pacing.frames_in_stretches as f64 >= PACED_SPIKE_SHARE * frames.len() as f64
                    && pacing.frames_in_stretches
                        > PACED_SPIKE_RATIO * pacing.frames_beside_interval,
                "{name}"
            );
        }
    }

    #[test]
    fn statistics_use_only_measured_frames() {
        let frame = |measured: bool, frame_ms: f64, ticks: u64| FrameSample {
            measured,
            frame_ms,
            ticks,
            full_copy_bytes: ticks * 100,
            ..FrameSample::default()
        };
        let samples = [
            frame(false, 100.0, 9),
            frame(true, 500.0, 1),
            frame(true, 500.0, 0),
        ];
        let stats = run_statistics(&samples);
        assert_eq!(stats.measured_frames, 2);
        assert_eq!(stats.measured_seconds, 1.0);
        assert_eq!(stats.ticks, 1);
        assert_eq!(stats.full_copy_bytes_per_second, 100.0);
        assert_eq!(stats.frames_without_tick, 1);
    }
}
