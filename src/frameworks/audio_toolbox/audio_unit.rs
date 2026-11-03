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
    AL_BUFFERS_PROCESSED, AL_BUFFERS_QUEUED, AL_FORMAT_STEREO16, AL_PLAYING, AL_SOURCE_STATE,
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

const kAudioOutputUnitProperty_EnableIO: AudioUnitPropertyID = 2003;
const kAudioOutputUnitProperty_IsRunning: AudioUnitPropertyID = 2001;

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

fn AudioUnitSetProperty(
    env: &mut Environment,
    in_unit: AudioUnit,
    in_id: AudioUnitPropertyID,
    in_scope: AudioUnitScope,
    in_element: AudioUnitElement,
    in_data: ConstVoidPtr,
    in_data_size: u32,
) -> OSStatus {
    if in_element != 0 || in_data.is_null() {
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
    if in_element != 0 || out_data.is_null() || io_data_size.is_null() {
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
    if element != 0
        || scope > kAudioUnitScope_Output
        || !audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .contains_key(&unit)
    {
        return paramErr;
    }
    let (size, writable) = match property {
        kAudioUnitProperty_StreamFormat => (guest_size_of::<AudioStreamBasicDescription>(), true),
        kAudioUnitProperty_SampleRate => (guest_size_of::<f64>(), true),
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
    let number_frames = ((elapsed_time.as_secs_f64() * sample_rate) as u32)
        .min(2048)
        .min(audio_unit_host_object.maximum_frames_per_slice);

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
                    0u32,
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
                0u32,
                call.frames,
                call.data,
            ),
        );
    }
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
    export_c_func!(AudioUnitReset(_, _, _)),
    export_c_func!(AudioOutputUnitStart(_)),
    export_c_func!(AudioOutputUnitStop(_)),
];
