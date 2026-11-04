/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! OpenAL.
//!
//! This is a thin layer on top of OpenAL Soft, see [crate::audio::openal].
//!
//! Resources:
//! - [OpenAL 1.1 specification](https://www.openal.org/documentation/openal-1.1-specification.pdf)
//! - Apple's [Technical Note TN2199: OpenAL FAQ for iPhone OS](https://web.archive.org/web/20090826202158/http://developer.apple.com/iPhone/library/technotes/tn2008/tn2199.html) (also available [here](https://developer.apple.com/library/archive/technotes/tn2199/_index.html))

use crate::abi::{CallFromHost, GuestFunction};
use crate::audio::openal as al;
use crate::audio::openal::al_types::*;
use crate::audio::openal::alc_types::*;
use crate::audio::openal::{
    OpenAL, OpenALContext, ALC_DEVICE_SPECIFIER, ALC_FREQUENCY, ALC_MONO_SOURCES, ALC_REFRESH,
    ALC_STEREO_SOURCES, ALC_SYNC, AL_EXTENSIONS, AL_RENDERER, AL_VENDOR, AL_VERSION,
};
use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::libc::string::strcmp;
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeWrite};
use crate::Environment;
use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/OpenAL.framework/OpenAL",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

#[derive(Default)]
pub struct State {
    devices: HashMap<MutPtr<GuestALCdevice>, *mut ALCdevice>,
    contexts: HashMap<MutPtr<GuestALCcontext>, OpenALContext>,
    strings_cache: HashMap<ALenum, ConstPtr<u8>>,
    current_ctx: MutPtr<GuestALCcontext>,
    warned_invalid_contexts: HashSet<MutPtr<GuestALCcontext>>,
    warned_invalid_devices: HashSet<MutPtr<GuestALCdevice>>,
    warned_null_device: bool,
    macosx_rendering_quality: ALint,
    warned_mixer_rate: bool,
    warned_mixer_get_rate: bool,
    warned_mixer_output_rate_proc: bool,
    warned_invalid_host_devices: HashSet<*mut ALCdevice>,
    /// Registered with `alSourceAddNotification` (see [handle_notifications]).
    notifications: Vec<SourceNotification>,
    audio_test_uploads: u32,
    audio_test_plays: u32,
    /// Buffers used in a streaming queue must never be edge-ramped.
    audio_test_stream_buffers: HashSet<(usize, ALuint)>,
    audio_test_original: HashMap<(usize, ALuint), AudioTestBuffer>,
    audio_test_meta: HashMap<(usize, ALuint), String>,
    audio_trace: crate::audio::sound_trace::Trace,
    audio_test_sources: HashSet<(usize, ALuint)>,
}
struct AudioTestBuffer {
    format: ALenum,
    rate: ALsizei,
    data: Vec<u8>,
}

/// Save one event in memory; write only around a user-marked noise.
fn audio_trace_event(env: &mut Environment, event: String) {
    if audio_test_mode(env).is_none() {
        return;
    }
    let at = crate::audio::sound_trace::timestamp_ms();
    let follow = State::get(env)
        .audio_trace
        .record(at, std::time::Instant::now(), event.clone());
    if follow {
        crate::performance::audio_event(&format!("AUDIO EVENT ms={at} {event}"));
    }
}

/// Snapshot the source before changing it. Queries do not consume AL
/// errors; invalid sources are checked without adding query errors.
fn audio_trace_source(env: &mut Environment, source: ALuint, action: &str) {
    let Some(mode) = audio_test_mode(env) else {
        return;
    };
    let Some(context) = State::try_make_current(env) else {
        return;
    };
    let mut values = [0; 6];
    let mut gain = 0.0;
    let mut pitch = 0.0;
    if unsafe { context.IsSource(source) } == 0 {
        return;
    }
    unsafe {
        for (param, value) in [0x1009, 0x1010, 0x1007, 0x1015, 0x1016, 0x1025]
            .into_iter()
            .zip(&mut values)
        {
            context.GetSourcei(source, param, value);
        }
        context.GetSourcef(source, 0x100A, &mut gain);
        context.GetSourcef(source, 0x1003, &mut pitch);
    }
    let state = State::get(env);
    let key = state.audio_test_key(values[0] as u32);
    let meta = state
        .audio_test_meta
        .get(&key)
        .map(String::as_str)
        .unwrap_or("queued_or_unknown_buffer");
    let event = format!("mode={mode:?} {action} source={source} buffer={} state={:#x} loop={} queued={} processed={} sample_offset={} gain={gain} pitch={pitch} {meta}", values[0], values[1], values[2], values[3], values[4], values[5]);
    audio_trace_event(env, event);
}

/// Poll on the emulation thread, never the Java UI or native mixer thread.
pub fn capture_audio_marker(env: &mut Environment) {
    if audio_test_mode(env).is_none() {
        return;
    }
    let marker = crate::audio::sound_trace::take_request();
    if marker == 0 {
        return;
    }
    let report = State::get(env)
        .audio_trace
        .capture(marker, std::time::Instant::now());
    crate::performance::audio_event(&report);
    let pcm_report = State::get(env).audio_trace.save_pcm(marker);
    match pcm_report {
        Ok(report) => crate::performance::audio_event(&report),
        Err(error) => crate::performance::audio_event(&format!("PCM capture failed: {error}")),
    }
    let state = State::get(env);
    let device = state.audio_test_key(0).0;
    let sources: Vec<_> = state
        .audio_test_sources
        .iter()
        .filter(|(d, _)| *d == device)
        .map(|&(_, source)| source)
        .take(64)
        .collect();
    for source in sources {
        audio_trace_source(env, source, "at_noise_mark");
    }
}

fn audio_test_mode(env: &Environment) -> Option<crate::audio::sound_test::Mode> {
    (env.bundle.bundle_identifier() == "com.rockstargames.gta3vc")
        .then_some(env.options.audio_test)
        .flatten()
}

impl State {
    fn audio_test_key(&self, buffer: ALuint) -> (usize, ALuint) {
        let device = self
            .contexts
            .get(&self.current_ctx)
            .map_or(0, |context| context.GetContextsDevice() as usize);
        (device, buffer)
    }

    fn get(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.openal
    }

    fn try_make_current(env: &mut Environment) -> Option<OpenAL<'_>> {
        let state = &mut env.framework_state.openal;
        state
            .contexts
            .get_mut(&state.current_ctx)
            .map(|ctx| ctx.make_current(&mut env.openal_manager))
    }
}

/// Opaque type in guest memory standing in for [ALCdevice] in host memory.
struct GuestALCdevice {
    _filler: u8,
}
impl SafeWrite for GuestALCdevice {}
/// Opaque type in guest memory standing in for [ALCcontext] in host memory.
struct GuestALCcontext {
    _filler: u8,
}
impl SafeWrite for GuestALCcontext {}

macro_rules! try_get_context {
    ($env: ident, $name: ident) => {
        let state = &mut $env.framework_state.openal;
        let ctx_ptr = state.current_ctx;
        let Some($name) = state
            .contexts
            .get_mut(&ctx_ptr)
            .map(|ctx| ctx.make_current(&mut $env.openal_manager))
        else {
            let state = State::get($env);
            if state.warned_invalid_contexts.insert(ctx_ptr) {
                log!(
                    "Attempted to get context but currently active context {:?} is invalid, skipping!",
                    ctx_ptr
                );
            }
            // TODO: set error
            return;
        };
    };
    ($env: ident, $name: ident, $rval: expr) => {
        let state = &mut $env.framework_state.openal;
        let ctx_ptr = state.current_ctx;
        let Some($name) = state
            .contexts
            .get_mut(&ctx_ptr)
            .map(|ctx| ctx.make_current(&mut $env.openal_manager))
        else {
            let state = State::get($env);
            if state.warned_invalid_contexts.insert(ctx_ptr) {
                log!(
                    "Attempted to get context but currently active context {:?} is invalid, skipping!",
                    ctx_ptr
                );
            }
            // TODO: set error
            return $rval;
        };
    };
}

// === alc.h ===

fn alcOpenDevice(env: &mut Environment, devicename: ConstPtr<u8>) -> MutPtr<GuestALCdevice> {
    // Some apps hardcode the device names from older Apple/Creative OpenAL
    // implementations. Treat those as a request for the default device.
    let is_generic_name = !devicename.is_null()
        && matches!(
            env.mem.cstr_at_utf8(devicename),
            Ok("Generic Hardware" | "Generic Software")
        );
    if !devicename.is_null() && !is_generic_name {
        // If device name name is not null, we check if it's the one which was
        // obtained from a call to alcGetString(NULL, ALC_DEVICE_SPECIFIER).
        // And if not, we return NULL.

        let d_name = alcGetString(env, Ptr::null(), ALC_DEVICE_SPECIFIER);
        if strcmp(env, d_name, devicename) != 0 {
            log!(
                "Unsupported device name {:?}, supported is {:?}. Returning NULL",
                env.mem.cstr_at_utf8(devicename),
                env.mem.cstr_at_utf8(d_name)
            );
            env.mem.free(d_name.cast_mut().cast());
            return Ptr::null();
        }
        env.mem.free(d_name.cast_mut().cast());
    }

    let res = unsafe { al::alcOpenDevice(std::ptr::null()) };
    if res.is_null() {
        log_dbg!("alcOpenDevice(NULL) returned NULL");
        return Ptr::null();
    }
    al::register_device(res);

    let guest_res = env.mem.alloc_and_write(GuestALCdevice { _filler: 0 });
    let state = State::get(env);
    state.devices.insert(guest_res, res);
    state.warned_invalid_devices.remove(&guest_res);
    // Remove from host-side device warnings as host pointer is reused.
    state.warned_invalid_host_devices.remove(&res);
    log_dbg!("alcOpenDevice(NULL) => {:?} (host: {:?})", guest_res, res,);
    guest_res
}
fn alcCloseDevice(env: &mut Environment, device: MutPtr<GuestALCdevice>) -> bool {
    if device.is_null() {
        let state = State::get(env);
        if !state.warned_null_device {
            log!("alcCloseDevice() is called with NULL device, ignoring");
            state.warned_null_device = true;
        }
        return false;
    }
    let Some(host_device) = State::get(env).devices.remove(&device) else {
        if State::get(env).warned_invalid_devices.insert(device) {
            log!(
                "alcCloseDevice({:?}) called for unknown/already-closed device, ignoring",
                device
            );
        }
        return false;
    };
    let state = State::get(env);
    state
        .audio_test_original
        .retain(|&(device, _), _| device != host_device as usize);
    state
        .audio_test_stream_buffers
        .retain(|&(device, _)| device != host_device as usize);
    state
        .audio_test_meta
        .retain(|&(device, _), _| device != host_device as usize);
    State::get(env)
        .audio_test_sources
        .retain(|&(device, _)| device != host_device as usize);
    env.mem.free(device.cast());
    al::unregister_device(host_device);
    let res = unsafe { al::alcCloseDevice(host_device) };
    log_dbg!("alcCloseDevice({:?}) => {:?}", device, res,);
    res != al::ALC_FALSE
}

fn alcGetError(env: &mut Environment, device: MutPtr<GuestALCdevice>) -> i32 {
    let Some(&host_device) = State::get(env).devices.get(&device) else {
        if State::get(env).warned_invalid_devices.insert(device) {
            log!(
                "alcGetError({:?}): unknown guest device, returning ALC_INVALID_DEVICE",
                device
            );
        }
        return al::ALC_INVALID_DEVICE;
    };

    let res = unsafe { al::alcGetError(host_device) };
    log_dbg!("alcGetError({:?}) => {:#x}", host_device, res);
    res
}

fn alcGetString(
    env: &mut Environment,
    device: MutPtr<GuestALCdevice>,
    param: ALenum,
) -> ConstPtr<u8> {
    assert!(device.is_null());

    let res = unsafe { al::alcGetString(std::ptr::null_mut(), param) };
    let s = unsafe { CStr::from_ptr(res) };
    log_dbg!("alcGetString({:?}) => {:?}", param, s);
    log!("TODO: alcGetString({}) leaks memory", param);
    env.mem.alloc_and_write_cstr(s.to_bytes()).cast_const()
}

const STANDARD_CONTEXT_ATTRIBUTES: [ALCint; 5] = [
    ALC_FREQUENCY,
    ALC_REFRESH,
    ALC_SYNC,
    ALC_MONO_SOURCES,
    ALC_STEREO_SOURCES,
];

fn alcCreateContext(
    env: &mut Environment,
    device: MutPtr<GuestALCdevice>,
    attr_list: ConstPtr<i32>,
) -> MutPtr<GuestALCcontext> {
    let attr_list_ptr: *const ALCint = if attr_list.is_null() {
        std::ptr::null()
    } else {
        let mut ptr: MutPtr<i32> = attr_list.cast_mut();
        // attribute list is NULL terminated
        while env.mem.read(ptr) != 0 {
            let attr = env.mem.read(ptr);
            log_dbg!(
                "alcCreateContext attribute {:#x} => {}",
                attr,
                env.mem.read(ptr + 1)
            );
            if !STANDARD_CONTEXT_ATTRIBUTES.contains(&attr) {
                // TODO: strip the attribute instead of passing through?
                log!(
                    "Warning: alcCreateContext requested non-standard attribute: {:#x} => {}",
                    attr,
                    env.mem.read(ptr + 1)
                );
            }
            ptr += 2;
        }

        let list_size = Ptr::to_bits(ptr) - Ptr::to_bits(attr_list);
        let attr_list_slice = env.mem.bytes_at(attr_list.cast(), list_size);
        attr_list_slice.as_ptr() as *const _
    };

    let state = State::get(env);
    let Some(&host_device) = state.devices.get(&device) else {
        if state.warned_invalid_devices.insert(device) {
            log!(
                "alcCreateContext({:?}, (...)): unknown guest device, returning NULL",
                device
            );
        }
        return Ptr::null();
    };

    let res = unsafe {
        OpenALContext::new_with_device_and_attrlist(
            env.openal_manager.as_mut(),
            host_device,
            attr_list_ptr,
        )
    };
    let Ok(ctx) = res else {
        log_dbg!("alcCreateContext({:?}, (...)) returned NULL", device);
        return Ptr::null();
    };

    let guest_res = env.mem.alloc_and_write(GuestALCcontext { _filler: 0 });

    log_dbg!(
        "alcCreateContext({:?}, NULL) => {:?} (host: {:?})",
        device,
        guest_res,
        ctx,
    );

    let state = State::get(env);
    state.contexts.insert(guest_res, ctx);
    state.warned_invalid_contexts.remove(&guest_res);
    guest_res
}

fn alcDestroyContext(env: &mut Environment, context: MutPtr<GuestALCcontext>) {
    if context.is_null() {
        log_dbg!("alcDestroyContext() is called with NULL context, ignoring");
        return;
    }

    let state = State::get(env);

    if state.current_ctx == context {
        state.current_ctx = Ptr::null();
    }

    let Some(_host_context) = state.contexts.remove(&context) else {
        // Don’t crash on shutdown / double-destroy.
        if state.warned_invalid_contexts.insert(context) {
            log!(
                "alcDestroyContext({:?}) called for unknown/already-destroyed context, ignoring",
                context
            );
        }

        return;
    };

    env.mem.free(context.cast());
    log!("alcDestroyContext({:?})", context);
}

fn alcProcessContext(env: &mut Environment, context: MutPtr<GuestALCcontext>) {
    if context.is_null() {
        log_dbg!("alcProcessContext() is called with NULL context, ignoring");
        return;
    }
    let state = State::get(env);
    let Some(host_context) = state.contexts.get_mut(&context) else {
        if state.warned_invalid_contexts.insert(context) {
            log!(
                "alcProcessContext({:?}): unknown guest context, ignoring",
                context
            );
        }
        return;
    };
    host_context.ProcessContext()
}
fn alcSuspendContext(env: &mut Environment, context: MutPtr<GuestALCcontext>) {
    if context.is_null() {
        log_dbg!("alcSuspendContext() is called with NULL context, ignoring");
        return;
    }
    let state = State::get(env);
    let Some(host_context) = state.contexts.get_mut(&context) else {
        if state.warned_invalid_contexts.insert(context) {
            log!(
                "alcSuspendContext({:?}): unknown guest context, ignoring",
                context
            );
        }
        return;
    };
    host_context.SuspendContext()
}

fn alcMakeContextCurrent(env: &mut Environment, context: MutPtr<GuestALCcontext>) -> bool {
    let res = if context.is_null() || State::get(env).contexts.contains_key(&context) {
        State::get(env).current_ctx = context;
        true
    } else {
        false
    };
    log_dbg!("alcMakeContextCurrent({:?}) => {}", context, res);
    res
}

fn alcGetCurrentContext(env: &mut Environment) -> MutPtr<GuestALCcontext> {
    State::get(env).current_ctx
}

fn alcGetContextsDevice(
    env: &mut Environment,
    context: MutPtr<GuestALCcontext>,
) -> MutPtr<GuestALCdevice> {
    if context.is_null() {
        log_dbg!("alcGetContextsDevice() is called with NULL context, ignoring");
        return Ptr::null();
    }

    let state = State::get(env);

    let Some(host_context) = state.contexts.get(&context) else {
        if state.warned_invalid_contexts.insert(context) {
            log!(
                "alcGetContextsDevice(): unknown guest context {:?}, returning NULL",
                context
            );
        }
        return Ptr::null();
    };

    let host_device = host_context.GetContextsDevice();

    if host_device.is_null() {
        log_dbg!("alcGetContextsDevice(): host device is NULL, ignoring");
        return Ptr::null();
    }

    let Some((&guest_device, _)) = state
        .devices
        .iter()
        .find(|(_guest, &host)| host == host_device)
    else {
        if state.warned_invalid_host_devices.insert(host_device) {
            log!(
                "alcGetContextsDevice(): host device {:?} not found in mapping (shutdown race?), returning NULL",
                host_device
            );
        }
        return Ptr::null();
    };

    guest_device
}

/// An Apple extension, with which apps ask to be told when a source's state
/// changes or it finishes playing buffers.
struct SourceNotification {
    context: MutPtr<GuestALCcontext>,
    source: ALuint,
    notification_id: ALuint,
    /// `void (*)(ALuint sid, ALuint notificationID, ALvoid *userData)`
    callback: GuestFunction,
    user_data: MutVoidPtr,
    /// Value of the source's corresponding property when last checked.
    last_value: ALint,
}

const AL_SOURCE_STATE: ALuint = 0x1010;
const AL_BUFFERS_PROCESSED: ALuint = 0x1016;
/// Sent when a looping source starts again from the start of its queue.
const AL_QUEUE_HAS_LOOPED: ALuint = 0x9000;
const AL_LOOPING: ALenum = 0x1007;
const AL_SAMPLE_OFFSET: ALenum = 0x1025;

fn alSourceAddNotification(
    env: &mut Environment,
    source: ALuint,
    notification_id: ALuint,
    callback: GuestFunction,
    user_data: MutVoidPtr,
) -> ALenum {
    if notification_id != AL_SOURCE_STATE
        && notification_id != AL_BUFFERS_PROCESSED
        && notification_id != AL_QUEUE_HAS_LOOPED
    {
        log!(
            "TODO: alSourceAddNotification({}, {:#x}, ...): unsupported notification",
            source,
            notification_id
        );
        return 0;
    }
    let context = State::get(env).current_ctx;
    let last_value = source_notification_value(env, context, source, notification_id);
    State::get(env).notifications.push(SourceNotification {
        context,
        source,
        notification_id,
        callback,
        user_data,
        last_value,
    });
    0 // AL_NO_ERROR
}

fn alSourceRemoveNotification(
    env: &mut Environment,
    source: ALuint,
    notification_id: ALuint,
    callback: GuestFunction,
    user_data: MutVoidPtr,
) {
    let context = State::get(env).current_ctx;
    State::get(env).notifications.retain(|n| {
        !(n.context == context
            && n.source == source
            && n.notification_id == notification_id
            && n.callback.addr_with_thumb_bit() == callback.addr_with_thumb_bit()
            && n.user_data == user_data)
    });
}

fn source_notification_value(
    env: &mut Environment,
    context: MutPtr<GuestALCcontext>,
    source: ALuint,
    notification_id: ALuint,
) -> ALint {
    let state = &mut env.framework_state.openal;
    let Some(context) = state.contexts.get_mut(&context) else {
        return 0;
    };
    let context = context.make_current(&mut env.openal_manager);
    let mut value = 0;
    if notification_id == AL_QUEUE_HAS_LOOPED {
        // Detected by the playback position going back while looping.
        let mut looping = 0;
        unsafe { context.GetSourcei(source, AL_LOOPING, &mut looping) };
        if looping == 0 {
            return -1;
        }
        unsafe { context.GetSourcei(source, AL_SAMPLE_OFFSET, &mut value) };
        return value;
    }
    unsafe { context.GetSourcei(source, notification_id as ALenum, &mut value) };
    value
}

/// For use by `NSRunLoop`: call the callbacks for source notifications whose
/// condition has happened since the last check.
pub fn handle_notifications(env: &mut Environment) {
    if State::get(env).notifications.is_empty() {
        return;
    }
    let mut to_call = Vec::new();
    for i in 0..State::get(env).notifications.len() {
        let SourceNotification {
            context,
            source,
            notification_id,
            callback,
            user_data,
            last_value,
        } = State::get(env).notifications[i];
        let value = source_notification_value(env, context, source, notification_id);
        let happened = match notification_id {
            AL_SOURCE_STATE => value != last_value,
            AL_QUEUE_HAS_LOOPED => value >= 0 && last_value >= 0 && value < last_value,
            _ => value > 0 && value != last_value,
        };
        State::get(env).notifications[i].last_value = value;
        if happened {
            to_call.push((context, source, notification_id, callback, user_data));
        }
    }
    for (context, source, notification_id, callback, user_data) in to_call {
        // The callback may use OpenAL, so it should see the source's context.
        let old_context = State::get(env).current_ctx;
        State::get(env).current_ctx = context;
        let () = callback.call_from_host(env, (source, notification_id, user_data));
        State::get(env).current_ctx = old_context;
    }
}

// Apple's ASA extension for reverb and occlusion effects. The effects aren't
// supported, so settings are ignored.
fn alcASASetListener(
    _env: &mut Environment,
    property: ALuint,
    _data: ConstVoidPtr,
    _data_size: ALuint,
) -> ALenum {
    log_dbg!("TODO: alcASASetListener({:#x}, ...) (ignored)", property);
    0 // AL_NO_ERROR
}
fn alcASASetSource(
    _env: &mut Environment,
    property: ALuint,
    source: ALuint,
    _data: ConstVoidPtr,
    _data_size: ALuint,
) -> ALenum {
    log_dbg!(
        "TODO: alcASASetSource({:#x}, {}, ...) (ignored)",
        property,
        source
    );
    0 // AL_NO_ERROR
}

fn alcGetProcAddress(
    env: &mut Environment,
    _device: ConstPtr<GuestALCdevice>,
    func_name: ConstPtr<u8>,
) -> MutVoidPtr {
    let mangled_func_name = format!("_{}", env.mem.cstr_at_utf8(func_name).unwrap());
    assert!(mangled_func_name.starts_with("_al"));

    if let Ok(ptr) = env
        .dyld
        .create_proc_address(&mut env.mem, &mut env.cpu, &mangled_func_name)
    {
        Ptr::from_bits(ptr.addr_with_thumb_bit())
    } else {
        if mangled_func_name == "_alcMacOSMixerOutputRate" {
            let state = State::get(env);
            if !state.warned_mixer_output_rate_proc {
                log!("Tolerating nonexistent alcMacOSMixerOutputRate() func in alcGetProcAddress(), returning NULL.");
                state.warned_mixer_output_rate_proc = true;
            }
            return Ptr::null();
        }
        panic!(
            "Request for procedure address for unimplemented OpenAL function {mangled_func_name}"
        );
    }
}

// TODO: more functions

// === al.h ===

fn alGetError(env: &mut Environment) -> i32 {
    // Super Monkey Ball and other apps try to use this function (rather than
    // alcGetError) to figure out whether opening the device succeeded. This
    // is not correct and seems to be a bug. Presumably iPhone OS doesn't mind
    // this, but OpenAL Soft returns an error in this case, and the game skips
    // the rest of its audio initialization.
    // Some other apps will try to call this on a context that is deleted
    // (typically from another thread), so we need to silently be ok with this.
    try_get_context!(env, context, al::AL_NO_ERROR);
    let res = unsafe { context.GetError() };
    log_dbg!("alGetError() => {:#x}", res);
    res
}

fn alDistanceModel(env: &mut Environment, value: ALenum) {
    try_get_context!(env, context);
    unsafe { context.DistanceModel(value) };
}

fn alGetEnumValue(env: &mut Environment, enumName: ConstPtr<u8>) -> ALenum {
    let s = env.mem.cstr_at_utf8(enumName).unwrap();
    let ss = CString::new(s).unwrap();

    let res = unsafe { OpenALContext::GetEnumValue(ss.as_ptr()) };
    log_dbg!("alGetEnumValue({:?}) => {:?}", s, res);
    res
}

fn alIsBuffer(env: &mut Environment, buffer: ALuint) -> ALboolean {
    try_get_context!(env, context, 0);
    unsafe { context.IsBuffer(buffer) }
}

fn alGetBufferi(env: &mut Environment, buffer: ALuint, param: ALenum, value: MutPtr<ALint>) {
    let value = env.mem.ptr_at(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetBufferi(buffer, param, value) }
}

fn alIsSource(env: &mut Environment, source: ALuint) -> ALboolean {
    try_get_context!(env, context, 0);
    unsafe { context.IsSource(source) }
}

fn alIsExtensionPresent(env: &mut Environment, ext_name: ConstPtr<u8>) -> ALboolean {
    try_get_context!(env, context, 0);
    let s = env.mem.cstr_at_utf8(ext_name).unwrap();
    let ss = CString::new(s).unwrap();
    unsafe { context.IsExtensionPresent(ss.as_ptr()) }
}

fn alEnable(env: &mut Environment, capability: ALenum) {
    try_get_context!(env, context);
    unsafe { context.Enable(capability) };
}

fn alGetString(env: &mut Environment, param: ALenum) -> ConstPtr<u8> {
    let res = if let Some(&str) = env.framework_state.openal.strings_cache.get(&param) {
        str
    } else {
        // Those values are extracted from the iPhone 3GS, iOS 4.0.1
        // (same values were seen on the iPhone Simulator)
        let s: &[u8] = match param {
            AL_VENDOR => b"Apple Inc.",
            AL_VERSION => b"1.1",
            AL_RENDERER => b"Software",
            AL_EXTENSIONS => b"AL_EXT_OFFSET AL_EXT_LINEAR_DISTANCE AL_EXT_EXPONENT_DISTANCE AL_EXT_STATIC_BUFFER",
            _ => unreachable!()
        };
        let new_str = env.mem.alloc_and_write_cstr(s).cast_const();
        env.framework_state
            .openal
            .strings_cache
            .insert(param, new_str);
        new_str
    };
    log_dbg!(
        "alGetString({}) => '{:?}'",
        param,
        env.mem.cstr_at_utf8(res)
    );
    res
}

fn alListenerf(env: &mut Environment, param: ALenum, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.Listenerf(param, value) };
}
fn alListenerfv(env: &mut Environment, param: ALenum, values: ConstPtr<ALfloat>) {
    // we assume that at least 1 parameter should be passed
    let values = env.mem.ptr_at(values, 1);
    try_get_context!(env, context);
    unsafe { context.Listenerfv(param, values) };
}
fn alListener3f(
    env: &mut Environment,

    param: ALenum,
    value1: ALfloat,
    value2: ALfloat,
    value3: ALfloat,
) {
    try_get_context!(env, context);
    unsafe { context.Listener3f(param, value1, value2, value3) };
}
fn alListeneri(env: &mut Environment, param: ALenum, value: ALint) {
    try_get_context!(env, context);
    unsafe { context.Listeneri(param, value) };
}
fn alListener3i(env: &mut Environment, param: ALenum, value1: ALint, value2: ALint, value3: ALint) {
    try_get_context!(env, context);
    unsafe { context.Listener3i(param, value1, value2, value3) };
}
fn alListeneriv(env: &mut Environment, param: ALenum, values: ConstPtr<ALint>) {
    let values = env.mem.ptr_at(values, 3); // upper bound
    try_get_context!(env, context);
    unsafe { context.Listeneriv(param, values) };
}

fn alGetListenerf(env: &mut Environment, param: ALenum, value: MutPtr<ALfloat>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetListenerf(param, value) };
}
fn alGetListener3f(
    env: &mut Environment,

    param: ALenum,
    value1: MutPtr<ALfloat>,
    value2: MutPtr<ALfloat>,
    value3: MutPtr<ALfloat>,
) {
    try_get_context!(env, context);
    let mut values = [0.0; 3];
    unsafe { context.GetListener3f(param, &mut values[0], &mut values[1], &mut values[2]) };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetListenerfv(env: &mut Environment, param: ALenum, values: MutPtr<ALfloat>) {
    let values = env.mem.ptr_at_mut(values, 3); // upper bound
    try_get_context!(env, context);
    unsafe { context.GetListenerfv(param, values) };
}
fn alGetListeneri(env: &mut Environment, param: ALenum, value: MutPtr<ALint>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetListeneri(param, value) };
}
fn alGetListener3i(
    env: &mut Environment,

    param: ALenum,
    value1: MutPtr<ALint>,
    value2: MutPtr<ALint>,
    value3: MutPtr<ALint>,
) {
    let mut values = [0; 3];
    try_get_context!(env, context);
    unsafe { context.GetListener3i(param, &mut values[0], &mut values[1], &mut values[2]) };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetListeneriv(env: &mut Environment, param: ALenum, values: MutPtr<ALint>) {
    let values = env.mem.ptr_at_mut(values, 3); // upper bound
    try_get_context!(env, context);
    unsafe { context.GetListeneriv(param, values) };
}

fn alGenSources(env: &mut Environment, n: ALsizei, sources: MutPtr<ALuint>) {
    let n_usize: GuestUSize = n.try_into().unwrap();
    let guest_sources = sources;
    let trace = audio_test_mode(env).is_some();
    let sources = env.mem.ptr_at_mut(sources, n_usize);
    try_get_context!(env, context);
    unsafe { context.GenSources(n, sources) };
    if trace {
        let names = env
            .mem
            .bytes_at(guest_sources.cast(), n_usize * 4)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|s| u32::from_le_bytes(*s))
            .collect::<Vec<_>>();
        let state = State::get(env);
        for name in names {
            state.audio_test_sources.insert(state.audio_test_key(name));
        }
    }
}
fn alDeleteSources(env: &mut Environment, n: ALsizei, sources: ConstPtr<ALuint>) {
    let n_usize: GuestUSize = n.try_into().unwrap();
    if audio_test_mode(env).is_some() {
        let names = env
            .mem
            .bytes_at(sources.cast(), n_usize * 4)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|s| u32::from_le_bytes(*s))
            .collect::<Vec<_>>();
        let state = State::get(env);
        for name in names {
            state.audio_test_sources.remove(&state.audio_test_key(name));
        }
    }
    let sources = env.mem.ptr_at(sources, n_usize);
    try_get_context!(env, context);
    unsafe { context.DeleteSources(n, sources) };
}

fn alSourcef(env: &mut Environment, source: ALuint, param: ALenum, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.Sourcef(source, param, value) };
}
fn alSourcefv(env: &mut Environment, source: ALuint, param: ALenum, values: ConstPtr<ALfloat>) {
    // we assume that at least 1 parameter should be passed
    let values = env.mem.ptr_at(values, 1);
    try_get_context!(env, context);
    unsafe { context.Sourcefv(source, param, values) };
}
fn alSource3f(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: ALfloat,
    value2: ALfloat,
    value3: ALfloat,
) {
    try_get_context!(env, context);
    unsafe { context.Source3f(source, param, value1, value2, value3) };
}
fn alSourcei(env: &mut Environment, source: ALuint, param: ALenum, value: ALint) {
    if audio_test_mode(env).is_some() && matches!(param, 0x1009 | 0x1007 | 0x1024..=0x1026) {
        audio_trace_source(
            env,
            source,
            &format!("set_i param={param:#x} value={value}"),
        );
    }
    try_get_context!(env, context);
    unsafe { context.Sourcei(source, param, value) };
}
fn alSource3i(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: ALint,
    value2: ALint,
    value3: ALint,
) {
    try_get_context!(env, context);
    unsafe { context.Source3i(source, param, value1, value2, value3) };
}
fn alSourceiv(env: &mut Environment, source: ALuint, param: ALenum, values: ConstPtr<ALint>) {
    let values = env.mem.ptr_at(values, 3); // upper bound
    try_get_context!(env, context);
    unsafe { context.Sourceiv(source, param, values) };
}

fn alGetSourcef(env: &mut Environment, source: ALuint, param: ALenum, value: MutPtr<ALfloat>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetSourcef(source, param, value) };
}
fn alGetSource3f(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: MutPtr<ALfloat>,
    value2: MutPtr<ALfloat>,
    value3: MutPtr<ALfloat>,
) {
    let mut values = [0.0; 3];
    try_get_context!(env, context);
    unsafe {
        context.GetSource3f(
            source,
            param,
            &mut values[0],
            &mut values[1],
            &mut values[2],
        )
    };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetSourcefv(env: &mut Environment, source: ALuint, param: ALenum, values: MutPtr<ALfloat>) {
    let values = env.mem.ptr_at_mut(values, 3); // upper bound
    try_get_context!(env, context);
    unsafe { context.GetSourcefv(source, param, values) };
}
fn alGetSourcei(env: &mut Environment, source: ALuint, param: ALenum, value: MutPtr<ALint>) {
    let value = env.mem.ptr_at_mut(value, 1);
    try_get_context!(env, context);
    unsafe { context.GetSourcei(source, param, value) };
}
fn alGetSource3i(
    env: &mut Environment,
    source: ALuint,
    param: ALenum,
    value1: MutPtr<ALint>,
    value2: MutPtr<ALint>,
    value3: MutPtr<ALint>,
) {
    let mut values = [0; 3];
    try_get_context!(env, context);
    unsafe {
        context.GetSource3i(
            source,
            param,
            &mut values[0],
            &mut values[1],
            &mut values[2],
        )
    };
    env.mem.write(value1, values[0]);
    env.mem.write(value2, values[1]);
    env.mem.write(value3, values[2]);
}
fn alGetSourceiv(env: &mut Environment, source: ALuint, param: ALenum, values: MutPtr<ALint>) {
    let values = env.mem.ptr_at_mut(values, 3); // upper bound
    try_get_context!(env, context);
    unsafe { context.GetSourceiv(source, param, values) };
}

fn alSourcePlay(env: &mut Environment, source: ALuint) {
    if let Some(mode) = audio_test_mode(env) {
        let state = State::get(env);
        if state.audio_test_plays < 64 {
            state.audio_test_plays += 1;
            crate::performance::audio_event(&format!("AUDIO mode={mode:?} play_source={source}"));
        }
    }
    audio_trace_source(env, source, "play");
    try_get_context!(env, context);
    unsafe { context.SourcePlay(source) };
}
fn alSourcePause(env: &mut Environment, source: ALuint) {
    audio_trace_source(env, source, "pause");
    try_get_context!(env, context);
    unsafe { context.SourcePause(source) };
}
fn alSourceStop(env: &mut Environment, source: ALuint) {
    audio_trace_source(env, source, "stop");
    try_get_context!(env, context);
    unsafe { context.SourceStop(source) };
}
fn alSourceRewind(env: &mut Environment, source: ALuint) {
    audio_trace_source(env, source, "rewind");
    try_get_context!(env, context);
    unsafe { context.SourceRewind(source) };
}

fn alSourcePlayv(env: &mut Environment, nsources: ALsizei, sources: ConstPtr<ALuint>) {
    if let Some(mode) = audio_test_mode(env) {
        let state = State::get(env);
        if state.audio_test_plays < 64 {
            state.audio_test_plays += 1;
            crate::performance::audio_event(&format!(
                "AUDIO mode={mode:?} play_batch_sources={nsources}"
            ));
        }
    }
    let nsources_usize: GuestUSize = nsources.try_into().unwrap();
    if audio_test_mode(env).is_some() {
        let names = env
            .mem
            .bytes_at(sources.cast(), nsources_usize * 4)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|s| u32::from_le_bytes(*s))
            .collect::<Vec<_>>();
        for source in names {
            audio_trace_source(env, source, "play_batch");
        }
    }
    let sources = env.mem.ptr_at(sources, nsources_usize);
    try_get_context!(env, context);
    unsafe { context.SourcePlayv(nsources, sources) };
}
fn alSourcePausev(env: &mut Environment, nsources: ALsizei, sources: ConstPtr<ALuint>) {
    let nsources_usize: GuestUSize = nsources.try_into().unwrap();
    let sources = env.mem.ptr_at(sources, nsources_usize);
    try_get_context!(env, context);
    unsafe { context.SourcePausev(nsources, sources) };
}
fn alSourceStopv(env: &mut Environment, nsources: ALsizei, sources: ConstPtr<ALuint>) {
    let nsources_usize: GuestUSize = nsources.try_into().unwrap();
    let sources = env.mem.ptr_at(sources, nsources_usize);
    try_get_context!(env, context);
    unsafe { context.SourceStopv(nsources, sources) };
}
fn alSourceRewindv(env: &mut Environment, nsources: ALsizei, sources: ConstPtr<ALuint>) {
    let nsources_usize: GuestUSize = nsources.try_into().unwrap();
    let sources = env.mem.ptr_at(sources, nsources_usize);
    try_get_context!(env, context);
    unsafe { context.SourceRewindv(nsources, sources) };
}

fn alSourceQueueBuffers(
    env: &mut Environment,
    source: ALuint,
    nb: ALsizei,
    buffers: ConstPtr<ALuint>,
) {
    audio_trace_source(env, source, "queue_requested");
    let nb_usize: GuestUSize = nb.try_into().unwrap();
    if audio_test_mode(env).is_some() {
        let names = env
            .mem
            .bytes_at(buffers.cast(), nb_usize * 4)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|s| u32::from_le_bytes(*s))
            .collect::<Vec<_>>();
        let mut originals = Vec::new();
        let state = State::get(env);
        for name in names {
            let key = state.audio_test_key(name);
            state.audio_test_stream_buffers.insert(key);
            if let Some(original) = state.audio_test_original.remove(&key) {
                originals.push((name, original));
            }
        }
        if let Some(context) = State::try_make_current(env) {
            for (name, original) in originals {
                // Streaming buffers must remain continuous: restore the raw
                // data before the first queue, and skip later transformations.
                unsafe {
                    context.BufferData(
                        name,
                        original.format,
                        original.data.as_ptr().cast(),
                        original.data.len() as i32,
                        original.rate,
                    );
                }
            }
        }
    }
    let buffers = env.mem.ptr_at(buffers, nb_usize);
    try_get_context!(env, context);
    unsafe { context.SourceQueueBuffers(source, nb, buffers) }
}
fn alSourceUnqueueBuffers(
    env: &mut Environment,
    source: ALuint,
    nb: ALsizei,
    buffers: MutPtr<ALuint>,
) {
    audio_trace_source(env, source, "unqueue_requested");
    // Apple's sample code for a looping sound effect contains a function called
    // SoundEngineEffect::ClearSourceBuffers() that has the following pattern:
    //
    //    alGetSourcei(source, AL_BUFFERS_QUEUED, &n);
    //    alSourceUnqueueBuffers(source, n, &buffers);
    //
    // Unfortunately, this is incorrect code in some circumstances: unqueueing
    // buffers while they are playing is not permitted by the OpenAL spec! Maybe
    // it worked with Apple's OpenAL implementation for some reason, but OpenAL
    // Soft does not tolerate this, so many apps that used this sample code
    // (e.g. Super Monkey Ball) run into an unexpected OpenAL error.
    //
    // Limiting the number dequeued seems to be an effective workaround for the
    // apps that have been tested. That sample code isn't interested in actually
    // using the returned buffer IDs, so it's no problem that we write too few.
    try_get_context!(env, context);
    let buffers_processed = {
        let mut val = 0;
        unsafe { context.GetSourcei(source, al::AL_BUFFERS_PROCESSED, &mut val) };
        val
    };
    let nb = if buffers_processed < nb {
        log_dbg!("Applying workaround for Apple sample code bug: ignoring unqueueing of {}/{} processed buffers from source {}", nb, buffers_processed, source);
        buffers_processed
    } else {
        nb
    };

    let nb_usize: GuestUSize = nb.try_into().unwrap();
    let buffers = env.mem.ptr_at_mut(buffers, nb_usize);
    unsafe { context.SourceUnqueueBuffers(source, nb, buffers) }
}

fn alGenBuffers(env: &mut Environment, n: ALsizei, buffers: MutPtr<ALuint>) {
    let n_usize: GuestUSize = n.try_into().unwrap();
    let buffers = env.mem.ptr_at_mut(buffers, n_usize);
    try_get_context!(env, context);
    unsafe { context.GenBuffers(n, buffers) };
}
fn alDeleteBuffers(env: &mut Environment, n: ALsizei, buffers: ConstPtr<ALuint>) {
    let n_usize: GuestUSize = n.try_into().unwrap();
    if audio_test_mode(env).is_some() {
        let names = env
            .mem
            .bytes_at(buffers.cast(), n_usize * 4)
            .as_chunks::<4>()
            .0
            .iter()
            .map(|s| u32::from_le_bytes(*s))
            .collect::<Vec<_>>();
        let state = State::get(env);
        for name in names {
            let key = state.audio_test_key(name);
            state.audio_test_original.remove(&key);
            state.audio_test_stream_buffers.remove(&key);
            state.audio_test_meta.remove(&key);
        }
    }
    let buffers = env.mem.ptr_at(buffers, n_usize);
    let ctx_ptr = State::get(env).current_ctx;
    let Some(context) = State::try_make_current(env) else {
        let state = State::get(env);
        if state.warned_invalid_contexts.insert(ctx_ptr) {
            log!(
                "Attempted alDeleteBuffers({}, {:?}) with inactive context {:?}, skipping!",
                n,
                buffers,
                ctx_ptr
            );
        }
        return;
    };
    unsafe { context.DeleteBuffers(n, buffers) };
}

fn alBufferData(
    env: &mut Environment,
    buffer: ALuint,
    format: ALenum,
    data: ConstVoidPtr,
    size: ALsizei,
    samplerate: ALsizei,
) {
    let size_usize: GuestUSize = size.try_into().unwrap();
    let mode = audio_test_mode(env);
    let key = if mode.is_some() {
        State::get(env).audio_test_key(buffer)
    } else {
        (0, buffer)
    };
    let streaming = mode.is_some() && State::get(env).audio_test_stream_buffers.contains(&key);
    if mode.is_some() && !data.is_null() {
        let meta = format!(
            "format={format:#x} rate={samplerate} {}",
            crate::audio::sound_trace::describe_pcm(env.mem.bytes_at(data.cast(), size_usize))
        );
        State::get(env).audio_test_meta.insert(key, meta);
    }
    let processed = if let Some(mode) = mode.filter(|_| !streaming && !data.is_null()) {
        crate::audio::sound_test::transform(
            mode,
            format,
            samplerate,
            env.mem.bytes_at(data.cast(), size_usize),
        )
    } else {
        None
    };
    if let Some(mode) = mode {
        let original = processed.as_ref().map(|_| AudioTestBuffer {
            format,
            rate: samplerate,
            data: env.mem.bytes_at(data.cast(), size_usize).to_vec(),
        });
        let state = State::get(env);
        state.audio_test_original.remove(&key);
        if let Some(original) = original {
            state.audio_test_original.insert(key, original);
        }
        if state.audio_test_uploads == 0 {
            let session = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            crate::performance::audio_event(&format!("\nAUDIO SESSION {session} mode={:?} version={} performance_timing=off_unless_separately_enabled; experimental_short_mono_effects_only", mode, crate::VERSION));
        }
        if state.audio_test_uploads < 24 {
            state.audio_test_uploads += 1;
            crate::performance::audio_event(&format!(
                "AUDIO mode={:?} buffer={} format={:#x} rate={} bytes={} transformed={} stream={}",
                mode,
                buffer,
                format,
                samplerate,
                size,
                processed.is_some(),
                streaming,
            ));
        }
    }
    let data_ptr: *const ALvoid = if let Some(processed) = &processed {
        processed.as_ptr().cast()
    } else if data.is_null() {
        std::ptr::null()
    } else {
        env.mem.bytes_at(data.cast(), size_usize).as_ptr().cast()
    };
    if mode.is_some() && !data_ptr.is_null() {
        let pcm = if let Some(processed) = &processed {
            processed.as_slice()
        } else {
            env.mem.bytes_at(data.cast(), size_usize)
        };
        let now = crate::audio::sound_trace::timestamp_ms();
        env.framework_state
            .openal
            .audio_trace
            .record_pcm(now, format, samplerate, pcm);
    }
    try_get_context!(env, context);
    unsafe { context.BufferData(buffer, format, data_ptr, size, samplerate) };
}

/// This is an Apple extension that treats the data passed as a static buffer
/// rather than a temporary one, which means it never has to be copied.
/// OpenAL Soft doesn't support this, so we pass through to `alBufferData`
/// and hope the guest app doesn't rely on the static-ness (it shouldn't).
fn alBufferDataStatic(
    env: &mut Environment,
    buffer: ALuint,
    format: ALenum,
    data: ConstVoidPtr,
    size: ALsizei,
    samplerate: ALsizei,
) {
    alBufferData(env, buffer, format, data, size, samplerate);
}

// Apple-specific extension to OpenAL
fn alcMacOSXMixerOutputRate(env: &mut Environment, value: ALdouble) {
    let state = State::get(env);
    if !state.warned_mixer_rate {
        log!("App wants to set mixer output sample rate to {} Hz", value);
        state.warned_mixer_rate = true;
    }
}
fn alcMacOSXGetMixerOutputRate(env: &mut Environment) -> ALdouble {
    // Default was checked on iPhone 3GS, iOS 4.0.1
    let state = State::get(env);
    if !state.warned_mixer_get_rate {
        log!("App wants to get mixer output sample rate, returning default 0");
        state.warned_mixer_get_rate = true;
    }
    0.0
}

fn alDopplerFactor(env: &mut Environment, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.DopplerFactor(value) };
}

fn alDopplerVelocity(env: &mut Environment, value: ALfloat) {
    // Apparently wolf3d sets doppler velocity to zero, but this results in
    // muting all of the audio with Open AL 1.1 soft implementation!
    // Check "A note for OpenAL library implementors regarding OpenAL 1.0" from
    // OpenAL 1.1 specs for more info
    let bundle_id = env.bundle.bundle_identifier();
    if bundle_id.starts_with("com.zodttd.wolf3d")
        || bundle_id.starts_with("com.idsoftware.wolf3d")
        || bundle_id.starts_with("nu.r3.wolf3d")
    {
        log_dbg!("Applying game-specific hack for Wolf3D-iOS: ignoring 0.0 doppler velocity.");
        assert_eq!(value, 0.0);
        return;
    }
    try_get_context!(env, context);
    unsafe { context.DopplerVelocity(value) };
}

fn alSpeedOfSound(env: &mut Environment, value: ALfloat) {
    try_get_context!(env, context);
    unsafe { context.SpeedOfSound(value) };
}

// TODO: more functions

// Note: For some reasons Wolf3d registers many OpenAl functions, but actually
// uses only few ones. To workaround this, we just provide stubs.

fn alcGetEnumValue(
    _env: &mut Environment,
    _device: MutPtr<GuestALCdevice>,
    _enumName: ConstPtr<u8>,
) -> ALenum {
    todo!();
}
fn alcGetIntegerv(
    _env: &mut Environment,
    _device: MutPtr<GuestALCdevice>,
    _param: ALenum,
    _size: ALCsizei,
    _values: MutPtr<ALCint>,
) {
    todo!();
}
fn alcIsExtensionPresent(
    _env: &mut Environment,
    _device: MutPtr<GuestALCdevice>,
    _extName: ConstPtr<u8>,
) -> ALCboolean {
    0
}
fn alGetBufferf(_env: &mut Environment, _buffer: ALuint, _param: ALenum, _value: MutPtr<ALfloat>) {
    todo!();
}
fn alDisable(_env: &mut Environment, _capability: ALenum) {
    todo!();
}
fn alGetBoolean(_env: &mut Environment, _param: ALenum) -> ALboolean {
    todo!();
}
fn alGetBooleanv(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALboolean>) {
    todo!();
}
fn alGetDouble(_env: &mut Environment, _param: ALenum) -> ALdouble {
    todo!();
}
fn alGetDoublev(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALdouble>) {
    todo!();
}
fn alGetFloat(_env: &mut Environment, _param: ALenum) -> ALfloat {
    todo!();
}
fn alGetFloatv(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALfloat>) {
    todo!();
}
fn alGetInteger(_env: &mut Environment, _param: ALenum) -> ALint {
    todo!();
}
fn alGetIntegerv(_env: &mut Environment, _param: ALenum, _values: MutPtr<ALint>) {
    todo!();
}
fn alGetProcAddress(env: &mut Environment, funcName: ConstPtr<u8>) -> MutVoidPtr {
    alcGetProcAddress(env, Ptr::null(), funcName)
}
fn alIsEnabled(_env: &mut Environment, _capability: ALenum) -> ALboolean {
    todo!();
}

/// `ALenum alcASAGetListener(const ALuint property, ALvoid *data,
///                           ALuint *dataSize)` (Apple).
fn alcASAGetListener(
    env: &mut Environment,
    property: ALuint,
    data: MutVoidPtr,
    data_size: MutPtr<ALuint>,
) -> ALenum {
    log_dbg!(
        "alcASAGetListener(property={:#x}) — unsupported, zeroing out",
        property
    );
    if !data_size.is_null() {
        let n = env.mem.read(data_size);
        if !data.is_null() && n > 0 {
            let dst = env.mem.bytes_at_mut(data.cast(), n);
            for b in dst.iter_mut() {
                *b = 0;
            }
        }
    }
    al::AL_NO_ERROR
}

/// `ALenum alcASAGetSource(const ALuint property, ALuint source,
///                         ALvoid *data, ALuint *dataSize)` (Apple).
///
/// ASA == Apple Sound API. Properties include
/// `ALC_ASA_REVERB_ON` (0x1) / `_ROOM_TYPE` (0x2) / `_PRESET` (0x3) /
/// `_OCCLUSION` (0x4) / `_OBSTRUCTION` (0x5).
fn alcASAGetSource(
    env: &mut Environment,
    property: ALuint,
    source: ALuint,
    data: MutVoidPtr,
    data_size: MutPtr<ALuint>,
) -> ALenum {
    log_dbg!(
        "alcASAGetSource(property={:#x}, source={}) — unsupported, zeroing out",
        property,
        source
    );
    // Per Apple's headers, write 0 / leave size at 0 when the
    // property is unknown. We zero the destination buffer if the
    // caller told us its size.
    if !data_size.is_null() {
        let n = env.mem.read(data_size);
        if !data.is_null() && n > 0 {
            let dst = env.mem.bytes_at_mut(data.cast(), n);
            for b in dst.iter_mut() {
                *b = 0;
            }
        }
    }
    al::AL_NO_ERROR
}

/// `ALint alcMacOSXGetRenderingQuality()` (Apple).
fn alcMacOSXGetRenderingQuality(env: &mut Environment) -> ALint {
    // Default: high quality, matching iPhone OS 4.x's documented value.
    let q = env.framework_state.openal.macosx_rendering_quality;
    if q == 0 {
        0x0002 // ALC_IPHONE_SPATIAL_RENDERING_QUALITY_HIGH
    } else {
        q
    }
}

/// `ALvoid alcMacOSXRenderingQuality(const ALint quality)` (Apple).
///
/// Selects the spatial-rendering quality:
/// `ALC_IPHONE_SPATIAL_RENDERING_QUALITY_LOW` (1) /
/// `ALC_IPHONE_SPATIAL_RENDERING_QUALITY_HIGH` (2). On stock OpenAL
/// Soft the rendering pipeline is configured at device-open time and
/// can't be changed on the fly, so we just remember the request and
/// hand it back to `alcMacOSXGetRenderingQuality`.
fn alcMacOSXRenderingQuality(env: &mut Environment, quality: ALint) {
    log_dbg!("alcMacOSXRenderingQuality({}) — remembered", quality);
    env.framework_state.openal.macosx_rendering_quality = quality;
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(alcOpenDevice(_)),
    export_c_func!(alcCloseDevice(_)),
    export_c_func!(alcGetError(_)),
    export_c_func!(alcCreateContext(_, _)),
    export_c_func!(alcDestroyContext(_)),
    export_c_func!(alcProcessContext(_)),
    export_c_func!(alcSuspendContext(_)),
    export_c_func!(alcMakeContextCurrent(_)),
    export_c_func!(alcGetProcAddress(_, _)),
    export_c_func!(alGetError()),
    export_c_func!(alDistanceModel(_)),
    export_c_func!(alListenerf(_, _)),
    export_c_func!(alListener3f(_, _, _, _)),
    export_c_func!(alListenerfv(_, _)),
    export_c_func!(alListeneri(_, _)),
    export_c_func!(alListener3i(_, _, _, _)),
    export_c_func!(alListeneriv(_, _)),
    export_c_func!(alGetListenerf(_, _)),
    export_c_func!(alGetListener3f(_, _, _, _)),
    export_c_func!(alGetListenerfv(_, _)),
    export_c_func!(alGetListeneri(_, _)),
    export_c_func!(alGetListener3i(_, _, _, _)),
    export_c_func!(alGetListeneriv(_, _)),
    export_c_func!(alGenSources(_, _)),
    export_c_func!(alDeleteSources(_, _)),
    export_c_func!(alSourcef(_, _, _)),
    export_c_func!(alSource3f(_, _, _, _, _)),
    export_c_func!(alSourcefv(_, _, _)),
    export_c_func!(alSourcei(_, _, _)),
    export_c_func!(alSource3i(_, _, _, _, _)),
    export_c_func!(alSourceiv(_, _, _)),
    export_c_func!(alGetSourcef(_, _, _)),
    export_c_func!(alGetSource3f(_, _, _, _, _)),
    export_c_func!(alGetSourcefv(_, _, _)),
    export_c_func!(alGetSourcei(_, _, _)),
    export_c_func!(alGetSource3i(_, _, _, _, _)),
    export_c_func!(alGetSourceiv(_, _, _)),
    export_c_func!(alSourcePlay(_)),
    export_c_func!(alSourcePause(_)),
    export_c_func!(alSourceStop(_)),
    export_c_func!(alSourceRewind(_)),
    export_c_func!(alSourceQueueBuffers(_, _, _)),
    export_c_func!(alSourceUnqueueBuffers(_, _, _)),
    export_c_func!(alGenBuffers(_, _)),
    export_c_func!(alDeleteBuffers(_, _)),
    export_c_func!(alBufferData(_, _, _, _, _)),
    export_c_func!(alBufferDataStatic(_, _, _, _, _)),
    export_c_func!(alcMacOSXMixerOutputRate(_)),
    export_c_func!(alSourceAddNotification(_, _, _, _)),
    export_c_func!(alcASASetListener(_, _, _)),
    export_c_func!(alcASASetSource(_, _, _, _)),
    export_c_func!(alSourceRemoveNotification(_, _, _, _)),
    export_c_func!(alcMacOSXGetMixerOutputRate()),
    export_c_func!(alcGetContextsDevice(_)),
    export_c_func!(alcGetCurrentContext()),
    export_c_func!(alcGetEnumValue(_, _)),
    export_c_func!(alcGetIntegerv(_, _, _, _)),
    export_c_func!(alcGetString(_, _)),
    export_c_func!(alcIsExtensionPresent(_, _)),
    export_c_func!(alIsBuffer(_)),
    export_c_func!(alGetBufferf(_, _, _)),
    export_c_func!(alGetBufferi(_, _, _)),
    export_c_func!(alEnable(_)),
    export_c_func!(alDisable(_)),
    export_c_func!(alDopplerFactor(_)),
    export_c_func!(alDopplerVelocity(_)),
    export_c_func!(alGetBoolean(_)),
    export_c_func!(alGetBooleanv(_, _)),
    export_c_func!(alGetDouble(_)),
    export_c_func!(alGetDoublev(_, _)),
    export_c_func!(alGetFloat(_)),
    export_c_func!(alGetFloatv(_, _)),
    export_c_func!(alGetInteger(_)),
    export_c_func!(alGetIntegerv(_, _)),
    export_c_func!(alGetEnumValue(_)),
    export_c_func!(alGetProcAddress(_)),
    export_c_func!(alGetString(_)),
    export_c_func!(alIsExtensionPresent(_)),
    export_c_func!(alIsEnabled(_)),
    export_c_func!(alIsSource(_)),
    export_c_func!(alSourcePlayv(_, _)),
    export_c_func!(alSourcePausev(_, _)),
    export_c_func!(alSourceStopv(_, _)),
    export_c_func!(alSourceRewindv(_, _)),
    export_c_func!(alSpeedOfSound(_)),
    export_c_func!(alcASAGetListener(_, _, _)),
    export_c_func!(alcASAGetSource(_, _, _, _)),
    export_c_func!(alcMacOSXGetRenderingQuality()),
    export_c_func!(alcMacOSXRenderingQuality(_)),
];
