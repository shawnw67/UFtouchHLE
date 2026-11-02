/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Decoding of movies (MPEG-4 files with H.264 video and AAC audio, which is
//! what iPhone OS apps normally use) for `MPMoviePlayerController`.
//!
//! Only as much of the MPEG-4 file format as is needed to find the video
//! frames is parsed here. The audio is decoded with [crate::audio].

use openh264::decoder::{DecodedYUV, Decoder, DecoderConfig, Flush};
use openh264::formats::YUVSource;
use openh264::OpenH264API;
use std::collections::VecDeque;
use std::time::Duration;

/// A decoded video frame.
pub struct Frame {
    /// RGBA pixels, row by row.
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A subtitle, shown from `start` to `end`.
#[derive(Clone)]
pub struct Subtitle {
    pub start: Duration,
    pub end: Duration,
    pub text: String,
}

/// Decoded audio: 16-bit little-endian interleaved PCM.
pub struct Audio {
    pub pcm: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u32,
}

/// A video sample (one frame) in the file.
struct Sample {
    offset: usize,
    size: usize,
    /// Presentation time, in the track's timescale.
    time: i64,
    /// Duration, in the track's timescale.
    duration: i64,
}

pub struct MovieDecoder {
    file: Vec<u8>,
    /// Subtitle tracks: language (ISO 639-2/T code, e.g. "eng") and subtitles.
    subtitle_tracks: Vec<(String, Vec<Subtitle>)>,
    width: u32,
    height: u32,
    /// The samples, in decode order.
    samples: Vec<Sample>,
    /// The presentation times of the samples, sorted. With B-frames the
    /// samples are not in presentation order, but the decoder outputs
    /// pictures in presentation order, so the Nth picture is shown at the Nth
    /// of these times.
    presentation_times: Vec<i64>,
    timescale: u32,
    /// The next sample to decode.
    next_sample: usize,
    /// How many pictures the decoder has output (or failed to) so far.
    outputs: usize,
    /// Whether the pictures the decoder was still holding back after the last
    /// sample have been taken out of it (into `held_back`).
    flushed: bool,
    held_back: VecDeque<Frame>,
    /// SPS and PPS NAL units, in Annex B format.
    parameter_sets: Vec<u8>,
    /// Length of the NAL unit length fields in samples.
    nal_length_size: usize,
    decoder: Decoder,
}

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(data: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(data.get(at..at + 8)?.try_into().ok()?))
}

/// Iterate over the boxes in `data`: (type, contents).
fn boxes(data: &[u8]) -> impl Iterator<Item = (&[u8], &[u8])> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let size = read_u32(data, at)? as usize;
        let kind = data.get(at + 4..at + 8)?;
        let (header, size) = match size {
            0 => (8, data.len() - at),
            1 => (16, read_u64(data, at + 8)? as usize),
            size => (8, size),
        };
        if size < header {
            return None;
        }
        let contents = data.get(at + header..at + size)?;
        at += size;
        Some((kind, contents))
    })
}

fn find_box<'a>(data: &'a [u8], kind: &[u8]) -> Option<&'a [u8]> {
    boxes(data)
        .find(|&(k, _)| k == kind)
        .map(|(_, contents)| contents)
}

/// Find a box by path, e.g. `["mdia", "minf"]`.
fn find_path<'a>(mut data: &'a [u8], path: &[&str]) -> Option<&'a [u8]> {
    for kind in path {
        data = find_box(data, kind.as_bytes())?;
    }
    Some(data)
}

/// Read a "full box" table: (entry count, entries).
fn table(data: &[u8]) -> Option<(usize, &[u8])> {
    Some((read_u32(data, 4)? as usize, data.get(8..)?))
}

/// Read a track's sample table (the `stbl` box): where each sample is in the
/// file, and its timing.
fn sample_table(stbl: &[u8]) -> Option<Vec<Sample>> {
    // Sample sizes
    let stsz = find_box(stbl, b"stsz")?;
    let fixed_size = read_u32(stsz, 4)? as usize;
    let count = read_u32(stsz, 8)? as usize;
    let sizes: Vec<usize> = (0..count)
        .map(|i| {
            if fixed_size != 0 {
                Some(fixed_size)
            } else {
                read_u32(stsz, 12 + i * 4).map(|s| s as usize)
            }
        })
        .collect::<Option<_>>()?;

    // Chunk offsets
    let chunk_offsets: Vec<usize> = if let Some(stco) = find_box(stbl, b"stco") {
        let (n, e) = table(stco)?;
        (0..n)
            .map(|i| read_u32(e, i * 4).map(|o| o as usize))
            .collect::<Option<_>>()?
    } else {
        let (n, e) = table(find_box(stbl, b"co64")?)?;
        (0..n)
            .map(|i| read_u64(e, i * 8).map(|o| o as usize))
            .collect::<Option<_>>()?
    };

    // Sample-to-chunk: (first chunk (1-based), samples per chunk)
    let (n, e) = table(find_box(stbl, b"stsc")?)?;
    let stsc: Vec<(usize, usize)> = (0..n)
        .map(|i| {
            Some((
                read_u32(e, i * 12)? as usize,
                read_u32(e, i * 12 + 4)? as usize,
            ))
        })
        .collect::<Option<_>>()?;

    // Decode times
    let (n, e) = table(find_box(stbl, b"stts")?)?;
    let mut decode_times = Vec::with_capacity(count);
    let mut durations = Vec::with_capacity(count);
    let mut time = 0i64;
    for i in 0..n {
        let sample_count = read_u32(e, i * 8)?;
        let delta = read_u32(e, i * 8 + 4)? as i64;
        for _ in 0..sample_count {
            decode_times.push(time);
            durations.push(delta);
            time += delta;
        }
    }

    // Composition offsets (optional)
    let mut composition_offsets = vec![0i64; count];
    if let Some(ctts) = find_box(stbl, b"ctts") {
        let (n, e) = table(ctts)?;
        let mut sample = 0;
        for i in 0..n {
            let sample_count = read_u32(e, i * 8)? as usize;
            let offset = read_u32(e, i * 8 + 4)? as i32 as i64;
            for _ in 0..sample_count {
                if let Some(o) = composition_offsets.get_mut(sample) {
                    *o = offset;
                }
                sample += 1;
            }
        }
    }

    let mut samples = Vec::with_capacity(count);
    for (chunk_index, &chunk_offset) in chunk_offsets.iter().enumerate() {
        let chunk_number = chunk_index + 1;
        let per_chunk = stsc
            .iter()
            .rev()
            .find(|&&(first, _)| first <= chunk_number)
            .map_or(0, |&(_, n)| n);
        let mut offset = chunk_offset;
        for _ in 0..per_chunk {
            let i = samples.len();
            let Some(&size) = sizes.get(i) else {
                break;
            };
            samples.push(Sample {
                offset,
                size,
                time: decode_times.get(i).copied().unwrap_or(0) + composition_offsets[i],
                duration: durations.get(i).copied().unwrap_or(0),
            });
            offset += size;
        }
    }

    Some(samples)
}

/// Read a 3-letter ISO 639-2/T language code from an `mdhd` box.
fn track_language(mdhd: &[u8]) -> Option<String> {
    let at = if mdhd[0] == 1 { 32 } else { 20 };
    let packed = u16::from_be_bytes(mdhd.get(at..at + 2)?.try_into().ok()?);
    Some(
        [10, 5, 0]
            .iter()
            .map(|shift| (((packed >> shift) & 0x1f) as u8 + 0x60) as char)
            .collect(),
    )
}

/// Parse a 3GPP timed text (`tx3g`) subtitle track.
fn parse_subtitle_track(file: &[u8], trak: &[u8]) -> Option<(String, Vec<Subtitle>)> {
    let mdia = find_box(trak, b"mdia")?;
    let handler = find_box(mdia, b"hdlr")?.get(8..12)?;
    if handler != b"text" && handler != b"sbtl" {
        return None;
    }
    let mdhd = find_box(mdia, b"mdhd")?;
    let timescale = if mdhd[0] == 1 {
        read_u32(mdhd, 20)?
    } else {
        read_u32(mdhd, 12)?
    } as f64;
    let language = track_language(mdhd)?;
    let stbl = find_path(mdia, &["minf", "stbl"])?;
    let (_, entries) = table(find_box(stbl, b"stsd")?)?;
    if boxes(entries).next()?.0 != b"tx3g" {
        return None;
    }

    let mut subtitles = Vec::new();
    for sample in sample_table(stbl)? {
        let Some(data) = file.get(sample.offset..sample.offset + sample.size) else {
            continue;
        };
        // A 16-bit length, then the text, then optional style boxes.
        let Some(length) = data.get(0..2) else {
            continue;
        };
        let length = u16::from_be_bytes([length[0], length[1]]) as usize;
        let Some(text) = data.get(2..2 + length) else {
            continue;
        };
        let text = if text.starts_with(&[0xfe, 0xff]) {
            let units: Vec<u16> = text[2..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_be_bytes(c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(text).into_owned()
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        subtitles.push(Subtitle {
            start: Duration::from_secs_f64(sample.time.max(0) as f64 / timescale),
            end: Duration::from_secs_f64((sample.time + sample.duration).max(0) as f64 / timescale),
            text,
        });
    }
    Some((language, subtitles))
}

/// The presentation times of `samples`, in order. The samples are in decode
/// order, which is not presentation order if there are B-frames.
fn presentation_times(samples: &[Sample]) -> Vec<i64> {
    let mut times: Vec<i64> = samples.iter().map(|sample| sample.time).collect();
    times.sort_unstable();
    times
}

/// Convert a decoded picture to RGBA.
fn to_frame(yuv: &DecodedYUV<'_>) -> Frame {
    let (width, height) = yuv.dimensions();
    let mut pixels = vec![0u8; width * height * 4];
    yuv.write_rgba8(&mut pixels);
    Frame {
        pixels,
        width: width as u32,
        height: height as u32,
    }
}

struct VideoTrack {
    width: u32,
    height: u32,
    samples: Vec<Sample>,
    timescale: u32,
    parameter_sets: Vec<u8>,
    nal_length_size: usize,
}

fn parse_video_track(trak: &[u8]) -> Option<VideoTrack> {
    let mdia = find_box(trak, b"mdia")?;
    // Handler type is after version/flags and pre_defined.
    if find_box(mdia, b"hdlr")?.get(8..12)? != b"vide" {
        return None;
    }
    let mdhd = find_box(mdia, b"mdhd")?;
    let timescale = if mdhd[0] == 1 {
        read_u32(mdhd, 20)?
    } else {
        read_u32(mdhd, 12)?
    };
    let stbl = find_path(mdia, &["minf", "stbl"])?;

    // Sample description: avc1 → avcC
    let (_, entries) = table(find_box(stbl, b"stsd")?)?;
    let (kind, avc1) = boxes(entries).next()?;
    if kind != b"avc1" {
        return None;
    }
    let width = u16::from_be_bytes(avc1.get(24..26)?.try_into().ok()?) as u32;
    let height = u16::from_be_bytes(avc1.get(26..28)?.try_into().ok()?) as u32;
    // Skip the VisualSampleEntry fields to get to the child boxes.
    let avcc = find_box(avc1.get(78..)?, b"avcC")?;
    let nal_length_size = (avcc.get(4)? & 3) as usize + 1;
    let mut parameter_sets = Vec::new();
    let mut at = 5;
    for count_mask in [0x1f, 0xff] {
        let count = avcc.get(at)? & count_mask;
        at += 1;
        for _ in 0..count {
            let length = u16::from_be_bytes(avcc.get(at..at + 2)?.try_into().ok()?) as usize;
            at += 2;
            parameter_sets.extend_from_slice(&[0, 0, 0, 1]);
            parameter_sets.extend_from_slice(avcc.get(at..at + length)?);
            at += length;
        }
    }

    let samples = sample_table(stbl)?;

    Some(VideoTrack {
        width,
        height,
        samples,
        timescale,
        parameter_sets,
        nal_length_size,
    })
}

impl MovieDecoder {
    /// Open a movie file. Also returns its audio, if it has any (decoded
    /// up-front, since it's small compared to the video).
    pub fn new(file: Vec<u8>) -> Result<(Self, Option<Audio>), String> {
        let audio = crate::audio::decode_audio_track_to_pcm(file.clone()).map(
            |(pcm, sample_rate, channels)| Audio {
                pcm,
                sample_rate,
                channels,
            },
        );

        let moov = find_box(&file, b"moov").ok_or("No moov box, not an MPEG-4 file?")?;
        let track = boxes(moov)
            .filter(|&(kind, _)| kind == b"trak")
            .find_map(|(_, trak)| parse_video_track(trak))
            .ok_or("No H.264 video track")?;

        let subtitle_tracks = boxes(moov)
            .filter(|&(kind, _)| kind == b"trak")
            .filter_map(|(_, trak)| parse_subtitle_track(&file, trak))
            .collect();

        // Flushing after each frame (the default) breaks decoding Main profile
        // streams (e.g. Dead Rising's movies): it makes OpenH264 run out of
        // pictures for the next frame. Without it, pictures come out in
        // presentation order, and for streams with B-frames (e.g. Monster
        // Hunter Freedom Unite's movies) only after some later samples have
        // been decoded, which decode_next_frame() takes into account.
        let config = DecoderConfig::new().flush_after_decode(Flush::NoFlush);
        let decoder = Decoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|e| format!("Couldn't create H.264 decoder: {e}"))?;
        Ok((
            MovieDecoder {
                file,
                subtitle_tracks,
                width: track.width,
                height: track.height,
                presentation_times: presentation_times(&track.samples),
                samples: track.samples,
                timescale: track.timescale,
                next_sample: 0,
                outputs: 0,
                flushed: false,
                held_back: VecDeque::new(),
                parameter_sets: track.parameter_sets,
                nal_length_size: track.nal_length_size,
                decoder,
            },
            audio,
        ))
    }

    /// The subtitles to show for the first of `languages` (e.g. "en") that
    /// there are subtitles for, or English ones otherwise.
    pub fn subtitles_for(&self, languages: &[String]) -> Option<&[Subtitle]> {
        // ISO 639-1 to ISO 639-2 (both the T and B codes are used in files).
        const CODES: &[(&str, &[&str])] = &[
            ("en", &["eng"]),
            ("fr", &["fra", "fre"]),
            ("de", &["deu", "ger"]),
            ("es", &["spa"]),
            ("it", &["ita"]),
            ("ja", &["jpn"]),
            ("ko", &["kor"]),
            ("zh", &["zho", "chi"]),
            ("pt", &["por"]),
            ("ru", &["rus"]),
            ("nl", &["nld", "dut"]),
        ];
        let track_for = |codes: &[&str]| {
            self.subtitle_tracks
                .iter()
                .find(|(language, _)| codes.contains(&language.as_str()))
                .map(|(_, subtitles)| &subtitles[..])
        };
        languages
            .iter()
            .filter_map(|language| {
                let short = language.split(['-', '_']).next().unwrap_or("");
                CODES.iter().find(|&&(code, _)| code == short)
            })
            .find_map(|&(_, codes)| track_for(codes))
            .or_else(|| track_for(&["eng"]))
    }

    /// The size of the video, in pixels.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// How long the video is.
    pub fn duration(&self) -> Duration {
        let end = self
            .samples
            .iter()
            .map(|sample| sample.time + sample.duration)
            .max()
            .unwrap_or(0);
        Duration::from_secs_f64(end.max(0) as f64 / self.timescale as f64)
    }

    /// When the next frame is due, relative to the start of the movie, or
    /// [None] if there are no more frames.
    pub fn next_frame_time(&self) -> Option<Duration> {
        let time = *self.presentation_times.get(self.outputs)?;
        Some(Duration::from_secs_f64(
            time.max(0) as f64 / self.timescale as f64,
        ))
    }

    /// Decode the next frame, in presentation order. Returns [None] at the
    /// end of the movie. A frame may not be produced for every call (e.g. if
    /// it can't be decoded), so the inner value can be [None].
    ///
    /// The samples are in decode order, and with B-frames a picture only
    /// comes out of the decoder after the samples that follow it in
    /// presentation order but precede it in decode order have been decoded.
    /// So this may decode several samples. Every call moves on to the next
    /// time that [Self::next_frame_time] returns, even if it produces no
    /// frame.
    pub fn decode_next_frame(&mut self) -> Option<Option<Frame>> {
        if self.outputs >= self.presentation_times.len() {
            return None;
        }
        loop {
            let Some(sample) = self.samples.get(self.next_sample) else {
                return self.next_held_back_frame();
            };
            let first = self.next_sample == 0;
            self.next_sample += 1;
            let Some(mut data) = self.file.get(sample.offset..sample.offset + sample.size) else {
                continue;
            };

            // Samples have length-prefixed NAL units, but the decoder wants
            // them with start codes (Annex B format).
            let mut annex_b = Vec::with_capacity(data.len() + 64);
            if first {
                annex_b.extend_from_slice(&self.parameter_sets);
            }
            while data.len() >= self.nal_length_size {
                let length = data[..self.nal_length_size]
                    .iter()
                    .fold(0usize, |acc, &b| (acc << 8) | b as usize);
                data = &data[self.nal_length_size..];
                if length > data.len() {
                    break;
                }
                annex_b.extend_from_slice(&[0, 0, 0, 1]);
                annex_b.extend_from_slice(&data[..length]);
                data = &data[length..];
            }

            match self.decoder.decode(&annex_b) {
                Ok(Some(yuv)) => {
                    self.outputs += 1;
                    return Some(Some(to_frame(&yuv)));
                }
                // The decoder is holding this picture back until it has seen
                // the ones that come before it in presentation order.
                Ok(None) => {}
                Err(e) => {
                    log!("Warning: couldn't decode video frame: {}", e);
                    self.outputs += 1;
                    return Some(None);
                }
            }
        }
    }

    /// Once all the samples have been decoded, the decoder may still be
    /// holding back some pictures. This returns them one by one, like
    /// [Self::decode_next_frame], and [None] when there are none left.
    fn next_held_back_frame(&mut self) -> Option<Option<Frame>> {
        if !self.flushed {
            self.flushed = true;
            if let Ok(pictures) = self.decoder.flush_remaining() {
                self.held_back = pictures.iter().map(to_frame).collect();
            }
        }
        if let Some(frame) = self.held_back.pop_front() {
            self.outputs += 1;
            Some(Some(frame))
        } else {
            self.outputs = self.presentation_times.len();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boxes() {
        // Two boxes: "abcd" with 1 byte, "efgh" with 2 bytes.
        let data = [
            0, 0, 0, 9, b'a', b'b', b'c', b'd', 1, 0, 0, 0, 10, b'e', b'f', b'g', b'h', 2, 3,
        ];
        let found: Vec<_> = boxes(&data).collect();
        assert_eq!(
            found,
            vec![(&b"abcd"[..], &[1][..]), (&b"efgh"[..], &[2, 3][..])]
        );
        assert_eq!(find_box(&data, b"efgh"), Some(&[2, 3][..]));
    }

    #[test]
    fn test_presentation_times() {
        let sample = |time| Sample {
            offset: 0,
            size: 0,
            time,
            duration: 1,
        };
        // An I-frame, a P-frame and two B-frames between them, in decode
        // order.
        let samples = [sample(0), sample(3), sample(1), sample(2)];
        assert_eq!(presentation_times(&samples), [0, 1, 2, 3]);
    }

    /// Decode a movie given with TOUCHHLE_TEST_MOVIE, and save its 100th
    /// frame next to it as a PPM image, for checking by eye.
    #[test]
    #[ignore]
    fn test_decode_movie() {
        let path = std::env::var("TOUCHHLE_TEST_MOVIE").unwrap();
        let (mut movie, audio) = MovieDecoder::new(std::fs::read(&path).unwrap()).unwrap();
        let audio = audio.unwrap();
        println!(
            "{} samples, audio {} Hz × {} channels, {} bytes",
            movie.samples.len(),
            audio.sample_rate,
            audio.channels,
            audio.pcm.len()
        );
        let mut decoded = 0;
        while decoded < 100 {
            if let Some(frame) = movie.decode_next_frame().unwrap() {
                decoded += 1;
                if decoded == 100 {
                    let mut ppm =
                        format!("P6\n{} {}\n255\n", frame.width, frame.height).into_bytes();
                    for pixel in frame.pixels.chunks(4) {
                        ppm.extend_from_slice(&pixel[..3]);
                    }
                    std::fs::write(format!("{path}.ppm"), ppm).unwrap();
                }
            }
        }
        println!("next frame time: {:?}", movie.next_frame_time());
        for (language, subtitles) in &movie.subtitle_tracks {
            println!("{} subtitles: {}", language, subtitles.len());
        }
        let chinese = movie.subtitles_for(&["zh-Hans".to_string()]).unwrap();
        for subtitle in chinese.iter().take(3) {
            println!(
                "{:?}-{:?}: {:?}",
                subtitle.start, subtitle.end, subtitle.text
            );
        }
    }
}
