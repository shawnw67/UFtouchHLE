/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioUnit.h` (Audio Unit Services)
//!
//! [Audio Unit Programming Guide](https://developer.apple.com/library/archive/documentation/MusicAudio/Conceptual/AudioUnitProgrammingGuide/TheAudioUnit/TheAudioUnit.html)

use std::time::Instant;

use crate::audio::openal::al_types::{ALuint, ALvoid};
use crate::audio::openal::{
    AL_BUFFERS_PROCESSED, AL_BUFFERS_QUEUED, AL_FORMAT_STEREO16, AL_GAIN, AL_PITCH, AL_PLAYING,
    AL_POSITION, AL_SOURCE_STATE,
};

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::audio_toolbox::audio_components;
use crate::frameworks::audio_toolbox::audio_queue::{
    is_supported_audio_format, log_if_broken_audio_format,
};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{
    kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat, kAudioFormatFlagIsNonInterleaved,
    kAudioFormatLinearPCM, AudioStreamBasicDescription, AudioTimeStamp, SMPTETime,
};
use crate::frameworks::core_foundation::cf_run_loop::CFRunLoopGetMain;
use crate::frameworks::foundation::ns_run_loop;
use crate::mem::{guest_size_of, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};

use super::audio_components::{AURenderCallbackStruct, AudioComponentInstance};
use super::audio_queue::{decode_buffer, float_pcm_to_i16};
use super::audio_session;

pub type AudioUnit = AudioComponentInstance;
type AudioUnitPropertyID = u32;
type AudioUnitScope = u32;
type AudioUnitElement = u32;

#[repr(C, packed)]
pub struct AudioBufferList<const COUNT: usize> {
    pub number_buffers: u32,
    pub buffers: [AudioBuffer; COUNT],
}
unsafe impl SafeRead for AudioBufferList<1> {}
unsafe impl SafeRead for AudioBufferList<2> {}

#[repr(C, packed)]
pub struct AudioBuffer {
    pub number_channels: u32,
    pub data_byte_size: u32,
    pub data: MutVoidPtr,
}

// TODO: Other scopes
const kAudioUnitScope_Global: AudioUnitScope = 0;
const kAudioUnitScope_Input: AudioUnitScope = 1;
const kAudioUnitScope_Output: AudioUnitScope = 2;

const kAudioUnitProperty_SampleRate: AudioUnitPropertyID = 2;
const kAudioUnitProperty_SetRenderCallback: AudioUnitPropertyID = 23;
const kAudioUnitProperty_MaximumFramesPerSlice: AudioUnitPropertyID = 14;
const kAudioUnitProperty_StreamFormat: AudioUnitPropertyID = 8;
const kAudioUnitProperty_ElementCount: AudioUnitPropertyID = 11;

const kAudioOutputUnitProperty_EnableIO: AudioUnitPropertyID = 2003;
const kAudioOutputUnitProperty_IsRunning: AudioUnitPropertyID = 2001;

fn apply_mixer_voice(env: &mut Environment, unit: AudioUnit) {
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&unit)
    else {
        return;
    };
    let Some(source) = object.al_source.filter(|_| object.mixer_voice) else {
        return;
    };
    let gain = if object.mixer_enabled && object.mixer_master_enabled {
        object.mixer_gain * object.mixer_master_gain
    } else {
        0.0
    };
    let rate = object.mixer_rate;
    let (azimuth, elevation, distance) = (
        object.mixer_azimuth.to_radians(),
        object.mixer_elevation.to_radians(),
        object.mixer_distance,
    );
    let spatial = object.mixer_spatial;
    let context = env
        .framework_state
        .audio_toolbox
        .al_context
        .make_al_context_current(env.openal_manager.as_mut());
    unsafe {
        context.Sourcef(source, AL_GAIN, gain);
        context.Sourcef(source, AL_PITCH, rate);
        if spatial {
            context.Source3f(
                source,
                AL_POSITION,
                distance * azimuth.sin() * elevation.cos(),
                distance * elevation.sin(),
                -distance * azimuth.cos() * elevation.cos(),
            );
        }
    }
}

pub(super) fn mixer_bus(env: &mut Environment, unit: AudioUnit, bus: u32) -> Option<AudioUnit> {
    if bus >= 256 {
        return None;
    }
    let object = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&unit)?;
    if object.disposing {
        return None;
    }
    let buses = object.mixer_buses.as_ref()?;
    if let Some(&child) = buses.get(&bus) {
        return Some(child);
    }
    let (initialized, started, master_gain, master_enabled, subtype) = (
        object.initialized,
        object.started,
        object.mixer_master_gain,
        object.mixer_enabled,
        object.mixer_subtype,
    );
    let child = audio_components::create_audio_unit_instance(env);
    let state = audio_components::State::get(&mut env.framework_state);
    let child_state = state.audio_component_instances.get_mut(&child).unwrap();
    child_state.callback_bus = bus;
    child_state.mixer_master_gain = master_gain;
    child_state.mixer_master_enabled = master_enabled;
    child_state.mixer_voice = true;
    child_state.mixer_spatial = subtype == Some(u32::from_be_bytes(*b"3dem"));
    let object = state.audio_component_instances.get_mut(&unit).unwrap();
    object.mixer_buses.as_mut().unwrap().insert(bus, child);
    object.mixer_bus_count = object.mixer_bus_count.max(bus + 1);
    if initialized && AudioUnitInitialize(env, child) != 0 {
        return None;
    }
    if started && AudioOutputUnitStart(env, child) != 0 {
        return None;
    }
    Some(child)
}

pub(super) fn AudioUnitInitialize(env: &mut Environment, in_unit: AudioUnit) -> OSStatus {
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        return paramErr;
    };
    if object.disposing {
        return paramErr;
    }
    if object.initialized {
        return 0;
    }
    object.initialized = true;
    let children = object
        .mixer_buses
        .as_ref()
        .map(|buses| buses.values().copied().collect::<Vec<_>>());
    if let Some(children) = children {
        for child in children {
            let result = AudioUnitInitialize(env, child);
            if result != 0 {
                return result;
            }
        }
        return 0;
    }
    let run_loop = CFRunLoopGetMain(env);
    ns_run_loop::add_audio_unit(env, run_loop, in_unit);
    0 // success
}

pub(super) fn AudioUnitUninitialize(env: &mut Environment, in_unit: AudioUnit) -> OSStatus {
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&in_unit)
    else {
        return paramErr;
    };
    if !object.initialized || object.started {
        return paramErr;
    }
    let children = object
        .mixer_buses
        .as_ref()
        .map(|buses| buses.values().copied().collect::<Vec<_>>());
    if let Some(children) = children {
        for child in children {
            let result = AudioUnitUninitialize(env, child);
            if result != 0 {
                return result;
            }
        }
        audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&in_unit)
            .unwrap()
            .initialized = false;
        return 0;
    }
    let run_loop = CFRunLoopGetMain(env);
    match ns_run_loop::remove_audio_unit(env, run_loop, in_unit) {
        Ok(_) => {
            audio_components::State::get(&mut env.framework_state)
                .audio_component_instances
                .get_mut(&in_unit)
                .unwrap()
                .initialized = false;
            0
        }
        Err(_) => paramErr, // TODO: handle different errors
    }
}

fn AudioUnitAddPropertyListener(
    env: &mut Environment,
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    callback: GuestFunction,
    context: MutVoidPtr,
) -> OSStatus {
    if callback.to_ptr().is_null() {
        return paramErr;
    }
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&unit)
    else {
        return paramErr;
    };
    object
        .property_listeners
        .push((property, callback, context));
    0
}

fn AudioUnitRemovePropertyListener(
    env: &mut Environment,
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    callback: GuestFunction,
) -> OSStatus {
    remove_property_listener(env, unit, property, callback, None)
}

fn AudioUnitRemovePropertyListenerWithUserData(
    env: &mut Environment,
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    callback: GuestFunction,
    context: MutVoidPtr,
) -> OSStatus {
    remove_property_listener(env, unit, property, callback, Some(context))
}

pub(super) fn AudioUnitAddRenderNotify(
    env: &mut Environment,
    unit: AudioUnit,
    callback: GuestFunction,
    context: ConstVoidPtr,
) -> OSStatus {
    add_render_notify(env, unit, callback, context, 0)
}

pub(super) fn add_render_notify(
    env: &mut Environment,
    unit: AudioUnit,
    callback: GuestFunction,
    context: ConstVoidPtr,
    owner: u32,
) -> OSStatus {
    if callback.to_ptr().is_null() {
        return paramErr;
    }
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&unit)
    else {
        return paramErr;
    };
    object.render_notifies.push((callback, context, owner));
    0
}

pub(super) fn AudioUnitRemoveRenderNotify(
    env: &mut Environment,
    unit: AudioUnit,
    callback: GuestFunction,
    context: ConstVoidPtr,
) -> OSStatus {
    remove_render_notify(env, unit, callback, context, 0)
}

pub(super) fn remove_render_notify(
    env: &mut Environment,
    unit: AudioUnit,
    callback: GuestFunction,
    context: ConstVoidPtr,
    owner: u32,
) -> OSStatus {
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&unit)
    else {
        return paramErr;
    };
    object
        .render_notifies
        .retain(|(registered, refcon, registration_owner)| {
            registered.to_ptr() != callback.to_ptr()
                || *refcon != context
                || *registration_owner != owner
        });
    0
}

fn remove_property_listener(
    env: &mut Environment,
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    callback: GuestFunction,
    context: Option<MutVoidPtr>,
) -> OSStatus {
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&unit)
    else {
        return paramErr;
    };
    if let Some(index) = object.property_listeners.iter().position(|&(p, c, ctx)| {
        p == property
            && c.to_ptr() == callback.to_ptr()
            && context.is_none_or(|wanted| wanted == ctx)
    }) {
        object.property_listeners.remove(index);
    }
    0
}

fn notify_property_listeners(
    env: &mut Environment,
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: AudioUnitElement,
) {
    let listeners = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&unit)
        .map(|object| {
            object
                .property_listeners
                .iter()
                .copied()
                .filter(|entry| entry.0 == property)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for (_, callback, context) in listeners {
        let (): () = callback.call_from_host(env, (context, unit, property, scope, element));
    }
}

pub(super) fn AudioUnitSetProperty(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    in_data: ConstVoidPtr,
    in_data_size: u32,
) -> OSStatus {
    if in_data.is_null() {
        return paramErr;
    }
    let is_mixer = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&in_unit)
        .is_some_and(|unit| unit.mixer_buses.is_some());
    if is_mixer && in_id == kAudioUnitProperty_ElementCount {
        if in_scope != kAudioUnitScope_Input
            || in_element != 0
            || in_data_size != guest_size_of::<u32>()
        {
            return paramErr;
        }
        let count = env.mem.read(in_data.cast::<u32>());
        if count > 256 {
            return paramErr;
        }
        let object = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&in_unit)
            .unwrap();
        if object
            .mixer_buses
            .as_ref()
            .unwrap()
            .keys()
            .any(|&bus| bus >= count)
        {
            return paramErr;
        }
        object.mixer_bus_count = count;
        return 0;
    }
    if is_mixer
        && in_scope == kAudioUnitScope_Input
        && matches!(
            in_id,
            kAudioUnitProperty_SetRenderCallback
                | kAudioUnitProperty_StreamFormat
                | kAudioUnitProperty_SampleRate
        )
    {
        let Some(child) = mixer_bus(env, in_unit, in_element) else {
            return paramErr;
        };
        let converter = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get(&child)
            .is_some_and(|unit| unit.passthrough_converter);
        if converter && in_id == kAudioUnitProperty_SetRenderCallback {
            return paramErr;
        }
        let child_scope = if converter {
            kAudioUnitScope_Output
        } else {
            in_scope
        };
        return AudioUnitSetProperty(env, child, in_id, child_scope, 0, in_data, in_data_size);
    }
    if in_element != 0 {
        return paramErr;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        return paramErr;
    };

    let result;
    match in_id {
        kAudioUnitProperty_SampleRate => {
            if in_data.is_null() || in_data_size != guest_size_of::<f64>() {
                return paramErr;
            }
            let rate = env.mem.read(in_data.cast::<f64>());
            if !rate.is_finite() || rate <= 0.0 {
                return paramErr;
            }
            if host_object.passthrough_converter {
                let other = match in_scope {
                    kAudioUnitScope_Input => host_object.output_stream_format,
                    kAudioUnitScope_Output => host_object.input_stream_format,
                    _ => None,
                };
                if other.is_some_and(|format| format.sample_rate != rate) {
                    return paramErr;
                }
            }
            match in_scope {
                kAudioUnitScope_Global => host_object.global_stream_format.sample_rate = rate,
                kAudioUnitScope_Input => {
                    let mut format = host_object
                        .input_stream_format
                        .unwrap_or(host_object.global_stream_format);
                    format.sample_rate = rate;
                    host_object.input_stream_format = Some(format);
                }
                kAudioUnitScope_Output => {
                    let mut format = host_object
                        .output_stream_format
                        .unwrap_or(host_object.global_stream_format);
                    format.sample_rate = rate;
                    host_object.output_stream_format = Some(format);
                }
                _ => return paramErr,
            }
            result = 0;
        }
        kAudioUnitProperty_MaximumFramesPerSlice => {
            if in_data.is_null()
                || in_data_size != guest_size_of::<u32>()
                || in_scope != kAudioUnitScope_Global
            {
                return paramErr;
            }
            let frames = env.mem.read(in_data.cast::<u32>());
            if frames == 0 {
                return paramErr;
            }
            host_object.maximum_frames_per_slice = frames;
            result = 0;
        }
        kAudioUnitProperty_SetRenderCallback => {
            if !matches!(in_scope, kAudioUnitScope_Global | kAudioUnitScope_Input)
                || in_data_size != guest_size_of::<AURenderCallbackStruct>()
            {
                return paramErr;
            }
            let render_callback = env.mem.read(in_data.cast::<AURenderCallbackStruct>());
            host_object.render_callback = if render_callback.input_proc.to_ptr().is_null() {
                None
            } else {
                Some(render_callback)
            };
            result = 0;
            log_dbg!("AudioUnitSetProperty({:?}, kAudioUnitProperty_SetRenderCallback, {:?}, {:?}, {:?}, {:?}) -> {:?}", in_unit, in_scope, in_element, render_callback, in_data_size, result);
        }
        kAudioUnitProperty_StreamFormat => {
            if in_data_size != guest_size_of::<AudioStreamBasicDescription>() {
                return paramErr;
            }
            let stream_format = env.mem.read(in_data.cast::<AudioStreamBasicDescription>());
            log_if_broken_audio_format(&stream_format);
            if host_object.passthrough_converter {
                let other = match in_scope {
                    kAudioUnitScope_Input => host_object.output_stream_format,
                    kAudioUnitScope_Output => host_object.input_stream_format,
                    _ => None,
                };
                if other.is_some_and(|format| format != stream_format) {
                    return paramErr;
                }
            }
            match in_scope {
                kAudioUnitScope_Global => host_object.global_stream_format = stream_format,
                kAudioUnitScope_Output => host_object.output_stream_format = Some(stream_format),
                kAudioUnitScope_Input => host_object.input_stream_format = Some(stream_format),
                _ => return paramErr,
            };
            result = 0;
            log_dbg!("AudioUnitSetProperty({:?}, kAudioUnitProperty_StreamFormat, {:?}, {:?}, {:?}, {:?}) -> {:?}", in_unit, in_scope, in_element, stream_format, in_data_size, result);
        }
        kAudioOutputUnitProperty_EnableIO => {
            if in_scope != kAudioUnitScope_Output || in_data_size != guest_size_of::<u32>() {
                return paramErr;
            }
            let enabled = env.mem.read(in_data.cast::<u32>());
            // Output is enabled by default.
            if enabled != 1 {
                return paramErr;
            }
            result = 0;
            log_dbg!("AudioUnitSetProperty({:?}, kAudioOutputUnitProperty_EnableIO, {:?}, {:?}, {:?}, {:?}) -> {:?}", in_unit, in_scope, in_element, enabled, in_data_size, result);
        }
        _ => return paramErr,
    };

    if result == 0 {
        notify_property_listeners(env, in_unit, in_id, in_scope, in_element);
    }
    result
}

fn AudioUnitGetProperty(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    out_data: MutVoidPtr,
    io_data_size: MutPtr<u32>,
) -> OSStatus {
    if out_data.is_null() || io_data_size.is_null() {
        return paramErr;
    }

    let is_mixer = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&in_unit)
        .is_some_and(|unit| unit.mixer_buses.is_some());
    if is_mixer && in_id == kAudioUnitProperty_ElementCount {
        if in_scope != kAudioUnitScope_Input
            || in_element != 0
            || env.mem.read(io_data_size) < guest_size_of::<u32>()
        {
            return paramErr;
        }
        let count = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get(&in_unit)
            .unwrap()
            .mixer_bus_count;
        env.mem.write(out_data.cast(), count);
        env.mem.write(io_data_size, guest_size_of::<u32>());
        return 0;
    }
    if is_mixer
        && in_scope == kAudioUnitScope_Input
        && matches!(
            in_id,
            kAudioUnitProperty_StreamFormat | kAudioUnitProperty_SampleRate
        )
    {
        let Some(child) = mixer_bus(env, in_unit, in_element) else {
            return paramErr;
        };
        let converter = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get(&child)
            .is_some_and(|unit| unit.passthrough_converter);
        let child_scope = if converter {
            kAudioUnitScope_Output
        } else {
            in_scope
        };
        return AudioUnitGetProperty(env, child, in_id, child_scope, 0, out_data, io_data_size);
    }
    if in_element != 0 {
        return paramErr;
    }

    let Some(host_object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_unit)
    else {
        return paramErr;
    };

    match in_id {
        kAudioOutputUnitProperty_IsRunning => {
            if in_scope != kAudioUnitScope_Global
                || env.mem.read(io_data_size) < guest_size_of::<u32>()
            {
                return paramErr;
            }
            env.mem
                .write(out_data.cast(), u32::from(host_object.started));
            env.mem.write(io_data_size, guest_size_of::<u32>());
        }
        kAudioUnitProperty_MaximumFramesPerSlice => {
            if in_scope != kAudioUnitScope_Global
                || env.mem.read(io_data_size) < guest_size_of::<u32>()
            {
                return paramErr;
            }
            let max_frames: u32 = host_object.maximum_frames_per_slice;
            env.mem.write(out_data.cast(), max_frames);
            env.mem.write(io_data_size.cast(), guest_size_of::<u32>());
        }
        kAudioUnitProperty_StreamFormat => {
            if env.mem.read(io_data_size) < guest_size_of::<AudioStreamBasicDescription>() {
                return paramErr;
            }
            let stream_format = match in_scope {
                kAudioUnitScope_Global => host_object.global_stream_format,
                kAudioUnitScope_Output => host_object
                    .output_stream_format
                    .unwrap_or(host_object.global_stream_format),
                kAudioUnitScope_Input => host_object
                    .input_stream_format
                    .unwrap_or(host_object.global_stream_format),
                _ => return paramErr,
            };
            env.mem.write(out_data.cast(), stream_format);
            env.mem.write(
                io_data_size.cast(),
                guest_size_of::<AudioStreamBasicDescription>(),
            );
        }
        kAudioUnitProperty_SampleRate => {
            if env.mem.read(io_data_size) < guest_size_of::<f64>() {
                return paramErr;
            }
            let sample_rate = match in_scope {
                kAudioUnitScope_Global => host_object.global_stream_format.sample_rate,
                kAudioUnitScope_Output => {
                    host_object
                        .output_stream_format
                        .unwrap_or(host_object.global_stream_format)
                        .sample_rate
                }
                kAudioUnitScope_Input => {
                    host_object
                        .input_stream_format
                        .unwrap_or(host_object.global_stream_format)
                        .sample_rate
                }
                _ => return paramErr,
            };
            env.mem.write(out_data.cast(), sample_rate);
            env.mem.write(io_data_size.cast(), guest_size_of::<f64>());
        }
        _ => return paramErr,
    };
    0 // success
}

fn AudioUnitGetPropertyInfo(
    env: &mut Environment,
    unit: AudioUnit,
    property: AudioUnitPropertyID,
    scope: AudioUnitScope,
    element: AudioUnitElement,
    out_size: MutPtr<u32>,
    out_writable: MutPtr<u8>,
) -> OSStatus {
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&unit)
    else {
        return paramErr;
    };
    let is_mixer = object.mixer_buses.is_some();
    if scope > kAudioUnitScope_Output
        || (element != 0 && (!is_mixer || scope != kAudioUnitScope_Input || element >= 256))
    {
        return paramErr;
    }
    let (size, writable) = match property {
        kAudioUnitProperty_StreamFormat => (guest_size_of::<AudioStreamBasicDescription>(), true),
        kAudioUnitProperty_SampleRate => (guest_size_of::<f64>(), true),
        kAudioUnitProperty_ElementCount
            if is_mixer && scope == kAudioUnitScope_Input && element == 0 =>
        {
            (guest_size_of::<u32>(), true)
        }
        kAudioOutputUnitProperty_IsRunning if scope == kAudioUnitScope_Global => {
            (guest_size_of::<u32>(), false)
        }
        kAudioUnitProperty_MaximumFramesPerSlice if scope == kAudioUnitScope_Global => {
            (guest_size_of::<u32>(), true)
        }
        kAudioUnitProperty_SetRenderCallback
            if matches!(scope, kAudioUnitScope_Global | kAudioUnitScope_Input) =>
        {
            (guest_size_of::<AURenderCallbackStruct>(), true)
        }
        _ => return paramErr,
    };
    if !out_size.is_null() {
        env.mem.write(out_size, size);
    }
    if !out_writable.is_null() {
        env.mem.write(out_writable, u8::from(writable));
    }
    0
}

fn AudioUnitSetParameter(
    env: &mut Environment,
    unit: AudioUnit,
    parameter: u32,
    scope: u32,
    element: u32,
    value: f32,
    offset: u32,
) -> OSStatus {
    if !value.is_finite() || offset != 0 {
        return paramErr;
    }
    let Some(subtype) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&unit)
        .and_then(|object| object.mixer_subtype)
    else {
        return paramErr;
    };
    let spatial = subtype == u32::from_be_bytes(*b"3dem");
    if scope == kAudioUnitScope_Global || scope == kAudioUnitScope_Output {
        if element != 0 {
            return paramErr;
        }
        let gain = if spatial && parameter == 3 && (-120.0..=20.0).contains(&value) {
            Some(10.0_f32.powf(value / 20.0))
        } else if !spatial && parameter == 0 && (0.0..=10.0).contains(&value) {
            Some(value)
        } else {
            None
        };
        let enabled = if !spatial && parameter == 1 && (value == 0.0 || value == 1.0) {
            Some(value != 0.0)
        } else {
            None
        };
        if gain.is_none() && enabled.is_none() {
            return paramErr;
        }
        let parent = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&unit)
            .unwrap();
        if let Some(gain) = gain {
            parent.mixer_master_gain = gain;
        }
        if let Some(enabled) = enabled {
            parent.mixer_enabled = enabled;
        }
        let (master_gain, master_enabled) = (parent.mixer_master_gain, parent.mixer_enabled);
        let children: Vec<_> = parent
            .mixer_buses
            .as_ref()
            .unwrap()
            .values()
            .copied()
            .collect();
        for child in children {
            let object = audio_components::State::get(&mut env.framework_state)
                .audio_component_instances
                .get_mut(&child)
                .unwrap();
            object.mixer_master_gain = master_gain;
            object.mixer_master_enabled = master_enabled;
            apply_mixer_voice(env, child);
        }
        return 0;
    }
    if scope != kAudioUnitScope_Input {
        return paramErr;
    }
    let valid = if spatial {
        match parameter {
            0 => (-180.0..=180.0).contains(&value),
            1 => (-90.0..=90.0).contains(&value),
            2 => (0.0..=10000.0).contains(&value),
            3 => (-120.0..=20.0).contains(&value),
            4 => (0.5..=2.0).contains(&value),
            _ => false,
        }
    } else {
        match parameter {
            0 => (0.0..=10.0).contains(&value),
            1 => value == 0.0 || value == 1.0,
            _ => false,
        }
    };
    if !valid {
        return paramErr;
    }
    let Some(child) = mixer_bus(env, unit, element) else {
        return paramErr;
    };
    let object = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&child)
        .unwrap();
    if spatial {
        match parameter {
            0 => object.mixer_azimuth = value,
            1 => object.mixer_elevation = value,
            2 => object.mixer_distance = value,
            3 => object.mixer_gain = 10.0_f32.powf(value / 20.0),
            4 => object.mixer_rate = value,
            _ => unreachable!(),
        }
    } else if parameter == 0 {
        object.mixer_gain = value;
    } else {
        object.mixer_enabled = value != 0.0;
    }
    apply_mixer_voice(env, child);
    0
}

fn AudioUnitGetParameter(
    env: &mut Environment,
    unit: AudioUnit,
    parameter: u32,
    scope: u32,
    element: u32,
    out_value: MutPtr<f32>,
) -> OSStatus {
    return_if_null!(out_value);
    let Some(parent) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&unit)
    else {
        return paramErr;
    };
    let Some(subtype) = parent.mixer_subtype else {
        return paramErr;
    };
    let spatial = subtype == u32::from_be_bytes(*b"3dem");
    let value = if scope == kAudioUnitScope_Global || scope == kAudioUnitScope_Output {
        if element != 0 {
            return paramErr;
        }
        match (spatial, parameter) {
            (true, 3) => 20.0 * parent.mixer_master_gain.log10(),
            (false, 0) => parent.mixer_master_gain,
            (false, 1) => u8::from(parent.mixer_enabled) as f32,
            _ => return paramErr,
        }
    } else if scope == kAudioUnitScope_Input {
        if (spatial && parameter > 4) || (!spatial && parameter > 1) {
            return paramErr;
        }
        let Some(child) = mixer_bus(env, unit, element) else {
            return paramErr;
        };
        let object = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get(&child)
            .unwrap();
        match (spatial, parameter) {
            (true, 0) => object.mixer_azimuth,
            (true, 1) => object.mixer_elevation,
            (true, 2) => object.mixer_distance,
            (true, 3) => 20.0 * object.mixer_gain.log10(),
            (true, 4) => object.mixer_rate,
            (false, 0) => object.mixer_gain,
            (false, 1) => u8::from(object.mixer_enabled) as f32,
            _ => return paramErr,
        }
    } else {
        return paramErr;
    };
    env.mem.write(out_value, value);
    0
}

fn AudioUnitReset(env: &mut Environment, unit: AudioUnit, scope: u32, element: u32) -> OSStatus {
    if scope > kAudioUnitScope_Output || element != 0 {
        return paramErr;
    }
    let Some(object) = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&unit)
    else {
        return paramErr;
    };
    object.sample_time = 0.0;
    let source = object.al_source;
    if object.started {
        object.last_render_time = Some(Instant::now());
    }
    if let Some(source) = source {
        let context = env
            .framework_state
            .audio_toolbox
            .al_context
            .make_al_context_current(env.openal_manager.as_mut());
        unsafe {
            context.SourceStop(source);
            let mut queued = 0;
            context.GetSourcei(source, AL_BUFFERS_QUEUED, &mut queued);
            if queued > 0 {
                let mut buffers = vec![0; queued as usize];
                context.SourceUnqueueBuffers(source, queued, buffers.as_mut_ptr());
                context.DeleteBuffers(queued, buffers.as_ptr());
            }
            context.SourcePlay(source);
            assert_eq!(context.GetError(), 0);
        }
    }
    0
}

pub(super) fn AudioOutputUnitStart(env: &mut Environment, ci: AudioUnit) -> OSStatus {
    match audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&ci)
    {
        Some(unit) if unit.disposing => return paramErr,
        Some(unit) if unit.started => return 0,
        None => return paramErr,
        _ => {}
    }
    let children = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&ci)
        .unwrap()
        .mixer_buses
        .as_ref()
        .map(|buses| buses.values().copied().collect::<Vec<_>>());
    if let Some(children) = children {
        for child in children {
            let result = AudioOutputUnitStart(env, child);
            if result != 0 {
                return result;
            }
        }
        audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&ci)
            .unwrap()
            .started = true;
        notify_property_listeners(
            env,
            ci,
            kAudioOutputUnitProperty_IsRunning,
            kAudioUnitScope_Global,
            0,
        );
        return 0;
    }
    let context = env
        .framework_state
        .audio_toolbox
        .make_al_context_current(env.openal_manager.as_mut());

    let mut source: ALuint = 0;
    unsafe {
        context.GenSources(1, &mut source);
        context.SourcePlay(source);
        assert_eq!(context.GetError(), 0);
    }

    let audio_components_state = audio_components::State::get(&mut env.framework_state);
    let audio_unit_state = audio_components_state
        .audio_component_instances
        .get_mut(&ci)
        .unwrap();
    audio_unit_state.al_source = Some(source);
    audio_unit_state.last_render_time = Some(Instant::now());
    audio_unit_state.sample_time = 0.0;
    audio_unit_state.started = true;

    apply_mixer_voice(env, ci);
    notify_property_listeners(
        env,
        ci,
        kAudioOutputUnitProperty_IsRunning,
        kAudioUnitScope_Global,
        0,
    );
    let result = 0; // Success
    log_dbg!("AudioOutputUnitStart({:?}) -> {:?}", ci, result);
    result
}

pub(super) fn AudioOutputUnitStop(env: &mut Environment, ci: AudioUnit) -> OSStatus {
    let was_running = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&ci)
        .is_some_and(|unit| unit.started);
    let children = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&ci)
        .and_then(|unit| unit.mixer_buses.as_ref())
        .map(|buses| buses.values().copied().collect::<Vec<_>>());
    if let Some(children) = children {
        audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&ci)
            .unwrap()
            .started = false;
        for child in children {
            AudioOutputUnitStop(env, child);
        }
        if was_running {
            notify_property_listeners(
                env,
                ci,
                kAudioOutputUnitProperty_IsRunning,
                kAudioUnitScope_Global,
                0,
            );
        }
        return 0;
    }
    let at_state = &mut env.framework_state.audio_toolbox;
    let context = at_state
        .al_context
        .make_al_context_current(env.openal_manager.as_mut());

    let audio_components_state = &mut at_state.audio_components;

    let result = if let Some(audio_unit_state) = audio_components_state
        .audio_component_instances
        .get_mut(&ci)
    {
        audio_unit_state.started = false;
        audio_unit_state.last_render_time = None;

        if let Some(al_source) = audio_unit_state.al_source {
            unsafe {
                context.SourceStop(al_source);
                let mut queued = 0;
                context.GetSourcei(al_source, AL_BUFFERS_QUEUED, &mut queued);
                if queued > 0 {
                    let mut buffers = vec![0; queued as usize];
                    context.SourceUnqueueBuffers(al_source, queued, buffers.as_mut_ptr());
                    context.DeleteBuffers(queued, buffers.as_ptr());
                }
                context.DeleteSources(1, &al_source);
                assert_eq!(context.GetError(), 0);
            }
        }
        audio_unit_state.al_source = None;
        0 // success
    } else {
        -1
    };
    if result == 0 && was_running {
        notify_property_listeners(
            env,
            ci,
            kAudioOutputUnitProperty_IsRunning,
            kAudioUnitScope_Global,
            0,
        );
    }
    log_dbg!("AudioOutputUnitStop({:?}) -> {:?}", ci, result);
    result
}

pub fn render_audio_unit(env: &mut Environment, audio_unit: AudioUnit) {
    if env.bundle.bundle_identifier().starts_with("com.ea.simcity") {
        // If enabled, we have some random crashes inside AURenderCallback ;(
        log_dbg!("Applying game-specific hack for SimCity: skipping rendering of audio units");
        return;
    }

    let at_state = &mut env.framework_state.audio_toolbox;
    let context = at_state
        .al_context
        .make_al_context_current(env.openal_manager.as_mut());

    let audio_session::State {
        current_hardware_sample_rate,
        ..
    } = at_state.audio_session;

    let audio_components_state = &mut at_state.audio_components;
    let Some(audio_unit_host_object) = audio_components_state
        .audio_component_instances
        .get_mut(&audio_unit)
    else {
        return;
    };

    if !audio_unit_host_object.started {
        return;
    }

    // A started output with no callback produces silence until one is set.
    if audio_unit_host_object.render_callback.is_none()
        && audio_unit_host_object.render_notifies.is_empty()
    {
        return;
    }

    if audio_unit_host_object.is_running_handler {
        return;
    }

    audio_unit_host_object.is_running_handler = true;

    let input_stream_format = audio_unit_host_object.input_stream_format;
    let output_stream_format = audio_unit_host_object.output_stream_format;
    let stream_format = if input_stream_format.is_some()
        && output_stream_format.is_some()
        && input_stream_format != output_stream_format
    {
        log_once!("AudioUnit stream format conversion is not implemented");
        audio_unit_host_object.is_running_handler = false;
        return;
    } else {
        // For purposes, the only important part is that format is supported
        // and playable by OpenAL. Thus, it doesn't really matter if input or
        // output format is defined by the application.
        // (but not both at the same time, see the check above)
        input_stream_format
            .unwrap_or(output_stream_format.unwrap_or(audio_unit_host_object.global_stream_format))
    };
    let sample_rate = if let Some(input_stream_format) = input_stream_format {
        input_stream_format.sample_rate
    } else if output_stream_format.is_some() {
        // TODO: confirm that this is the general behaviour
        // (and not only RE4 thing)
        current_hardware_sample_rate
    } else {
        stream_format.sample_rate
    };

    let planar_stereo_float = stream_format.format_id == kAudioFormatLinearPCM
        && stream_format.channels_per_frame == 2
        && stream_format.bits_per_channel == 32
        && stream_format.bytes_per_frame == 4
        && stream_format.sample_rate.is_finite()
        && stream_format.sample_rate > 0.0
        && stream_format.sample_rate <= f64::from(i32::MAX)
        && stream_format.format_flags
            & (kAudioFormatFlagIsFloat | kAudioFormatFlagIsNonInterleaved)
            == (kAudioFormatFlagIsFloat | kAudioFormatFlagIsNonInterleaved)
        && stream_format.format_flags & kAudioFormatFlagIsBigEndian == 0;
    let sample_rate = if planar_stereo_float {
        stream_format.sample_rate
    } else {
        sample_rate
    };
    if !planar_stereo_float && !is_supported_audio_format(&stream_format) {
        log_once!("AudioUnit stream format is not supported");
        audio_unit_host_object.is_running_handler = false;
        return;
    }

    let al_source = audio_unit_host_object.al_source.unwrap();
    let render_started_at = audio_unit_host_object.last_render_time;
    let mut al_buffers = Vec::new();
    unsafe {
        let mut buffers_processed = 0;
        context.GetSourcei(al_source, AL_BUFFERS_PROCESSED, &mut buffers_processed);
        while buffers_processed > 0 {
            let mut al_buffer = 0;
            context.SourceUnqueueBuffers(al_source, 1, &mut al_buffer);
            al_buffers.push(al_buffer);
            context.GetSourcei(al_source, AL_BUFFERS_PROCESSED, &mut buffers_processed);
        }
        assert_eq!(context.GetError(), 0);
    }

    let now = Instant::now();

    // Calculate number of frames by checking how much time passed since
    // the last render. Limit to 100ms to prevent delay from adding up
    // if it's been too long since the last render.
    // Ace Combat Xi relies on it being 2048 frames (at 48000Hz, 42ms) or under
    // If it's higher, flawed game logic causes it to call memset in a loop for
    // every frame over 2048 until it reaches the provided frame number.
    // TODO: Verify if this behavior is right
    let elapsed_time = now.duration_since(audio_unit_host_object.last_render_time.unwrap());
    let render_rate = if audio_unit_host_object.mixer_voice {
        sample_rate * f64::from(audio_unit_host_object.mixer_rate)
    } else {
        sample_rate
    };
    let number_frames = render_frame_count(
        elapsed_time.as_secs_f64(),
        render_rate,
        audio_unit_host_object.maximum_frames_per_slice,
    );

    let bytes_per_channel = stream_format.bits_per_channel / 8;
    let actual_bytes_per_frame = stream_format.channels_per_frame * bytes_per_channel;

    let buffer_size = number_frames
        * if planar_stereo_float {
            bytes_per_channel
        } else {
            actual_bytes_per_frame
        };

    // Alloc callback arguments
    let action_flags = env.mem.alloc_and_write(0);

    let (audio_buffer_list, buffer1Data, buffer2Data): (
        MutVoidPtr,
        MutVoidPtr,
        Option<MutVoidPtr>,
    ) = if input_stream_format.is_some() && !planar_stereo_float {
        let bufferData = env.mem.alloc(buffer_size);
        let audio_buffer_list: AudioBufferList<1> = AudioBufferList {
            number_buffers: 1,
            buffers: [AudioBuffer {
                number_channels: stream_format.channels_per_frame,
                data_byte_size: buffer_size,
                data: bufferData,
            }],
        };
        (
            env.mem.alloc_and_write(audio_buffer_list).cast(),
            bufferData,
            None,
        )
    } else {
        // Resident Evil 4 expects 2 buffers
        // though it copies the same data to both
        let buffer1Data = env.mem.alloc(buffer_size);
        let buffer2Data = env.mem.alloc(buffer_size);
        let audio_buffer_list: AudioBufferList<2> = AudioBufferList {
            number_buffers: 2,
            buffers: [
                AudioBuffer {
                    number_channels: if planar_stereo_float {
                        1
                    } else {
                        stream_format.channels_per_frame
                    },
                    data_byte_size: buffer_size,
                    data: buffer1Data,
                },
                AudioBuffer {
                    number_channels: if planar_stereo_float {
                        1
                    } else {
                        stream_format.channels_per_frame
                    },
                    data_byte_size: buffer_size,
                    data: buffer2Data,
                },
            ],
        };
        (
            env.mem.alloc_and_write(audio_buffer_list).cast(),
            buffer1Data,
            Some(buffer2Data),
        )
    };

    // Run render callback
    let input_callback = audio_unit_host_object.render_callback;
    let callback_bus = audio_unit_host_object.callback_bus;
    let sample_time = audio_unit_host_object.sample_time;
    let host_time = crate::libc::mach::time::mach_absolute_time(env);
    let timestamp = env.mem.alloc_and_write(AudioTimeStamp {
        sample_time,
        host_time,
        rate_scalar: 1.0,
        world_clock_type: 0,
        SMPTE_time: SMPTETime {
            subframes: 0,
            subframe_divisor: 0,
            counter: 0,
            type_: 0,
            flags: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
            frames: 0,
        },
        flags: 1 | 2 | 4,
        _reserved: 0,
    });
    let call = RenderNotification {
        unit: audio_unit,
        source: al_source,
        started_at: render_started_at,
        flags: action_flags,
        timestamp: timestamp.cast_const().cast(),
        frames: number_frames,
        data: audio_buffer_list,
        bus: callback_bus,
    };
    call_render_notifies(env, &call, 1 << 2);
    if render_is_active(env, &call) {
        env.mem.write(action_flags, 0);
        if let Some(AURenderCallbackStruct {
            input_proc,
            input_proc_ref_con,
        }) = input_callback
        {
            let _: OSStatus = input_proc.call_from_host(
                env,
                (
                    input_proc_ref_con,
                    action_flags,
                    timestamp.cast_const(),
                    callback_bus,
                    number_frames,
                    audio_buffer_list,
                ),
            );
        }
    }
    if render_is_active(env, &call) {
        call_render_notifies(env, &call, 1 << 3);
    }
    env.mem.free(timestamp.cast());

    // Guest callbacks may stop or dispose the unit while a render is pending.
    let object = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&audio_unit);
    let still_rendering = object.as_ref().is_some_and(|unit| {
        unit.started
            && unit.al_source == Some(al_source)
            && unit.last_render_time == render_started_at
    });
    if !still_rendering {
        if let Some(unit) = object {
            unit.is_running_handler = false;
        }
        if !al_buffers.is_empty() {
            let context = env
                .framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(env.openal_manager.as_mut());
            unsafe {
                context.DeleteBuffers(al_buffers.len() as i32, al_buffers.as_ptr());
            }
        }
        env.mem.free(action_flags.cast());
        env.mem.free(buffer1Data.cast());
        if let Some(buffer) = buffer2Data {
            env.mem.free(buffer.cast());
        }
        env.mem.free(audio_buffer_list.cast());
        return;
    }

    let at_state = &mut env.framework_state.audio_toolbox;
    let context = at_state
        .al_context
        .make_al_context_current(env.openal_manager.as_mut());

    let (al_format, processed_data) = if planar_stereo_float {
        let output = interleave_float_stereo(
            env.mem.bytes_at(buffer1Data.cast(), buffer_size),
            env.mem.bytes_at(buffer2Data.unwrap().cast(), buffer_size),
        );
        (AL_FORMAT_STEREO16, output)
    } else {
        let (format, _, data) =
            decode_buffer(&env.mem, &stream_format, buffer1Data.cast(), buffer_size);
        (format, data)
    };

    unsafe {
        // Get an unqueued buffer or create a new one
        let al_buffer = al_buffers.pop().unwrap_or_else(|| {
            let mut al_buffer = 0;
            context.GenBuffers(1, &mut al_buffer);
            al_buffer
        });

        context.BufferData(
            al_buffer,
            al_format,
            processed_data.as_ptr() as *const ALvoid,
            processed_data.len().try_into().unwrap(),
            sample_rate as i32,
        );
        context.SourceQueueBuffers(al_source, 1, &al_buffer);

        let mut al_source_state = 0;
        context.GetSourcei(al_source, AL_SOURCE_STATE, &mut al_source_state);
        if al_source_state != AL_PLAYING {
            context.SourcePlay(al_source);
        }

        // TODO: Play buffer 2 (In RE4 its the same as buffer 1 though)

        // Clear unused buffers
        if !al_buffers.is_empty() {
            context.DeleteBuffers(al_buffers.len() as i32, al_buffers.as_ptr());
        }

        assert_eq!(context.GetError(), 0);
    }

    // TODO: Do something with the action flags?
    env.mem.free(action_flags.cast_void());

    env.mem.free(buffer1Data.cast_void());
    if let Some(buffer2Data) = buffer2Data {
        env.mem.free(buffer2Data.cast_void());
    }

    env.mem.free(audio_buffer_list.cast_void());

    let audio_unit_host_object = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&audio_unit)
        .unwrap();
    // Reborrow as mutable to update the last render time

    audio_unit_host_object.last_render_time = Some(now);
    audio_unit_host_object.sample_time = sample_time + f64::from(number_frames);
    audio_unit_host_object.is_running_handler = false;
}

struct RenderNotification {
    unit: AudioUnit,
    source: ALuint,
    started_at: Option<Instant>,
    flags: MutPtr<u32>,
    timestamp: ConstVoidPtr,
    frames: u32,
    data: MutVoidPtr,
    bus: u32,
}

fn render_is_active(env: &mut Environment, call: &RenderNotification) -> bool {
    audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&call.unit)
        .is_some_and(|unit| {
            unit.started
                && unit.al_source == Some(call.source)
                && unit.last_render_time == call.started_at
        })
}

fn call_render_notifies(env: &mut Environment, call: &RenderNotification, phase: u32) {
    let callbacks = audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get(&call.unit)
        .map(|unit| unit.render_notifies.clone())
        .unwrap_or_default();
    for (callback, context, _) in callbacks {
        if !render_is_active(env, call) {
            break;
        }
        env.mem.write(call.flags, phase);
        let _: OSStatus = callback.call_from_host(
            env,
            (
                context,
                call.flags,
                call.timestamp,
                call.bus,
                call.frames,
                call.data,
            ),
        );
    }
}

fn render_frame_count(elapsed: f64, rate: f64, maximum: u32) -> u32 {
    ((elapsed * rate) as u32).min(2048).min(maximum)
}

fn interleave_float_stereo(left: &[u8], right: &[u8]) -> Vec<u8> {
    let left = float_pcm_to_i16(left);
    let right = float_pcm_to_i16(right);
    let mut output = Vec::with_capacity(left.len() + right.len());
    for (l, r) in left.as_chunks::<2>().0.iter().zip(right.as_chunks::<2>().0) {
        output.extend_from_slice(l);
        output.extend_from_slice(r);
    }
    output
}

#[cfg(test)]
#[test]
fn planar_float_audio_interleaves_channels() {
    let left: Vec<u8> = [0.5_f32, -1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let right: Vec<u8> = [-0.5_f32, 1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    let audio = interleave_float_stereo(&left, &right);
    let samples: Vec<i16> = audio
        .as_chunks::<2>()
        .0
        .iter()
        .map(|bytes| i16::from_le_bytes(*bytes))
        .collect();
    assert_eq!(samples, [16384, -16384, -32767, 32767]);
}

#[cfg(test)]
#[test]
fn mixer_frame_count_tracks_playback_rate_and_caps() {
    assert_eq!(render_frame_count(0.01, 44100.0 * 0.5, 2048), 220);
    assert_eq!(render_frame_count(0.01, 44100.0 * 2.0, 2048), 882);
    assert_eq!(render_frame_count(1.0, 44100.0, 1024), 1024);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioUnitInitialize(_)),
    export_c_func!(AudioUnitUninitialize(_)),
    export_c_func!(AudioUnitAddPropertyListener(_, _, _, _)),
    export_c_func!(AudioUnitRemovePropertyListener(_, _, _)),
    export_c_func!(AudioUnitRemovePropertyListenerWithUserData(_, _, _, _)),
    export_c_func!(AudioUnitAddRenderNotify(_, _, _)),
    export_c_func!(AudioUnitRemoveRenderNotify(_, _, _)),
    export_c_func!(AudioUnitSetProperty(_, _, _, _, _, _)),
    export_c_func!(AudioUnitGetProperty(_, _, _, _, _, _)),
    export_c_func!(AudioUnitGetPropertyInfo(_, _, _, _, _, _)),
    export_c_func!(AudioUnitSetParameter(_, _, _, _, _, _)),
    export_c_func!(AudioUnitGetParameter(_, _, _, _, _)),
    export_c_func!(AudioUnitReset(_, _, _)),
    export_c_func!(AudioOutputUnitStart(_)),
    export_c_func!(AudioOutputUnitStop(_)),
];
