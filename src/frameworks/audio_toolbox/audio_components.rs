/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioComponent.h` (Audio Component Services)

use std::collections::HashMap;
use std::time::Instant;

use crate::abi::GuestFunction;
use crate::audio::openal::al_types::ALuint;
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::frameworks::core_audio_types::{
    fourcc, kAudioFormatFlagIsAlignedHigh, kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked,
    kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM, AudioStreamBasicDescription,
};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};

const kAudioUnitType_Output: u32 = fourcc(b"auou");
const kAudioUnitSubType_RemoteIO: u32 = fourcc(b"rioc");
const kAudioUnitManufacturer_Apple: u32 = fourcc(b"appl");
const AVAILABLE_COMPONENTS: &[(u32, u32)] = &[
    (kAudioUnitType_Output, kAudioUnitSubType_RemoteIO),
    (fourcc(b"aumx"), fourcc(b"mcmx")),
    (fourcc(b"aumx"), fourcc(b"3dem")),
    (fourcc(b"aufc"), fourcc(b"conv")),
];

#[derive(Default)]
pub struct State {
    components: HashMap<AudioComponent, (u32, u32)>,
    pub audio_component_instances:
        HashMap<AudioComponentInstance, AudioComponentInstanceHostObject>,
}
impl State {
    pub fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.audio_components
    }
}

#[derive(Clone)]
pub struct AudioComponentInstanceHostObject {
    pub started: bool,
    pub initialized: bool,
    pub disposing: bool,
    pub maximum_frames_per_slice: u32,
    pub global_stream_format: AudioStreamBasicDescription,
    pub input_stream_format: Option<AudioStreamBasicDescription>,
    pub output_stream_format: Option<AudioStreamBasicDescription>,
    pub render_callback: Option<AURenderCallbackStruct>,
    pub last_render_time: Option<Instant>,
    pub sample_time: f64,
    pub al_source: Option<ALuint>,
    pub is_running_handler: bool,
    pub property_listeners: Vec<(u32, GuestFunction, MutVoidPtr)>,
    pub render_notifies: Vec<(GuestFunction, ConstVoidPtr, u32)>,
    pub mixer_buses: Option<HashMap<u32, AudioComponentInstance>>,
    pub mixer_bus_count: u32,
    pub callback_bus: u32,
    pub passthrough_converter: bool,
    pub mixer_subtype: Option<u32>,
    pub mixer_gain: f32,
    pub mixer_master_gain: f32,
    pub mixer_enabled: bool,
    pub mixer_master_enabled: bool,
    pub mixer_voice: bool,
    pub mixer_spatial: bool,
    pub mixer_rate: f32,
    pub mixer_azimuth: f32,
    pub mixer_elevation: f32,
    pub mixer_distance: f32,
}
impl Default for AudioComponentInstanceHostObject {
    fn default() -> Self {
        // Default values obtained from an iPod Touch 4 running iOS 6.1.6
        // through a test app built targetting iOS 2.0
        AudioComponentInstanceHostObject {
            started: false,
            initialized: false,
            disposing: false,
            // returning 1024 based on https://developer.apple.com/documentation/audiotoolbox/kaudiounitproperty_maximumframesperslice
            maximum_frames_per_slice: 1024,
            global_stream_format: AudioStreamBasicDescription {
                sample_rate: 44100.0,
                format_id: kAudioFormatLinearPCM,
                format_flags: kAudioFormatFlagIsFloat
                    | kAudioFormatFlagIsSignedInteger
                    | kAudioFormatFlagIsPacked
                    | kAudioFormatFlagIsAlignedHigh,
                bytes_per_packet: 4,
                frames_per_packet: 1,
                bytes_per_frame: 4,
                channels_per_frame: 2,
                bits_per_channel: 32,
                _reserved: 0,
            },
            input_stream_format: None,
            output_stream_format: None,
            render_callback: None,
            last_render_time: None,
            sample_time: 0.0,
            al_source: None,
            is_running_handler: false,
            property_listeners: Vec::new(),
            render_notifies: Vec::new(),
            mixer_buses: None,
            mixer_bus_count: 1,
            callback_bus: 0,
            passthrough_converter: false,
            mixer_subtype: None,
            mixer_gain: 1.0,
            mixer_master_gain: 1.0,
            mixer_enabled: true,
            mixer_master_enabled: true,
            mixer_voice: false,
            mixer_spatial: false,
            mixer_rate: 1.0,
            mixer_azimuth: 0.0,
            mixer_elevation: 0.0,
            mixer_distance: 0.0,
        }
    }
}

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
#[allow(dead_code)]
pub struct AURenderCallbackStruct {
    pub input_proc: AURenderCallback,
    pub input_proc_ref_con: ConstVoidPtr,
}
unsafe impl SafeRead for AURenderCallbackStruct {}

#[repr(C, packed)]
pub struct OpaqueAudioComponent {
    _pad: u8,
}
unsafe impl SafeRead for OpaqueAudioComponent {}

type AudioComponent = MutPtr<OpaqueAudioComponent>;

pub type AURenderCallback = GuestFunction;

#[repr(C, packed)]
pub struct OpaqueAudioComponentInstance {
    _pad: u8,
}
unsafe impl SafeRead for OpaqueAudioComponentInstance {}

pub type AudioComponentInstance = MutPtr<OpaqueAudioComponentInstance>;

#[repr(C, packed)]
struct AudioComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}
unsafe impl SafeRead for AudioComponentDescription {}

fn AudioComponentFindNext(
    env: &mut Environment,
    in_component: AudioComponent,
    in_desc: ConstPtr<AudioComponentDescription>,
) -> AudioComponent {
    if in_desc.is_null() {
        return MutPtr::null();
    }

    let desc = env.mem.read(in_desc);
    if (desc.component_manufacturer != 0
        && desc.component_manufacturer != kAudioUnitManufacturer_Apple)
        || desc.component_flags & desc.component_flags_mask != 0
    {
        return MutPtr::null();
    }
    let start = if in_component.is_null() {
        0
    } else {
        let Some(previous) = State::get(&mut env.framework_state)
            .components
            .get(&in_component)
            .copied()
        else {
            return MutPtr::null();
        };
        let Some(index) = AVAILABLE_COMPONENTS
            .iter()
            .position(|&candidate| candidate == previous)
        else {
            return MutPtr::null();
        };
        index + 1
    };
    let Some(&selected) = AVAILABLE_COMPONENTS[start..]
        .iter()
        .find(|&&(kind, subtype)| {
            (desc.component_type == 0 || desc.component_type == kind)
                && (desc.component_sub_type == 0 || desc.component_sub_type == subtype)
        })
    else {
        return MutPtr::null();
    };
    if let Some((&component, _)) = State::get(&mut env.framework_state)
        .components
        .iter()
        .find(|(_, value)| **value == selected)
    {
        return component;
    }
    let out_component = env.mem.alloc_and_write(OpaqueAudioComponent { _pad: 0 });
    State::get(&mut env.framework_state)
        .components
        .insert(out_component, selected);

    log_dbg!(
        "AudioComponentFindNext({:?}, {:?}) -> {:?}",
        in_component,
        in_desc,
        out_component
    );
    out_component
}

fn AudioComponentInstanceNew(
    env: &mut Environment,
    in_component: AudioComponent,
    out_instance: MutPtr<AudioComponentInstance>,
) -> OSStatus {
    if out_instance.is_null() {
        return paramErr;
    }
    let Some((kind, subtype)) = State::get(&mut env.framework_state)
        .components
        .get(&in_component)
        .copied()
    else {
        return paramErr;
    };
    let guest_instance = create_audio_unit_instance(env);
    let object = State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&guest_instance)
        .unwrap();
    if kind == fourcc(b"aumx") {
        object.mixer_buses = Some(HashMap::new());
        object.mixer_subtype = Some(subtype);
    } else if kind == fourcc(b"aufc") {
        object.passthrough_converter = true;
    }

    env.mem.write(out_instance, guest_instance);

    let result = 0; // success
    log_dbg!(
        "AudioComponentInstanceNew({:?}, {:?}) -> {:?}",
        in_component,
        out_instance,
        result
    );
    result
}

pub(super) fn create_audio_unit_instance(env: &mut Environment) -> AudioComponentInstance {
    let guest_instance = env
        .mem
        .alloc_and_write(OpaqueAudioComponentInstance { _pad: 0 });
    State::get(&mut env.framework_state)
        .audio_component_instances
        .insert(guest_instance, AudioComponentInstanceHostObject::default());
    guest_instance
}

pub(super) fn AudioComponentInstanceDispose(
    env: &mut Environment,
    in_instance: AudioComponentInstance,
) -> OSStatus {
    let Some(object) = State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_instance)
    else {
        return paramErr;
    };
    if object.disposing {
        return paramErr;
    }
    object.disposing = true;
    let (started, initialized) = (object.started, object.initialized);
    if started {
        super::audio_unit::AudioOutputUnitStop(env, in_instance);
    }
    if initialized {
        super::audio_unit::AudioUnitUninitialize(env, in_instance);
    }
    let children = State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&in_instance)
        .and_then(|object| object.mixer_buses.take());
    if let Some(children) = children {
        for child in children.into_values() {
            AudioComponentInstanceDispose(env, child);
        }
    }
    State::get(&mut env.framework_state)
        .audio_component_instances
        .remove(&in_instance);
    env.mem.free(in_instance.cast());
    let result = 0;
    log_dbg!(
        "AudioComponentInstanceDispose({:?}) -> {:?}",
        in_instance,
        result
    );
    result
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioComponentFindNext(_, _)),
    export_c_func!(AudioComponentInstanceNew(_, _)),
    export_c_func!(AudioComponentInstanceDispose(_)),
];
