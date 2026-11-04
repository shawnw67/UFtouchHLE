/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! AudioConverter entry points based on HyperHLE's LPCM conversion support.
//! Accepts packed interleaved or planar LPCM at matching sample rates.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::kAudioFormatFlagIsBigEndian;
use crate::frameworks::core_audio_types::kAudioFormatFlagIsFloat;
use crate::frameworks::core_audio_types::kAudioFormatLinearPCM;
use crate::frameworks::core_audio_types::{fourcc, AudioStreamBasicDescription};
use crate::mem::{guest_size_of, ConstPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;
use std::collections::HashMap;

const FORMAT: OSStatus = fourcc(b"fmt?") as OSStatus;
const INPUT_SIZE: OSStatus = fourcc(b"insz") as OSStatus;
const OUTPUT_SIZE: OSStatus = fourcc(b"otsz") as OSStatus;
const PROPERTY: OSStatus = fourcc(b"prop") as OSStatus;
const PROPERTY_SIZE: OSStatus = fourcc(b"!siz") as OSStatus;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Buffer {
    channels: u32,
    size: u32,
    data: MutVoidPtr,
}
unsafe impl SafeRead for Buffer {}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct BufferList {
    count: u32,
    buffer: Buffer,
}
unsafe impl SafeRead for BufferList {}

#[repr(C, packed)]
struct PacketDescription {
    offset: i64,
    frames: u32,
    size: u32,
}
unsafe impl SafeRead for PacketDescription {}

#[derive(Clone, Copy, PartialEq)]
struct Shape {
    channels: u32,
    bits: u32,
    float: bool,
    signed: bool,
    big: bool,
    planar: bool,
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
            || flags & !(15 | 32 | 64) != 0
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
            planar: flags & 32 != 0,
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
        if self.planar {
            self.bits / 8
        } else {
            self.channels * (self.bits / 8)
        }
    }
    fn interleaved(self) -> Self {
        Self {
            planar: false,
            ..self
        }
    }
    fn buffer_count(self) -> u32 {
        if self.planar {
            self.channels
        } else {
            1
        }
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
    if converter.input.planar || converter.output.planar {
        return FORMAT;
    }
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

fn buffers(
    env: &Environment,
    list: ConstPtr<BufferList>,
    shape: Shape,
) -> Result<Vec<Buffer>, OSStatus> {
    if list.is_null() {
        return Err(paramErr);
    }
    let count: u32 = env.mem.read(list.cast());
    if count != shape.buffer_count() {
        return Err(FORMAT);
    }
    let pointer: ConstPtr<Buffer> = (list.cast::<u8>() + 4).cast();
    let mut result = Vec::new();
    for index in 0..count {
        let buffer: Buffer = env.mem.read(pointer + index);
        if buffer.channels != if shape.planar { 1 } else { shape.channels } {
            return Err(FORMAT);
        }
        result.push(buffer);
    }
    Ok(result)
}

fn set_buffer_sizes(env: &mut Environment, list: MutPtr<BufferList>, values: &[Buffer], size: u32) {
    let pointer: MutPtr<Buffer> = (list.cast::<u8>() + 4).cast();
    for (index, &buffer) in values.iter().enumerate() {
        env.mem
            .write(pointer + index as u32, Buffer { size, ..buffer });
    }
}

fn write_list(
    env: &mut Environment,
    converter: Converter,
    frames: u32,
    input_pointer: ConstPtr<BufferList>,
    output_pointer: MutPtr<BufferList>,
    descriptions: MutPtr<PacketDescription>,
) -> OSStatus {
    let input = match buffers(env, input_pointer, converter.input) {
        Ok(buffers) => buffers,
        Err(status) => return status,
    };
    let output = match buffers(env, output_pointer.cast_const(), converter.output) {
        Ok(buffers) => buffers,
        Err(status) => return status,
    };
    let Some(input_size) = frames.checked_mul(converter.input.frame_bytes()) else {
        return INPUT_SIZE;
    };
    let Some(output_size) = frames.checked_mul(converter.output.frame_bytes()) else {
        return OUTPUT_SIZE;
    };
    let Some(packed_size) = frames.checked_mul(converter.input.interleaved().frame_bytes()) else {
        return INPUT_SIZE;
    };
    if frames
        .checked_mul(converter.output.interleaved().frame_bytes())
        .is_none()
    {
        return OUTPUT_SIZE;
    }
    for buffer in &input {
        let data = buffer.data;
        if buffer.size < input_size {
            return INPUT_SIZE;
        }
        if frames != 0 && data.is_null() {
            return paramErr;
        }
    }
    for buffer in &output {
        let data = buffer.data;
        if buffer.size < output_size {
            return OUTPUT_SIZE;
        }
        if frames != 0 && data.is_null() {
            return paramErr;
        }
    }
    if frames != 0 {
        // Snapshot every input plane before writing any output, including
        // when callers reuse their input storage as output storage.
        let planes: Vec<_> = input
            .iter()
            .map(|buffer| {
                env.mem
                    .bytes_at(buffer.data.cast_const().cast(), input_size)
                    .to_vec()
            })
            .collect();
        let source = if converter.input.planar {
            let width = converter.input.bits as usize / 8;
            let mut packed = vec![0; packed_size as usize];
            for frame in 0..frames as usize {
                for (channel, plane) in planes.iter().enumerate() {
                    let index = (frame * planes.len() + channel) * width;
                    packed[index..index + width]
                        .copy_from_slice(&plane[frame * width..(frame + 1) * width]);
                }
            }
            packed
        } else {
            planes.into_iter().next().unwrap()
        };
        let converted = convert(
            &source,
            converter.input.interleaved(),
            converter.output.interleaved(),
        );
        if converter.output.planar {
            let width = converter.output.bits as usize / 8;
            for (channel, buffer) in output.iter().enumerate() {
                let target = env.mem.bytes_at_mut(buffer.data.cast(), output_size);
                for frame in 0..frames as usize {
                    let index = (frame * output.len() + channel) * width;
                    target[frame * width..(frame + 1) * width]
                        .copy_from_slice(&converted[index..index + width]);
                }
            }
        } else {
            env.mem
                .bytes_at_mut(output[0].data.cast(), output_size)
                .copy_from_slice(&converted);
        }
    }
    set_buffer_sizes(env, output_pointer, &output, output_size);
    if !descriptions.is_null() {
        for index in 0..frames {
            env.mem.write(
                descriptions + index,
                PacketDescription {
                    offset: i64::from(index) * i64::from(converter.output.frame_bytes()),
                    frames: 0,
                    size: converter.output.frame_bytes(),
                },
            );
        }
    }
    0
}

fn AudioConverterConvertComplexBuffer(
    env: &mut Environment,
    handle: MutVoidPtr,
    frames: u32,
    input: ConstPtr<BufferList>,
    output: MutPtr<BufferList>,
) -> OSStatus {
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
    write_list(env, converter, frames, input, output, MutPtr::null())
}

fn AudioConverterFillComplexBuffer(
    env: &mut Environment,
    handle: MutVoidPtr,
    callback: GuestFunction,
    context: MutVoidPtr,
    output_packets: MutPtr<u32>,
    output: MutPtr<BufferList>,
    descriptions: MutPtr<PacketDescription>,
) -> OSStatus {
    if output_packets.is_null() || callback.to_ptr().is_null() {
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
    let output_buffers = match buffers(env, output.cast_const(), converter.output) {
        Ok(buffers) => buffers,
        Err(status) => return status,
    };
    let capacity = output_buffers
        .iter()
        .map(|buffer| buffer.size / converter.output.frame_bytes())
        .min()
        .unwrap();
    let frames = env.mem.read(output_packets).min(capacity);
    env.mem.write(output_packets, 0);
    if frames == 0 {
        set_buffer_sizes(env, output, &output_buffers, 0);
        return 0;
    }
    if output_buffers.iter().any(|buffer| {
        let data = buffer.data;
        data.is_null()
    }) {
        return paramErr;
    }
    let Some(input_size) = frames.checked_mul(converter.input.frame_bytes()) else {
        return INPUT_SIZE;
    };
    let Some(packed_size) = frames.checked_mul(converter.input.interleaved().frame_bytes()) else {
        return INPUT_SIZE;
    };
    let Some(aligned) = packed_size.checked_add(3).map(|n| n & !3) else {
        return INPUT_SIZE;
    };
    let list_size = 4 + converter.input.buffer_count() * guest_size_of::<Buffer>();
    let Some(total) = aligned.checked_add(list_size + 8) else {
        return INPUT_SIZE;
    };
    let Some(allocation) = env.mem.try_alloc(total) else {
        return -108;
    };
    let bytes: MutPtr<u8> = allocation.cast();
    let input_list: MutPtr<BufferList> = (bytes + aligned).cast();
    let input_buffers: MutPtr<Buffer> = (input_list.cast::<u8>() + 4).cast();
    env.mem
        .write(input_list.cast::<u32>(), converter.input.buffer_count());
    for index in 0..converter.input.buffer_count() {
        env.mem.write(
            input_buffers + index,
            Buffer {
                channels: if converter.input.planar {
                    1
                } else {
                    converter.input.channels
                },
                size: input_size,
                data: (bytes + index * input_size).cast(),
            },
        );
    }
    let packets: MutPtr<u32> = (bytes + aligned + list_size).cast();
    let packet_pointer: MutPtr<MutPtr<PacketDescription>> = (packets + 1).cast();
    env.mem.write(packets, frames);
    env.mem
        .write(packet_pointer, MutPtr::<PacketDescription>::null());
    let status: OSStatus =
        callback.call_from_host(env, (handle, packets, input_list, packet_pointer, context));
    let status = if status != 0 {
        status
    } else if !env
        .framework_state
        .audio_toolbox
        .audio_converter
        .converters
        .contains_key(&handle)
    {
        paramErr
    } else {
        let returned = env.mem.read(packets);
        if returned > frames {
            INPUT_SIZE
        } else {
            let status = write_list(
                env,
                converter,
                returned,
                input_list.cast_const(),
                output,
                descriptions,
            );
            if status == 0 {
                env.mem.write(output_packets, returned);
            }
            status
        }
    };
    env.mem.free(allocation);
    status
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

#[repr(C, packed)]
#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub struct AudioStreamPacketDescription {
    pub mStartOffset: i64,
    pub mVariableFramesInPacket: u32,
    pub mDataByteSize: u32,
}

unsafe impl SafeRead for AudioStreamPacketDescription {}

/// Extract a [PcmShape] from an ASBD. Returns [None] for non-LPCM formats or
/// nonsensical ones.
#[allow(dead_code)]
pub fn pcm_shape_from_asbd(asbd: &AudioStreamBasicDescription) -> Option<PcmShape> {
    if asbd.format_id != kAudioFormatLinearPCM {
        return None;
    }
    let channels = asbd.channels_per_frame;
    let bits = asbd.bits_per_channel;
    if channels == 0 || channels > 8 || !matches!(bits, 8 | 16 | 24 | 32) {
        return None;
    }
    let bytes_per_channel = bits / 8;
    let bytes_per_frame = if asbd.bytes_per_frame > 0 {
        asbd.bytes_per_frame
    } else {
        channels * bytes_per_channel
    };
    if bytes_per_frame < channels * bytes_per_channel {
        return None;
    }
    Some(PcmShape {
        sample_rate: asbd.sample_rate,
        channels,
        bits,
        is_float: (asbd.format_flags & kAudioFormatFlagIsFloat) != 0,
        is_big_endian: (asbd.format_flags & kAudioFormatFlagIsBigEndian) != 0,
        bytes_per_frame,
    })
}

/// Convert interleaved PCM bytes from one shape to another. Handles sample
/// rate (linear interpolation), channel count (up/downmix) and sample
/// encoding (bit depth / float / endian) conversion.
#[allow(dead_code)]
#[allow(clippy::manual_checked_ops)]
pub fn convert_pcm(src: &[u8], src_shape: &PcmShape, dst_shape: &PcmShape) -> Vec<u8> {
    let src_bytes_per_frame = src_shape.bytes_per_frame as usize;
    let src_frames = if src_bytes_per_frame > 0 {
        src.len() / src_bytes_per_frame
    } else {
        return Vec::new();
    };

    // 1. Decode to interleaved f32.
    let mut samples: Vec<f32> = Vec::with_capacity(src_frames * src_shape.channels as usize);
    for frame in 0..src_frames {
        for ch in 0..src_shape.channels as usize {
            samples.push(read_f32_sample(src, src_shape, frame, ch));
        }
    }

    // 2. Sample-rate conversion (linear interpolation).
    let src_rate = src_shape.sample_rate.max(1.0);
    let dst_rate = dst_shape.sample_rate.max(1.0);
    let resampled: Vec<f32> = if (src_rate - dst_rate).abs() < f64::EPSILON {
        samples
    } else {
        let dst_frames = ((src_frames as f64) * dst_rate / src_rate).round() as usize;
        let mut out = Vec::with_capacity(dst_frames * src_shape.channels as usize);
        for i in 0..dst_frames {
            let pos = (i as f64) * src_rate / dst_rate;
            let idx = pos.floor() as usize;
            let frac = (pos - idx as f64) as f32;
            let idx1 = idx.min(src_frames.saturating_sub(1));
            let idx2 = (idx + 1).min(src_frames.saturating_sub(1));
            for ch in 0..src_shape.channels as usize {
                let a = samples[idx1 * src_shape.channels as usize + ch];
                let b = samples[idx2 * src_shape.channels as usize + ch];
                out.push(a + (b - a) * frac);
            }
        }
        out
    };
    let frames = resampled.len() / src_shape.channels as usize;

    // 3. Channel conversion.
    let src_ch = src_shape.channels as usize;
    let dst_ch = dst_shape.channels as usize;
    let mixed: Vec<f32> = if src_ch == dst_ch {
        resampled
    } else {
        let mut out = Vec::with_capacity(frames * dst_ch);
        for frame in 0..frames {
            let frame_samples = &resampled[frame * src_ch..(frame + 1) * src_ch];
            match (src_ch, dst_ch) {
                (1, _) => {
                    // Mono -> N: duplicate.
                    for _ in 0..dst_ch {
                        out.push(frame_samples[0]);
                    }
                }
                (_, 1) => {
                    // N -> Mono: average.
                    let sum: f32 = frame_samples.iter().sum();
                    out.push(sum / src_ch as f32);
                }
                _ => {
                    // General N -> M: average when narrowing, duplicate the
                    // last available channel when widening (keeps stereo
                    // width for 5.1 -> stereo-style cases reasonable).
                    for c in 0..dst_ch {
                        if c < src_ch {
                            out.push(frame_samples[c]);
                        } else {
                            out.push(frame_samples[src_ch - 1]);
                        }
                    }
                }
            }
        }
        out
    };

    // 4. Encode to destination sample format.
    let mut out = Vec::with_capacity(frames * dst_shape.bytes_per_frame as usize);
    for frame in 0..frames {
        for ch in 0..dst_ch {
            write_f32_sample(&mut out, dst_shape, mixed[frame * dst_ch + ch]);
        }
    }
    out
}

/// The parts of an `AudioStreamBasicDescription` needed to interpret a
/// byte buffer of interleaved linear PCM data.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(dead_code)]
pub struct PcmShape {
    pub sample_rate: f64,
    pub channels: u32,
    pub bits: u32,
    pub is_float: bool,
    pub is_big_endian: bool,
    pub bytes_per_frame: u32,
}

/// Whether two shapes need no conversion at all.
#[allow(dead_code)]
pub fn pcm_shapes_equal(a: &PcmShape, b: &PcmShape) -> bool {
    a.sample_rate == b.sample_rate
        && a.channels == b.channels
        && a.bits == b.bits
        && a.is_float == b.is_float
        && a.is_big_endian == b.is_big_endian
        && a.bytes_per_frame == b.bytes_per_frame
}

#[allow(dead_code)]
fn read_f32_sample(data: &[u8], shape: &PcmShape, frame: usize, channel: usize) -> f32 {
    let bytes_per_channel = (shape.bits / 8) as usize;
    let offset = frame * shape.bytes_per_frame as usize + channel * bytes_per_channel;
    let bytes = &data[offset..offset + bytes_per_channel];
    let sample = if shape.is_big_endian {
        match shape.bits {
            8 => ((bytes[0] as i32) - 128) as f64 / 128.0,
            16 => i16::from_be_bytes(bytes.try_into().unwrap()) as f64 / 32768.0,
            24 => {
                let v = ((bytes[0] as i32) << 16) | ((bytes[1] as i32) << 8) | (bytes[2] as i32);
                let v = (v << 8) >> 8; // sign-extend 24 -> 32
                v as f64 / 8388608.0
            }
            32 => {
                if shape.is_float {
                    f32::from_be_bytes(bytes.try_into().unwrap()) as f64
                } else {
                    i32::from_be_bytes(bytes.try_into().unwrap()) as f64 / 2147483648.0
                }
            }
            _ => 0.0,
        }
    } else {
        match shape.bits {
            8 => ((bytes[0] as i32) - 128) as f64 / 128.0,
            16 => i16::from_le_bytes(bytes.try_into().unwrap()) as f64 / 32768.0,
            24 => {
                let v = ((bytes[0] as i32) << 16) | ((bytes[1] as i32) << 8) | (bytes[2] as i32);
                let v = (v << 8) >> 8; // sign-extend 24 -> 32
                v as f64 / 8388608.0
            }
            32 => {
                if shape.is_float {
                    f32::from_le_bytes(bytes.try_into().unwrap()) as f64
                } else {
                    i32::from_le_bytes(bytes.try_into().unwrap()) as f64 / 2147483648.0
                }
            }
            _ => 0.0,
        }
    };
    // 8-bit WAV PCM is unsigned, handled above; float samples can exceed
    // [-1, 1] slightly — clamp at the end of conversion instead of here so
    // intermediate math keeps headroom.
    sample as f32
}

#[allow(dead_code)]
fn write_f32_sample(out: &mut Vec<u8>, shape: &PcmShape, sample: f32) {
    let sample = sample.clamp(-1.0, 1.0);
    let bytes_per_channel = (shape.bits / 8) as usize;
    let mut buf = [0u8; 4];
    match shape.bits {
        8 => {
            buf[0] = ((sample * 127.0) as i32 + 128).clamp(0, 255) as u8;
        }
        16 => {
            let v = (sample * 32767.0) as i16;
            if shape.is_big_endian {
                buf[..2].copy_from_slice(&v.to_be_bytes());
            } else {
                buf[..2].copy_from_slice(&v.to_le_bytes());
            }
        }
        24 => {
            let v = (sample * 8388607.0) as i32;
            if shape.is_big_endian {
                buf[..3].copy_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
            } else {
                buf[..3].copy_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8]);
            }
        }
        32 => {
            if shape.is_float {
                let v = sample;
                if shape.is_big_endian {
                    buf.copy_from_slice(&v.to_be_bytes());
                } else {
                    buf.copy_from_slice(&v.to_le_bytes());
                }
            } else {
                let v = (sample * 2147483647.0) as i32;
                if shape.is_big_endian {
                    buf.copy_from_slice(&v.to_be_bytes());
                } else {
                    buf.copy_from_slice(&v.to_le_bytes());
                }
            }
        }
        _ => {}
    }
    out.extend_from_slice(&buf[..bytes_per_channel]);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioConverterNew(_, _, _)),
    export_c_func!(AudioConverterDispose(_)),
    export_c_func!(AudioConverterReset(_)),
    export_c_func!(AudioConverterConvertBuffer(_, _, _, _, _)),
    export_c_func!(AudioConverterConvertComplexBuffer(_, _, _, _)),
    export_c_func!(AudioConverterFillComplexBuffer(_, _, _, _, _, _)),
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
            planar: false,
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
            planar: false,
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
            planar: false,
        };
        assert_eq!(convert(&input, source, destination), [0, 0]);
    }
}
