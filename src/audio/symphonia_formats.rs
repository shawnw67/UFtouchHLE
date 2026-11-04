/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Quick-and-dirty decoding of miscellaneous formats (MP3, AAC) to linear PCM.
//!
//! This should be the only module in touchHLE that makes use of [symphonia].
//! For AAC, Only the LC profile and MPEG-4 container format are supported (see
//! feature list in Cargo.toml).

use std::io::Cursor;
use symphonia::core::audio::AudioSpec;
use symphonia::core::audio::Channels;
use symphonia::core::audio::Position;
use symphonia::core::codecs::audio::well_known::{
    CODEC_ID_AAC, CODEC_ID_ADPCM_IMA_QT, CODEC_ID_ADPCM_IMA_WAV, CODEC_ID_ALAC, CODEC_ID_MP3,
    CODEC_ID_PCM_S16BE, CODEC_ID_PCM_S16LE,
};
use symphonia::core::codecs::audio::AudioCodecParameters;
use symphonia::core::codecs::audio::AudioDecoder;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::packet::PacketRef;
use symphonia::core::units::Duration;
use symphonia::core::units::Timestamp;

/// PCM data decoded from an miscellaneous format file.
pub struct SymphoniaDecodedToPcm {
    /// 16-bit little-endian PCM samples, grouped in frames (one sample per
    /// channel in each frame).
    pub bytes: Vec<u8>,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u32,
}

pub fn decode_symphonia_to_pcm(file: Cursor<Vec<u8>>) -> Result<SymphoniaDecodedToPcm, ()> {
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // If this failed, the container format is not supported.
    let mut probed = symphonia::default::get_probe()
        .probe(
            &Default::default(),
            mss,
            Default::default(),
            Default::default(),
        )
        .map_err(|_| ())?;

    let track = probed
        .tracks()
        .iter()
        .find(|t| {
            if let Some(codec_params) = &t.codec_params {
                if let Some(audio_codec_params) = codec_params.audio() {
                    audio_codec_params.codec == CODEC_ID_AAC
                        || audio_codec_params.codec == CODEC_ID_ADPCM_IMA_WAV
                        || audio_codec_params.codec == CODEC_ID_ADPCM_IMA_QT
                        || audio_codec_params.codec == CODEC_ID_ALAC
                        || audio_codec_params.codec == CODEC_ID_MP3
                        || audio_codec_params.codec == CODEC_ID_PCM_S16LE
                        || audio_codec_params.codec == CODEC_ID_PCM_S16BE
                } else {
                    false
                }
            } else {
                false
            }
        })
        .ok_or(())?;
    let track_id = track.id;

    // Not sure why this would fail, maybe an unusual AAC track.
    let audio_codec_params = track.codec_params.as_ref().unwrap().audio().unwrap();
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(audio_codec_params, &Default::default())
        .map_err(|_| ())?;

    let mut out_pcm = Vec::<u8>::new();
    let mut audio_spec: Option<AudioSpec> = None;
    {
        let mut tmp_raw_s16_buf: Option<Vec<u8>> = None;
        loop {
            let packet = match probed.next_packet() {
                Ok(packet) => match packet {
                    Some(packet) => packet,
                    // "If Ok(None) is returned, the media has ended and
                    // no more packets will be produced until the reader
                    // is seeked to a new position."
                    None => break,
                },
                // Assume I/O errors can only mean end-of-file, because the
                // entire file is in-memory.
                Err(symphonia::core::errors::Error::IoError(_)) => break,
                Err(_) => return Err(()),
            };

            if packet.track_id != track_id {
                continue;
            }
            let Ok(decoded_packet) = decoder.decode(&packet) else {
                break;
            };

            // For some reason, the "audio spec" (number of channels etc)
            // is reported per-packet? This is weird because it must be the same
            // for all of them.
            let audio_spec = audio_spec.get_or_insert_with(|| decoded_packet.spec().clone());
            assert_eq!(audio_spec, decoded_packet.spec());

            // Note that this assumes every packet's buffer's capacity is the
            // same, which is a dubious assumption, but Symphonia's own example
            // code does it, so maybe it's fine?
            let tmp_raw_s16_buf = tmp_raw_s16_buf
                .get_or_insert_with(|| Vec::with_capacity(decoded_packet.capacity()));
            tmp_raw_s16_buf.clear();
            decoded_packet.copy_bytes_to_vec_interleaved_as::<i16>(tmp_raw_s16_buf);

            out_pcm.extend_from_slice(tmp_raw_s16_buf);
        }
    }
    let audio_spec = audio_spec.ok_or(())?;
    Ok(SymphoniaDecodedToPcm {
        bytes: out_pcm,
        sample_rate: audio_spec.rate(),
        channels: audio_spec.channels().count().try_into().unwrap(),
    })
}
/// MP3 or AAC decoder fed one frame at a time, keeping its state between
/// frames.
///
/// Layer III frames borrow bits from the frames before them (the "bit
/// reservoir"), and AAC frames overlap-add with the previous frame's MDCT
/// output, so a stream that arrives a frame or two at a time, like the packets
/// an app enqueues on an `AudioQueue`, can't be decoded by starting afresh for
/// each chunk.
#[allow(dead_code)]
pub struct FrameDecoder {
    decoder: Box<dyn AudioDecoder>,
    packet_bytes: Vec<u8>,
}

impl FrameDecoder {
    #[allow(dead_code)]
    pub fn new_mp3() -> Option<Self> {
        let mut params = AudioCodecParameters::new();
        params.for_codec(CODEC_ID_MP3);
        Self::with_params(&params, "MP3")
    }

    /// AAC-LC decoder for raw access units (no ADTS header) of a stream with
    /// the given sample rate and channel count.
    #[allow(dead_code)]
    pub fn new_aac(sample_rate: u32, channels: u32) -> Option<Self> {
        let channels = match channels {
            1 => Position::FRONT_CENTER,
            2 => Position::FRONT_LEFT | Position::FRONT_RIGHT,
            _ => {
                log!("AAC: unsupported channel count {}", channels);
                return None;
            }
        };
        let mut params = AudioCodecParameters::new();
        params
            .for_codec(CODEC_ID_AAC)
            .with_sample_rate(sample_rate)
            .with_channels(Channels::Positioned(channels));
        Self::with_params(&params, "AAC")
    }

    fn with_params(params: &AudioCodecParameters, name: &str) -> Option<Self> {
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(|e| log!("Symphonia failed to create {} decoder: {:?}", name, e))
            .ok()?;
        Some(FrameDecoder {
            decoder,
            packet_bytes: Vec::new(),
        })
    }

    /// Decodes a single frame (an MPEG audio frame with its header, or a raw
    /// AAC access unit) to 16-bit little-endian interleaved PCM. Returns the
    /// PCM with its sample rate and channel count, or [None] if the frame
    /// couldn't be decoded, which is normal for the first frames after a
    /// discontinuity.
    #[allow(dead_code)]
    pub fn decode_frame(&mut self, frame: &[u8]) -> Option<(&[u8], u32, u32)> {
        let packet = PacketRef::new(0, Timestamp::new(0), Duration::new(0), frame);
        let decoder = &mut self.decoder;
        let packet_bytes = &mut self.packet_bytes;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            decoder.decode_ref(&packet).map(|decoded| {
                let spec = decoded.spec();
                let spec_pair = (spec.rate(), spec.channels().count());
                decoded.copy_bytes_to_vec_interleaved_as::<i16>(packet_bytes);
                spec_pair
            })
        }));
        match result {
            Ok(Ok((rate, channels))) => Some((&self.packet_bytes, rate, channels as u32)),
            Ok(Err(e)) => {
                log_dbg!("Audio frame could not be decoded: {:?}", e);
                None
            }
            Err(_) => {
                log!("Symphonia frame decoder panicked; resetting it");
                self.decoder.reset();
                None
            }
        }
    }
}
