/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! `AudioFileStream.h` — incremental ("streaming") parsing of audio files.
//!
//! Apps that play data of unknown length (music players waiting on a download,
//! games whose sound banks are unpacked at runtime) call
//! `AudioFileStreamOpen`, hand every chunk to `AudioFileStreamParseBytes`, and
//! learn about the format through the property listener, and about the audio
//! data through the packets callback. Nothing is decoded here: the packets go
//! straight to an `AudioQueue` or `AudioConverter`, exactly as on iOS.
//!
//! Apple's header documents the contract we follow: the property listener is
//! called as soon as a property's value means something, `kAudioFileStream
//! Property_ReadyToProducePackets` is reported before the first packet
//! callback, and packet descriptions carry the byte offset of each packet in
//! the buffer handed to the callback.
//! <https://developer.apple.com/documentation/audiotoolbox/audio_file_stream_services>

use super::audio_converter::AudioStreamPacketDescription;
use crate::abi::{CallFromHost, GuestFunction};
use crate::audio::{AudioFile, AudioFormat};
use crate::environment::Environment;
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::{debug_fourcc, fourcc, AudioStreamBasicDescription};
use crate::mem::{guest_size_of, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};
use std::collections::HashMap;

pub type AudioFileStreamID = MutPtr<OpaqueAudioFileStreamID>;
type AudioFileTypeID = u32;

/// kAudioFileStreamError_UnsupportedFileType
const kAudioFileStreamError_UnsupportedFileType: OSStatus = fourcc(b"typ?") as i32;
/// kAudioFileStreamError_UnsupportedDataFormat
const kAudioFileStreamError_UnsupportedDataFormat: OSStatus = fourcc(b"fmt?") as i32;
/// kAudioFileStreamError_PropertyNotSupported
const kAudioFileStreamError_PropertyNotSupported: OSStatus = fourcc(b"prp?") as i32;

pub type AudioFileStreamPropertyID = u32;
const kAudioFileStreamProperty_ReadyToProducePackets: AudioFileStreamPropertyID = fourcc(b"redy");
const kAudioFileStreamProperty_FileFormat: AudioFileStreamPropertyID = fourcc(b"ffmt");
const kAudioFileStreamProperty_DataFormat: AudioFileStreamPropertyID = fourcc(b"dfmt");
const kAudioFileStreamProperty_FormatList: AudioFileStreamPropertyID = fourcc(b"flst");
const kAudioFileStreamProperty_AudioDataByteCount: AudioFileStreamPropertyID = fourcc(b"bcnt");
const kAudioFileStreamProperty_AudioDataPacketCount: AudioFileStreamPropertyID = fourcc(b"pcnt");
const kAudioFileStreamProperty_MaximumPacketSize: AudioFileStreamPropertyID = fourcc(b"psze");
const kAudioFileStreamProperty_PacketSizeUpperBound: AudioFileStreamPropertyID = fourcc(b"pksz");
const kAudioFileStreamProperty_MagicCookieData: AudioFileStreamPropertyID = fourcc(b"mgc ");
const kAudioFileStreamProperty_BitRate: AudioFileStreamPropertyID = fourcc(b"brat");

/// Apple `AudioFileStreamPropertyFlags`.
const kAudioFileStreamPropertyFlag_PropertyIsCached: u32 = 1;
#[allow(dead_code)]
const kAudioFileStreamPropertyFlag_CacheProperty: u32 = 2;

type AudioFileStreamPropertyListenerProc = GuestFunction;
type AudioFileStreamPacketsProc = GuestFunction;

#[repr(C, packed)]
pub struct OpaqueAudioFileStreamID {
    _pad: u8,
}
unsafe impl SafeRead for OpaqueAudioFileStreamID {}

/// What `AudioFileStreamParseBytes` learns from the bytes seen so far.
#[derive(Clone)]
struct ParsedStream {
    /// The packets found so far. Re-parsed as more data arrives, because a
    /// later chunk can complete a packet that was cut in half.
    packets: crate::audio::CompressedPackets,
    description: AudioStreamBasicDescription,
    file_type: AudioFileTypeID,
    /// Set once the property listener has been told about it, so each
    /// property is reported exactly once.
    reported: ReportedProperties,
}

#[derive(Default, Clone)]
struct ReportedProperties {
    data_format: bool,
    file_format: bool,
    magic_cookie: bool,
    bit_rate: bool,
    ready_to_produce_packets: bool,
}

pub struct AudioFileStreamHostObject {
    client_data: MutVoidPtr,
    property_listener: Option<AudioFileStreamPropertyListenerProc>,
    packets_proc: Option<AudioFileStreamPacketsProc>,
    #[allow(dead_code)]
    file_type_hint: AudioFileTypeID,
    /// Everything the app has parsed so far. The parser is incremental in the
    /// sense that it re-reads this buffer, not in the sense that it keeps a
    /// partially consumed parse state.
    buffer: Vec<u8>,
    parsed: Option<ParsedStream>,
    /// Index of the first packet the app has not been given yet.
    next_packet: u64,
    /// A magic cookie the app supplied itself, for formats where the cookie
    /// is not inside the stream (the app gets it from elsewhere, e.g. an
    /// `AudioFile`).
    client_magic_cookie: Option<Vec<u8>>,
}

#[derive(Default)]
pub struct State {
    pub audio_file_streams: HashMap<AudioFileStreamID, AudioFileStreamHostObject>,
}
impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.audio_file_stream
    }
}

fn registration(
    env: &mut Environment,
    stream: AudioFileStreamID,
) -> Option<&mut AudioFileStreamHostObject> {
    State::get(&mut env.framework_state)
        .audio_file_streams
        .get_mut(&stream)
}

/// Reports a property to the app's listener, with the flags iOS uses: a
/// property the app asked to have cached comes back flagged as cached.
fn notify_property(
    env: &mut Environment,
    stream: AudioFileStreamID,
    property_id: AudioFileStreamPropertyID,
) {
    let Some(host_object) = registration(env, stream) else {
        return;
    };
    let (Some(proc), user_data) = (host_object.property_listener, host_object.client_data) else {
        return;
    };
    let flags = kAudioFileStreamPropertyFlag_PropertyIsCached;
    <GuestFunction as CallFromHost<(), (MutVoidPtr, AudioFileStreamID, u32, u32)>>::call_from_host(
        &proc,
        env,
        (user_data, stream, property_id, flags),
    );
}

fn take_magic_cookie(host_object: &AudioFileStreamHostObject) -> Option<Vec<u8>> {
    if let Some(cookie) = &host_object.client_magic_cookie {
        return Some(cookie.clone());
    }
    let parsed = host_object.parsed.as_ref()?;
    if parsed.packets.magic_cookie.is_empty() {
        None
    } else {
        Some(parsed.packets.magic_cookie.clone())
    }
}

pub fn AudioFileStreamOpen(
    env: &mut Environment,
    in_client_data: MutVoidPtr,
    in_property_listener_proc: MutVoidPtr,
    in_packets_proc: MutVoidPtr,
    in_file_type_hint: AudioFileTypeID,
    out_audio_file_stream: MutPtr<AudioFileStreamID>,
) -> OSStatus {
    let property_listener = if in_property_listener_proc.is_null() {
        None
    } else {
        Some(GuestFunction::from_addr_with_thumb_bit(
            in_property_listener_proc.to_bits(),
        ))
    };
    let packets_proc = if in_packets_proc.is_null() {
        None
    } else {
        Some(GuestFunction::from_addr_with_thumb_bit(
            in_packets_proc.to_bits(),
        ))
    };

    let stream: AudioFileStreamID = env.mem.alloc_and_write(OpaqueAudioFileStreamID { _pad: 0 });
    State::get(&mut env.framework_state)
        .audio_file_streams
        .insert(
            stream,
            AudioFileStreamHostObject {
                client_data: in_client_data,
                property_listener,
                packets_proc,
                file_type_hint: in_file_type_hint,
                buffer: Vec::new(),
                parsed: None,
                next_packet: 0,
                client_magic_cookie: None,
            },
        );
    env.mem.write(out_audio_file_stream, stream);

    log_dbg!(
        "AudioFileStreamOpen(client_data={:?}, hint={}) -> {:?}",
        in_client_data,
        debug_fourcc(in_file_type_hint),
        stream
    );
    0
}

pub fn AudioFileStreamClose(
    env: &mut Environment,
    in_audio_file_stream: AudioFileStreamID,
) -> OSStatus {
    if in_audio_file_stream.is_null() {
        return kAudioFileStreamError_UnsupportedFileType;
    }
    State::get(&mut env.framework_state)
        .audio_file_streams
        .remove(&in_audio_file_stream);
    env.mem.free(in_audio_file_stream.cast());
    0
}

/// Builds the ASBD the stream is described with. Mirrors what
/// `AudioFileGetProperty(kAudioFilePropertyDataFormat)` reports for the same
/// file (see `audio_file.rs`), so an app that switches from one API to the
/// other sees the same format.
fn describe_format(description: &crate::audio::AudioDescription) -> AudioStreamBasicDescription {
    // `CoreAudioTypes.h`: for linear PCM a packet is one frame, so the frame
    // and packet byte sizes are the same and neither may be zero. Compressed
    // formats keep a zero frame size because frames are not byte-aligned.
    let is_linear_pcm = matches!(description.format, AudioFormat::LinearPcm { .. });
    AudioStreamBasicDescription {
        sample_rate: description.sample_rate,
        format_id: match description.format {
            AudioFormat::LinearPcm { .. } => fourcc(b"lpcm"),
        },
        format_flags: 0,
        bytes_per_packet: description.bytes_per_packet,
        frames_per_packet: if is_linear_pcm {
            1
        } else {
            description.frames_per_packet
        },
        bytes_per_frame: if is_linear_pcm {
            description.bytes_per_packet
        } else {
            0
        },
        channels_per_frame: description.channels_per_frame,
        bits_per_channel: description.bits_per_channel,
        _reserved: 0,
    }
}

/// Delivers every packet completed since the last call to `ParseBytes`.
fn deliver_packets(env: &mut Environment, stream: AudioFileStreamID) {
    // Take the stream out of the map for the duration of the loop: the
    // callbacks run guest code, and holding a borrow of `env.framework_state`
    // would make `env.mem` unusable.
    let Some(mut host_object) = State::get(&mut env.framework_state)
        .audio_file_streams
        .remove(&stream)
    else {
        return;
    };
    deliver_packets_inner(env, &mut host_object);
    State::get(&mut env.framework_state)
        .audio_file_streams
        .insert(stream, host_object);
}

fn deliver_packets_inner(env: &mut Environment, host_object: &mut AudioFileStreamHostObject) {
    let Some(proc) = host_object.packets_proc else {
        return;
    };
    let Some(parsed) = host_object.parsed.clone() else {
        return;
    };
    let client_data = host_object.client_data;
    let total_packets = parsed.packets.packet_count();
    let mut index = host_object.next_packet;
    while index < total_packets {
        // One callback per run of packets that are contiguous in the buffer,
        // which is how iOS batches them too.
        let Some((start, _)) = parsed.packets.packet_info(index) else {
            break;
        };
        let mut run_end = index;
        let mut run_bytes = 0u32;
        let mut descriptions = Vec::new();
        while run_end < total_packets {
            let Some((offset, size)) = parsed.packets.packet_info(run_end) else {
                break;
            };
            descriptions.push(AudioStreamPacketDescription {
                mStartOffset: (offset - start) as i64,
                mVariableFramesInPacket: 0,
                mDataByteSize: size,
            });
            run_bytes += size;
            run_end += 1;
        }
        if descriptions.is_empty() {
            break;
        }
        let packet_count = descriptions.len() as u32;
        let data = parsed
            .packets
            .packet_bytes
            .get(start as usize..(start + u64::from(run_bytes)) as usize)
            .unwrap_or_default();
        // The data must live in guest memory: the callback receives a plain
        // pointer, and iOS only guarantees it for the duration of the call.
        let guest_data: MutPtr<u8> = env.mem.alloc(data.len() as u32).cast();
        env.mem
            .bytes_at_mut(guest_data, data.len() as u32)
            .copy_from_slice(data);
        let guest_descriptions = if desc_is_uniform(&descriptions) {
            // iOS passes NULL when every packet has the same size, and the
            // size is then `number_bytes / number_packets`.
            ConstVoidPtr::null()
        } else {
            let notes: MutPtr<AudioStreamPacketDescription> = env
                .mem
                .alloc(descriptions.len() as u32 * guest_size_of::<AudioStreamPacketDescription>())
                .cast();
            for (index, description) in descriptions.iter().enumerate() {
                env.mem.write(notes + index as u32, *description);
            }
            notes.cast_const().cast()
        };

        <GuestFunction as CallFromHost<
            (),
            (MutVoidPtr, u32, u32, ConstVoidPtr, ConstVoidPtr),
        >>::call_from_host(
            &proc,
            env,
            (
                client_data,
                run_bytes,
                packet_count,
                guest_data.cast_const().cast(),
                guest_descriptions,
            ),
        );

        env.mem.free(guest_data.cast());
        if !guest_descriptions.is_null() {
            env.mem.free(guest_descriptions.cast().cast_mut());
        }
        host_object.next_packet = run_end;
        index = run_end;
    }
}

fn desc_is_uniform(descriptions: &[AudioStreamPacketDescription]) -> bool {
    let Some(first) = descriptions.first() else {
        return true;
    };
    let size = first.mDataByteSize;
    descriptions.iter().all(|desc| desc.mDataByteSize == size)
}

pub fn AudioFileStreamParseBytes(
    env: &mut Environment,
    in_audio_file_stream: AudioFileStreamID,
    in_data_byte_size: u32,
    in_data: ConstVoidPtr,
    _in_discontinuity: u32,
) -> OSStatus {
    let data = env.mem.bytes_at(in_data.cast(), in_data_byte_size).to_vec();

    let Some(host_object) = registration(env, in_audio_file_stream) else {
        return kAudioFileStreamError_UnsupportedFileType;
    };
    host_object.buffer.extend_from_slice(&data);

    // Re-parse the whole buffer: an incomplete packet at the end of the last
    // chunk is completed by the next one, so the parse has to start over.
    let buffer = host_object.buffer.clone();
    let Ok(file) = AudioFile::read_from_vec(buffer) else {
        // The header may simply not have arrived yet; nothing to report.
        return 0;
    };
    let Some(packets) = file.compressed_packets().cloned() else {
        log!(
            "AudioFileStreamParseBytes: audiostream data is {} and is decoded up \
             front, not packetised; cannot report packets",
            debug_fourcc(describe_format(&file.audio_description()).format_id)
        );
        return kAudioFileStreamError_UnsupportedDataFormat;
    };
    let description = file.audio_description();
    let file_type = match description.format {
        AudioFormat::LinearPcm { .. } => fourcc(b"lpcm"),
    };

    let host_object = registration(env, in_audio_file_stream).unwrap();
    let first_parse = host_object.parsed.is_none();
    host_object.parsed = Some(ParsedStream {
        packets,
        description: describe_format(&description),
        file_type,
        reported: if first_parse {
            Default::default()
        } else {
            host_object
                .parsed
                .as_ref()
                .map(|parsed| ReportedProperties {
                    data_format: parsed.reported.data_format,
                    file_format: parsed.reported.file_format,
                    magic_cookie: parsed.reported.magic_cookie,
                    bit_rate: parsed.reported.bit_rate,
                    ready_to_produce_packets: parsed.reported.ready_to_produce_packets,
                })
                .unwrap_or_default()
        },
    });

    // Report the properties in the order iOS does: format first, because the
    // app sizes its buffers from the ASBD, then the cookie (needed by the
    // decoder), then the bit rate, and ReadyToProducePackets last.
    report_once(
        env,
        in_audio_file_stream,
        |reported| &mut reported.data_format,
        kAudioFileStreamProperty_DataFormat,
    );
    report_once(
        env,
        in_audio_file_stream,
        |reported| &mut reported.file_format,
        kAudioFileStreamProperty_FileFormat,
    );
    if take_magic_cookie(registration(env, in_audio_file_stream).unwrap()).is_some() {
        report_once(
            env,
            in_audio_file_stream,
            |reported| &mut reported.magic_cookie,
            kAudioFileStreamProperty_MagicCookieData,
        );
    }
    report_once(
        env,
        in_audio_file_stream,
        |reported| &mut reported.bit_rate,
        kAudioFileStreamProperty_BitRate,
    );
    report_once(
        env,
        in_audio_file_stream,
        |reported| &mut reported.ready_to_produce_packets,
        kAudioFileStreamProperty_ReadyToProducePackets,
    );

    deliver_packets(env, in_audio_file_stream);
    0
}

fn report_once(
    env: &mut Environment,
    stream: AudioFileStreamID,
    reported: impl FnOnce(&mut ReportedProperties) -> &mut bool,
    property_id: AudioFileStreamPropertyID,
) {
    let Some(host_object) = registration(env, stream) else {
        return;
    };
    let Some(parsed) = host_object.parsed.as_mut() else {
        return;
    };
    let flag = reported(&mut parsed.reported);
    if *flag {
        return;
    }
    *flag = true;
    notify_property(env, stream, property_id);
}

fn bit_rate(parsed: &ParsedStream) -> u32 {
    let seconds = if parsed.description.sample_rate > 0.0 {
        parsed.packets.packet_count() as f64 * parsed.description.frames_per_packet as f64
            / parsed.description.sample_rate
    } else {
        0.0
    };
    if seconds > 0.0 {
        (parsed.packets.byte_count() as f64 * 8.0 / seconds) as u32
    } else {
        0
    }
}

fn property_size(
    host_object: &AudioFileStreamHostObject,
    property_id: AudioFileStreamPropertyID,
) -> Option<u32> {
    let parsed = host_object.parsed.as_ref();
    let size = match property_id {
        kAudioFileStreamProperty_FileFormat
        | kAudioFileStreamProperty_BitRate
        | kAudioFileStreamProperty_PacketSizeUpperBound
        | kAudioFileStreamProperty_MaximumPacketSize
        | kAudioFileStreamProperty_ReadyToProducePackets => 4,
        kAudioFileStreamProperty_AudioDataByteCount
        | kAudioFileStreamProperty_AudioDataPacketCount => 8,
        kAudioFileStreamProperty_DataFormat => {
            std::mem::size_of::<AudioStreamBasicDescription>() as u32
        }
        // A FormatList is one FormatListItem: an ASBD plus the channel layout
        // tag that follows it.
        kAudioFileStreamProperty_FormatList => {
            (std::mem::size_of::<AudioStreamBasicDescription>() + 4) as u32
        }
        kAudioFileStreamProperty_MagicCookieData => {
            return Some(take_magic_cookie(host_object)?.len() as u32)
        }
        _ => return None,
    };
    parsed?;
    Some(size)
}

fn is_writable_property(property_id: AudioFileStreamPropertyID) -> bool {
    // Apple lets the app correct these two: the cookie for streams that carry
    // it out of band, and the bit rate for what it is about to play.
    matches!(
        property_id,
        kAudioFileStreamProperty_MagicCookieData | kAudioFileStreamProperty_BitRate
    )
}

pub fn AudioFileStreamGetPropertyInfo(
    env: &mut Environment,
    in_audio_file_stream: AudioFileStreamID,
    in_property_id: AudioFileStreamPropertyID,
    out_property_data_size: MutPtr<u32>,
    out_writable: MutPtr<u8>,
) -> OSStatus {
    let Some(host_object) = registration(env, in_audio_file_stream) else {
        return kAudioFileStreamError_UnsupportedFileType;
    };
    let Some(size) = property_size(host_object, in_property_id) else {
        return kAudioFileStreamError_PropertyNotSupported;
    };
    env.mem.write(out_property_data_size, size);
    if !out_writable.is_null() {
        env.mem
            .write(out_writable, u8::from(is_writable_property(in_property_id)));
    }
    0
}

/// What `AudioFileStreamGetProperty` should hand back. The value is computed
/// while the stream is borrowed, then written after the borrow ends, because
/// `env.mem` and `env.framework_state` cannot both be borrowed at once.
fn property_is_known(property_id: AudioFileStreamPropertyID) -> bool {
    matches!(
        property_id,
        kAudioFileStreamProperty_FileFormat
            | kAudioFileStreamProperty_DataFormat
            | kAudioFileStreamProperty_FormatList
            | kAudioFileStreamProperty_AudioDataByteCount
            | kAudioFileStreamProperty_AudioDataPacketCount
            | kAudioFileStreamProperty_MaximumPacketSize
            | kAudioFileStreamProperty_PacketSizeUpperBound
            | kAudioFileStreamProperty_MagicCookieData
            | kAudioFileStreamProperty_BitRate
            | kAudioFileStreamProperty_ReadyToProducePackets
    )
}

fn read_property(
    host_object: &AudioFileStreamHostObject,
    property_id: AudioFileStreamPropertyID,
) -> Option<Vec<u8>> {
    // Before the header is parsed every property reads as zero, which is what
    // an app polling them early sees on iOS.
    let parsed = host_object.parsed.as_ref();
    match property_id {
        kAudioFileStreamProperty_FileFormat => Some(
            parsed
                .map_or(0u32, |parsed| parsed.file_type)
                .to_le_bytes()
                .to_vec(),
        ),
        kAudioFileStreamProperty_DataFormat => Some(
            parsed
                .map_or([0u8; 40], |parsed| asbd_bytes(&parsed.description))
                .to_vec(),
        ),
        kAudioFileStreamProperty_FormatList => {
            let Some(parsed) = parsed else {
                return Some(Vec::new());
            };
            let mut data = asbd_bytes(&parsed.description).to_vec();
            // kAudioChannelLayoutTag_UseChannelDescriptions is zero, and a
            // layout tag of zero means "use the ASBD": what iOS reports for a
            // plain AAC/MP3 stream.
            data.extend_from_slice(&0u32.to_le_bytes());
            Some(data)
        }
        kAudioFileStreamProperty_AudioDataByteCount => Some(
            parsed
                .map_or(0u64, |p| p.packets.byte_count())
                .to_le_bytes()
                .to_vec(),
        ),
        kAudioFileStreamProperty_AudioDataPacketCount => Some(
            parsed
                .map_or(0u64, |p| p.packets.packet_count())
                .to_le_bytes()
                .to_vec(),
        ),
        kAudioFileStreamProperty_PacketSizeUpperBound
        | kAudioFileStreamProperty_MaximumPacketSize => Some(
            parsed
                .map_or(0u32, |p| p.packets.packet_size_upper_bound())
                .to_le_bytes()
                .to_vec(),
        ),
        kAudioFileStreamProperty_MagicCookieData => {
            let cookie = parsed
                .map(|p| p.packets.magic_cookie.clone())
                .or_else(|| host_object.client_magic_cookie.clone())?;
            Some(cookie)
        }
        kAudioFileStreamProperty_BitRate => {
            Some(parsed.map_or(0u32, bit_rate).to_le_bytes().to_vec())
        }
        kAudioFileStreamProperty_ReadyToProducePackets => {
            Some(u32::from(parsed.is_some()).to_le_bytes().to_vec())
        }
        _ => None,
    }
}

pub fn AudioFileStreamGetProperty(
    env: &mut Environment,
    in_audio_file_stream: AudioFileStreamID,
    in_property_id: AudioFileStreamPropertyID,
    io_property_data_size: MutPtr<u32>,
    out_property_data: MutVoidPtr,
) -> OSStatus {
    if !property_is_known(in_property_id) {
        log!(
            "AudioFileStreamGetProperty: property {} is not implemented",
            debug_fourcc(in_property_id)
        );
        return kAudioFileStreamError_PropertyNotSupported;
    }
    let Some(reply) = registration(env, in_audio_file_stream)
        .and_then(|host_object| read_property(host_object, in_property_id))
    else {
        return kAudioFileStreamError_PropertyNotSupported;
    };

    let size = env.mem.read(io_property_data_size) as usize;
    let n = size.min(reply.len());
    env.mem
        .bytes_at_mut(out_property_data.cast(), n as u32)
        .copy_from_slice(&reply[..n]);
    env.mem.write(io_property_data_size, n as u32);
    0
}

fn asbd_bytes(description: &AudioStreamBasicDescription) -> [u8; 40] {
    let mut bytes = [0u8; 40];
    bytes[0..4].copy_from_slice(&description.sample_rate.to_le_bytes());
    bytes[4..8].copy_from_slice(&description.format_id.to_le_bytes());
    bytes[8..12].copy_from_slice(&description.format_flags.to_le_bytes());
    bytes[12..16].copy_from_slice(&description.bytes_per_packet.to_le_bytes());
    bytes[16..20].copy_from_slice(&description.frames_per_packet.to_le_bytes());
    bytes[20..24].copy_from_slice(&description.bytes_per_frame.to_le_bytes());
    bytes[24..28].copy_from_slice(&description.channels_per_frame.to_le_bytes());
    bytes[28..32].copy_from_slice(&description.bits_per_channel.to_le_bytes());
    bytes
}

pub fn AudioFileStreamSetProperty(
    env: &mut Environment,
    in_audio_file_stream: AudioFileStreamID,
    in_property_id: AudioFileStreamPropertyID,
    in_property_data_size: u32,
    in_property_data: ConstVoidPtr,
) -> OSStatus {
    let data = env
        .mem
        .bytes_at(in_property_data.cast(), in_property_data_size)
        .to_vec();
    let Some(host_object) = registration(env, in_audio_file_stream) else {
        return kAudioFileStreamError_UnsupportedFileType;
    };
    match in_property_id {
        kAudioFileStreamProperty_MagicCookieData => {
            host_object.client_magic_cookie = Some(data);
            0
        }
        kAudioFileStreamProperty_BitRate | kAudioFileStreamProperty_AudioDataPacketCount => {
            // Advisory: nothing in the pipeline uses a corrected count yet.
            0
        }
        _ => {
            log!(
                "AudioFileStreamSetProperty: property {} is not writable",
                debug_fourcc(in_property_id)
            );
            kAudioFileStreamError_PropertyNotSupported
        }
    }
}

pub const FUNCTIONS: crate::dyld::FunctionExports = &[
    crate::dyld::export_c_func!(AudioFileStreamOpen(_, _, _, _, _)),
    crate::dyld::export_c_func!(AudioFileStreamClose(_)),
    crate::dyld::export_c_func!(AudioFileStreamParseBytes(_, _, _, _)),
    crate::dyld::export_c_func!(AudioFileStreamGetPropertyInfo(_, _, _, _)),
    crate::dyld::export_c_func!(AudioFileStreamGetProperty(_, _, _, _)),
    crate::dyld::export_c_func!(AudioFileStreamSetProperty(_, _, _, _)),
];
