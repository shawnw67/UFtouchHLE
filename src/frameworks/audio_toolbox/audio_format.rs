/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Audio format metadata. Hyper's entry points are implemented for LPCM;
//! unknown properties fail instead of returning a successful empty result.

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{fourcc, AudioStreamBasicDescription};
use crate::frameworks::foundation::ns_string;
use crate::mem::{guest_size_of, ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::Environment;

const PROPERTY: OSStatus = fourcc(b"prop") as OSStatus;
const SIZE: OSStatus = fourcc(b"!siz") as OSStatus;
const SPECIFIER: OSStatus = fourcc(b"!spc") as OSStatus;
const FORMAT: OSStatus = fourcc(b"fmt?") as OSStatus;

fn complete(
    mut description: AudioStreamBasicDescription,
) -> Result<AudioStreamBasicDescription, OSStatus> {
    if description.format_id != fourcc(b"lpcm")
        || !description.sample_rate.is_finite()
        || description.sample_rate < 0.0
        || description.channels_per_frame > 8
    {
        return Err(FORMAT);
    }
    let channels = if description.format_flags & 32 != 0 {
        1
    } else {
        description.channels_per_frame
    };
    if description.bits_per_channel == 0 && channels > 0 && description.bytes_per_frame > 0 {
        if !description.bytes_per_frame.is_multiple_of(channels) {
            return Err(FORMAT);
        }
        description.bits_per_channel = (description.bytes_per_frame / channels)
            .checked_mul(8)
            .ok_or(FORMAT)?;
    }
    if description.bits_per_channel != 0
        && !matches!(description.bits_per_channel, 8 | 16 | 24 | 32 | 64)
    {
        return Err(FORMAT);
    }
    if description.bits_per_channel == 64 && description.format_flags & 1 == 0 {
        return Err(FORMAT);
    }
    if description.format_flags & 1 != 0 && !matches!(description.bits_per_channel, 0 | 32 | 64) {
        return Err(FORMAT);
    }
    if description.frames_per_packet > 1 {
        return Err(FORMAT);
    }
    description.frames_per_packet = 1;
    if description.bytes_per_frame == 0 && channels > 0 && description.bits_per_channel > 0 {
        description.bytes_per_frame = channels * (description.bits_per_channel / 8);
        description.format_flags |= 8;
    }
    let natural = channels * (description.bits_per_channel / 8);
    if description.bytes_per_frame < natural
        || (description.format_flags & 8 != 0
            && natural != 0
            && description.bytes_per_frame != natural)
    {
        return Err(FORMAT);
    }
    if description.bytes_per_packet == 0 {
        description.bytes_per_packet = description.bytes_per_frame;
    }
    if description.bytes_per_packet != description.bytes_per_frame {
        return Err(FORMAT);
    }
    description._reserved = 0;
    Ok(description)
}

fn property_size(
    env: &Environment,
    property: u32,
    specifier_size: u32,
    specifier: ConstVoidPtr,
) -> Result<u32, OSStatus> {
    if property == fourcc(b"fmti") {
        if specifier_size != 0 && specifier.is_null() {
            return Err(SPECIFIER);
        }
        return Ok(guest_size_of::<AudioStreamBasicDescription>());
    }
    if property == fourcc(b"aven") || property == fourcc(b"avde") {
        if specifier_size != 4 || specifier.is_null() {
            return Err(SPECIFIER);
        }
        // Compressed AudioConverter components are not yet exposed.
        return Ok(0);
    }
    if [fourcc(b"fnam"), fourcc(b"fvbr"), fourcc(b"fexf")].contains(&property) {
        if specifier_size != guest_size_of::<AudioStreamBasicDescription>() || specifier.is_null() {
            return Err(SPECIFIER);
        }
        let description: AudioStreamBasicDescription = env.mem.read(specifier.cast());
        complete(description)?;
        return Ok(4);
    }
    Err(PROPERTY)
}

fn AudioFormatGetPropertyInfo(
    env: &mut Environment,
    property: u32,
    specifier_size: u32,
    specifier: ConstVoidPtr,
    size: MutPtr<u32>,
) -> OSStatus {
    if size.is_null() {
        return paramErr;
    }
    match property_size(env, property, specifier_size, specifier) {
        Ok(length) => {
            env.mem.write(size, length);
            0
        }
        Err(status) => status,
    }
}

fn AudioFormatGetProperty(
    env: &mut Environment,
    property: u32,
    specifier_size: u32,
    specifier: ConstVoidPtr,
    size: MutPtr<u32>,
    output: MutVoidPtr,
) -> OSStatus {
    if size.is_null() {
        return paramErr;
    }
    let required = match property_size(env, property, specifier_size, specifier) {
        Ok(size) => size,
        Err(status) => return status,
    };
    if env.mem.read(size) < required {
        return SIZE;
    }
    if required != 0 && output.is_null() {
        return paramErr;
    }
    if property == fourcc(b"fmti") {
        let input = env.mem.read(output.cast::<AudioStreamBasicDescription>());
        let description = match complete(input) {
            Ok(description) => description,
            Err(status) => return status,
        };
        env.mem.write(output.cast(), description);
    } else if property == fourcc(b"fnam") {
        let description: AudioStreamBasicDescription = env.mem.read(specifier.cast());
        let bits = description.bits_per_channel;
        let channels = description.channels_per_frame;
        let layout = if description.format_flags & 32 != 0 {
            "planar"
        } else {
            "interleaved"
        };
        let name = ns_string::from_rust_string(
            env,
            format!("Linear PCM, {bits}-bit, {channels} channels, {layout}"),
        );
        env.mem.write(output.cast(), name);
    } else if required == 4 {
        env.mem.write(output.cast(), 0u32);
    }
    env.mem.write(size, required);
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioFormatGetPropertyInfo(_, _, _, _)),
    export_c_func!(AudioFormatGetProperty(_, _, _, _, _)),
];
