/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioServices.h` (Audio Services)

use crate::frameworks::core_audio_types::debug_fourcc;
use crate::mem::ConstVoidPtr;
use std::collections::HashMap;

use crate::abi::{CallFromHost, GuestFunction};
use crate::audio::openal as al;
use crate::audio::openal::al_types::*;
use crate::audio::openal::{OpenAL, OpenALManager};

use super::audio_queue::decode_buffer;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{fourcc, AudioStreamBasicDescription};
use crate::frameworks::core_foundation::cf_run_loop::{
    kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoopGetMain,
};
use crate::frameworks::core_foundation::cf_url::CFURLRef;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::ns_url::to_rust_path;
use crate::mem::{GuestUSize, MutPtr, MutVoidPtr};
use crate::objc::{id, nil, release, retain};
use crate::{audio, Environment};

/// Usually a FourCC.
type AudioServicesPropertyID = u32;
type SystemSoundID = u32;

const kAudioServicesUnsupportedPropertyError: OSStatus = fourcc(b"pty?") as _;
const kSystemSoundID_Vibrate: SystemSoundID = 0x00000FFF;
const kAudioServicesSystemSoundUnspecifiedError: OSStatus = -1500;

const INITIAL_SYSTEM_SOUND_ID: SystemSoundID = 0x1001;

struct SystemSoundData {
    al_source: ALuint,
    al_buffer: ALuint,
}

#[derive(Clone, Copy)]
struct Completion {
    callback: GuestFunction,
    context: MutVoidPtr,
    run_loop: id,
    armed: bool,
    generation: u64,
}

pub struct State {
    sounds: HashMap<SystemSoundID, SystemSoundData>,
    data_top: SystemSoundID,
    completions: HashMap<SystemSoundID, Completion>,
    next_generation: u64,
}

impl Default for State {
    fn default() -> Self {
        Self {
            sounds: Default::default(),
            data_top: INITIAL_SYSTEM_SOUND_ID,
            completions: HashMap::new(),
            next_generation: 0,
        }
    }
}

impl State {
    fn get_with_context<'s, 'm: 's>(
        framework_state: &'s mut crate::frameworks::State,
        manager: &'m mut OpenALManager,
    ) -> (&'s mut Self, OpenAL<'s>) {
        let toolbox = &mut framework_state.audio_toolbox;
        (
            &mut toolbox.audio_services,
            toolbox.al_context.make_al_context_current(manager),
        )
    }
}

fn AudioServicesCreateSystemSoundID(
    env: &mut Environment,
    file: CFURLRef,
    out_system_sound_id: MutPtr<SystemSoundID>,
) -> OSStatus {
    if file == nil || out_system_sound_id.is_null() {
        return paramErr;
    }
    let path = to_rust_path(env, file);
    let Ok(mut audio_file) = audio::AudioFile::open_for_reading(&path, &env.fs) else {
        log!(
            "Warning: Failed to open audio file {:?} for AudioServicesCreateSystemSoundID()",
            path
        );
        return kAudioServicesSystemSoundUnspecifiedError;
    };

    let mut data = vec![0; audio_file.byte_count().try_into().unwrap()];
    let format =
        AudioStreamBasicDescription::from_audio_description(audio_file.audio_description());
    let size = audio_file.read_bytes(0, data.as_mut_slice()).unwrap();
    let tmp = env.mem.alloc(size as GuestUSize);
    env.mem
        .bytes_at_mut(tmp.cast(), size as GuestUSize)
        .copy_from_slice(data.as_slice());
    let (al_format, al_frequency, data) =
        decode_buffer(&env.mem, &format, tmp.cast(), size as GuestUSize);
    env.mem.free(tmp.cast());

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    // TODO: This should only support linear pcm and ima4, but also supports
    // mp3 here since AudioFile supports it. We also aren't checking for length.
    let mut al_source = 0;
    unsafe {
        context.GenSources(1, &mut al_source);
        assert!(context.GetError() == 0);
    }

    let mut al_buffer = 0;
    unsafe {
        context.GenBuffers(1, &mut al_buffer);
        context.BufferData(
            al_buffer,
            al_format,
            data.as_ptr() as *const ALvoid,
            data.len().try_into().unwrap(),
            al_frequency,
        );
        context.Sourcei(al_source, al::AL_BUFFER, al_buffer.try_into().unwrap());
        assert!(context.GetError() == 0);
    }
    let sys_data = SystemSoundData {
        al_source,
        al_buffer,
    };
    state.sounds.insert(state.data_top, sys_data);
    env.mem.write(out_system_sound_id, state.data_top);
    state.data_top = state.data_top.checked_add(1).unwrap();
    0
}

fn AudioServicesGetProperty(
    _env: &mut Environment,
    in_property_id: AudioServicesPropertyID,
    _in_specifier_size: u32,
    _in_specifier: crate::mem::ConstVoidPtr,
    _io_property_data_size: MutPtr<u32>,
    _out_property_data: MutVoidPtr,
) -> OSStatus {
    // Crash Bandicoot Nitro Kart 3D tries to use this property ID, which does
    // not seem to be documented anywhere? Assuming this is a bug.
    if in_property_id == 0xfff {
        kAudioServicesUnsupportedPropertyError
    } else {
        unimplemented!();
    }
}

fn AudioServicesPlaySystemSound(env: &mut Environment, sys_sound_id: SystemSoundID) {
    if sys_sound_id == kSystemSoundID_Vibrate {
        log!("TODO: vibration (AudioServicesPlaySystemSound)");
    } else {
        let (state, context) =
            State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

        if let Some(SystemSoundData {
            al_source,
            al_buffer: _,
        }) = state.sounds.get(&sys_sound_id)
        {
            unsafe {
                let al_source = *al_source;
                context.SourcePlay(al_source);
                assert!(context.GetError() == 0);
                let mut al_state: i32 = 0;
                context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_state as *mut i32);
                assert!(context.GetError() == 0);
                assert!(
                    al_state == al::AL_PLAYING || al_state == al::AL_STOPPED,
                    "Unexpected source state after SourcePlay: {:#x}",
                    al_state
                );
            }
        } else {
            panic!(
                "Incorrect/unsupported system sound {:x} played!",
                sys_sound_id
            );
        }
    }
    let state = &mut env.framework_state.audio_toolbox.audio_services;
    state.next_generation = state.next_generation.wrapping_add(1);
    if let Some(completion) = state.completions.get_mut(&sys_sound_id) {
        completion.armed = true;
        completion.generation = state.next_generation;
    }
    // TODO: implement other system sounds
}

fn AudioServicesDisposeSystemSoundID(
    env: &mut Environment,
    sys_sound_id: SystemSoundID,
) -> OSStatus {
    AudioServicesRemoveSystemSoundCompletion(env, sys_sound_id);
    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    if let Some(SystemSoundData {
        al_source,
        al_buffer,
    }) = state.sounds.remove(&sys_sound_id)
    {
        unsafe {
            context.SourceStop(al_source);
            context.DeleteSources(1, &al_source as *const ALuint);
            context.DeleteBuffers(1, &al_buffer as *const ALuint);
            assert!(context.GetError() == 0);
        }
        0
    } else {
        // This is also true of kSystemSoundID_Vibrate.
        log!("Tried to dispose of invalid system sound {}!", sys_sound_id);
        kAudioServicesSystemSoundUnspecifiedError
    }
}

fn AudioServicesPlayAlertSound(env: &mut Environment, sound: SystemSoundID) {
    AudioServicesPlaySystemSound(env, sound);
}

fn AudioServicesRemoveSystemSoundCompletion(env: &mut Environment, sound: SystemSoundID) {
    if let Some(completion) = env
        .framework_state
        .audio_toolbox
        .audio_services
        .completions
        .remove(&sound)
    {
        release(env, completion.run_loop);
    }
}

fn AudioServicesAddSystemSoundCompletion(
    env: &mut Environment,
    sound: SystemSoundID,
    run_loop: id,
    mode: id,
    callback: GuestFunction,
    context: MutVoidPtr,
) -> OSStatus {
    if callback.to_ptr().is_null() {
        AudioServicesRemoveSystemSoundCompletion(env, sound);
        return 0;
    }
    if sound != kSystemSoundID_Vibrate
        && !env
            .framework_state
            .audio_toolbox
            .audio_services
            .sounds
            .contains_key(&sound)
    {
        return kAudioServicesSystemSoundUnspecifiedError;
    }
    if mode != nil {
        let mode = ns_string::to_rust_string(env, mode);
        if mode != kCFRunLoopDefaultMode && mode != kCFRunLoopCommonModes {
            return kAudioServicesSystemSoundUnspecifiedError;
        }
    }
    let state = &env.framework_state.audio_toolbox.audio_services;
    let mut armed = state
        .completions
        .get(&sound)
        .is_some_and(|completion| completion.armed);
    let source = state.sounds.get(&sound).map(|data| data.al_source);
    if !armed {
        if let Some(source) = source {
            let (_, context) =
                State::get_with_context(&mut env.framework_state, &mut env.openal_manager);
            let mut status = 0;
            unsafe {
                context.GetSourcei(source, al::AL_SOURCE_STATE, &mut status);
            }
            armed = unsafe { context.GetError() } == 0 && status == al::AL_PLAYING;
        }
    }
    let run_loop = if run_loop == nil {
        CFRunLoopGetMain(env)
    } else {
        run_loop
    };
    retain(env, run_loop);
    let state = &mut env.framework_state.audio_toolbox.audio_services;
    state.next_generation = state.next_generation.wrapping_add(1);
    let old = state.completions.insert(
        sound,
        Completion {
            callback,
            context,
            run_loop,
            armed,
            generation: state.next_generation,
        },
    );
    if let Some(old) = old {
        release(env, old.run_loop);
    }
    0
}

/// Observe completed OpenAL sources on the registered run loop, then invoke
/// guest callbacks without retaining borrows across guest reentry.
pub(crate) fn tick_system_sound_completions(env: &mut Environment, run_loop: id) {
    let state = &env.framework_state.audio_toolbox.audio_services;
    let mut finished = Vec::new();
    let mut pending = Vec::new();
    for (&sound, completion) in &state.completions {
        if !completion.armed || completion.run_loop != run_loop {
            continue;
        }
        if sound == kSystemSoundID_Vibrate {
            finished.push((sound, completion.generation));
        } else if let Some(data) = state.sounds.get(&sound) {
            pending.push((sound, data.al_source, completion.generation));
        }
    }
    if !pending.is_empty() {
        let (_, context) =
            State::get_with_context(&mut env.framework_state, &mut env.openal_manager);
        for (sound, source, generation) in pending {
            let mut status = 0;
            unsafe {
                context.GetSourcei(source, al::AL_SOURCE_STATE, &mut status);
            }
            let error = unsafe { context.GetError() };
            if error == 0 && status == al::AL_STOPPED {
                finished.push((sound, generation));
            }
        }
    }
    for (sound, generation) in finished {
        let Some(completion) = env
            .framework_state
            .audio_toolbox
            .audio_services
            .completions
            .get_mut(&sound)
        else {
            continue;
        };
        if !completion.armed || completion.generation != generation {
            continue;
        }
        completion.armed = false;
        let callback = completion.callback;
        let context = completion.context;
        let _: () = callback.call_from_host(env, (sound, context));
    }
}

fn AudioServicesSetProperty(
    _env: &mut Environment,
    in_property_id: AudioServicesPropertyID,
    _in_specifier_size: u32,
    _in_specifier: ConstVoidPtr,
    _in_property_data_size: u32,
    _in_property_data: ConstVoidPtr,
) -> OSStatus {
    log!(
        "AudioServicesSetProperty: property {} is unimplemented",
        debug_fourcc(in_property_id)
    );
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioServicesCreateSystemSoundID(_, _)),
    export_c_func!(AudioServicesGetProperty(_, _, _, _, _)),
    export_c_func!(AudioServicesPlaySystemSound(_)),
    export_c_func!(AudioServicesPlayAlertSound(_)),
    export_c_func!(AudioServicesAddSystemSoundCompletion(_, _, _, _, _)),
    export_c_func!(AudioServicesRemoveSystemSoundCompletion(_)),
    export_c_func!(AudioServicesDisposeSystemSoundID(_)),
    export_c_func!(AudioServicesSetProperty(_, _, _, _, _)),
];
