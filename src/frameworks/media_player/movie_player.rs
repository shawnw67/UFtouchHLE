/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `MPMoviePlayerController` etc.

use super::movie_decoder::{MovieDecoder, Subtitle};
use crate::audio::openal::al_types::*;
use crate::audio::openal::{
    AL_BUFFER, AL_FORMAT_MONO16, AL_FORMAT_STEREO16, AL_PITCH, AL_SOURCE_STATE, AL_STOPPED,
};
use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::{cg_image, CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_string, ns_url, NSInteger, NSTimeInterval, NSUInteger};
use crate::frameworks::uikit::ui_device::UIDeviceOrientation;
use crate::frameworks::uikit::ui_device::{
    UIDeviceOrientationLandscapeLeft, UIDeviceOrientationLandscapeRight,
    UIDeviceOrientationPortraitUpsideDown,
};
use crate::frameworks::uikit::ui_font::{UILineBreakModeWordWrap, UITextAlignmentCenter};
use crate::image::Image;
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, todo_objc_setter,
    ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::io::Seek;
use std::io::SeekFrom;
use std::time::{Duration, Instant};

/// A movie that's being played.
struct Playback {
    /// `MPMoviePlayerController*`
    player: id,
    decoder: MovieDecoder,
    /// Full-screen black view on top of the app's window, which can be tapped
    /// to skip the movie (like the "Done" button on iPhone OS).
    view: id,
    embedded: bool,
    /// The view showing the video frames.
    video_view: id,
    /// OpenAL source and buffer for the audio, if there is any.
    audio: Option<(ALuint, ALuint)>,
    started: Instant,
    elapsed: Duration,
    rate: f32,
    /// Subtitles in the user's language (or English), if there are any.
    subtitles: Vec<Subtitle>,
    /// `UILabel*` showing the current subtitle.
    subtitle_label: id,
    /// Index in `subtitles` of the subtitle being shown.
    current_subtitle: Option<usize>,
}

const SUBTITLE_FONT_SIZE: CGFloat = 15.0;

/// `AL_SEC_OFFSET`
const AL_SEC_OFFSET: ALenum = 0x1024;

#[derive(Default)]
pub struct State {
    active_player: Option<id>,
    playback: Option<Playback>,
    /// Various apps (e.g. Crash Bandicoot Nitro Kart 3D and Spore Origins)
    /// create or start a player and await some kind of notification, but can't
    /// handle it if that notification happens immediately. This queue lets us
    /// delay such notifications until the app next returns to the run loop,
    /// which seems to be late enough.
    pending_notifications: VecDeque<(&'static str, id, Instant)>,
    /// The `MPMoviePlayerController` of each `MPMoviePlayerViewController`
    /// (which uses `UIViewController`'s host object).
    view_controller_players: HashMap<id, id>,
}
impl State {
    fn get(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.media_player.movie_player
    }
}

type MPMovieScalingMode = NSInteger;
type MPMovieControlStyle = NSInteger;

type MPMoviePlaybackState = NSInteger;
const MPMoviePlaybackStateStopped: MPMoviePlaybackState = 0;

// Values might not be correct, but as these are linked symbol constants, it
// shouldn't matter.
pub const MPMoviePlayerPlaybackDidFinishNotification: &str =
    "MPMoviePlayerPlaybackDidFinishNotification";
/// Apparently an undocumented, private API. Spore Origins uses it.
pub const MPMoviePlayerContentPreloadDidFinishNotification: &str =
    "MPMoviePlayerContentPreloadDidFinishNotification";
pub const MPMoviePlayerScalingModeDidChangeNotification: &str =
    "MPMoviePlayerScalingModeDidChangeNotification";
pub const MPMoviePlayerDidExitFullscreenNotification: &str =
    "MPMoviePlayerDidExitFullscreenNotification";
// TODO: More notifications?
const MPMoviePlayerPlaybackDidFinishReasonUserInfoKey: &str =
    "MPMoviePlayerPlaybackDidFinishReasonUserInfoKey";

/// `NSNotificationName` values and other constants.
/// Read the duration (in seconds) of an MPEG-4/QuickTime file from the `mvhd`
/// box inside its top-level `moov` box. Returns `None` if the file can't be
/// parsed. See ISO/IEC 14496-12, "Movie Header Box".
#[allow(dead_code)]
pub(super) fn read_mp4_duration<F: Read + Seek>(file: &mut F) -> Option<f64> {
    /// Reads a box header, returning (box start, box type, box size).
    fn box_header<F: Read + Seek>(file: &mut F) -> Option<(u64, [u8; 4], u64)> {
        let start = file.stream_position().ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;
        let mut size = u32::from_be_bytes(header[0..4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = header[4..8].try_into().unwrap();
        let mut header_len = 8;
        if size == 1 {
            let mut large = [0u8; 8];
            file.read_exact(&mut large).ok()?;
            size = u64::from_be_bytes(large);
            header_len = 16;
        } else if size == 0 {
            // Box extends to the end of the file.
            let end = file.seek(SeekFrom::End(0)).ok()?;
            file.seek(SeekFrom::Start(start + header_len)).ok()?;
            size = end - start;
        }
        if size < header_len {
            return None;
        }
        Some((start, kind, size))
    }

    file.seek(SeekFrom::Start(0)).ok()?;
    let (moov_start, moov_size) = loop {
        let (start, kind, size) = box_header(file)?;
        if &kind == b"moov" {
            break (start, size);
        }
        file.seek(SeekFrom::Start(start + size)).ok()?;
    };
    loop {
        let (start, kind, size) = box_header(file)?;
        if start >= moov_start + moov_size {
            return None;
        }
        if &kind == b"mvhd" {
            let mut version_flags = [0u8; 4];
            file.read_exact(&mut version_flags).ok()?;
            let (timescale, duration) = if version_flags[0] == 1 {
                // creation/modification time are 64-bit in version 1
                let mut b = [0u8; 28];
                file.read_exact(&mut b).ok()?;
                (
                    u32::from_be_bytes(b[16..20].try_into().unwrap()),
                    u64::from_be_bytes(b[20..28].try_into().unwrap()),
                )
            } else {
                let mut b = [0u8; 16];
                file.read_exact(&mut b).ok()?;
                (
                    u32::from_be_bytes(b[8..12].try_into().unwrap()),
                    u32::from_be_bytes(b[12..16].try_into().unwrap()) as u64,
                )
            };
            if timescale == 0 || duration == 0 {
                return None;
            }
            return Some(duration as f64 / timescale as f64);
        }
        file.seek(SeekFrom::Start(start + size)).ok()?;
    }
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_MPMoviePlayerPlaybackDidFinishNotification",
        HostConstant::NSString(MPMoviePlayerPlaybackDidFinishNotification),
    ),
    (
        "_MPMoviePlayerContentPreloadDidFinishNotification",
        HostConstant::NSString(MPMoviePlayerContentPreloadDidFinishNotification),
    ),
    (
        "_MPMoviePlayerScalingModeDidChangeNotification",
        HostConstant::NSString(MPMoviePlayerScalingModeDidChangeNotification),
    ),
    (
        "_MPMoviePlayerDidExitFullscreenNotification",
        HostConstant::NSString(MPMoviePlayerDidExitFullscreenNotification),
    ),
    (
        "_MPMoviePlayerPlaybackDidFinishReasonUserInfoKey",
        HostConstant::NSString(MPMoviePlayerPlaybackDidFinishReasonUserInfoKey),
    ),
];

struct MPMoviePlayerControllerHostObject {
    // NSURL *
    content_url: id,
    source_type: NSInteger,
    playback_rate: f32,
    repeat_mode: NSInteger,
    view: id,
}
impl HostObject for MPMoviePlayerControllerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation MPMoviePlayerController: NSObject

// TODO: actual playback

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(MPMoviePlayerControllerHostObject {
        content_url: nil,
        source_type: 0,
        playback_rate: 1.0,
        repeat_mode: 0,
        view: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithContentURL:(id)url { // NSURL*
    log!(
        "TODO: [(MPMoviePlayerController*){:?} initWithContentURL:{:?} ({:?})]",
        this,
        url,
        ns_url::to_rust_path(env, url),
    );

    retain(env, url);
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).content_url = url;

    // Act as if loading immediately completed (Spore Origins waits for this).
    State::get(env).pending_notifications.push_back(
        (MPMoviePlayerContentPreloadDidFinishNotification, this, Instant::now())
    );
    State::get(env).pending_notifications.push_back(
        ("MPMoviePlayerLoadStateDidChangeNotification", this, Instant::now())
    );

    this
}

- (())dealloc {
    let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(this);
    let (url, view) = (host.content_url, host.view);
    release(env, url);
    release(env, view);

    env.objc.dealloc_object(this, &mut env.mem);
}

- (id)contentURL {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).content_url
}

- (NSInteger)movieSourceType {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).source_type
}

- (())setMovieSourceType:(NSInteger)source_type {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).source_type = source_type;
}

- (f32)currentPlaybackRate {
    env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).playback_rate
}

- (())setCurrentPlaybackRate:(f32)rate {
    if !rate.is_finite() || !(0.0..=16.0).contains(&rate) {
        return;
    }
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).playback_rate = rate;
    let Some(playback) = State::get(env).playback.as_mut().filter(|p| p.player == this) else {
        return;
    };
    playback.elapsed += playback.started.elapsed().mul_f32(playback.rate);
    playback.started = Instant::now();
    let was_paused = playback.rate == 0.0;
    playback.rate = rate;
    if let Some((source, _)) = playback.audio {
        let al = env.framework_state.audio_toolbox.make_al_context_current(&mut env.openal_manager);
        unsafe {
            if rate == 0.0 {
                al.SourcePause(source);
            } else {
                al.Sourcef(source, AL_PITCH, rate);
                if was_paused {
                    al.SourcePlay(source);
                }
            }
        }
    }
}

- (id)backgroundColor {
    msg_class![env; UIColor blackColor] // TODO
}
- (())setBackgroundColor:(id)color { // UIColor*
    todo_objc_setter!(this, color);
}

- (())setScalingMode:(MPMovieScalingMode)mode {
    todo_objc_setter!(this, mode);
}
- (())setUseApplicationAudioSession:(bool)use_session {
    todo_objc_setter!(this, use_session);
}
- (())setControlStyle:(MPMovieControlStyle)style {
    todo_objc_setter!(this, style);
}
- (())setFullscreen:(bool)fullsreen {
    todo_objc_setter!(this, fullsreen);
}
- (())setFullscreen:(bool)fullscreen animated:(bool)_animated {
    // Movies are always shown full screen.
    todo_objc_setter!(this, fullscreen);
}
- (bool)isFullscreen {
    true
}
- (())setRepeatMode:(NSInteger)mode {
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).repeat_mode = mode;
}
- (())setInitialPlaybackTime:(NSTimeInterval)initial_time {
    todo_objc_setter!(this, initial_time);
}

- (id)view {
    let existing = env.objc.borrow::<MPMoviePlayerControllerHostObject>(this).view;
    if existing != nil {
        return existing;
    }
    // Apps can add controls to this view (BioShock's main menu is built on top
    // of its looping background movie), so it has to accept touches. Unhandled
    // ones still reach the superview through the responder chain.
    let view: id = msg_class![env; UIView new];
    env.objc.borrow_mut::<MPMoviePlayerControllerHostObject>(this).view = view;
    view
}

- (MPMoviePlaybackState)playbackState {
    if State::get(env).active_player == Some(this) { 1 } else { MPMoviePlaybackStateStopped }
}

- (NSUInteger)loadState {
    // These players use local files: both playable and playthroughOK.
    1 | 2
}

- (bool)isPreparedToPlay {
    true
}

- (())prepareToPlay {
    // Local files are prepared when the player is initialized.
}

// Apparently an undocumented, private API, but Spore Origins uses it.
- (())setMovieControlMode:(NSInteger)_mode {
    // As this is undocumented and we don't have real video playback yet, let's
    // ignore it.
}

// Another undocumented one! But some apps may still use it :/
// https://stackoverflow.com/a/1390079/2241008
- (())setOrientation:(UIDeviceOrientation)_orientation animated:(bool)_animated {

}

// MPMediaPlayback implementation
- (())play {
    log!("[(MPMoviePlayerController*){:?} play]", this);
    if let Some(old) = env.framework_state.media_player.movie_player.active_player {
        let _: () = msg![env; old stop];
    }
    assert!(env.framework_state.media_player.movie_player.active_player.is_none());
    // Movie player is retained by the runtime until it is stopped
    retain(env, this);
    env.framework_state.media_player.movie_player.active_player = Some(this);

    if start_playback(env, this) {
        return;
    }

    // If the movie can't be played, act as if playback immediately completed
    // after 1 second (various apps wait for this, such as BIA and Hero of
    // Sparta).
    let notif = (MPMoviePlayerPlaybackDidFinishNotification, this, Instant::now().checked_add(Duration::from_millis(1000)).unwrap());
    for (name, obj, _) in &mut State::get(env).pending_notifications {
        // De-duplicate similar notifications. This can happen if app is calling
        // `play` twice on the same player object (case of NOVA2).
        if *name == MPMoviePlayerPlaybackDidFinishNotification && *obj == this {
            return;
        }
    }
    State::get(env).pending_notifications.push_back(notif);
}

- (())pause {
    log!("TODO: [(MPMoviePlayerController*){:?} pause]", this);
}

- (())stop {
    log!("[(MPMoviePlayerController*){:?} stop]", this);
    if State::get(env).playback.as_ref().is_some_and(|p| p.player == this) {
        let playback = State::get(env).playback.take().unwrap();
        finish_playback(env, playback, /* notify: */ false);
    }
    // Some applications (like NOVA2) may send 2 `stop` messages for each
    // 1 `play` message for the player. In that case, we want to release
    // the active player only once. BioShock also stops a new player before
    // playing it, which must leave the player that's still active alone.
    if env.framework_state.media_player.movie_player.active_player == Some(this) {
        env.framework_state.media_player.movie_player.active_player = None;
        release(env, this);
    }
}

@end

// The full-screen view shown while a movie plays, see [Playback].
@implementation _Podroid_MoviePlayerView: UIView

- (())touchesEnded:(id)_touches withEvent:(id)_event {
    log!("Movie skipped by tapping");
    if let Some(playback) = State::get(env).playback.take() {
        finish_playback(env, playback, /* notify: */ true);
    }
}

@end

@implementation MPMoviePlayerViewController: UIViewController

// The movie itself is shown by the MPMoviePlayerController (see
// [start_playback]), so this view controller's view is just an empty view the
// app can add and remove.

- (id)initWithContentURL:(id)url {
    let this: id = msg_super![env; this initWithNibName:nil bundle:nil];
    let player: id = msg_class![env; MPMoviePlayerController alloc];
    let player: id = msg![env; player initWithContentURL:url];
    State::get(env).view_controller_players.insert(this, player);
    this
}

- (())dealloc {
    if let Some(player) = State::get(env).view_controller_players.remove(&this) {
        release(env, player);
    }
    msg_super![env; this dealloc]
}

- (id)moviePlayer {
    State::get(env)
        .view_controller_players
        .get(&this)
        .copied()
        .unwrap_or(nil)
}

- (())loadView {
    let view: id = msg_class![env; UIView alloc];
    let screen: id = msg_class![env; UIScreen mainScreen];
    let frame: CGRect = msg![env; screen bounds];
    let view: id = msg![env; view initWithFrame:frame];
    () = msg![env; this setView:view];
    release(env, view);
}

@end

};

/// Start playing the movie of `player`. Returns `false` if it can't be played.
fn start_playback(env: &mut Environment, player: id) -> bool {
    let host = env.objc.borrow::<MPMoviePlayerControllerHostObject>(player);
    let (url, rate, player_view) = (host.content_url, host.playback_rate, host.view);
    if url == nil {
        return false;
    }
    let path = ns_url::to_rust_path(env, url);
    let Ok(bytes) = env.fs.read(&path) else {
        log!("Warning: couldn't read movie {:?}, not playing it", path);
        return false;
    };
    let (decoder, audio) = match MovieDecoder::new(bytes) {
        Ok(movie) => movie,
        Err(e) => {
            log!("Warning: can't play movie {:?}: {}", path, e);
            return false;
        }
    };
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    let mut window: id = msg![env; ui_application keyWindow];
    if window == nil {
        // Some apps (e.g. Dead Rising) never make their window key, which
        // doesn't seem to matter on iPhone OS. Use the top visible window
        // then, so the movie isn't hidden by it. (The list is ordered
        // back-to-front.)
        let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
        window = windows
            .into_iter()
            .rev()
            .find(|&window| !msg![env; window isHidden])
            .unwrap_or(nil);
    }
    if window == nil {
        log!("Warning: no key window to show movie {:?} in", path);
        return false;
    }
    log!("Playing movie {:?}", path);

    let parent: id = msg![env; player_view superview];
    let embedded = parent != nil;
    let (view, bounds) = if embedded {
        retain(env, player_view);
        let bounds: CGRect = msg![env; player_view bounds];
        (player_view, bounds)
    } else {
        let bounds: CGRect = msg![env; window bounds];
        let view: id = msg_class![env; _Podroid_MoviePlayerView alloc];
        let view: id = msg![env; view initWithFrame:bounds];
        let black: id = msg_class![env; UIColor blackColor];
        () = msg![env; view setBackgroundColor:black];
        (view, bounds)
    };

    // Movies are shown in landscape, in the same orientation as the app if
    // it's a landscape app.
    let orientation: NSInteger = msg![env; ui_application statusBarOrientation];
    let landscape_view = embedded && bounds.size.width >= bounds.size.height;
    let angle: CGFloat = match orientation {
        _ if landscape_view => 0.0,
        // These are interface orientations, whose values are swapped
        // compared to the device orientation constants for landscape.
        UIDeviceOrientationLandscapeLeft => std::f32::consts::FRAC_PI_2,
        UIDeviceOrientationLandscapeRight => -std::f32::consts::FRAC_PI_2,
        UIDeviceOrientationPortraitUpsideDown => -std::f32::consts::FRAC_PI_2,
        _ => std::f32::consts::FRAC_PI_2,
    };
    let (screen_width, screen_height) = if landscape_view {
        (bounds.size.width, bounds.size.height)
    } else {
        (bounds.size.height, bounds.size.width)
    };
    let (video_width, video_height) = decoder.dimensions();
    let scale =
        (screen_width / video_width as CGFloat).min(screen_height / video_height as CGFloat);
    let video_view: id = msg_class![env; UIView alloc];
    let video_view: id = msg![env; video_view initWithFrame:(CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: video_width as CGFloat * scale,
            height: video_height as CGFloat * scale,
        },
    })];
    () = msg![env; video_view setUserInteractionEnabled:false];
    () = msg![env; video_view setCenter:(CGPoint {
        x: bounds.origin.x + bounds.size.width / 2.0,
        y: bounds.origin.y + bounds.size.height / 2.0,
    })];
    () = msg![env; video_view setTransform:(CGAffineTransform::make_rotation(angle))];
    // Subtitles, at the bottom of the video.
    let languages = crate::window::get_preferred_language_codes(env);
    let subtitles = decoder
        .subtitles_for(&languages)
        .map(<[Subtitle]>::to_vec)
        .unwrap_or_default();
    let subtitle_label: id = msg_class![env; UILabel alloc];
    let subtitle_label: id = msg![env; subtitle_label init];
    () = msg![env; subtitle_label setNumberOfLines:(0 as NSInteger)];
    let font: id = msg_class![env; UIFont systemFontOfSize:SUBTITLE_FONT_SIZE];
    () = msg![env; subtitle_label setFont:font];
    let white: id = msg_class![env; UIColor whiteColor];
    () = msg![env; subtitle_label setTextColor:white];
    let shade: id =
        msg_class![env; UIColor colorWithWhite:(0.0 as CGFloat) alpha:(0.55 as CGFloat)];
    () = msg![env; subtitle_label setBackgroundColor:shade];
    () = msg![env; subtitle_label setTextAlignment:UITextAlignmentCenter];
    () = msg![env; subtitle_label setHidden:true];
    () = msg![env; video_view addSubview:subtitle_label];

    () = msg![env; view addSubview:video_view];
    if !embedded {
        () = msg![env; window addSubview:view];
    }

    let audio = audio.map(|audio| {
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
            if rate > 0.0 {
                al.Sourcef(source, AL_PITCH, rate);
                al.SourcePlay(source);
            }
        }
        (source, buffer)
    });

    State::get(env).playback = Some(Playback {
        player,
        decoder,
        view,
        embedded,
        video_view,
        audio,
        started: Instant::now(),
        elapsed: Duration::ZERO,
        rate,
        subtitles,
        subtitle_label,
        current_subtitle: None,
    });
    true
}

/// Show the subtitle for `time`, if any.
fn update_subtitle(env: &mut Environment, playback: &mut Playback, time: Duration) {
    let index = playback
        .subtitles
        .iter()
        .position(|subtitle| subtitle.start <= time && time < subtitle.end);
    if index == playback.current_subtitle {
        return;
    }
    playback.current_subtitle = index;
    let label = playback.subtitle_label;
    let Some(index) = index else {
        () = msg![env; label setHidden:true];
        return;
    };

    let text = ns_string::from_rust_string(env, playback.subtitles[index].text.clone());
    () = msg![env; label setText:text];
    let font: id = msg![env; label font];
    let bounds: CGRect = msg![env; (playback.video_view) bounds];
    let max_width = bounds.size.width - 24.0;
    let size: CGSize = msg![env; text sizeWithFont:font
                                 constrainedToSize:(CGSize { width: max_width, height: bounds.size.height })
                                     lineBreakMode:UILineBreakModeWordWrap];
    release(env, text);
    let (width, height) = (size.width.ceil() + 12.0, size.height.ceil() + 6.0);
    () = msg![env; label setFrame:(CGRect {
        origin: CGPoint {
            x: (bounds.size.width - width) / 2.0,
            y: bounds.size.height - height - 10.0,
        },
        size: CGSize { width, height },
    })];
    () = msg![env; label setHidden:false];
}

/// Show the movie's current frame, and finish playback if it's over.
fn update_playback(env: &mut Environment) {
    let Some(mut playback) = State::get(env).playback.take() else {
        return;
    };
    if playback.rate == 0.0 {
        State::get(env).playback = Some(playback);
        return;
    }

    // The audio is the clock if there is any, so that the video stays in
    // sync with it, including when touchHLE is paused.
    let audio_clock = playback.audio.and_then(|(source, _)| {
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
        // If the audio output doesn't work (e.g. a Bluetooth speaker that
        // isn't playing), the audio never starts, so it can't be the clock.
        if seconds <= 0.0
            && state != AL_STOPPED
            && playback.started.elapsed() > Duration::from_secs(1)
        {
            return None;
        }
        Some((
            Duration::from_secs_f32(seconds.max(0.0)),
            state == AL_STOPPED,
        ))
    });
    let audio_clock_works = audio_clock.is_some();
    let (time, audio_finished) = audio_clock.unwrap_or_else(|| {
        (
            playback.elapsed + playback.started.elapsed().mul_f32(playback.rate),
            false,
        )
    });

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
    update_subtitle(env, &mut playback, time);

    if let Some(frame) = latest_frame {
        let image = Image::from_pixel_vec(frame.pixels, (frame.width, frame.height));
        let image = cg_image::from_image(env, image);
        let layer: id = msg![env; (playback.video_view) layer];
        () = msg![env; layer setContents:image];
        release(env, image);
    }

    if audio_finished || (video_finished && !audio_clock_works) {
        let repeats = env
            .objc
            .borrow::<MPMoviePlayerControllerHostObject>(playback.player)
            .repeat_mode
            == 1;
        if repeats {
            match playback.decoder.rewind() {
                Ok(()) => {
                    playback.started = Instant::now();
                    playback.elapsed = Duration::ZERO;
                    if let Some((source, _)) = playback.audio {
                        let al = env
                            .framework_state
                            .audio_toolbox
                            .make_al_context_current(&mut env.openal_manager);
                        unsafe {
                            al.SourceRewind(source);
                            al.SourcePlay(source);
                        }
                    }
                    State::get(env).playback = Some(playback);
                    return;
                }
                Err(error) => log!("Couldn't rewind movie: {error}"),
            }
        }
        finish_playback(env, playback, /* notify: */ true);
    } else {
        State::get(env).playback = Some(playback);
    }
}

/// Stop showing and playing the movie, and tell the app if `notify` is set.
fn finish_playback(env: &mut Environment, playback: Playback, notify: bool) {
    log!("Movie playback finished");
    let view = if playback.embedded {
        playback.video_view
    } else {
        playback.view
    };
    () = msg![env; view removeFromSuperview];
    release(env, playback.subtitle_label);
    release(env, playback.video_view);
    release(env, playback.view);
    if let Some((source, buffer)) = playback.audio {
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
    if notify {
        State::get(env).pending_notifications.push_back((
            MPMoviePlayerPlaybackDidFinishNotification,
            playback.player,
            Instant::now(),
        ));
    }
}

/// For use by `NSRunLoop` via [super::handle_players]: check movie players'
/// status, send notifications if necessary.
pub(super) fn handle_players(env: &mut Environment) {
    update_playback(env);

    let mut notifs_to_run = Vec::new();
    let pending_notifs = &mut State::get(env).pending_notifications;
    let mut i = 0;
    while i < pending_notifs.len() {
        let (name_str, object, time) = pending_notifs[i];
        if Instant::now() >= time {
            notifs_to_run.push((name_str, object));
            pending_notifs.swap_remove_back(i);
        } else {
            i += 1;
        }
    }
    for (name_str, object) in notifs_to_run {
        let name = ns_string::get_static_str(env, name_str);
        let center: id = msg_class![env; NSNotificationCenter defaultCenter];
        // TODO: should there be some user info attached?
        let _: () = msg![env; center postNotificationName:name object:object];
    }
}
