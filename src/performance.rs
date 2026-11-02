/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Opt-in performance comparisons. Timings are host wall time, not GPU queries.

use crate::options::Options;
use crate::Environment;
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const REPORT_FILE: &str = "Podroid_performance_tests.txt";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Reference,
    LowResolution,
    QuietLog,
    LowResolutionQuietLog,
}
impl Mode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "reference" => Ok(Self::Reference),
            "low-resolution" => Ok(Self::LowResolution),
            "quiet-log" => Ok(Self::QuietLog),
            "low-resolution-quiet-log" => Ok(Self::LowResolutionQuietLog),
            _ => Err(format!("Unknown performance test {value:?}")),
        }
    }
    pub fn quiet(self) -> bool {
        matches!(self, Self::QuietLog | Self::LowResolutionQuietLog)
    }
    pub fn low_resolution(self) -> bool {
        matches!(self, Self::LowResolution | Self::LowResolutionQuietLog)
    }
}

/// Applied after device selection so all comparisons use the same model and
/// canvas. Only Retina and short-read logging differ between the four modes.
pub fn configure(options: &mut Options) {
    let Some(mode) = options.performance_test else {
        return;
    };
    options.retina = !mode.low_resolution();
    options.device_screen_scale = Some(if mode.low_resolution() { 1 } else { 2 });
    options.scale_hack = std::num::NonZeroU32::new(1).unwrap();
    options.fps_limit = Some(60.0);
    options.filter_mode = crate::options::FilterMode::Sharp;
}

#[derive(Default)]
pub struct State {
    pub cpu: Duration,
    pub gl: Duration,
    pub gl_calls: u64,
    pub short_reads: u64,
    last_frame: Option<Instant>,
    start: Option<Instant>,
    intervals: Vec<f64>,
    present: Duration,
    readback: Duration,
    requested_wait: Duration,
    slow_frames: u64,
    session: Option<u128>,
    reported_io_error: bool,
}

/// Append compact audio events to the same report as the performance tests.
pub fn audio_event(line: &str) {
    log!("{}", line);
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(crate::paths::user_data_base_path().join(REPORT_FILE))?;
        writeln!(file, "{line}")
    })();
    if let Err(error) = result {
        log!("Could not append audio test report: {}", error);
    }
}

fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() as f64 * fraction).ceil() as usize).saturating_sub(1)]
}

impl State {
    fn append(&mut self, line: &str) {
        log!("{}", line);
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(crate::paths::user_data_base_path().join(REPORT_FILE))?;
            writeln!(file, "{line}")
        })();
        if let Err(error) = result {
            if !self.reported_io_error {
                self.reported_io_error = true;
                log!("Could not append performance report: {}", error);
            }
        }
    }

    fn clear_window(&mut self, now: Instant) {
        self.cpu = Duration::ZERO;
        self.gl = Duration::ZERO;
        self.gl_calls = 0;
        self.short_reads = 0;
        self.present = Duration::ZERO;
        self.readback = Duration::ZERO;
        self.requested_wait = Duration::ZERO;
        self.slow_frames = 0;
        self.intervals.clear();
        self.start = Some(now);
        self.last_frame = Some(now);
    }
}

/// Record only actual presentations. The readback component is included in
/// present time; CPU, GL calls and requested pacing are separate measurements.
/// GL calls measure driver submission/stalls, not asynchronous GPU execution.
pub fn frame(
    env: &mut Environment,
    started: Instant,
    slow: bool,
    readback: Duration,
    requested_wait: Option<Duration>,
) {
    let Some(mode) = env.options.performance_test else {
        return;
    };
    let now = Instant::now();
    let model = env.options.device_model.unwrap_or("iPhone1,1");
    let size = env.window.as_ref().unwrap().size_unrotated_unscaled();
    let scale = env.options.screen_scale();
    let state = &mut env.framework_state.performance;
    if state.session.is_none() {
        let session = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        state.session = Some(session);
        state.append(&format!(
            "\nSESSION {session} mode={mode:?} version={} model={model} pixels={}x{} scale=1 fps_limit=60; times=host_wall_ms; readback_included_in_present; timings_may_overlap_do_not_add; diagnostic_overhead_present; GL_is_submission_not_GPU; use_same_gameplay_scene_30s",
            crate::VERSION, size.0 * scale, size.1 * scale,
        ));
    }
    // Do not count a long loading/pause gap as a normal gameplay frame.
    let Some(previous) = state.last_frame else {
        state.clear_window(now);
        return;
    };
    let interval = now.saturating_duration_since(previous);
    if interval > Duration::from_secs(2) {
        state.append("Sampling restarted after a loading/pause gap longer than 2s.");
        state.clear_window(now);
        return;
    }
    state.last_frame = Some(now);
    state.intervals.push(interval.as_secs_f64() * 1000.0);
    state.present += now.saturating_duration_since(started);
    state.readback += readback;
    state.requested_wait += requested_wait.unwrap_or_default();
    state.slow_frames += u64::from(slow);
    let elapsed = now.saturating_duration_since(state.start.unwrap());
    if elapsed < Duration::from_secs(5) {
        return;
    }
    let frames = state.intervals.len() as f64;
    let per_frame = |time: Duration| time.as_secs_f64() * 1000.0 / frames;
    let line = format!(
        "session={} mode={mode:?} seconds={:.1} frames={} FPS={:.2} frame_p50_ms={:.2} frame_p95_ms={:.2} CPU_ms/frame={:.2} GL_ms/frame={:.2} GL_calls/frame={:.1} present_ms/frame={:.2} readback_path_ms/frame={:.2} slow_frames={} requested_wait_ms/frame={:.2} short_reads={}",
        state.session.unwrap(), elapsed.as_secs_f64(), state.intervals.len(),
        frames / elapsed.as_secs_f64(), percentile(&state.intervals, 0.5),
        percentile(&state.intervals, 0.95), per_frame(state.cpu), per_frame(state.gl),
        state.gl_calls as f64 / frames, per_frame(state.present), per_frame(state.readback),
        state.slow_frames, per_frame(state.requested_wait), state.short_reads,
    );
    state.append(&line);
    state.clear_window(now);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modes_change_only_the_two_comparison_variables() {
        for (name, low, quiet) in [
            ("reference", false, false),
            ("low-resolution", true, false),
            ("quiet-log", false, true),
            ("low-resolution-quiet-log", true, true),
        ] {
            let mode = Mode::parse(name).unwrap();
            assert_eq!(mode.low_resolution(), low);
            assert_eq!(mode.quiet(), quiet);
            let mut options = Options {
                performance_test: Some(mode),
                device_model: Some("iPhone4,1"),
                ..Options::default()
            };
            configure(&mut options);
            assert_eq!(options.device_model, Some("iPhone4,1"));
            assert_eq!(options.retina, !low);
            assert_eq!(options.scale_hack.get(), 1);
        }
        assert!(Mode::parse("unknown").is_err());
    }
    #[test]
    fn frame_percentiles_and_resets_are_deterministic() {
        assert_eq!(percentile(&[10.0, 50.0, 20.0, 30.0, 40.0], 0.5), 30.0);
        assert_eq!(percentile(&[10.0, 50.0, 20.0, 30.0, 40.0], 0.95), 50.0);
        let now = Instant::now();
        let mut state = State {
            cpu: Duration::from_secs(1),
            short_reads: 12,
            session: Some(123),
            ..State::default()
        };
        state.clear_window(now);
        assert_eq!(state.cpu, Duration::ZERO);
        assert_eq!(state.short_reads, 0);
        assert_eq!(state.session, Some(123));
        assert_eq!(state.last_frame, Some(now));
    }
}
