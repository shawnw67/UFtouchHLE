/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Bounded, opt-in audio history for a user-marked transient noise.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static REQUESTED_AT: AtomicU64 = AtomicU64::new(0);
const CAPACITY: usize = 192;
const HISTORY_MS: u64 = 15_000;

pub fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Called by the Android UI thread; all guest/audio access stays on the
/// emulation thread. Repeated taps before polling coalesce into one request.
#[cfg(target_os = "android")]
pub fn request_capture() {
    REQUESTED_AT.store(timestamp_ms(), Ordering::Release);
}

pub fn take_request() -> u64 {
    REQUESTED_AT.swap(0, Ordering::Acquire)
}

#[derive(Default)]
pub struct Trace {
    events: VecDeque<(u64, String)>,
    follow_until: Option<Instant>,
    pcm: VecDeque<(u64, i32, i32, Vec<u8>)>,
    pcm_bytes: usize,
    captures: u32,
}
impl Trace {
    /// Returns true if this event belongs to a marker's follow-up window.
    pub fn record(&mut self, at: u64, now: Instant, event: String) -> bool {
        self.events.push_back((at, event));
        if self.events.len() > CAPACITY {
            self.events.pop_front();
        }
        self.follow_until.is_some_and(|until| now <= until)
    }

    /// Retain at most 4 MiB / 6 seconds of recently uploaded PCM, not the
    /// entire sound library. Normal launches never call this method.
    pub fn record_pcm(&mut self, at: u64, format: i32, rate: i32, data: &[u8]) {
        const LIMIT: usize = 4 * 1024 * 1024;
        if data.len() > LIMIT {
            return;
        }
        self.pcm_bytes += data.len();
        self.pcm.push_back((at, format, rate, data.to_vec()));
        while self
            .pcm
            .front()
            .is_some_and(|(time, _, _, _)| time.saturating_add(6000) < at)
            || self.pcm_bytes > LIMIT
        {
            if let Some((_, _, _, data)) = self.pcm.pop_front() {
                self.pcm_bytes -= data.len();
            }
        }
    }

    /// Raw samples retain their exact uploaded byte representation. The
    /// accompanying report gives format/rate/length for offline decoding.
    pub fn save_pcm(&mut self, marker: u64) -> std::io::Result<String> {
        if self.captures >= 8 {
            return Ok("PCM capture limit reached for this run.\n".into());
        }
        self.captures += 1;
        let relative = format!("Podroid_audio_captures/{marker}");
        let directory = crate::paths::user_data_base_path().join(&relative);
        std::fs::create_dir_all(&directory)?;
        let mut report = format!(
            "PCM CAPTURE dir={relative}; raw_PCM_not_microphone; last_6s_uploads_up_to_4MiB\n"
        );
        for (index, (time, format, rate, data)) in self.pcm.iter().enumerate() {
            if time.saturating_add(6000) < marker {
                continue;
            }
            let name = format!("{index:03}_{time}_{format:04x}_{rate}.pcm");
            std::fs::write(directory.join(&name), data)?;
            report.push_str(&format!(
                "PCM file={name} format={format:#x} rate={rate} {}\n",
                describe_pcm(data)
            ));
        }
        std::fs::write(directory.join("capture.txt"), &report)?;
        Ok(report)
    }

    pub fn capture(&mut self, marker: u64, now: Instant) -> String {
        self.follow_until = Some(now + Duration::from_secs(3));
        let mut report = format!("\nNOISE MARK requested_ms={marker} captured_ms={} recent_audio_events; subsequent_3s_follow; diagnostic_capture_can_stall_temporarily\n", timestamp_ms());
        for (at, event) in &self.events {
            if at.saturating_add(HISTORY_MS) >= marker {
                report.push_str(&format!("AUDIO EVENT ms={at} {event}\n"));
            }
        }
        report
    }
}

/// Metadata fingerprint reads at most 128 bytes, even for large buffers.
/// No samples are retained or changed by tracing.
pub fn describe_pcm(data: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in data.iter().take(64).chain(data.iter().rev().take(64)) {
        hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
    }
    let first = data
        .get(..2)
        .map(|s| i16::from_le_bytes(s.try_into().unwrap()));
    let last = data
        .len()
        .checked_sub(2)
        .and_then(|i| data.get(i..))
        .map(|s| i16::from_le_bytes(s.try_into().unwrap()));
    format!(
        "bytes={} first16={first:?} last16={last:?} edge_hash={hash:016x}",
        data.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_history_keeps_recent_events_and_follow_up_expires() {
        let now = Instant::now();
        let mut trace = Trace::default();
        for i in 0..500 {
            assert!(!trace.record(i * 100, now, format!("event_{i}")));
        }
        assert_eq!(trace.events.len(), CAPACITY);
        let report = trace.capture(49_900, now);
        assert!(report.contains("event_499\n"));
        assert!(!report.contains("event_300\n"));
        assert!(trace.record(50_000, now + Duration::from_secs(1), "after".into()));
        assert!(!trace.record(54_000, now + Duration::from_secs(4), "later".into()));
    }
    #[test]
    fn pcm_history_is_bounded_by_time_and_bytes() {
        let mut trace = Trace::default();
        let data = vec![7; 1024 * 1024];
        for i in 0..10 {
            trace.record_pcm(i * 100, 0x1101, 22050, &data);
        }
        assert_eq!(trace.pcm.len(), 4);
        assert_eq!(trace.pcm_bytes, 4 * 1024 * 1024);
        trace.record_pcm(10_000, 0x1101, 22050, &[1, 2]);
        assert_eq!(trace.pcm.len(), 1);
        assert_eq!(trace.pcm_bytes, 2);
    }

    #[test]
    fn metadata_is_bounded_and_handles_empty_data() {
        assert!(describe_pcm(&[]).contains("first16=None"));
        let mut data = vec![0; 4096];
        let before = describe_pcm(&data);
        data[2048] = 255;
        assert_eq!(before, describe_pcm(&data));
        data[0] = 1;
        assert_ne!(before, describe_pcm(&data));
    }
}
