/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AVPlayer`, `AVPlayerItem`, `AVPlayerLayer` and audio mixes.
//!
//! Videos are decoded with the same decoder as `MPMoviePlayerController`'s
//! (H.264 and AAC in an MPEG-4 file). Frames are shown in the
//! `AVPlayerLayer`s of the player, and the audio is played with OpenAL.
//! Audio mixes are accepted but have no effect.

use crate::audio::openal::al_types::*;
use crate::audio::openal::{
    AL_BUFFER, AL_FORMAT_MONO16, AL_FORMAT_STEREO16, AL_SOURCE_STATE, AL_STOPPED,
};
use crate::frameworks::core_graphics::{cg_image, CGRect};
use crate::frameworks::core_media::{kCMTimeFlags_Valid, CMTime};
use crate::frameworks::foundation::{ns_array, ns_string, ns_url, NSInteger};
use crate::frameworks::media_player::movie_decoder::{Audio, MovieDecoder};
use crate::image::Image;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_super, nil, objc_classes, release, retain, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// `AL_SEC_OFFSET`
const AL_SEC_OFFSET: ALenum = 0x1024;
/// `AL_GAIN`
const AL_GAIN: ALenum = 0x100A;

const ZERO_TIME: CMTime = CMTime {
    value: 0,
    timescale: 1,
    flags: kCMTimeFlags_Valid,
    epoch: 0,
};

#[derive(Default)]
pub struct State {
    /// `AVPlayer*` of each `AVPlayerLayer*`, retained.
    layer_players: HashMap<id, id>,
    /// Playback state of each `AVPlayer*` that has started playing.
    playbacks: HashMap<id, Playback>,
}

struct Playback {
    decoder: MovieDecoder,
    /// OpenAL source and buffer for the audio, if there is any.
    audio: Option<(ALuint, ALuint)>,
    /// Movie time at `clock_started`, or the current time when paused.
    clock_base: Duration,
    /// When the wall clock started counting from `clock_base`, if playing.
    clock_started: Option<Instant>,
    ended: bool,
}

fn cmtime_from_duration(duration: Duration) -> CMTime {
    CMTime {
        value: duration.as_millis() as i64,
        timescale: 1000,
        flags: kCMTimeFlags_Valid,
        epoch: 0,
    }
}

fn duration_from_cmtime(time: CMTime) -> Duration {
    let CMTime {
        value, timescale, ..
    } = time;
    if timescale <= 0 || value <= 0 {
        Duration::ZERO
    } else {
        Duration::from_secs_f64(value as f64 / timescale as f64)
    }
}

/// Open an item's movie file.
fn open_movie(env: &mut Environment, item: id) -> Option<(MovieDecoder, Option<Audio>)> {
    let url = env.objc.borrow::<AVPlayerItemHostObject>(item).url;
    if url == nil {
        return None;
    }
    let path = ns_url::to_rust_path(env, url);
    let Ok(bytes) = env.fs.read(&path) else {
        log!("Warning: AVPlayerItem couldn't read {:?}", path);
        return None;
    };
    match MovieDecoder::new(bytes) {
        Ok(movie) => Some(movie),
        Err(e) => {
            log!("Warning: AVPlayerItem can't play {:?}: {}", path, e);
            None
        }
    }
}

fn delete_audio(env: &mut Environment, audio: Option<(ALuint, ALuint)>) {
    let Some((source, buffer)) = audio else {
        return;
    };
    let al = env
        .framework_state
        .audio_toolbox
        .make_al_context_current(&mut env.openal_manager);
    unsafe {
        al.SourceStop(source);
        al.DeleteSources(1, &source);
        al.DeleteBuffers(1, &buffer);
    }
}

/// Stop playback and forget about it.
fn stop_playback(env: &mut Environment, player: id) {
    if let Some(playback) = env
        .framework_state
        .avfoundation
        .av_player
        .playbacks
        .remove(&player)
    {
        delete_audio(env, playback.audio);
    }
}

/// Start playing the player's current item from `start`.
fn start_playback(env: &mut Environment, player: id, start: Duration) -> bool {
    stop_playback(env, player);
    let item = env.objc.borrow::<AVPlayerHostObject>(player).current_item;
    if item == nil {
        return false;
    }
    let prepared = env
        .objc
        .borrow_mut::<AVPlayerItemHostObject>(item)
        .prepared
        .take();
    let Some((mut decoder, audio)) = prepared.or_else(|| open_movie(env, item)) else {
        return false;
    };
    // Skip the frames before the start. They have to be decoded, since later
    // frames depend on them.
    while decoder.next_frame_time().is_some_and(|time| time < start) {
        decoder.decode_next_frame();
    }
    let volume = env.objc.borrow::<AVPlayerHostObject>(player).volume;
    let audio = audio.filter(|audio| !audio.pcm.is_empty()).map(|audio| {
        let format = if audio.channels == 1 {
            AL_FORMAT_MONO16
        } else {
            AL_FORMAT_STEREO16
        };
        let al = env
            .framework_state
            .audio_toolbox
            .make_al_context_current(&mut env.openal_manager);
        let mut source: ALuint = 0;
        let mut buffer: ALuint = 0;
        unsafe {
            al.GenBuffers(1, &mut buffer);
            al.BufferData(
                buffer,
                format,
                audio.pcm.as_ptr().cast(),
                audio.pcm.len() as ALsizei,
                audio.sample_rate as ALsizei,
            );
            al.GenSources(1, &mut source);
            al.Sourcei(source, AL_BUFFER, buffer as ALint);
            al.Sourcef(source, AL_GAIN, volume);
            al.Sourcef(source, AL_SEC_OFFSET, start.as_secs_f32());
            al.SourcePlay(source);
        }
        (source, buffer)
    });
    env.framework_state.avfoundation.av_player.playbacks.insert(
        player,
        Playback {
            decoder,
            audio,
            clock_base: start,
            clock_started: Some(Instant::now()),
            ended: false,
        },
    );
    true
}

/// The current time of a playback. The audio is the clock if there is any
/// (so that the video stays in sync with it), unless the audio output doesn't
/// work.
fn playback_time(env: &mut Environment, player: id) -> Option<Duration> {
    let playback = env
        .framework_state
        .avfoundation
        .av_player
        .playbacks
        .get(&player)?;
    let wall_time = match playback.clock_started {
        Some(started) => playback.clock_base + started.elapsed(),
        None => playback.clock_base,
    };
    let Some((source, _)) = playback.audio else {
        return Some(wall_time);
    };
    let paused = playback.clock_started.is_none();
    let al = env
        .framework_state
        .audio_toolbox
        .make_al_context_current(&mut env.openal_manager);
    let mut seconds: ALfloat = 0.0;
    let mut state: ALint = 0;
    unsafe {
        al.GetSourcef(source, AL_SEC_OFFSET, &mut seconds);
        al.GetSourcei(source, AL_SOURCE_STATE, &mut state);
    }
    if paused || state == AL_STOPPED {
        return Some(wall_time);
    }
    let audio_time = Duration::from_secs_f32(seconds.max(0.0));
    // If the audio hasn't moved for a while, the audio output probably doesn't
    // work (e.g. a Bluetooth speaker that isn't playing).
    if audio_time.is_zero() && wall_time > Duration::from_secs(1) {
        Some(wall_time)
    } else {
        Some(audio_time)
    }
}

/// Fit a video frame to the layer's aspect ratio by adding transparent bars,
/// like `AVLayerVideoGravityResizeAspect` (the layer stretches its contents).
/// The bars are transparent like those of a real `AVPlayerLayer`, so whatever
/// is behind the layer (e.g. a decorative border) shows through.
fn letterbox(pixels: Vec<u8>, width: u32, height: u32, bounds: CGRect) -> (Vec<u8>, u32, u32) {
    let (layer_width, layer_height) = (bounds.size.width, bounds.size.height);
    if layer_width <= 0.0 || layer_height <= 0.0 || width == 0 || height == 0 {
        return (pixels, width, height);
    }
    let layer_aspect = layer_width / layer_height;
    let frame_aspect = width as f32 / height as f32;
    if (layer_aspect - frame_aspect).abs() < 0.01 {
        return (pixels, width, height);
    }
    let (new_width, new_height) = if frame_aspect > layer_aspect {
        (width, (width as f32 / layer_aspect).round() as u32)
    } else {
        ((height as f32 * layer_aspect).round() as u32, height)
    };
    let mut out = [0u8, 0, 0, 0].repeat((new_width * new_height) as usize);
    let (x0, y0) = ((new_width - width) / 2, (new_height - height) / 2);
    for y in 0..height {
        let src = (y * width * 4) as usize;
        let dst = (((y + y0) * new_width + x0) * 4) as usize;
        out[dst..dst + (width * 4) as usize]
            .copy_from_slice(&pixels[src..src + (width * 4) as usize]);
    }
    (out, new_width, new_height)
}

/// For use by `NSRunLoop`: show the frames that are due, and tell the app
/// about items that finished playing.
pub fn handle_players(env: &mut Environment) {
    let players: Vec<id> = env
        .framework_state
        .avfoundation
        .av_player
        .playbacks
        .iter()
        .filter(|(_, playback)| !playback.ended)
        .map(|(&player, _)| player)
        .collect();
    for player in players {
        let Some(time) = playback_time(env, player) else {
            continue;
        };
        let playback = env
            .framework_state
            .avfoundation
            .av_player
            .playbacks
            .get_mut(&player)
            .unwrap();
        let mut latest_frame = None;
        let mut video_finished = false;
        loop {
            match playback.decoder.next_frame_time() {
                None => {
                    video_finished = true;
                    break;
                }
                Some(frame_time) if frame_time <= time => {
                    if let Some(Some(frame)) = playback.decoder.decode_next_frame() {
                        latest_frame = Some(frame);
                    }
                }
                Some(_) => break,
            }
        }
        let duration = playback.decoder.duration();
        if let Some(frame) = latest_frame {
            let layers: Vec<id> = env
                .framework_state
                .avfoundation
                .av_player
                .layer_players
                .iter()
                .filter(|(_, &layer_player)| layer_player == player)
                .map(|(&layer, _)| layer)
                .collect();
            for layer in layers {
                let bounds: CGRect = msg![env; layer bounds];
                let (pixels, width, height) =
                    letterbox(frame.pixels.clone(), frame.width, frame.height, bounds);
                let image =
                    cg_image::from_image(env, Image::from_pixel_vec(pixels, (width, height)));
                () = msg![env; layer setContents:image];
                release(env, image);
            }
        }
        if video_finished && time >= duration {
            let playback = env
                .framework_state
                .avfoundation
                .av_player
                .playbacks
                .get_mut(&player)
                .unwrap();
            playback.ended = true;
            playback.clock_base = duration;
            playback.clock_started = None;
            let audio = playback.audio.take();
            delete_audio(env, audio);
            env.objc.borrow_mut::<AVPlayerHostObject>(player).rate = 0.0;
            let item = env.objc.borrow::<AVPlayerHostObject>(player).current_item;
            if item != nil {
                retain(env, item);
                let center: id = msg_class![env; NSNotificationCenter defaultCenter];
                let name =
                    ns_string::get_static_str(env, "AVPlayerItemDidPlayToEndTimeNotification");
                () = msg![env; center postNotificationName:name object:item];
                release(env, item);
            }
        }
    }
}

struct AVPlayerItemHostObject {
    /// `NSURL*`
    url: id,
    /// `AVAudioMix*`
    audio_mix: id,
    /// The movie, opened when the item is created (so its duration is
    /// known), until it's played.
    prepared: Option<(MovieDecoder, Option<Audio>)>,
    duration: Duration,
}
impl HostObject for AVPlayerItemHostObject {}

/// The player playing an item, if any.
fn player_for_item(env: &Environment, item: id) -> Option<id> {
    env.framework_state
        .avfoundation
        .av_player
        .playbacks
        .keys()
        .copied()
        .find(|&player| env.objc.borrow::<AVPlayerHostObject>(player).current_item == item)
}

struct AVPlayerHostObject {
    /// `AVPlayerItem*`
    current_item: id,
    rate: f32,
    volume: f32,
}
impl HostObject for AVPlayerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation AVPlayerItem: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(AVPlayerItemHostObject {
        url: nil,
        audio_mix: nil,
        prepared: None,
        duration: Duration::ZERO,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)playerItemWithURL:(id)url { // NSURL*
    let item: id = msg![env; this alloc];
    let item: id = msg![env; item initWithURL:url];
    autorelease(env, item)
}

- (id)initWithURL:(id)url { // NSURL*
    retain(env, url);
    env.objc.borrow_mut::<AVPlayerItemHostObject>(this).url = url;
    if let Some((decoder, audio)) = open_movie(env, this) {
        let host_object = env.objc.borrow_mut::<AVPlayerItemHostObject>(this);
        host_object.duration = decoder.duration();
        host_object.prepared = Some((decoder, audio));
    }
    this
}

- (())dealloc {
    let &AVPlayerItemHostObject { url, audio_mix, .. } = env.objc.borrow(this);
    release(env, url);
    release(env, audio_mix);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)asset {
    // Assets aren't implemented. Returning nil means e.g. asking for its
    // tracks gives nil.
    nil
}

- (id)audioMix {
    env.objc.borrow::<AVPlayerItemHostObject>(this).audio_mix
}
- (())setAudioMix:(id)audio_mix {
    let audio_mix: id = msg![env; audio_mix copy];
    let old = std::mem::replace(
        &mut env.objc.borrow_mut::<AVPlayerItemHostObject>(this).audio_mix,
        audio_mix,
    );
    release(env, old);
}

- (CMTime)duration {
    cmtime_from_duration(env.objc.borrow::<AVPlayerItemHostObject>(this).duration)
}
- (CMTime)currentTime {
    match player_for_item(env, this) {
        Some(player) => msg![env; player currentTime],
        None => ZERO_TIME,
    }
}
- (NSInteger)status {
    // AVPlayerItemStatusReadyToPlay or AVPlayerItemStatusFailed
    if env.objc.borrow::<AVPlayerItemHostObject>(this).duration.is_zero() { 2 } else { 1 }
}

@end

@implementation AVPlayer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(AVPlayerHostObject {
        current_item: nil,
        rate: 0.0,
        volume: 1.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)playerWithPlayerItem:(id)item {
    let player: id = msg![env; this alloc];
    let player: id = msg![env; player initWithPlayerItem:item];
    autorelease(env, player)
}
+ (id)playerWithURL:(id)url {
    let player: id = msg![env; this alloc];
    let player: id = msg![env; player initWithURL:url];
    autorelease(env, player)
}

- (id)init {
    this
}
- (id)initWithPlayerItem:(id)item {
    retain(env, item);
    env.objc.borrow_mut::<AVPlayerHostObject>(this).current_item = item;
    this
}
- (id)initWithURL:(id)url {
    let item: id = msg_class![env; AVPlayerItem playerItemWithURL:url];
    msg![env; this initWithPlayerItem:item]
}

- (())dealloc {
    stop_playback(env, this);
    let item = env.objc.borrow::<AVPlayerHostObject>(this).current_item;
    release(env, item);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)currentItem {
    env.objc.borrow::<AVPlayerHostObject>(this).current_item
}
- (())replaceCurrentItemWithPlayerItem:(id)item {
    stop_playback(env, this);
    retain(env, item);
    let host_object = env.objc.borrow_mut::<AVPlayerHostObject>(this);
    let old = std::mem::replace(&mut host_object.current_item, item);
    host_object.rate = 0.0;
    release(env, old);
}

- (())play {
    env.objc.borrow_mut::<AVPlayerHostObject>(this).rate = 1.0;
    let state = &mut env.framework_state.avfoundation.av_player;
    if let Some(playback) = state.playbacks.get_mut(&this) {
        if playback.ended {
            // Like on iPhone OS, playing at the end doesn't restart.
            return;
        }
        if playback.clock_started.is_none() {
            playback.clock_started = Some(Instant::now());
            if let Some((source, _)) = playback.audio {
                let al = env
                    .framework_state
                    .audio_toolbox
                    .make_al_context_current(&mut env.openal_manager);
                unsafe { al.SourcePlay(source) };
            }
        }
        return;
    }
    if !start_playback(env, this, Duration::ZERO) {
        // Act as if playback immediately completed, so apps waiting for it
        // carry on.
        let item = env.objc.borrow::<AVPlayerHostObject>(this).current_item;
        log!("Warning: [(AVPlayer*){:?} play]: can't play {:?}, finishing it", this, item);
        if item != nil {
            let center: id = msg_class![env; NSNotificationCenter defaultCenter];
            let name = ns_string::get_static_str(env, "AVPlayerItemDidPlayToEndTimeNotification");
            () = msg![env; center postNotificationName:name object:item];
        }
    }
}
- (())pause {
    let time = playback_time(env, this);
    env.objc.borrow_mut::<AVPlayerHostObject>(this).rate = 0.0;
    let state = &mut env.framework_state.avfoundation.av_player;
    if let Some(playback) = state.playbacks.get_mut(&this) {
        playback.clock_base = time.unwrap_or(playback.clock_base);
        playback.clock_started = None;
        if let Some((source, _)) = playback.audio {
            let al = env
                .framework_state
                .audio_toolbox
                .make_al_context_current(&mut env.openal_manager);
            unsafe { al.SourcePause(source) };
        }
    }
}
- (f32)rate {
    env.objc.borrow::<AVPlayerHostObject>(this).rate
}
- (())setRate:(f32)rate {
    // TODO: rates other than 0 and 1
    if rate != 0.0 {
        () = msg![env; this play];
    } else {
        () = msg![env; this pause];
    }
}
- (f32)volume {
    env.objc.borrow::<AVPlayerHostObject>(this).volume
}
- (())setVolume:(f32)volume {
    env.objc.borrow_mut::<AVPlayerHostObject>(this).volume = volume;
    let audio = env.framework_state.avfoundation.av_player.playbacks.get(&this).and_then(|p| p.audio);
    if let Some((source, _)) = audio {
        let al = env
            .framework_state
            .audio_toolbox
            .make_al_context_current(&mut env.openal_manager);
        unsafe { al.Sourcef(source, AL_GAIN, volume) };
    }
}

- (CMTime)currentTime {
    cmtime_from_duration(playback_time(env, this).unwrap_or(Duration::ZERO))
}
- (())seekToTime:(CMTime)time {
    let time = duration_from_cmtime(time);
    let playing = env.objc.borrow::<AVPlayerHostObject>(this).rate != 0.0;
    if !start_playback(env, this, time) {
        return;
    }
    if !playing {
        // Seeking while paused stays paused.
        () = msg![env; this pause];
    }
}
- (())seekToTime:(CMTime)time
  toleranceBefore:(CMTime)_before
   toleranceAfter:(CMTime)_after {
    () = msg![env; this seekToTime:time];
}

@end

@implementation AVPlayerLayer: CALayer

+ (id)playerLayerWithPlayer:(id)player {
    let layer: id = msg![env; this new];
    () = msg![env; layer setPlayer:player];
    autorelease(env, layer)
}

- (id)player {
    env.framework_state
        .avfoundation
        .av_player
        .layer_players
        .get(&this)
        .copied()
        .unwrap_or(nil)
}
- (())setPlayer:(id)player {
    retain(env, player);
    let old = env
        .framework_state
        .avfoundation
        .av_player
        .layer_players
        .insert(this, player);
    if let Some(old) = old {
        release(env, old);
    }
}

- (id)videoGravity {
    crate::frameworks::foundation::ns_string::get_static_str(env, "AVLayerVideoGravityResizeAspect")
}
- (())setVideoGravity:(id)_gravity {
}

- (())dealloc {
    let player = env
        .framework_state
        .avfoundation
        .av_player
        .layer_players
        .remove(&this);
    if let Some(player) = player {
        release(env, player);
    }
    msg_super![env; this dealloc]
}

@end

// Audio mixes are accepted but have no effect.
@implementation AVAudioMix: NSObject
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}
- (id)inputParameters {
    let array = ns_array::from_vec(env, Vec::new());
    autorelease(env, array)
}
@end

@implementation AVMutableAudioMix: AVAudioMix
+ (id)audioMix {
    let mix: id = msg![env; this new];
    autorelease(env, mix)
}
- (())setInputParameters:(id)_parameters {
}
@end

@implementation AVAudioMixInputParameters: NSObject
@end

@implementation AVMutableAudioMixInputParameters: AVAudioMixInputParameters
+ (id)audioMixInputParameters {
    let parameters: id = msg![env; this new];
    autorelease(env, parameters)
}
+ (id)audioMixInputParametersWithTrack:(id)_track {
    let parameters: id = msg![env; this new];
    autorelease(env, parameters)
}
- (())setVolume:(f32)_volume atTime:(CMTime)_time {
}
- (())setTrackID:(i32)_track_id {
}
@end

};
