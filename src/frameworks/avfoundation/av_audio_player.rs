/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! AVAudioPlayer
//!
//! Implemented using Audio Queue Services based on [the PlayingAudio example](https://developer.apple.com/library/archive/documentation/MusicAudio/Conceptual/AudioQueueProgrammingGuide/AQPlayback/PlayingAudio.html)

use std::time::Instant;

use crate::dyld::HostFunction;
use crate::frameworks::audio_toolbox::audio_converter::{
    convert_pcm, pcm_shape_from_asbd, PcmShape,
};
use crate::frameworks::audio_toolbox::audio_file::{
    self, guest_audio_file_read_from_vec, kAudioFileCAFType, kAudioFilePropertyDataFormat,
    kAudioFilePropertyPacketSizeUpperBound, kAudioFileReadPermission, AudioFileClose,
    AudioFileCreateWithURL, AudioFileGetProperty, AudioFileID, AudioFileOpenURL,
    AudioFileReadPackets, AudioFileWriteBytes,
};
use crate::frameworks::audio_toolbox::audio_queue::{
    kAudioQueueParam_Volume, AudioQueueAllocateBuffer, AudioQueueBufferRef, AudioQueueDispose,
    AudioQueueEnqueueBuffer, AudioQueueGetParameter, AudioQueueNewOutput, AudioQueueOutputCallback,
    AudioQueuePause, AudioQueueRef, AudioQueueSetParameter, AudioQueueStart, AudioQueueStop,
};
use crate::frameworks::carbon_core::eofErr;
use crate::frameworks::core_audio_types::{
    fourcc, kAudioFormatFlagIsBigEndian, kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked,
    kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM, AudioStreamBasicDescription,
};
use crate::frameworks::core_foundation::cf_run_loop::kCFRunLoopCommonModes;
use crate::frameworks::foundation::ns_error::NSOSStatusErrorDomain;
use crate::frameworks::foundation::{ns_string, NSInteger, NSTimeInterval, NSUInteger};
use crate::mem::{guest_size_of, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, todo_objc_setter, Class,
    ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

const kNumberBuffers: usize = 3;

struct AVAudioPlayerHostObject {
    audio_file_url: id,
    output_callback: AudioQueueOutputCallback,
    audio_file_id: Option<AudioFileID>,
    audio_desc: Option<AudioStreamBasicDescription>,
    audio_queue: Option<AudioQueueRef>,
    audio_queue_buffers: Option<MutPtr<AudioQueueBufferRef>>,
    num_packets_to_read: u32,
    current_packet: i64,
    // The time set by calling setCurrentTime is stored here in case it's set
    // before prepareToPlay is called; so it can be applied when it's called
    set_current_time: NSTimeInterval,
    volume: f32,
    is_playing: bool,
    num_of_loops: NSInteger,
}
impl HostObject for AVAudioPlayerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation AVAudioPlayer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let symb = "__Podroid_AVAudioPlayerOutputBufferHelper";
    let hf: HostFunction = &(_Podroid_AVAudioPlayerOutputBufferHelper as fn(&mut Environment, _, _, _) -> _);
    let callback = env
        .dyld
        .create_guest_function(&mut env.mem, symb, hf);

    let host_object = Box::new(AVAudioPlayerHostObject {
        audio_file_url: nil,
        output_callback: callback,
        audio_file_id: None,
        audio_desc: None,
        audio_queue: None,
        audio_queue_buffers: None,
        num_packets_to_read: 0,
        current_packet: 0,
        set_current_time: 0.0,
        volume: 1.0,
        is_playing: false,
        num_of_loops: 0
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithContentsOfURL:(id)url // NSURL*
                      error:(MutPtr<id>)out_error { // NSError**
    let path: id = msg![env; url path];
    let path_str = ns_string::to_rust_string(env, path);
    log_dbg!("[(AVAudioPlayer*){:?} initWithContentsOfURL:{:?} {} outError:{:?}]", this, url, path_str, out_error);

    retain(env, url);
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_file_url = url;

    // Check for errors. Return nil and write them to error if there are
    let tmp_afi_ptr: MutPtr<AudioFileID> = env.mem.alloc(guest_size_of::<AudioFileID>()).cast();
    let status = AudioFileOpenURL(env, url, kAudioFileReadPermission, 0, tmp_afi_ptr) as NSInteger;
    let audio_file_id = env.mem.read(tmp_afi_ptr);
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_file_id = Some(audio_file_id);
    env.mem.free(tmp_afi_ptr.cast());
    if status != 0 {
        if !out_error.is_null() {
            let domain = ns_string::get_static_str(env, NSOSStatusErrorDomain);
            let error = msg_class![env; NSError alloc];
            let error = msg![env; error initWithDomain:domain code:status userInfo:nil];
            autorelease(env, error);
            env.mem.write(out_error, error);
        }
        release(env, this);
        return nil;
    }

    this
}

- (id)initWithData:(id)data // NSData*
             error:(MutPtr<id>)out_error { // NSError**
    let bytes: ConstVoidPtr = msg![env; data bytes];
    let length: NSUInteger = msg![env; data length];
    let data_vec = env
        .mem
        .bytes_at(bytes.cast(), length)
        .to_vec();

    assert_eq!(env.objc.borrow::<AVAudioPlayerHostObject>(this).audio_file_url, nil);

    match guest_audio_file_read_from_vec(env, data_vec) {
        Ok(audio_file_id) => {
            assert!(env.objc.borrow::<AVAudioPlayerHostObject>(this).audio_file_id.is_none());
            env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_file_id = Some(audio_file_id);
            this
        }
        Err(_) => {
            let domain = ns_string::get_static_str(env, NSOSStatusErrorDomain);
            let error = msg_class![env; NSError alloc];
            let code = -1; // TODO: set a proper code
            let error = msg![env; error initWithDomain:domain code:code userInfo:nil];
            autorelease(env, error);
            env.mem.write(out_error, error);

            release(env, this);
            nil
        }
    }
}

- (())setDelegate:(id)delegate {
    todo_objc_setter!(this, delegate);
}
- (())setMeteringEnabled:(bool)enabled {
    todo_objc_setter!(this, enabled);
}

- (f32)volume {
    let aq_ref = env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_queue;
    if aq_ref.is_none() {
        // TODO: is it correct? can we always return it instead of querying?
        return env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).volume;
    }

    let tmp: MutPtr<f32> = env.mem.alloc(guest_size_of::<f32>()).cast();
    let status = AudioQueueGetParameter(env, aq_ref.unwrap(), kAudioQueueParam_Volume, tmp);
    assert_eq!(status, 0);
    let res = env.mem.read(tmp);
    env.mem.free(tmp.cast());
    res
}
- (())setVolume:(f32)volume {
    let host_object = env.objc.borrow_mut::<AVAudioPlayerHostObject>(this);
    host_object.volume = volume;
    if let Some(aq_ref) = host_object.audio_queue {
        let status = AudioQueueSetParameter(env, aq_ref, kAudioQueueParam_Volume, volume);
        assert_eq!(status, 0);
    }
}

- (())prepareToPlay {
    let audio_queue = env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_queue;
    if audio_queue.is_some() {
        return;
    }

    let audio_file_id = env.objc.borrow::<AVAudioPlayerHostObject>(this).audio_file_id.unwrap();
    let callback = env.objc.borrow::<AVAudioPlayerHostObject>(this).output_callback;

    let size = guest_size_of::<AudioStreamBasicDescription>();
    let tmp_size_ptr: MutPtr<GuestUSize> = env.mem.alloc(guest_size_of::<GuestUSize>()).cast();
    env.mem.write(tmp_size_ptr, size);
    let tmp_data_ptr: MutPtr<AudioStreamBasicDescription> = env.mem.alloc(size).cast();
    let status = AudioFileGetProperty(
        env, audio_file_id, kAudioFilePropertyDataFormat, tmp_size_ptr, tmp_data_ptr.cast()
    );
    assert_eq!(status, 0);
    assert_eq!(size, env.mem.read(tmp_size_ptr));
    let audio_desc = env.mem.read(tmp_data_ptr);
    log_dbg!("audio_desc {:?}", audio_desc);
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_desc = Some(audio_desc);

    let aq_ref_ptr: MutPtr<AudioQueueRef> = env.mem.alloc(guest_size_of::<AudioQueueRef>()).cast();
    let common_modes = ns_string::get_static_str(env, kCFRunLoopCommonModes);
    let status = AudioQueueNewOutput(
        env, tmp_data_ptr.cast_const(), callback, this.cast(),
        Ptr::null(), common_modes, 0, aq_ref_ptr
    );
    assert_eq!(status, 0);
    let aq_ref = env.mem.read(aq_ref_ptr);
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_queue = Some(aq_ref);

    // Reapply the previously set current time and volume in case
    // setVolume/setCurrentTime were called before prepareToPlay
    let volume = env.objc.borrow::<AVAudioPlayerHostObject>(this).volume;
    () = msg![env; this setVolume:volume];
    let set_current_time = env.objc.borrow::<AVAudioPlayerHostObject>(this).set_current_time;
    () = msg![env; this setCurrentTime:set_current_time];

    let size = guest_size_of::<u32>();
    env.mem.write(tmp_size_ptr, size);
    let prop_size_ptr: MutPtr<u32> = env.mem.alloc(size).cast();
    let status = AudioFileGetProperty(
        env, audio_file_id, kAudioFilePropertyPacketSizeUpperBound, tmp_size_ptr, prop_size_ptr.cast()
    );
    assert_eq!(status, 0);
    assert_eq!(size, env.mem.read(tmp_size_ptr));
    let prop_size = env.mem.read(prop_size_ptr);

    let (buffer_byte_size, num_packets_to_read) = derive_buffer_size(audio_desc, prop_size, 0.5);
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).num_packets_to_read = num_packets_to_read;

    let buffers: MutPtr<AudioQueueBufferRef> = env.mem.alloc(kNumberBuffers as GuestUSize * guest_size_of::<AudioQueueBufferRef>()).cast();
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_queue_buffers = Some(buffers);

    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).is_playing = true;
    for i in 0..kNumberBuffers {
        let status = AudioQueueAllocateBuffer(env, aq_ref, buffer_byte_size, buffers + i as u32);
        assert_eq!(status, 0);

        _Podroid_AVAudioPlayerOutputBufferHelper(env, this.cast(), aq_ref, env.mem.read(buffers + i as u32));
    }
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).is_playing = false;

    env.mem.free(tmp_size_ptr.cast());
    env.mem.free(aq_ref_ptr.cast());
    env.mem.free(tmp_data_ptr.cast());
}

- (bool)isPlaying {
    env.objc.borrow::<AVAudioPlayerHostObject>(this).is_playing
}

- (bool)play {
    () = msg![env; this prepareToPlay];

    let aq_ref = env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).audio_queue.unwrap();

    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).is_playing = true;

    let status = AudioQueueStart(env, aq_ref, Ptr::null());
    assert_eq!(status, 0);

    true
}

- (())pause {
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).is_playing = false;
    if let Some(aq_ref) = env.objc.borrow::<AVAudioPlayerHostObject>(this).audio_queue {
        AudioQueuePause(env, aq_ref);
    }
}

- (())stop {
    let &mut AVAudioPlayerHostObject {
        audio_queue,
        audio_queue_buffers,
        ..
    } = env.objc.borrow_mut::<AVAudioPlayerHostObject>(this);
    if audio_queue.is_none() {
        // already being stopped
        return;
    }
    AudioQueueDispose(env, audio_queue.unwrap(), true);
    env.mem.free(audio_queue_buffers.unwrap().cast());

    let &AVAudioPlayerHostObject { audio_file_url, output_callback, num_of_loops, audio_file_id, .. } = env.objc.borrow(this);
    *env.objc.borrow_mut::<AVAudioPlayerHostObject>(this) = AVAudioPlayerHostObject {
        audio_file_url,
        output_callback,
        num_of_loops,
        audio_file_id,
        audio_desc: None,
        audio_queue: None,
        audio_queue_buffers: None,
        num_packets_to_read: 0,
        current_packet: 0,
        set_current_time: 0.0,
        volume: 1.0,
        is_playing: false
    };
}

- (())setNumberOfLoops:(NSInteger)numberOfLoops {
    log_dbg!("[(AVAudioPlayer *) {:?} setNumberOfLoops:{:?}]", this, numberOfLoops);
    env.objc.borrow_mut::<AVAudioPlayerHostObject>(this).num_of_loops = numberOfLoops;
}

- (())dealloc {
    () = msg![env; this stop];
    let &AVAudioPlayerHostObject {audio_file_url, audio_file_id, ..} = env.objc.borrow(this);
    release(env, audio_file_url);
    if let Some(audio_file_id) = audio_file_id {
        AudioFileClose(env, audio_file_id);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (NSTimeInterval)currentTime {
    let host_object = env.objc.borrow::<AVAudioPlayerHostObject>(this);
    let current_time = if let Some(audio_desc) = host_object.audio_desc {
        let current_frame = (host_object.current_packet as f64) * (audio_desc.frames_per_packet as f64);
        current_frame / audio_desc.sample_rate
    } else {
        0.0
    };
    log_dbg!("[(AVAudioPlayer *) {:?} currentTime] -> {:?}", this, current_time);
    current_time
}
- (())setCurrentTime:(NSTimeInterval)currentTime {
    // TODO: Support setting current time before having an audio description
    let host_object = env.objc.borrow_mut::<AVAudioPlayerHostObject>(this);
    host_object.set_current_time = currentTime;
    if let (Some(audio_desc), Some(audio_file_id)) = (host_object.audio_desc, host_object.audio_file_id) {
        let total_packets = audio_file::State::get(&mut env.framework_state).audio_files.get(&audio_file_id).unwrap().packet_count();
        let total_frames = total_packets * audio_desc.frames_per_packet as u64;
        let new_current_frame = audio_desc.sample_rate * currentTime;
        if new_current_frame < 0.0 || new_current_frame > total_frames as f64 {
            host_object.current_packet = 0;
        } else {
            host_object.current_packet = (new_current_frame / (audio_desc.frames_per_packet as f64)) as i64;
        }
    }
    log_dbg!("[(AVAudioPlayer *) {:?} setCurrentTime: {}]", this, currentTime);
}

- (NSTimeInterval)duration {
    let host_object = env.objc.borrow::<AVAudioPlayerHostObject>(this);
    let audio_file_id = host_object.audio_file_id.unwrap();
    let audio_file = &audio_file::State::get(&mut env.framework_state)
        .audio_files
        .get(&audio_file_id)
        .unwrap();
    audio_file.estimated_duration()
}

@end

@implementation AVAudioRecorder: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(AVAudioRecorderHostObject {
        url: nil,
        is_recording: false,
        metering_enabled: false,
        delegate: nil,
        audio_file_id: None,
        requested_format: None,
        format: None,
        write_offset: 0,
        mic_started: false,
        last_pump: None,
        average_power: -160.0,
        peak_power: -160.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithURL:(id)url
         settings:(id)settings
            error:(MutPtr<id>)out_error {
    if url == nil {
        if !out_error.is_null() {
            env.mem.write(out_error, nil);
        }
        return nil;
    }
    let Some(format) = recorder_format_from_settings(env, settings) else {
        if !out_error.is_null() {
            let domain = ns_string::get_static_str(env, NSOSStatusErrorDomain);
            let error = msg_class![env; NSError alloc];
            let code = fourcc(b"fmt?") as NSInteger;
            let error = msg![env; error initWithDomain:domain code:code userInfo:nil];
            env.mem.write(out_error, error);
        }
        return nil;
    };
    retain(env, url);
    let host = env.objc.borrow_mut::<AVAudioRecorderHostObject>(this);
    host.url = url;
    host.requested_format = Some(format);
    this
}

- (bool)recordForDuration:(NSTimeInterval)_duration {
    msg![env; this record]
}

- (bool)prepareToRecord {
    // The real capture session (destination CAF file + host mic) is created
    // lazily by `record`; games that call prepareToRecord first then record
    // get the same behaviour either way.
    log_dbg!("[(AVAudioRecorder *){:?} prepareToRecord]", this);
    true
}

- (())pause {
    recorder_stop_capture(env, this);
}

- (())stop {
    recorder_stop_capture(env, this);
}

- (bool)isRecording {
    pump_recorder(env, this);
    env.objc
        .borrow::<AVAudioRecorderHostObject>(this)
        .is_recording
}

- (bool)deleteRecording {
    recorder_stop_capture(env, this);
    true
}

- (id)url {
    env.objc.borrow::<AVAudioRecorderHostObject>(this).url
}

- (NSTimeInterval)currentTime {
    pump_recorder(env, this);
    let host = env.objc.borrow::<AVAudioRecorderHostObject>(this);
    let bytes_per_frame = host
        .format
        .map(|f| (f.bytes_per_frame.max(1)) as u64)
        .unwrap_or(2);
    let sample_rate = host
        .format
        .map(|f| if f.sample_rate > 0.0 { f.sample_rate } else { 44100.0 })
        .unwrap_or(44100.0);
    (host.write_offset / bytes_per_frame) as f64 / sample_rate
}

- (())setMeteringEnabled:(bool)enabled {
    env.objc
        .borrow_mut::<AVAudioRecorderHostObject>(this)
        .metering_enabled = enabled;
}

- (bool)isMeteringEnabled {
    env.objc
        .borrow::<AVAudioRecorderHostObject>(this)
        .metering_enabled
}

- (())updateMeters {
    pump_recorder(env, this);
}

- (f32)averagePowerForChannel:(NSInteger)_channel {
    pump_recorder(env, this);
    let _ = _channel;
    env.objc
        .borrow::<AVAudioRecorderHostObject>(this)
        .average_power
}

- (f32)peakPowerForChannel:(NSInteger)_channel {
    pump_recorder(env, this);
    let _ = _channel;
    env.objc
        .borrow::<AVAudioRecorderHostObject>(this)
        .peak_power
}

- (id)delegate {
    env.objc.borrow::<AVAudioRecorderHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    env.objc
        .borrow_mut::<AVAudioRecorderHostObject>(this)
        .delegate = delegate;
}

- (())dealloc {
    let url = env
        .objc
        .borrow::<AVAudioRecorderHostObject>(this)
        .url;
    if url != nil {
        release(env, url);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (bool)record {
    // Real microphone capture when the host provides one (Android: JNI ->
    // AudioRecord; elsewhere: graceful degradation to the old silent stub).
    let (url, already, requested_format) = {
        let host = env.objc.borrow::<AVAudioRecorderHostObject>(this);
        (host.url, host.is_recording, host.requested_format)
    };
    if already {
        return true;
    }
    if url == nil {
        return false;
    }

    if !crate::android_media::has_microphone() {
        log!(
            "[(AVAudioRecorder *){:?} record] no host microphone available; \
             recording silence",
            this
        );

    }

    let Some(asbd) = requested_format else {
        return false;
    };
    let tmp_afi_ptr: MutPtr<AudioFileID> = env.mem.alloc(guest_size_of::<AudioFileID>()).cast();
    let asbd_ptr = env.mem.alloc_and_write(asbd);
    let status = AudioFileCreateWithURL(
        env,
        url.cast(),
        kAudioFileCAFType,
        asbd_ptr.cast_const(),
        0,
        tmp_afi_ptr,
    );
    env.mem.free(asbd_ptr.cast());
    let audio_file_id = env.mem.read(tmp_afi_ptr);
    env.mem.free(tmp_afi_ptr.cast());
    if status != 0 {
        log!(
            "[(AVAudioRecorder *){:?} record] AudioFileCreateWithURL failed: {}",
            this,
            status
        );
        return false;
    }

    let mic_started = crate::android_media::start_mic();
    if !mic_started {
        log!(
            "[(AVAudioRecorder *){:?} record] mic unavailable, recording silence",
            this
        );
    }

    {
        let host = env.objc.borrow_mut::<AVAudioRecorderHostObject>(this);
        host.audio_file_id = Some(audio_file_id);
        host.format = Some(asbd);
        host.write_offset = 0;
        host.mic_started = mic_started;
        host.last_pump = Some(Instant::now());
        host.is_recording = true;
        host.average_power = -160.0;
        host.peak_power = -160.0;
    }
    log!(
        "[(AVAudioRecorder *){:?} record] real capture {}",
        this,
        if mic_started { "started" } else { "(silence)" }
    );
    true
}
@end

};

// Listing 3-7 from `Deriving a playback audio queue buffer size`
// from the Apple's guide
fn derive_buffer_size(
    audio_desc: AudioStreamBasicDescription,
    max_packet_size: u32,
    seconds: f64,
) -> (u32, u32) {
    let mut out_buffer_size;

    const max_buffer_size: u32 = 0x50000;
    const min_buffer_size: u32 = 0x4000;

    if audio_desc.frames_per_packet != 0 {
        let num_packets_to_time =
            audio_desc.sample_rate / audio_desc.frames_per_packet as f64 * seconds;
        out_buffer_size = num_packets_to_time as u32 * max_packet_size;
    } else {
        out_buffer_size = if max_buffer_size > max_packet_size {
            max_buffer_size
        } else {
            max_packet_size
        }
    }

    if out_buffer_size > max_buffer_size && out_buffer_size > max_packet_size {
        out_buffer_size = max_buffer_size
    } else if out_buffer_size < min_buffer_size {
        out_buffer_size = min_buffer_size
    }

    let out_num_packets_to_read = out_buffer_size / max_packet_size;
    (out_buffer_size, out_num_packets_to_read)
}

/// (*void)(void *in_user_data, AudioQueueRef in_aq, AudioQueueBufferRef in_buf)
fn _Podroid_AVAudioPlayerOutputBufferHelper(
    env: &mut Environment,
    in_user_data: MutVoidPtr,
    in_aq: AudioQueueRef,
    in_buf: AudioQueueBufferRef,
) {
    let av_audio_player: id = in_user_data.cast();
    let class: Class = msg![env; av_audio_player class];
    log_dbg!(
        "_Podroid_AVAudioPlayerOutputBufferHelper on object of class: {}",
        env.objc.get_class_name(class)
    );
    let audio_player_class = env.objc.get_known_class("AVAudioPlayer", &mut env.mem);
    assert!(env.objc.class_is_subclass_of(class, audio_player_class));

    let &AVAudioPlayerHostObject {
        audio_file_id,
        audio_queue,
        num_packets_to_read,
        current_packet,
        is_playing,
        ..
    } = env.objc.borrow(av_audio_player);
    let aq = audio_queue.unwrap();
    assert_eq!(aq, in_aq);

    if !is_playing {
        return;
    }

    let num_bytes_ptr: MutPtr<u32> = env.mem.alloc(guest_size_of::<u32>()).cast();
    let num_packets_ptr: MutPtr<u32> = env.mem.alloc(guest_size_of::<u32>()).cast();
    env.mem.write(num_packets_ptr, num_packets_to_read);
    let mut audio_queue_buffer = env.mem.read(in_buf);
    let status = AudioFileReadPackets(
        env,
        audio_file_id.unwrap(),
        false,
        num_bytes_ptr,
        Ptr::null(),
        current_packet,
        num_packets_ptr,
        audio_queue_buffer.audio_data,
    );
    let num_packets = env.mem.read(num_packets_ptr);
    let num_bytes = env.mem.read(num_bytes_ptr);
    env.mem.free(num_packets_ptr.cast());
    env.mem.free(num_bytes_ptr.cast());

    if num_packets > 0 {
        assert!(status == 0 || status == eofErr);
        audio_queue_buffer.audio_data_byte_size = num_bytes;
        env.mem.write(in_buf, audio_queue_buffer);
        let status = AudioQueueEnqueueBuffer(env, aq, in_buf, 0, Ptr::null());
        assert_eq!(status, 0);
        env.objc
            .borrow_mut::<AVAudioPlayerHostObject>(av_audio_player)
            .current_packet = current_packet + num_packets as i64;
    } else {
        assert_eq!(status, eofErr);
        let number_of_loops = env
            .objc
            .borrow::<AVAudioPlayerHostObject>(av_audio_player)
            .num_of_loops;
        if number_of_loops == 0 {
            let status = AudioQueueStop(env, aq, false);
            assert_eq!(status, 0);
            env.objc
                .borrow_mut::<AVAudioPlayerHostObject>(av_audio_player)
                .is_playing = false;
        } else {
            if number_of_loops > 0 {
                env.objc
                    .borrow_mut::<AVAudioPlayerHostObject>(av_audio_player)
                    .num_of_loops -= 1;
            }
            env.objc
                .borrow_mut::<AVAudioPlayerHostObject>(av_audio_player)
                .current_packet = 0;
            _Podroid_AVAudioPlayerOutputBufferHelper(env, in_user_data, in_aq, in_buf);
        }
    }
}
#[derive(Default)]
struct AVAudioRecorderHostObject {
    url: id,
    is_recording: bool,
    metering_enabled: bool,
    delegate: id,
    /// Real microphone capture state (all None/zero when the host has no
    /// mic — in that case the recorder degrades to its old silent stub).
    audio_file_id: Option<AudioFileID>,
    requested_format: Option<AudioStreamBasicDescription>,
    format: Option<AudioStreamBasicDescription>,
    write_offset: u64,
    mic_started: bool,
    last_pump: Option<Instant>,
    average_power: f32,
    peak_power: f32,
}

impl HostObject for AVAudioRecorderHostObject {}

fn recorder_setting(env: &mut Environment, settings: id, key: &'static str) -> id {
    if settings == nil {
        return nil;
    }
    let key = ns_string::get_static_str(env, key);
    msg![env; settings objectForKey:key]
}

fn recorder_format_from_settings(
    env: &mut Environment,
    settings: id,
) -> Option<AudioStreamBasicDescription> {
    let format_value = recorder_setting(env, settings, "AVFormatIDKey");
    let format_id = if format_value == nil {
        kAudioFormatLinearPCM
    } else {
        msg![env; format_value unsignedIntValue]
    };
    if format_id != kAudioFormatLinearPCM {
        return None;
    }

    let sample_rate_value = recorder_setting(env, settings, "AVSampleRateKey");
    let sample_rate: f64 = if sample_rate_value == nil {
        44100.0
    } else {
        msg![env; sample_rate_value doubleValue]
    };
    let channel_value = recorder_setting(env, settings, "AVNumberOfChannelsKey");
    let channels: u32 = if channel_value == nil {
        1
    } else {
        msg![env; channel_value unsignedIntValue]
    };
    let bit_depth_value = recorder_setting(env, settings, "AVLinearPCMBitDepthKey");
    let bits: u32 = if bit_depth_value == nil {
        16
    } else {
        msg![env; bit_depth_value unsignedIntValue]
    };
    let float_value = recorder_setting(env, settings, "AVLinearPCMIsFloatKey");
    let is_float = float_value != nil && msg![env; float_value boolValue];
    let big_endian_value = recorder_setting(env, settings, "AVLinearPCMIsBigEndianKey");
    let is_big_endian = big_endian_value != nil && msg![env; big_endian_value boolValue];

    if !sample_rate.is_finite()
        || sample_rate <= 0.0
        || channels == 0
        || channels > 8
        || !matches!(bits, 16 | 24 | 32)
        || (is_float && bits != 32)
    {
        return None;
    }
    let bytes_per_frame = channels.checked_mul(bits / 8)?;
    let format_flags = kAudioFormatFlagIsPacked
        | if is_float {
            kAudioFormatFlagIsFloat
        } else {
            kAudioFormatFlagIsSignedInteger
        }
        | if is_big_endian {
            kAudioFormatFlagIsBigEndian
        } else {
            0
        };
    Some(AudioStreamBasicDescription {
        sample_rate,
        format_id: kAudioFormatLinearPCM,
        format_flags,
        bytes_per_packet: bytes_per_frame,
        frames_per_packet: 1,
        bytes_per_frame,
        channels_per_frame: channels,
        bits_per_channel: bits,
        _reserved: 0,
    })
}

#[allow(dead_code)]
fn pcm_meter_levels(
    pcm: &[u8],
    channels: usize,
    bits_per_sample: u32,
) -> Option<(Vec<f32>, Vec<f32>)> {
    if channels == 0 || !matches!(bits_per_sample, 8 | 16) {
        return None;
    }
    let bytes_per_sample = (bits_per_sample / 8) as usize;
    let frame_size = channels.checked_mul(bytes_per_sample)?;
    let frames = pcm.len() / frame_size;
    if frames == 0 {
        return None;
    }
    let mut sums = vec![0.0f64; channels];
    let mut peaks = vec![0.0f64; channels];
    for frame in pcm.chunks_exact(frame_size) {
        for channel in 0..channels {
            let offset = channel * bytes_per_sample;
            let sample = if bits_per_sample == 8 {
                (frame[offset] as i32 - 128) * 256
            } else {
                i16::from_le_bytes([frame[offset], frame[offset + 1]]) as i32
            };
            let magnitude = sample.abs() as f64;
            sums[channel] += magnitude * magnitude;
            peaks[channel] = peaks[channel].max(magnitude);
        }
    }
    let to_db = |magnitude: f64| {
        if magnitude <= 0.0 {
            -160.0
        } else {
            (20.0 * (magnitude / 32768.0).log10()).clamp(-160.0, 0.0) as f32
        }
    };
    let average_power = sums
        .into_iter()
        .map(|sum| to_db((sum / frames as f64).sqrt()))
        .collect();
    let peak_power = peaks.into_iter().map(to_db).collect();
    Some((average_power, peak_power))
}

/// Stop capture: halt the host mic stream and close the audio file.
fn recorder_stop_capture(env: &mut Environment, this: id) {
    let (was_recording, mic_started, file_id) = {
        let host = env.objc.borrow::<AVAudioRecorderHostObject>(this);
        (host.is_recording, host.mic_started, host.audio_file_id)
    };
    if !was_recording {
        return;
    }
    if mic_started {
        crate::android_media::stop_mic();
    }
    if let Some(file_id) = file_id {
        AudioFileClose(env, file_id);
    }
    let host = env.objc.borrow_mut::<AVAudioRecorderHostObject>(this);
    host.is_recording = false;
    host.mic_started = false;
    host.audio_file_id = None;
}

/// Pump pending mic samples into the recorder's audio file. Called lazily
/// from currentTime/updateMeters/isRecording so no background thread is
/// needed. On platforms without a host mic this is a no-op.
fn pump_recorder(env: &mut Environment, this: id) {
    let (recording, file_id, format, mic_started, last_pump) = {
        let host = env.objc.borrow::<AVAudioRecorderHostObject>(this);
        (
            host.is_recording,
            host.audio_file_id,
            host.format,
            host.mic_started,
            host.last_pump,
        )
    };
    if !recording {
        return;
    }
    let Some(file_id) = file_id else { return };
    let Some(format) = format else { return };

    let now = Instant::now();
    let elapsed = last_pump
        .map(|last| now.duration_since(last).as_secs_f64())
        .unwrap_or_default();
    env.objc
        .borrow_mut::<AVAudioRecorderHostObject>(this)
        .last_pump = Some(now);

    let samples: Vec<i16> = if mic_started {
        crate::android_media::read_mic_chunk()
    } else {
        let count = (elapsed * crate::android_media::MIC_SAMPLE_RATE as f64).round() as usize;
        vec![0; count]
    };
    if samples.is_empty() {
        return;
    }

    let peak = samples
        .iter()
        .map(|&sample| (sample as i32).abs() as f64)
        .fold(0.0, f64::max);
    let average = samples
        .iter()
        .map(|&sample| (sample as i32).abs() as f64)
        .sum::<f64>()
        / samples.len() as f64;
    let to_db = |magnitude: f64| {
        if magnitude <= 0.0 {
            -160.0
        } else {
            (20.0 * (magnitude / 32768.0).log10()).clamp(-160.0, 0.0) as f32
        }
    };

    let source_shape = PcmShape {
        sample_rate: crate::android_media::MIC_SAMPLE_RATE as f64,
        channels: 1,
        bits: 16,
        is_float: false,
        is_big_endian: false,
        bytes_per_frame: 2,
    };
    let Some(destination_shape) = pcm_shape_from_asbd(&format) else {
        return;
    };
    let source_bytes = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    let bytes = convert_pcm(&source_bytes, &source_shape, &destination_shape);
    if bytes.is_empty() {
        return;
    }

    let bytes_ptr = env.mem.alloc(bytes.len() as u32);
    env.mem
        .bytes_at_mut(bytes_ptr.cast(), bytes.len() as u32)
        .copy_from_slice(&bytes);
    let io_ptr = env.mem.alloc_and_write(bytes.len() as u32);
    let write_offset = env
        .objc
        .borrow::<AVAudioRecorderHostObject>(this)
        .write_offset as i64;
    let status = AudioFileWriteBytes(
        env,
        file_id,
        false,
        write_offset,
        io_ptr,
        bytes_ptr.cast_const(),
    );
    let bytes_written = env.mem.read(io_ptr);
    env.mem.free(io_ptr.cast());
    env.mem.free(bytes_ptr.cast_void());
    if status == 0 {
        let host = env.objc.borrow_mut::<AVAudioRecorderHostObject>(this);
        host.write_offset += bytes_written as u64;
        host.average_power = to_db(average);
        host.peak_power = to_db(peak);
    }
}
