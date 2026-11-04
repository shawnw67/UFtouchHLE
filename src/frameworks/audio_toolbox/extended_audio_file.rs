/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ExtendedAudioFile.h` (Extended Audio File Services)
//!
//! Currently implemented as a wrapper around Audio File Services. The only
//! format conversion supported is from IMA4 to 16-bit PCM, which is done by
//! decoding the whole file when the client format is set.

use super::audio_file::{
    kAudioFileBadPropertySizeError, kAudioFilePropertyDataFormat, kAudioFileReadPermission,
    property_size, AudioFileClose, AudioFileCreateWithURL, AudioFileGetProperty, AudioFileID,
    AudioFileOpenURL, AudioFileReadBytes,
};
use super::audio_queue::is_supported_audio_format;
use super::audio_unit::AudioBufferList;
use crate::audio::decode_ima4;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::{eofErr, paramErr, OSStatus};
use crate::frameworks::core_audio_types::{
    debug_fourcc, fourcc, kAudioFormatAppleIMA4, kAudioFormatLinearPCM, AudioStreamBasicDescription,
};
use crate::frameworks::core_foundation::cf_url::CFURLRef;
use crate::frameworks::foundation::ns_url::to_rust_path;
use crate::mem::{guest_size_of, ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    pub extended_audio_files: HashMap<ExtAudioFileRef, ExtAudioFileHostObject>,
}
impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.extended_audio_file
    }
}

pub struct ExtAudioFileHostObject {
    guest_audio_file: AudioFileID,
    /// The guest owns [Self::guest_audio_file], so don't close it on dispose.
    wrapped: bool,
    client_data_format: Option<AudioStreamBasicDescription>,
    /// Position in the data in the client format.
    current_bytes_read: i64,
    /// The whole file in the client format, if it needs converting.
    decoded: Option<Vec<u8>>,
}

#[repr(C, packed)]
pub struct OpaqueExtAudioFile {
    _filler: u8,
}
unsafe impl SafeRead for OpaqueExtAudioFile {}

type ExtAudioFileRef = MutPtr<OpaqueExtAudioFile>;

/// Usually a FourCC.
type ExtAudioFilePropertyID = u32;
const kExtAudioFileProperty_FileDataFormat: ExtAudioFilePropertyID = fourcc(b"ffmt");
const kExtAudioFileProperty_ClientDataFormat: ExtAudioFilePropertyID = fourcc(b"cfmt");
const kExtAudioFileProperty_FileLengthFrames: ExtAudioFilePropertyID = fourcc(b"#frm");

const kExtAudioFileError_InvalidProperty: OSStatus = fourcc(b"pty?") as _;

fn file_data_format(env: &mut Environment, file: ExtAudioFileRef) -> AudioStreamBasicDescription {
    let guest_audio_file =
        State::get(&mut env.framework_state).extended_audio_files[&file].guest_audio_file;
    env.framework_state.audio_toolbox.audio_file.audio_files[&guest_audio_file].data_format()
}

/// Format the app reads data in.
fn client_data_format(env: &mut Environment, file: ExtAudioFileRef) -> AudioStreamBasicDescription {
    match State::get(&mut env.framework_state).extended_audio_files[&file].client_data_format {
        Some(format) => format,
        None => file_data_format(env, file),
    }
}

/// Decode a whole IMA4 file to interleaved 16-bit PCM.
fn decode_ima4_file(env: &mut Environment, file: ExtAudioFileRef, channels: u32) -> Vec<u8> {
    let guest_audio_file =
        State::get(&mut env.framework_state).extended_audio_files[&file].guest_audio_file;
    let audio_file = &mut env
        .framework_state
        .audio_toolbox
        .audio_file
        .audio_files
        .get_mut(&guest_audio_file)
        .unwrap()
        .as_real_mut()
        .unwrap();
    let mut data = vec![0u8; audio_file.byte_count() as usize];
    let read = audio_file.read_bytes(0, &mut data).unwrap();
    data.truncate(read);
    let packets: Vec<[i16; 64]> = data.as_chunks::<34>().0.iter().map(decode_ima4).collect();
    let mut pcm = Vec::with_capacity(packets.len() * 64 * 2);
    // Stereo IMA4 alternates between packets for the left and right channel.
    for frame_packets in packets.chunks_exact(channels as usize) {
        for i in 0..64 {
            for channel_packet in frame_packets {
                pcm.extend_from_slice(&channel_packet[i].to_le_bytes());
            }
        }
    }
    pcm
}

fn ExtAudioFileOpenURL(
    env: &mut Environment,
    in_url: CFURLRef,
    out_ext_audio_file: MutPtr<ExtAudioFileRef>,
) -> OSStatus {
    return_if_null!(in_url);

    let path = to_rust_path(env, in_url);
    log_dbg!(
        "ExtAudioFileOpenURL({:?} '{:?}', {:?})",
        in_url,
        path,
        out_ext_audio_file
    );

    let audio_file_ptr: MutPtr<AudioFileID> = env.mem.alloc(guest_size_of::<AudioFileID>()).cast();
    let res = AudioFileOpenURL(env, in_url, kAudioFileReadPermission, 0, audio_file_ptr);
    let guest_audio_file = env.mem.read(audio_file_ptr);
    env.mem.free(audio_file_ptr.cast());
    if res != 0 {
        log!(
            "ExtAudioFileOpenURL({:?} '{:?}', {:?}) failed, error: {:?}",
            in_url,
            path,
            out_ext_audio_file,
            res
        );
        return res;
    }

    register_ext_audio_file(env, guest_audio_file, false, out_ext_audio_file)
}

fn ExtAudioFileGetProperty(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_property_id: ExtAudioFilePropertyID,
    io_property_data_size: MutPtr<u32>,
    out_property_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);

    if in_property_id == kExtAudioFileProperty_FileLengthFrames {
        if env.mem.read(io_property_data_size) != guest_size_of::<i64>() {
            return kAudioFileBadPropertySizeError;
        }
        let format = file_data_format(env, in_ext_audio_file);
        let guest_audio_file = State::get(&mut env.framework_state).extended_audio_files
            [&in_ext_audio_file]
            .guest_audio_file;
        let audio_file = &env.framework_state.audio_toolbox.audio_file.audio_files
            [&guest_audio_file]
            .as_real()
            .unwrap();
        let frames = if format.format_id == kAudioFormatAppleIMA4 {
            // Both channels' packets count as one frame.
            audio_file.packet_count() as i64 * 64 / format.channels_per_frame as i64
        } else {
            audio_file.byte_count() as i64 / format.bytes_per_frame as i64
        };
        env.mem.write(out_property_data.cast(), frames);
        return 0;
    }

    let audio_file_property_id = match in_property_id {
        kExtAudioFileProperty_FileDataFormat => kAudioFilePropertyDataFormat,
        _ => unimplemented!(
            "Unimplemented property ID: {}",
            debug_fourcc(in_property_id)
        ),
    };

    let required_size = property_size(audio_file_property_id);
    if env.mem.read(io_property_data_size) != required_size {
        log!("Warning: ExtAudioFileGetProperty() failed");
        return kAudioFileBadPropertySizeError;
    }

    let host_object = env
        .framework_state
        .audio_toolbox
        .extended_audio_file
        .extended_audio_files
        .get(&in_ext_audio_file)
        .unwrap();

    AudioFileGetProperty(
        env,
        host_object.guest_audio_file,
        audio_file_property_id,
        io_property_data_size,
        out_property_data,
    )
}

fn ExtAudioFileSetProperty(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_property_id: ExtAudioFilePropertyID,
    in_property_data_size: u32,
    in_property_data: ConstVoidPtr,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);

    assert_eq!(in_property_id, kExtAudioFileProperty_ClientDataFormat);
    assert_eq!(
        in_property_data_size,
        guest_size_of::<AudioStreamBasicDescription>()
    );

    let audio_desc_ptr: ConstPtr<AudioStreamBasicDescription> = in_property_data.cast();
    let client_audio_desc = env.mem.read(audio_desc_ptr);
    log_dbg!("ExtAudioFileSetProperty {:?}", client_audio_desc);
    let format_id = client_audio_desc.format_id;
    assert_eq!(format_id, kAudioFormatLinearPCM);
    assert!(is_supported_audio_format(&client_audio_desc));

    let host_object = env
        .framework_state
        .audio_toolbox
        .extended_audio_file
        .extended_audio_files
        .get_mut(&in_ext_audio_file)
        .unwrap();
    // P2G reapplies the client format after seeking to its loop start. This
    // must not reset the read position or discard already converted audio.
    if host_object.client_data_format == Some(client_audio_desc) {
        return 0;
    }
    assert!(host_object.client_data_format.is_none());
    host_object.client_data_format = Some(client_audio_desc);

    let host_object = env
        .framework_state
        .audio_toolbox
        .extended_audio_file
        .extended_audio_files
        .get(&in_ext_audio_file)
        .unwrap();
    let other_host_object = env
        .framework_state
        .audio_toolbox
        .audio_file
        .audio_files
        .get(&host_object.guest_audio_file)
        .unwrap();
    let audio_desc = AudioStreamBasicDescription::from_audio_description(
        other_host_object.as_real().unwrap().audio_description(),
    );
    if audio_desc == client_audio_desc {
        return 0; // success
    }
    // TODO: support more audio format conversions
    assert!(
        audio_desc.format_id == kAudioFormatAppleIMA4
            && client_audio_desc.bits_per_channel == 16
            && client_audio_desc.channels_per_frame == audio_desc.channels_per_frame
            && client_audio_desc.sample_rate == audio_desc.sample_rate,
        "TODO: ExtAudioFile conversion from {audio_desc:?} to {client_audio_desc:?}"
    );
    let decoded = decode_ima4_file(env, in_ext_audio_file, audio_desc.channels_per_frame);
    State::get(&mut env.framework_state)
        .extended_audio_files
        .get_mut(&in_ext_audio_file)
        .unwrap()
        .decoded = Some(decoded);

    0 // success
}

fn ExtAudioFileRead(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    io_number_frames: MutPtr<u32>,
    io_data: MutPtr<AudioBufferList<1>>,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);

    let mut audio_buffer_list = env.mem.read(io_data);
    let num_buffers = audio_buffer_list.number_buffers;
    assert_eq!(num_buffers, 1);

    let host_object = env
        .framework_state
        .audio_toolbox
        .extended_audio_file
        .extended_audio_files
        .get(&in_ext_audio_file)
        .unwrap();

    audio_buffer_list.buffers[0].number_channels =
        host_object.client_data_format.unwrap().channels_per_frame;

    let number_frames = env.mem.read(io_number_frames);
    let bytes_per_frame = host_object.client_data_format.unwrap().bytes_per_frame;
    let number_of_bytes = number_frames.checked_mul(bytes_per_frame).unwrap();

    if let Some(decoded) = &host_object.decoded {
        let start = (host_object.current_bytes_read as usize).min(decoded.len());
        let end = (start + number_of_bytes as usize).min(decoded.len());
        let count = (end - start) as u32;
        let dest = audio_buffer_list.buffers[0].data.cast::<u8>();
        let chunk = decoded[start..end].to_vec();
        env.mem.bytes_at_mut(dest, count).copy_from_slice(&chunk);
        env.mem.write(io_number_frames, count / bytes_per_frame);
        audio_buffer_list.buffers[0].data_byte_size = count;
        env.mem.write(io_data, audio_buffer_list);
        State::get(&mut env.framework_state)
            .extended_audio_files
            .get_mut(&in_ext_audio_file)
            .unwrap()
            .current_bytes_read += count as i64;
        return 0;
    }

    let number_of_bytes_ptr = env.mem.alloc_and_write(number_of_bytes);
    let res = AudioFileReadBytes(
        env,
        host_object.guest_audio_file,
        false,
        host_object.current_bytes_read,
        number_of_bytes_ptr,
        audio_buffer_list.buffers[0].data,
    );
    let number_of_bytes_read = env.mem.read(number_of_bytes_ptr);
    env.mem.free(number_of_bytes_ptr.cast());
    if res != 0 {
        if res == eofErr {
            env.mem.write(io_number_frames, 0);
            return 0;
        }
        log!(
            "ExtAudioFileRead({:?}, {:?}, {:?}) failed, error: {:?}",
            in_ext_audio_file,
            io_number_frames,
            io_data,
            res
        );
        return res;
    }
    env.mem.write(
        io_number_frames,
        number_of_bytes_read.checked_div(bytes_per_frame).unwrap(),
    );
    audio_buffer_list.buffers[0].data_byte_size = number_of_bytes_read;

    let host_object = env
        .framework_state
        .audio_toolbox
        .extended_audio_file
        .extended_audio_files
        .get_mut(&in_ext_audio_file)
        .unwrap();
    host_object.current_bytes_read += number_of_bytes_read as i64;

    env.mem.write(io_data, audio_buffer_list);

    0 // success
}

fn ExtAudioFileSeek(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_frame_offset: i64,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let bytes_per_frame = client_data_format(env, in_ext_audio_file).bytes_per_frame;
    log_dbg!(
        "ExtAudioFileSeek({:?}, {})",
        in_ext_audio_file,
        in_frame_offset
    );
    assert!(bytes_per_frame != 0, "TODO: seeking in a compressed file");
    State::get(&mut env.framework_state)
        .extended_audio_files
        .get_mut(&in_ext_audio_file)
        .unwrap()
        .current_bytes_read = in_frame_offset * bytes_per_frame as i64;
    0 // success
}

fn register_ext_audio_file(
    env: &mut Environment,
    guest_audio_file: AudioFileID,
    wrapped: bool,
    out_ext_audio_file: MutPtr<ExtAudioFileRef>,
) -> OSStatus {
    let host_object = ExtAudioFileHostObject {
        guest_audio_file,
        wrapped,
        client_data_format: None,
        current_bytes_read: 0,
        decoded: None,
    };

    let guest_extended_audio_file = env.mem.alloc_and_write(OpaqueExtAudioFile { _filler: 0 });
    State::get(&mut env.framework_state)
        .extended_audio_files
        .insert(guest_extended_audio_file, host_object);

    env.mem.write(out_ext_audio_file, guest_extended_audio_file);

    0 // success
}

fn ExtAudioFileCreateWithURL(
    env: &mut Environment,
    in_url: CFURLRef,
    file_type: u32,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    _in_channel_layout: ConstVoidPtr,
    in_flags: u32,
    out_ext_audio_file: MutPtr<ExtAudioFileRef>,
) -> OSStatus {
    if out_ext_audio_file.is_null() {
        return paramErr;
    }
    let audio_file_ptr: MutPtr<AudioFileID> = env.mem.alloc(guest_size_of::<AudioFileID>()).cast();
    let res = AudioFileCreateWithURL(env, in_url, file_type, in_format, in_flags, audio_file_ptr);
    let guest_audio_file = env.mem.read(audio_file_ptr);
    env.mem.free(audio_file_ptr.cast());
    if res != 0 {
        return res;
    }
    register_ext_audio_file(env, guest_audio_file, false, out_ext_audio_file)
}

fn ExtAudioFileWrapAudioFileID(
    env: &mut Environment,
    in_audio_file: AudioFileID,
    _in_for_writing: bool,
    out_ext_audio_file: MutPtr<ExtAudioFileRef>,
) -> OSStatus {
    return_if_null!(in_audio_file);
    register_ext_audio_file(env, in_audio_file, true, out_ext_audio_file)
}

fn ExtAudioFileGetPropertyInfo(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    in_property_id: ExtAudioFilePropertyID,
    out_size: MutPtr<u32>,
    out_writable: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let (size, writable) = match in_property_id {
        kExtAudioFileProperty_FileDataFormat => {
            (guest_size_of::<AudioStreamBasicDescription>(), false)
        }
        kExtAudioFileProperty_ClientDataFormat => {
            (guest_size_of::<AudioStreamBasicDescription>(), true)
        }
        kExtAudioFileProperty_FileLengthFrames => (guest_size_of::<i64>(), false),
        _ => {
            log!(
                "Warning: ExtAudioFileGetPropertyInfo() unknown property {}",
                debug_fourcc(in_property_id)
            );
            return kExtAudioFileError_InvalidProperty;
        }
    };
    if !out_size.is_null() {
        env.mem.write(out_size, size);
    }
    if !out_writable.is_null() {
        env.mem.write(out_writable, writable as u32);
    }
    0 // success
}

fn ExtAudioFileTell(
    env: &mut Environment,
    in_ext_audio_file: ExtAudioFileRef,
    out_frame_offset: MutPtr<i64>,
) -> OSStatus {
    return_if_null!(in_ext_audio_file);
    let format = file_data_format(env, in_ext_audio_file);
    let host_object =
        &State::get(&mut env.framework_state).extended_audio_files[&in_ext_audio_file];
    let bytes_per_frame = host_object
        .client_data_format
        .unwrap_or(format)
        .bytes_per_frame;
    assert!(bytes_per_frame != 0, "TODO: telling in a compressed file");
    let offset = host_object.current_bytes_read / i64::from(bytes_per_frame);
    env.mem.write(out_frame_offset, offset);
    0 // success
}

fn ExtAudioFileDispose(env: &mut Environment, in_ext_audio_file: ExtAudioFileRef) -> OSStatus {
    return_if_null!(in_ext_audio_file);

    let host_object = env
        .framework_state
        .audio_toolbox
        .extended_audio_file
        .extended_audio_files
        .get(&in_ext_audio_file)
        .unwrap();

    if !host_object.wrapped {
        let res = AudioFileClose(env, host_object.guest_audio_file);
        assert_eq!(res, 0); // success
    }

    let _host_object = env
        .framework_state
        .audio_toolbox
        .extended_audio_file
        .extended_audio_files
        .remove(&in_ext_audio_file)
        .unwrap();
    env.mem.free(in_ext_audio_file.cast());

    0 // success
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(ExtAudioFileCreateWithURL(_, _, _, _, _, _)),
    export_c_func!(ExtAudioFileOpenURL(_, _)),
    export_c_func!(ExtAudioFileWrapAudioFileID(_, _, _)),
    export_c_func!(ExtAudioFileGetPropertyInfo(_, _, _, _)),
    export_c_func!(ExtAudioFileTell(_, _)),
    export_c_func!(ExtAudioFileGetProperty(_, _, _, _)),
    export_c_func!(ExtAudioFileSetProperty(_, _, _, _)),
    export_c_func!(ExtAudioFileSeek(_, _)),
    export_c_func!(ExtAudioFileRead(_, _, _)),
    export_c_func!(ExtAudioFileDispose(_)),
];
