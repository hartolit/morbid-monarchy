//! Per-frame samples shared by the baseline driver (writer) and the bench runner (reader), and
//! the statistics computed from them.
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
    /// Main-world schedule time, from `First` to `Last`.
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
/// A spike must hold this share of frames and exceed its neighbourhood this many times.
const PACED_SPIKE_SHARE: f64 = 0.01;
const PACED_SPIKE_RATIO: usize = 4;

/// Evidence that a compositor, not the application's work, set the frame times.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pacing {
    pub interval_ms: f64,
    /// Frames within ±0.15 ms of the interval.
    pub frames_on_interval: usize,
    /// Frames 0.45-0.75 ms either side of it: twice the band's width, so an unpaced, smooth
    /// distribution puts about twice as many frames here as on the interval.
    pub frames_beside_interval: usize,
    pub longest_stretch: usize,
}

/// Detects presentation pacing in measured frames. Uncapped presentation produces a smooth
/// frame-time distribution; a compositor that throttles a covered or unfocused window produces
/// long runs of frames on one refresh interval and a spike there. Isolated frames that happen to
/// match an interval are expected and do not count.
pub fn presentation_pacing(samples: &[FrameSample]) -> Option<Pacing> {
    let frames: Vec<f64> = samples
        .iter()
        .filter(|sample| sample.measured)
        .map(|sample| sample.frame_ms)
        .collect();
    REFRESH_INTERVALS_MS
        .iter()
        .map(|&interval| {
            let mut longest_stretch = 0;
            let mut stretch = 0;
            for frame in &frames {
                stretch = if (frame - interval).abs() < PACING_BAND_MS {
                    stretch + 1
                } else {
                    0
                };
                longest_stretch = longest_stretch.max(stretch);
            }
            let distance = |frame: &&f64| (**frame - interval).abs();
            Pacing {
                interval_ms: interval,
                frames_on_interval: frames
                    .iter()
                    .filter(|f| distance(f) < PACING_BAND_MS)
                    .count(),
                frames_beside_interval: frames
                    .iter()
                    .filter(|f| (3.0 * PACING_BAND_MS..5.0 * PACING_BAND_MS).contains(&distance(f)))
                    .count(),
                longest_stretch,
            }
        })
        .find(|pacing| {
            let spike = pacing.frames_on_interval as f64 >= PACED_SPIKE_SHARE * frames.len() as f64
                && pacing.frames_on_interval > PACED_SPIKE_RATIO * pacing.frames_beside_interval;
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

    #[test]
    fn compositor_pacing_is_told_apart_from_coincidental_frame_times() {
        let frames = |times: &[f64]| -> Vec<FrameSample> {
            times
                .iter()
                .map(|&frame_ms| FrameSample {
                    measured: true,
                    frame_ms,
                    ..FrameSample::default()
                })
                .collect()
        };
        // A smooth uncapped run: frame times spread over 2-10 ms, some near 4.17 by chance.
        let smooth: Vec<f64> = (0..2000)
            .map(|i| 2.0 + (i as f64 * 0.6180339).fract() * 8.0)
            .collect();
        assert_eq!(presentation_pacing(&frames(&smooth)), None);
        // The same run with 100 consecutive frames held to 60 Hz.
        let mut throttled = smooth.clone();
        throttled.splice(500..600, std::iter::repeat_n(16.667, 100));
        let pacing = presentation_pacing(&frames(&throttled)).expect("pacing detected");
        assert_eq!(pacing.longest_stretch, 100);
        assert!((pacing.interval_ms - 1000.0 / 60.0).abs() < 1e-9);
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
