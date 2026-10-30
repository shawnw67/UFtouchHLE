/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Audio file decoding and OpenAL bindings.
//!
//! The audio file decoding support is an abstraction over various libraries
//! (currently [hound] and [symphonia]), usage of which should be
//! confined to this module.
//!
//! Resources:
//! - [Apple Core Audio Format Specification 1.0](https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_intro/CAF_intro.html)

mod ima4;
pub mod openal;
mod symphonia_formats;

pub use ima4::decode_ima4;

/// Decode the audio track of a file (e.g. the AAC track of an MPEG-4 movie)
/// to 16-bit little-endian interleaved PCM. Returns the samples, the sample
/// rate and the channel count.
pub fn decode_audio_track_to_pcm(bytes: Vec<u8>) -> Option<(Vec<u8>, u32, u32)> {
    let pcm = symphonia_formats::decode_symphonia_to_pcm(Cursor::new(bytes)).ok()?;
    Some((pcm.bytes, pcm.sample_rate, pcm.channels))
}

use crate::fs::{Fs, GuestPath};
use std::io::Cursor;

#[derive(Debug)]
pub enum AudioFileOpenError {
    FileReadError,
    FileDecodeError,
}

#[derive(Debug)]
pub enum AudioFormat {
    LinearPcm {
        is_float: bool,
        is_little_endian: bool,
    },
}
/// Fields have the same meanings as in the Core Audio Format's
/// Audio Description chunk, which is in turn similar to Core Audio Types'
/// `AudioStreamBasicDescription`.
#[derive(Debug)]
pub struct AudioDescription {
    /// Hz
    pub sample_rate: f64,
    pub format: AudioFormat,
    pub bytes_per_packet: u32,
    pub frames_per_packet: u32,
    pub channels_per_frame: u32,
    pub bits_per_channel: u32,
}

pub struct AudioFile(AudioFileInner, Vec<(u32, Vec<u8>)>);

/// Preserve CAF metadata verbatim. Chunk headers are big endian, but payloads
/// are opaque to AudioFileGetUserData (some games write little-endian markers).
fn caf_user_data(bytes: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut chunks = Vec::new();
    if bytes.get(..8) != Some(b"caff\0\x01\0\0") {
        return chunks;
    }
    let mut offset = 8;
    while let Some(header) = bytes.get(offset..).and_then(|b| b.get(..12)) {
        let id = u32::from_be_bytes(header[..4].try_into().unwrap());
        let size = i64::from_be_bytes(header[4..].try_into().unwrap());
        let Ok(size) = usize::try_from(size) else {
            break; // A final data chunk may extend to EOF (size -1).
        };
        let start = offset + 12;
        let Some(end) = start.checked_add(size) else {
            break;
        };
        let Some(data) = bytes.get(start..end) else {
            break;
        };
        // Audio data and padding aren't user metadata, and can be very large.
        if !matches!(&header[..4], b"data" | b"free") {
            chunks.push((id, data.to_vec()));
        }
        offset = end;
    }
    chunks
}
enum AudioFileInner {
    Wave(hound::WavReader<Cursor<Vec<u8>>>),
    Symphonia(symphonia_formats::SymphoniaDecodedToPcm),
}

impl AudioFile {
    pub fn open_for_reading<P: AsRef<GuestPath>>(
        path: P,
        fs: &Fs,
    ) -> Result<Self, AudioFileOpenError> {
        // TODO: it would be better not to load the whole file at once
        let Ok(bytes) = fs.read(path.as_ref()) else {
            // TODO: Handle other FS related errors?
            return Err(AudioFileOpenError::FileReadError);
        };

        if let Ok(bytes) = Self::read_from_vec(bytes) {
            Ok(bytes)
        } else {
            log!(
                "Could not decode audio file at path {:?}, likely an unimplemented file format.",
                path.as_ref()
            );
            Err(AudioFileOpenError::FileReadError)
        }
    }

    pub fn read_from_vec(bytes: Vec<u8>) -> Result<Self, AudioFileOpenError> {
        let user_data = caf_user_data(&bytes);
        // WavReader::new() consume the reader (in this case, a Cursor)
        // passed to it. This is a bit annoying considering we don't know
        // which is appropriate for the file. This is worked around here by
        // using temporary readers for checking if the file is the supported
        // format, then recreating the reader if that works.
        if hound::WavReader::new(Cursor::new(&bytes)).is_ok() {
            let reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
            Ok(AudioFile(AudioFileInner::Wave(reader), user_data))
        // TODO: Real MP3/MP4/Non-linear PCM container handling. Currently we
        // are immediately decoding the entire file to PCM and acting as if
        // it's a PCM file, simply because because this is easier. Full MP3
        // support would require a lot of changes in Audio Toolbox.
        } else if let Ok(pcm) = symphonia_formats::decode_symphonia_to_pcm(Cursor::new(bytes)) {
            Ok(AudioFile(AudioFileInner::Symphonia(pcm), user_data))
        } else {
            Err(AudioFileOpenError::FileDecodeError)
        }
    }

    pub fn audio_description(&self) -> AudioDescription {
        match self.0 {
            AudioFileInner::Wave(ref wave_reader) => {
                let hound::WavSpec {
                    channels,
                    sample_rate,
                    bits_per_sample,
                    sample_format,
                } = wave_reader.spec();
                // Hound supports unsigned 8-bit, signed 16-bit, signed 24-bit
                // and floating-point 32-bit linear PCM. We should expose all of
                // these eventually, but we should only expose formats we've
                // tested.
                assert!(matches!(bits_per_sample, 8 | 16));
                assert!(sample_format == hound::SampleFormat::Int);

                AudioDescription {
                    sample_rate: sample_rate.into(),
                    format: AudioFormat::LinearPcm {
                        is_float: false,
                        is_little_endian: true,
                    },
                    bytes_per_packet: u32::from(channels * bits_per_sample / 8),
                    frames_per_packet: 1,
                    channels_per_frame: channels.into(),
                    bits_per_channel: bits_per_sample as u32,
                }
            }
            AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm {
                sample_rate,
                channels,
                ..
            }) => AudioDescription {
                sample_rate: f64::from(sample_rate),
                format: AudioFormat::LinearPcm {
                    is_float: false,
                    is_little_endian: true,
                },
                bytes_per_packet: channels * 2,
                frames_per_packet: 1,
                channels_per_frame: channels,
                bits_per_channel: 16,
            },
        }
    }

    pub fn user_data(&self, id: u32, index: u32) -> Option<&[u8]> {
        self.1
            .iter()
            .filter(|(chunk_id, _)| *chunk_id == id)
            .nth(index as usize)
            .map(|(_, data)| data.as_slice())
    }

    fn bytes_per_sample(&self) -> u64 {
        let AudioDescription {
            format,
            bytes_per_packet,
            frames_per_packet,
            channels_per_frame,
            ..
        } = self.audio_description();
        if !matches!(format, AudioFormat::LinearPcm { .. }) {
            panic!("{format:?} is a compressed format!");
        }
        ((bytes_per_packet / frames_per_packet) / channels_per_frame).into()
    }

    pub fn byte_count(&self) -> u64 {
        match self.0 {
            AudioFileInner::Wave(ref wave_reader) => {
                let sample_count = wave_reader.len(); // position-independent
                u64::from(sample_count) * self.bytes_per_sample()
            }
            AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm {
                ref bytes,
                ..
            }) => bytes.len() as u64,
        }
    }

    pub fn packet_count(&self) -> u64 {
        match self.0 {
            AudioFileInner::Wave(_)
            | AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm { .. }) => {
                // never variable-size
                self.byte_count() / u64::from(self.packet_size_fixed())
            }
        }
    }

    /// Returns the packet size if this audio format has a constant packet size,
    /// panics if not.
    pub fn packet_size_fixed(&self) -> u32 {
        let AudioDescription {
            bytes_per_packet, ..
        } = self.audio_description();
        assert!(bytes_per_packet != 0);
        bytes_per_packet
    }

    pub fn packet_size_upper_bound(&self) -> u32 {
        self.packet_size_fixed() // variable size not implemented
    }

    /// Read `buffer.len()` bytes of audio data from byte offset `offset`.
    /// Returns the number of bytes read.
    pub fn read_bytes(&mut self, offset: u64, buffer: &mut [u8]) -> Result<usize, ()> {
        match self.0 {
            AudioFileInner::Wave(_) => {
                let bytes_per_sample = self.bytes_per_sample();
                assert!(offset.is_multiple_of(bytes_per_sample));
                assert!(u64::try_from(buffer.len())
                    .unwrap()
                    .is_multiple_of(bytes_per_sample));

                let sample_count = u64::try_from(buffer.len()).unwrap() / bytes_per_sample;
                let sample_count: usize = sample_count.try_into().unwrap();

                let AudioFileInner::Wave(ref mut wave_reader) = self.0 else {
                    unreachable!()
                };

                let channels: u64 = wave_reader.spec().channels.into();
                // WavReader expects number of samples which are
                // independent of the number of channels here
                wave_reader
                    .seek((offset / (bytes_per_sample * channels)).try_into().unwrap())
                    .map_err(|_| ())?;

                let mut byte_offset = 0;
                for sample in wave_reader.samples().take(sample_count) {
                    let sample: i16 = sample.map_err(|_| ())?;
                    match bytes_per_sample {
                        // From the OpenAL docs: 8-bit PCM data is expressed as
                        // an unsigned value over the range 0 to 255, 128 being
                        // an audio output level of zero. Loaded wav samples
                        // must be converted to that from signed with 0 as
                        // output level 0.
                        1 => buffer[byte_offset] = (sample + 128) as u8,
                        2 => buffer[byte_offset..][..2].copy_from_slice(&sample.to_le_bytes()),
                        _ => todo!(),
                    }
                    byte_offset += bytes_per_sample as usize;
                }
                Ok(byte_offset)
            }
            AudioFileInner::Symphonia(symphonia_formats::SymphoniaDecodedToPcm {
                ref bytes,
                ..
            }) => {
                let bytes = bytes.get(offset as usize..).ok_or(())?;
                let bytes_to_read = buffer.len().min(bytes.len());
                let bytes = &bytes[..bytes_to_read];
                buffer[..bytes_to_read].copy_from_slice(bytes);
                Ok(bytes_to_read)
            }
        }
    }

    pub fn estimated_duration(&self) -> f64 {
        let AudioDescription {
            sample_rate,
            bytes_per_packet,
            frames_per_packet,
            ..
        } = self.audio_description();
        assert!(bytes_per_packet != 0); // TODO
        self.byte_count() as f64 * frames_per_packet as f64
            / (bytes_per_packet as f64 * sample_rate)
    }
}

pub mod sound_test;

pub mod sound_trace;

#[cfg(test)]
mod metadata_tests {
    use super::*;

    #[test]
    fn large_id3_tag_does_not_hide_mp3_audio() {
        // Synthetic MPEG-1 Layer III silent frames, no game asset involved.
        let mut frames = Vec::new();
        for _ in 0..4 {
            frames.extend_from_slice(&[0xff, 0xfb, 0x90, 0x64]);
            frames.extend_from_slice(&[0; 413]);
        }
        let reference = decode_audio_track_to_pcm(frames.clone()).unwrap();
        let size = 1_048_600u32;
        let mut tagged = b"ID3\x04\0\0".to_vec();
        for shift in [21, 14, 7, 0] {
            tagged.push(((size >> shift) & 0x7f) as u8);
        }
        tagged.resize(10 + size as usize, 0);
        tagged.extend_from_slice(&frames);
        let decoded = decode_audio_track_to_pcm(tagged).unwrap();
        assert!(!decoded.0.is_empty());
        assert_eq!(decoded, reference);
    }

    #[test]
    fn aiff_big_endian_samples_decode_to_little_endian_pcm() {
        let mut file = b"FORM\0\0\0\x32AIFFCOMM\0\0\0\x12".to_vec();
        file.extend_from_slice(b"\0\x01\0\0\0\x02\0\x10");
        // IEEE 80-bit extended value for 44100 Hz.
        file.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
        file.extend_from_slice(b"SSND\0\0\0\x0c\0\0\0\0\0\0\0\0\x12\x34\xfe\xdc");
        let mut audio = AudioFile::read_from_vec(file).unwrap();
        assert_eq!(audio.audio_description().sample_rate, 44100.0);
        assert_eq!(audio.audio_description().channels_per_frame, 1);
        let mut output = [0; 4];
        assert_eq!(audio.read_bytes(0, &mut output), Ok(4));
        assert_eq!(output, [0x34, 0x12, 0xdc, 0xfe]);
    }

    fn chunk(file: &mut Vec<u8>, id: &[u8; 4], payload: &[u8]) {
        file.extend_from_slice(id);
        file.extend_from_slice(&(payload.len() as i64).to_be_bytes());
        file.extend_from_slice(payload);
    }

    #[test]
    fn caf_markers_survive_decoding_without_byte_swapping() {
        let mut file = b"caff\0\x01\0\0".to_vec();
        let mut description = 44100.0_f64.to_be_bytes().to_vec();
        description.extend_from_slice(b"lpcm");
        for value in [2_u32, 2, 1, 1, 16] {
            description.extend_from_slice(&value.to_be_bytes());
        }
        chunk(&mut file, b"desc", &description);
        let markers = b"\0\0\0\0\x03\0\0\0gbls\0\0\0\0\xd6\x44\x22\x41";
        chunk(&mut file, b"mark", markers);
        chunk(&mut file, b"free", &[0; 4]);
        chunk(&mut file, b"mark", b"second");
        chunk(&mut file, b"data", &[0; 8]);
        let audio = AudioFile::read_from_vec(file).unwrap();
        assert_eq!(
            audio.user_data(u32::from_be_bytes(*b"mark"), 0),
            Some(markers.as_slice())
        );
        assert_eq!(
            audio.user_data(u32::from_be_bytes(*b"mark"), 1),
            Some(b"second".as_slice())
        );
        assert_eq!(audio.user_data(u32::from_be_bytes(*b"mark"), 2), None);
        assert_eq!(audio.user_data(u32::from_be_bytes(*b"data"), 0), None);
        assert_eq!(audio.user_data(u32::from_be_bytes(*b"free"), 0), None);
    }

    #[test]
    fn caf_metadata_handles_truncated_and_unbounded_chunks() {
        for size in [i64::MAX, -2, -1, 10] {
            let mut file = b"caff\0\x01\0\0mark".to_vec();
            file.extend_from_slice(&size.to_be_bytes());
            assert!(caf_user_data(&file).is_empty());
        }
        assert!(caf_user_data(b"RIFF\0\0\0\0WAVE").is_empty());
        assert!(caf_user_data(b"caff\0\x01\0\0mar").is_empty());
    }
}
