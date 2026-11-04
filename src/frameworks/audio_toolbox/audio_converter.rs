/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! AudioConverter entry points based on HyperHLE's LPCM conversion support.
//! Currently accepts packed interleaved LPCM at matching sample rates.

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{fourcc, AudioStreamBasicDescription};
use crate::mem::{guest_size_of, ConstPtr, MutPtr, MutVoidPtr};
use crate::Environment;
use std::collections::HashMap;

const FORMAT: OSStatus = fourcc(b"fmt?") as OSStatus;
const INPUT_SIZE: OSStatus = fourcc(b"insz") as OSStatus;
const OUTPUT_SIZE: OSStatus = fourcc(b"otsz") as OSStatus;
const PROPERTY: OSStatus = fourcc(b"prop") as OSStatus;
const PROPERTY_SIZE: OSStatus = fourcc(b"!siz") as OSStatus;

#[derive(Clone, Copy, PartialEq)]
struct Shape {
    channels: u32,
    bits: u32,
    float: bool,
    signed: bool,
    big: bool,
}
impl Shape {
    fn parse(format: AudioStreamBasicDescription) -> Option<Self> {
        let flags = format.format_flags;
        let channels = format.channels_per_frame;
        if format.format_id != fourcc(b"lpcm")
            || !format.sample_rate.is_finite()
            || format.sample_rate <= 0.0
            || !(1..=2).contains(&channels)
            || !matches!(format.bits_per_channel, 8 | 16 | 24 | 32)
            || flags & !(15 | 64) != 0
            || flags & 8 == 0
            || (flags & 1 != 0 && format.bits_per_channel != 32)
        {
            return None;
        }
        let shape = Self {
            channels: format.channels_per_frame,
            bits: format.bits_per_channel,
            float: flags & 1 != 0,
            signed: flags & 4 != 0,
            big: flags & 2 != 0,
        };
        if format.frames_per_packet != 1
            || format.bytes_per_frame != shape.frame_bytes()
            || format.bytes_per_packet != shape.frame_bytes()
        {
            return None;
        }
        Some(shape)
    }
    fn frame_bytes(self) -> u32 {
        self.channels * (self.bits / 8)
    }
    fn read(self, input: &[u8]) -> f64 {
        let mut raw = 0u32;
        for (index, &byte) in input.iter().enumerate() {
            let shift = if self.big {
                input.len() - index - 1
            } else {
                index
            };
            raw |= u32::from(byte) << (8 * shift);
        }
        if self.float {
            return f32::from_bits(raw) as f64;
        }
        let midpoint = (1u64 << (self.bits - 1)) as f64;
        if self.signed {
            ((raw << (32 - self.bits)) as i32 >> (32 - self.bits)) as f64 / midpoint
        } else {
            (raw as f64 - midpoint) / midpoint
        }
    }
    fn write(self, sample: f64, output: &mut [u8]) {
        let raw = if self.float {
            (sample as f32).to_bits()
        } else {
            let sample = if sample.is_nan() {
                0.0
            } else {
                sample.clamp(-1.0, 1.0)
            };
            let midpoint = (1u64 << (self.bits - 1)) as f64;
            let value = (sample * midpoint).round().clamp(-midpoint, midpoint - 1.0);
            if self.signed {
                value as i64 as u32
            } else {
                (value + midpoint) as u32
            }
        };
        let count = output.len();
        for (index, byte) in output.iter_mut().enumerate() {
            let shift = if self.big { count - index - 1 } else { index };
            *byte = (raw >> (8 * shift)) as u8;
        }
    }
}

fn convert(input: &[u8], source: Shape, destination: Shape) -> Vec<u8> {
    if source == destination {
        return input.to_vec();
    }
    let frames = input.len() / source.frame_bytes() as usize;
    let mut output = vec![0; frames * destination.frame_bytes() as usize];
    for (input, output) in input
        .chunks_exact(source.frame_bytes() as usize)
        .zip(output.chunks_exact_mut(destination.frame_bytes() as usize))
    {
        let width = source.bits as usize / 8;
        let left = source.read(&input[..width]);
        let right = if source.channels == 2 {
            source.read(&input[width..2 * width])
        } else {
            left
        };
        let samples = if destination.channels == 1 {
            [(left + right) / 2.0, 0.0]
        } else {
            [left, right]
        };
        for (channel, output) in output
            .chunks_exact_mut(destination.bits as usize / 8)
            .enumerate()
        {
            destination.write(samples[channel], output);
        }
    }
    output
}

#[derive(Clone, Copy)]
struct Converter {
    source: AudioStreamBasicDescription,
    destination: AudioStreamBasicDescription,
    input: Shape,
    output: Shape,
    quality: u32,
    complexity: u32,
}

#[derive(Default)]
pub(super) struct State {
    converters: HashMap<MutVoidPtr, Converter>,
}

fn AudioConverterNew(
    env: &mut Environment,
    source: ConstPtr<AudioStreamBasicDescription>,
    destination: ConstPtr<AudioStreamBasicDescription>,
    result: MutPtr<MutVoidPtr>,
) -> OSStatus {
    if source.is_null() || destination.is_null() || result.is_null() {
        return paramErr;
    }
    env.mem.write(result, MutVoidPtr::null());
    let source = env.mem.read(source);
    let destination = env.mem.read(destination);
    let (Some(input), Some(output)) = (Shape::parse(source), Shape::parse(destination)) else {
        return FORMAT;
    };
    if source.sample_rate != destination.sample_rate {
        return FORMAT;
    }
    let Some(handle) = env.mem.try_alloc(4) else {
        return -108;
    };
    env.framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .insert(
            handle,
            Converter {
                source,
                destination,
                input,
                output,
                quality: 0x40,
                complexity: fourcc(b"norm"),
            },
        );
    env.mem.write(result, handle);
    0
}

fn AudioConverterDispose(env: &mut Environment, handle: MutVoidPtr) -> OSStatus {
    if env
        .framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .remove(&handle)
        .is_none()
    {
        return paramErr;
    }
    env.mem.free(handle);
    0
}
fn AudioConverterReset(env: &mut Environment, handle: MutVoidPtr) -> OSStatus {
    if env
        .framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .contains_key(&handle)
    {
        0
    } else {
        paramErr
    }
}

fn AudioConverterConvertBuffer(
    env: &mut Environment,
    handle: MutVoidPtr,
    size: u32,
    input: ConstPtr<u8>,
    output_size: MutPtr<u32>,
    output: MutVoidPtr,
) -> OSStatus {
    if output_size.is_null() || (size != 0 && input.is_null()) {
        return paramErr;
    }
    let Some(converter) = env
        .framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .get(&handle)
        .copied()
    else {
        return paramErr;
    };
    if !size.is_multiple_of(converter.input.frame_bytes()) {
        return INPUT_SIZE;
    }
    let Some(required) =
        (size / converter.input.frame_bytes()).checked_mul(converter.output.frame_bytes())
    else {
        return OUTPUT_SIZE;
    };
    if env.mem.read(output_size) < required {
        return OUTPUT_SIZE;
    }
    if required != 0 && output.is_null() {
        return paramErr;
    }
    if required != 0 {
        let result = convert(
            env.mem.bytes_at(input, size),
            converter.input,
            converter.output,
        );
        env.mem
            .bytes_at_mut(output.cast(), required)
            .copy_from_slice(&result);
    }
    env.mem.write(output_size, required);
    0
}

fn property_size(property: u32) -> Option<u32> {
    if property == fourcc(b"acid") || property == fourcc(b"acod") {
        Some(guest_size_of::<AudioStreamBasicDescription>())
    } else if [
        fourcc(b"srcq"),
        fourcc(b"srca"),
        fourcc(b"mibs"),
        fourcc(b"mobs"),
        fourcc(b"xips"),
        fourcc(b"xops"),
        fourcc(b"cibs"),
        fourcc(b"cobs"),
    ]
    .contains(&property)
    {
        Some(4)
    } else {
        None
    }
}

fn AudioConverterGetPropertyInfo(
    env: &mut Environment,
    handle: MutVoidPtr,
    property: u32,
    size: MutPtr<u32>,
    writable: MutPtr<bool>,
) -> OSStatus {
    if !env
        .framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .contains_key(&handle)
    {
        return paramErr;
    }
    let Some(length) = property_size(property) else {
        return PROPERTY;
    };
    if !size.is_null() {
        env.mem.write(size, length);
    }
    if !writable.is_null() {
        env.mem.write(
            writable,
            property == fourcc(b"srcq") || property == fourcc(b"srca"),
        );
    }
    0
}

fn AudioConverterGetProperty(
    env: &mut Environment,
    handle: MutVoidPtr,
    property: u32,
    size: MutPtr<u32>,
    data: MutVoidPtr,
) -> OSStatus {
    if size.is_null() || data.is_null() {
        return paramErr;
    }
    let Some(converter) = env
        .framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .get(&handle)
        .copied()
    else {
        return paramErr;
    };
    let Some(length) = property_size(property) else {
        return PROPERTY;
    };
    if env.mem.read(size) < length {
        return PROPERTY_SIZE;
    }
    if property == fourcc(b"acid") {
        env.mem.write(data.cast(), converter.source);
    } else if property == fourcc(b"acod") {
        env.mem.write(data.cast(), converter.destination);
    } else {
        let value = if property == fourcc(b"srcq") {
            converter.quality
        } else if property == fourcc(b"srca") {
            converter.complexity
        } else if property == fourcc(b"mibs") || property == fourcc(b"xips") {
            converter.input.frame_bytes()
        } else if property == fourcc(b"mobs") || property == fourcc(b"xops") {
            converter.output.frame_bytes()
        } else {
            let size: u32 = env.mem.read(data.cast());
            let (source, destination) = if property == fourcc(b"cibs") {
                (converter.output, converter.input)
            } else {
                (converter.input, converter.output)
            };
            let Some(bytes) = size
                .div_ceil(source.frame_bytes())
                .checked_mul(destination.frame_bytes())
            else {
                return PROPERTY_SIZE;
            };
            bytes
        };
        env.mem.write(data.cast(), value);
    }
    env.mem.write(size, length);
    0
}

fn AudioConverterSetProperty(
    env: &mut Environment,
    handle: MutVoidPtr,
    property: u32,
    size: u32,
    data: ConstPtr<u32>,
) -> OSStatus {
    if size != 4 {
        return PROPERTY_SIZE;
    }
    if data.is_null() {
        return paramErr;
    }
    let value = env.mem.read(data);
    let Some(converter) = env
        .framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .get_mut(&handle)
    else {
        return paramErr;
    };
    if property == fourcc(b"srcq") {
        if value > 0x7f {
            return paramErr;
        }
        converter.quality = value;
    } else if property == fourcc(b"srca") {
        if ![
            fourcc(b"line"),
            fourcc(b"norm"),
            fourcc(b"bats"),
            fourcc(b"minp"),
        ]
        .contains(&value)
        {
            return paramErr;
        }
        converter.complexity = value;
    } else {
        return PROPERTY;
    }
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioConverterNew(_, _, _)),
    export_c_func!(AudioConverterDispose(_)),
    export_c_func!(AudioConverterReset(_)),
    export_c_func!(AudioConverterConvertBuffer(_, _, _, _, _)),
    export_c_func!(AudioConverterGetPropertyInfo(_, _, _, _)),
    export_c_func!(AudioConverterGetProperty(_, _, _, _)),
    export_c_func!(AudioConverterSetProperty(_, _, _, _)),
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_24_bit_little_endian_and_unsigned_endpoints() {
        let source = Shape {
            channels: 1,
            bits: 24,
            float: false,
            signed: true,
            big: false,
        };
        let destination = Shape {
            bits: 16,
            big: true,
            ..source
        };
        let bytes = convert(&[0, 0, 0x80, 0xff, 0xff, 0x7f], source, destination);
        assert_eq!(bytes, [0x80, 0, 0x7f, 0xff]);
        let unsigned = Shape {
            bits: 8,
            signed: false,
            ..source
        };
        assert_eq!(
            convert(&[0, 128, 255], unsigned, destination),
            [0x80, 0, 0, 0, 0x7f, 0]
        );
    }
    #[test]
    fn stereo_downmix_and_identical_format_copy() {
        let source = Shape {
            channels: 2,
            bits: 32,
            float: true,
            signed: false,
            big: false,
        };
        let mut input = Vec::new();
        input.extend_from_slice(&1.0f32.to_le_bytes());
        input.extend_from_slice(&(-1.0f32).to_le_bytes());
        assert_eq!(convert(&input, source, source), input);
        let destination = Shape {
            channels: 1,
            bits: 16,
            float: false,
            signed: true,
            big: false,
        };
        assert_eq!(convert(&input, source, destination), [0, 0]);
    }
}
