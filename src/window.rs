/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Abstraction of window setup, OpenGL context creation and event handling.
//!
//! Implemented using the sdl2 crate (a Rust wrapper for SDL2). All usage of
//! SDL should be confined to this module.
//!
//! There is currently no separation of concerns between a single window and
//! window system interaction in general, because it is assumed only one window
//! will be needed for the runtime of the app.

use crate::gles::present::present_frame;
use crate::gles::{create_gles1_ctx_no_parent_stack, GLESContext, GLES};
use crate::image::Image;
use crate::matrix::Matrix;
use crate::options::Options;
use crate::Environment;
use sdl2::mouse::MouseButton;
use sdl2::pixels::PixelFormatEnum;
use sdl2::surface::Surface;
use sdl2_sys::SDL_PowerState;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::f32::consts::{FRAC_PI_2, PI};
use std::num::NonZeroU32;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DeviceFamily {
    iPhone,
    iPad,
}
impl std::fmt::Display for DeviceFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
impl DeviceFamily {
    pub fn portrait_size(&self) -> (u32, u32) {
        match self {
            DeviceFamily::iPhone => (320, 480),
            DeviceFamily::iPad => (768, 1024),
        }
    }
}
impl TryFrom<u64> for DeviceFamily {
    type Error = ();
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(DeviceFamily::iPhone),
            2 => Ok(DeviceFamily::iPad),
            _ => Err(()),
        }
    }
}
impl TryFrom<&str> for DeviceFamily {
    type Error = ();
    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "iphone" => Ok(DeviceFamily::iPhone),
            "ipad" => Ok(DeviceFamily::iPad),
            _ => Err(()),
        }
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum DeviceOrientation {
    Portrait,
    PortraitUpsideDown,
    LandscapeLeft,
    LandscapeRight,
}
fn size_for_orientation(
    portrait_size: (u32, u32),
    orientation: DeviceOrientation,
    scale_hack: NonZeroU32,
) -> (u32, u32) {
    let (width, height) = portrait_size;
    let scale_hack = scale_hack.get();
    match orientation {
        DeviceOrientation::Portrait => (width * scale_hack, height * scale_hack),
        DeviceOrientation::PortraitUpsideDown => (width * scale_hack, height * scale_hack),
        DeviceOrientation::LandscapeLeft => (height * scale_hack, width * scale_hack),
        DeviceOrientation::LandscapeRight => (height * scale_hack, width * scale_hack),
    }
}
fn transform_screen_coords(
    (in_x, in_y): Coords,
    (vx, vy, vw, vh): (i32, i32, u32, u32),
    output_size: (u32, u32),
    rotation: Matrix<2>,
) -> Coords {
    let x = (in_x - vx as f32) / vw as f32 - 0.5;
    let y = (in_y - vy as f32) / vh as f32 - 0.5;
    let [x, y] = rotation.inverse().unwrap().transform([x, y]);
    let (width, height) = output_size;
    let coords = ((x + 0.5) * width as f32, (y + 0.5) * height as f32);
    snap_to_screen_edge((coords.0.round(), coords.1.round()), output_size)
}

/// Touches just outside the app's screen, e.g. at the edge of the black bars
/// around it on a phone with a wider aspect ratio, are moved onto the edge of
/// the screen, so that buttons at the edge are easier to hit. Touches further
/// away are left alone.
fn snap_to_screen_edge((x, y): Coords, (width, height): (u32, u32)) -> Coords {
    /// In points (the app's co-ordinates), so about 5% of the iPhone screen's
    /// width.
    const MARGIN: f32 = 16.0;
    fn snap(v: f32, size: u32) -> f32 {
        let max = size as f32 - 1.0;
        if (-MARGIN..0.0).contains(&v) {
            0.0
        } else if v > max && v <= max + MARGIN {
            max
        } else {
            v
        }
    }
    (snap(x, width), snap(y, height))
}

#[cfg(test)]
#[test]
fn test_snap_to_screen_edge() {
    let size = (320, 480);
    // Inside: unchanged.
    assert_eq!(snap_to_screen_edge((0.0, 479.0), size), (0.0, 479.0));
    assert_eq!(snap_to_screen_edge((100.0, 200.0), size), (100.0, 200.0));
    // Just outside: moved onto the edge.
    assert_eq!(snap_to_screen_edge((-10.0, 200.0), size), (0.0, 200.0));
    assert_eq!(snap_to_screen_edge((325.0, 490.0), size), (319.0, 479.0));
    // Far outside: unchanged.
    assert_eq!(snap_to_screen_edge((-100.0, 600.0), size), (-100.0, 600.0));
}

fn rotate_fullscreen_size(orientation: DeviceOrientation, screen_size: (u32, u32)) -> (u32, u32) {
    let (short_side, long_side) = if screen_size.0 < screen_size.1 {
        (screen_size.0, screen_size.1)
    } else {
        (screen_size.1, screen_size.0)
    };
    match orientation {
        DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => {
            (short_side, long_side)
        }
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
            (long_side, short_side)
        }
    }
}
/// Where to draw an app screen of size `app` (`base` is the size without the
/// scale hack) in a fullscreen window of size `screen`. See [Window::viewport].
fn compute_viewport(
    mode: crate::options::AspectMode,
    app: (u32, u32),
    base: (u32, u32),
    screen: (u32, u32),
) -> (i32, i32, u32, u32) {
    use crate::options::AspectMode;
    let (app_width, app_height) = app;
    let (base_width, base_height) = base;
    let (screen_width, screen_height) = screen;
    let app_aspect = app_width as f32 / app_height as f32;
    let screen_aspect = screen_width as f32 / screen_height as f32;
    let fit = |cover: bool| {
        if (app_aspect < screen_aspect) != cover {
            (
                (screen_height as f32 * app_aspect).round() as u32,
                screen_height,
            )
        } else {
            (
                screen_width,
                (screen_width as f32 / app_aspect).round() as u32,
            )
        }
    };
    let (scaled_width, scaled_height) = match mode {
        AspectMode::Fit => fit(false),
        AspectMode::Fill => fit(true),
        AspectMode::Stretch => (screen_width, screen_height),
        AspectMode::Integer => {
            let factor = (screen_width / base_width)
                .min(screen_height / base_height)
                .max(1);
            (base_width * factor, base_height * factor)
        }
    };
    let x = (screen_width as i32 - scaled_width as i32) / 2;
    let y = (screen_height as i32 - scaled_height as i32) / 2;
    (x, y, scaled_width, scaled_height)
}

#[cfg(test)]
#[test]
fn test_compute_viewport() {
    use crate::options::AspectMode::*;
    let landscape = ((480, 320), (480, 320));
    // 20:9 phone in landscape (like a 2400x1080 screen)
    let phone = (2400, 1080);
    assert_eq!(
        compute_viewport(Fit, landscape.0, landscape.1, phone),
        (390, 0, 1620, 1080)
    );
    assert_eq!(
        compute_viewport(Stretch, landscape.0, landscape.1, phone),
        (0, 0, 2400, 1080)
    );
    // 2400 wide at 3:2 is 1600 tall: cropped top and bottom
    assert_eq!(
        compute_viewport(Fill, landscape.0, landscape.1, phone),
        (0, -260, 2400, 1600)
    );
    // 1080/320 = 3.375 -> 3x
    assert_eq!(
        compute_viewport(Integer, landscape.0, landscape.1, phone),
        (480, 60, 1440, 960)
    );
    // 4:3 tablet: bars top and bottom instead
    let tablet = (2048, 1536);
    assert_eq!(
        compute_viewport(Fit, landscape.0, landscape.1, tablet),
        (0, 85, 2048, 1365)
    );
    assert_eq!(
        compute_viewport(Fill, landscape.0, landscape.1, tablet),
        (-128, 0, 2304, 1536)
    );
    assert_eq!(
        compute_viewport(Integer, landscape.0, landscape.1, tablet),
        (64, 128, 1920, 1280)
    );
    // Scale hack: integer scaling is based on the unscaled size
    assert_eq!(
        compute_viewport(Integer, (960, 640), landscape.1, phone),
        (480, 60, 1440, 960)
    );
    // Screen smaller than the app: never scale below 1x
    assert_eq!(
        compute_viewport(Integer, landscape.0, landscape.1, (400, 300)),
        (-40, -10, 480, 320)
    );
}

/// Tell SDL2 what orientation we want. Only useful on Android.
///
/// Both orientations of the same kind (portrait or landscape) are allowed, so
/// the user can turn the device around 180°. The content is drawn relative to
/// the window, so it stays upright; see [Window::is_physically_flipped] for
/// how the accelerometer is corrected.
fn set_sdl2_orientation(orientation: DeviceOrientation) {
    // Despite the name, this hint works on Android too.
    sdl2::hint::set(
        "SDL_IOS_ORIENTATIONS",
        match orientation {
            DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => {
                "Portrait PortraitUpsideDown"
            }
            DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
                "LandscapeLeft LandscapeRight"
            }
        },
    );
}

/// Making a GL context current can fail on Android while touchHLE is going
/// into the background (the surface is destroyed by the OS), so don't panic.
fn make_current_or_warn(window: &sdl2::video::Window, gl_ctx: &GLContext) {
    if let Err(e) = window.gl_make_current(&gl_ctx.0) {
        log!("Warning: couldn't make OpenGL context current: {}", e);
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum FingerId {
    Mouse,
    Touch(i64),
    VirtualCursor,
    ButtonToTouch(crate::options::Button),
    StickToTouch,
    RightStickToTouch,
    DpadToTouch,
}
pub type Coords = (f32, f32);

struct DpadState {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    active: bool,
}

#[derive(Debug)]
pub enum TextInputEvent {
    Text(String),
    Backspace,
    Return,
}

#[derive(Debug)]
pub enum Event {
    /// User requested quit.
    Quit,
    /// OS has informed touchHLE it will soon become inactive.
    /// (iOS `applicationWillResignActive:`, Android `onPause()`)
    AppWillResignActive,
    /// OS has informed touchHLE it is active again after
    /// [Event::AppWillResignActive].
    /// (iOS `applicationDidBecomeActive:`, Android `onResume()`)
    AppDidBecomeActive,
    /// OS has informed touchHLE it will soon terminate.
    /// (iOS `applicationWillTerminate:`, Android `onDestroy()`)
    AppWillTerminate,
    TouchesDown(HashMap<FingerId, Coords>),
    TouchesMove(HashMap<FingerId, Coords>),
    TouchesUp(HashMap<FingerId, Coords>),
    /// User pressed F12, requesting that execution be paused and the debugger
    /// take over.
    EnterDebugger,
    /// Internal no-op event, used for events that are handled inside
    /// [Window::poll_for_events] and shouldn't be forwarded.
    Noop,
    TextInput(TextInputEvent),
}

pub enum BatteryState {
    Unknown,
    OnBattery,
    NoBattery,
    Charging,
    Full,
}

pub enum GLVersion {
    /// OpenGL ES 1.1
    GLES11,
    /// OpenGL ES 2.0
    GLES20,
    /// OpenGL ES 3.0
    GLES30,
    /// OpenGL 2.1 compatibility profile
    GL21Compat,
    /// OpenGL 3.3 Core profile
    GL33Core,
}

pub struct GLContext(sdl2::video::GLContext);

impl GLContext {
    pub unsafe fn make_current_in(&self, window: &sdl2::video::Window) {
        window
            .gl_make_current(&self.0)
            .expect("Could not activate offscreen context");
    }

    pub fn is_current(&self) -> bool {
        self.0.is_current()
    }
}

fn surface_from_image(image: &Image) -> Surface<'_> {
    let src_pixels = image.pixels();
    let (width, height) = image.dimensions();

    let mut surface = Surface::new(width, height, PixelFormatEnum::RGBA32).unwrap();
    let (width, height) = (width as usize, height as usize);
    let pitch = surface.pitch() as usize;
    surface.with_lock_mut(|dst_pixels| {
        for y in 0..height {
            for x in 0..width {
                for channel in 0..4 {
                    let src_idx = y * width * 4 + x * 4 + channel;
                    let dst_idx = y * pitch + x * 4 + channel;
                    dst_pixels[dst_idx] = src_pixels[src_idx];
                }
            }
        }
    });
    surface
}

/// Drop stale paused input, but retain controller releases for guest-held
/// touches.
fn discard_paused_input(queue: &mut VecDeque<Event>, count: usize) {
    let mut releases = HashMap::new();
    for event in queue.drain(..count) {
        if let Event::TouchesUp(points) = event {
            releases.extend(points.into_iter().filter(|(id, _)| {
                matches!(
                    id,
                    FingerId::VirtualCursor
                        | FingerId::ButtonToTouch(_)
                        | FingerId::StickToTouch
                        | FingerId::RightStickToTouch
                        | FingerId::DpadToTouch
                )
            }));
        }
    }
    if !releases.is_empty() {
        queue.push_front(Event::TouchesUp(releases));
    }
}

/// Current host controller state. Axes use SDL order and SDL Y direction
/// (down positive).
#[derive(Clone, Debug)]
pub struct ControllerSnapshot {
    pub instance_id: u32,
    pub name: String,
    /// LeftX, LeftY, RightX, RightY range from -1 to 1; triggers from 0 to 1.
    pub axes: [f32; 6],
    pub buttons: Vec<sdl2::controller::Button>,
}

impl ControllerSnapshot {
    fn with_focus(mut self, focused: bool) -> Self {
        if !focused {
            self.axes = [0.0; 6];
            self.buttons.clear();
        }
        self
    }
}

fn normalize_controller_axis(value: i16, deadzone: f32) -> f32 {
    let deadzone = if deadzone.is_finite() {
        deadzone.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if deadzone == 1.0 {
        return 0.0;
    }
    let value = (value as f32 / i16::MAX as f32).clamp(-1.0, 1.0);
    ((value.abs() - deadzone).max(0.0) / (1.0 - deadzone)).copysign(value)
}

/// Hysteresis prevents noisy triggers from chattering at the press threshold.
fn trigger_pressed(value: i16, held: bool) -> bool {
    value as f32 / i16::MAX as f32 >= if held { 0.4 } else { 0.5 }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum ControllerTouchPhase {
    Down,
    Move,
    Up,
}

/// Integrate a swipe in logical screen pixels, ending before reseeding so a
/// boundary never causes a backwards drag. State is independent of SDL.
fn step_controller_swipe(
    position: Option<Coords>,
    stick: Coords,
    region: (f32, f32, f32, f32),
    elapsed: Duration,
) -> (Option<Coords>, Vec<(ControllerTouchPhase, Coords)>) {
    use ControllerTouchPhase::{Down, Move, Up};
    if stick == (0.0, 0.0) {
        return (None, position.map(|p| vec![(Up, p)]).unwrap_or_default());
    }
    let (x, y, w, h) = region;
    let center = (x + w * 0.5, y + h * 0.5);
    let mut events = Vec::new();
    let start = position.unwrap_or_else(|| {
        events.push((Down, center));
        center
    });
    let distance = 160.0 * elapsed.min(Duration::from_millis(50)).as_secs_f32();
    let target = (start.0 + stick.0 * distance, start.1 + stick.1 * distance);
    let next = (target.0.clamp(x, x + w), target.1.clamp(y, y + h));
    events.push((Move, next));
    if target.0 <= x || target.0 >= x + w || target.1 <= y || target.1 >= y + h {
        events.push((Up, next));
        events.push((Down, center));
        (Some(center), events)
    } else {
        (Some(next), events)
    }
}

pub struct Window {
    _sdl_ctx: sdl2::Sdl,
    video_ctx: sdl2::VideoSubsystem,
    window: sdl2::video::Window,
    event_pump: sdl2::EventPump,
    event_queue: VecDeque<Event>,
    last_polled: Instant,
    /// Separate queue for extremely high-priority events (e.g. app about to
    /// terminate).
    high_priority_event: Option<Event>,
    /// Whether the window is minimized, or (on Android) touchHLE is in the
    /// background. Nothing is drawn then, as the surface may be gone.
    minimized: bool,
    /// Set when the window's contents may have been lost or need redrawing
    /// (e.g. it was exposed or resized), see [Window::take_needs_redraw].
    needs_redraw: std::cell::Cell<bool>,
    enable_event_polling: bool,
    #[cfg(target_os = "macos")]
    max_height: u32,
    #[cfg(target_os = "macos")]
    viewport_y_offset: u32,
    /// Copy of `fullscreen` on [Options]. Note that this is meaningless when
    /// [Self::rotatable_fullscreen] returns [true].
    fullscreen: bool,
    scale_hack: NonZeroU32,
    internal_gl_ins: Option<Box<dyn GLESContext>>,
    screenshot_requested: std::cell::Cell<bool>,
    splash_image: Option<Image>,
    device_family: DeviceFamily,
    portrait_size: (u32, u32),
    p2g_wide_width: Option<u32>,
    touch_map_size: Option<(u32, u32)>,
    device_orientation: DeviceOrientation,
    controller_ctx: sdl2::GameControllerSubsystem,
    controllers: Vec<sdl2::controller::GameController>,
    dpad_state: DpadState,
    stick_active: bool,
    right_stick_active: bool,
    right_swipe_position: Option<Coords>,
    native_controller_input: bool,
    controller_input_focused: bool,
    controller_touches: HashMap<FingerId, Coords>,
    held_buttons: std::collections::HashSet<(u32, crate::options::Button)>,
    _sensor_ctx: sdl2::SensorSubsystem,
    accelerometer: Option<sdl2::sensor::Sensor>,
    virtual_cursor_last: Option<(f32, f32, bool, bool)>,
    virtual_cursor_last_unsticky: Option<(f32, f32, Instant)>,
    virtual_accelerometer_last: Option<(f32, f32, bool)>,
    /// Whether or not we are on the "main" environment stack (rather than
    /// a coroutine stack). Checked in various functions to make sure that
    /// certain SDL functions (that call JNI functions) are on the main
    /// stack on Android.
    pub(super) on_main_stack: bool,
    aspect_mode: crate::options::AspectMode,
    filter_mode: crate::options::FilterMode,
    keep_screen_on: bool,
}

impl Window {
    /// Disable synthetic controller input while preserving SDL polling and
    /// lifecycle events.
    pub fn set_native_controller_input(&mut self, enabled: bool) {
        if self.native_controller_input != enabled {
            self.release_controller_touches();
            self.native_controller_input = enabled;
        }
    }

    fn release_controller_touches(&mut self) {
        if !self.controller_touches.is_empty() {
            self.event_queue.push_back(Event::TouchesUp(std::mem::take(
                &mut self.controller_touches,
            )));
        }
        self.held_buttons.clear();
        self.stick_active = false;
        self.right_stick_active = false;
        self.right_swipe_position = None;
        self.dpad_state = DpadState {
            left: false,
            right: false,
            up: false,
            down: false,
            active: false,
        };
        self.virtual_cursor_last = None;
        self.virtual_cursor_last_unsticky = None;
    }

    fn track_controller_touch(&mut self) {
        match self.event_queue.back() {
            Some(Event::TouchesDown(points) | Event::TouchesMove(points)) => self
                .controller_touches
                .extend(points.iter().map(|(&id, &coords)| (id, coords))),
            Some(Event::TouchesUp(points)) => {
                for id in points.keys() {
                    self.controller_touches.remove(id);
                }
            }
            _ => {}
        }
    }

    /// Report connected host controllers, with neutral input while unfocused.
    /// Stick axes use the supplied deadzone; triggers retain their full range.
    pub fn controller_snapshots(&self, deadzone: f32) -> Vec<ControllerSnapshot> {
        use sdl2::controller::{Axis, Button};
        self.controllers
            .iter()
            .filter(|c| c.attached())
            .map(|c| ControllerSnapshot {
                instance_id: c.instance_id(),
                name: c.name(),
                axes: [
                    Axis::LeftX,
                    Axis::LeftY,
                    Axis::RightX,
                    Axis::RightY,
                    Axis::TriggerLeft,
                    Axis::TriggerRight,
                ]
                .map(|axis| {
                    let value = c.axis(axis);
                    if matches!(axis, Axis::TriggerLeft | Axis::TriggerRight) {
                        normalize_controller_axis(value.max(0), 0.0)
                    } else {
                        normalize_controller_axis(value, deadzone)
                    }
                }),
                buttons: [
                    Button::A,
                    Button::B,
                    Button::X,
                    Button::Y,
                    Button::Back,
                    Button::Guide,
                    Button::Start,
                    Button::LeftStick,
                    Button::RightStick,
                    Button::LeftShoulder,
                    Button::RightShoulder,
                    Button::DPadUp,
                    Button::DPadDown,
                    Button::DPadLeft,
                    Button::DPadRight,
                    Button::Misc1,
                    Button::Paddle1,
                    Button::Paddle2,
                    Button::Paddle3,
                    Button::Paddle4,
                    Button::Touchpad,
                ]
                .into_iter()
                .filter(|&b| c.button(b))
                .collect(),
            })
            .map(|snapshot| snapshot.with_focus(self.controller_input_focused))
            .collect()
    }

    /// Returns [true] if touchHLE is running on a device where we should always
    /// display fullscreen, but SDL2 will let us control the orientation, i.e.
    /// Android devices.
    pub fn rotatable_fullscreen() -> bool {
        env::consts::OS == "android"
    }
    pub fn new(
        title: &str,
        icon: Option<Image>,
        launch_image: Option<Image>,
        options: &mut Options,
        support: Option<&crate::device_profiles::DeviceSupport>,
    ) -> Window {
        if env::consts::OS == "android" {
            // Don't block the event loop while the app is paused (touchHLE
            // pauses the app itself, see Window::wait_until_active). SDL reads
            // this when the video subsystem is initialized, so it has to be
            // set before that.
            sdl2::hint::set("SDL_ANDROID_BLOCK_ON_PAUSE", "0");
        }

        let sdl_ctx = sdl2::init().unwrap();
        let video_ctx = sdl_ctx.video().unwrap();
        if let Some(support) = support {
            let host = video_ctx.display_bounds(0).ok().map(|bounds| {
                let size = bounds.size();
                // Only Android uses the physical size (see device_profiles),
                // and only there does SDL report the screen's own density.
                let inches = (env::consts::OS == "android")
                    .then(|| video_ctx.display_dpi(0).ok())
                    .flatten()
                    .filter(|&(diagonal_dpi, _, _)| diagonal_dpi > 0.0)
                    .map(|(diagonal_dpi, _, _)| {
                        (size.0 as f32).hypot(size.1 as f32) / diagonal_dpi
                    });
                crate::device_profiles::HostScreen { size, inches }
            });
            crate::device_profiles::apply(options, support, host);
        } else {
            // The app picker keeps its legacy geometry, without auto-selection.
            options.device_screen_size = Some(
                options
                    .device_family
                    .unwrap_or(DeviceFamily::iPhone)
                    .portrait_size(),
            );
        }

        // The "hidapi" feature of rust-sdl2 is enabled so that sdl2::sensor
        // is available, but we don't want to enable SDL's HIDAPI controller
        // drivers because they cause duplicated controllers on macOS
        // (https://github.com/libsdl-org/SDL/issues/7479). Once that's fixed,
        // remove this (https://github.com/touchHLE/touchHLE/issues/85).
        sdl2::hint::set("SDL_JOYSTICK_HIDAPI", "0");

        if env::consts::OS == "android" {
            // It's important to set context version BEFORE window creation
            // ref. https://wiki.libsdl.org/SDL2/SDL_GLattr
            let attr = video_ctx.gl_attr();
            attr.set_context_version(1, 1);
            attr.set_context_profile(sdl2::video::GLProfile::GLES);
        }

        // Separate mouse and touch events
        sdl2::hint::set("SDL_TOUCH_MOUSE_EVENTS", "0");

        // SDL2 disables the screen saver by default, but iPhone OS enables
        // the idle timer that triggers sleep by default, so we turn it back on
        // here, and then the app can disable it if it wants to.
        if options.keep_screen_on {
            video_ctx.disable_screen_saver();
        } else {
            video_ctx.enable_screen_saver();
        }

        let scale_hack = options.scale_hack;
        // TODO: some apps specify their orientation in Info.plist, we could use
        // that here.
        let device_family = options.device_family.unwrap_or(DeviceFamily::iPhone);
        let portrait_size = options
            .device_screen_size
            .unwrap_or(device_family.portrait_size());
        let p2g_wide_width = if options.p2g_wide_screen {
            let host = video_ctx.display_bounds(0).ok().map(|b| b.size());
            let width = host
                .map(|(w, h)| {
                    (320.0 * w.max(h) as f64 / w.min(h).max(1) as f64 / 2.0).round() as u32 * 2
                })
                .filter(|&w| w > 568)
                .unwrap_or(694)
                .clamp(568, 800);
            options.p2g_wide_width = Some(width);
            log!(
                "P2G expanded view: {}x320 points, centered 568x320 controls.",
                width
            );
            Some(width)
        } else {
            None
        };
        let display_size = p2g_wide_width.map(|w| (320, w)).unwrap_or(portrait_size);
        let device_orientation = options.initial_orientation;
        let fullscreen = options.fullscreen;

        let mut window = if Self::rotatable_fullscreen() {
            // Without this, SDL will force fullscreen mode to be portrait.
            set_sdl2_orientation(device_orientation);
            let screen_size = video_ctx.display_bounds(0).unwrap().size();
            let (width, height) = rotate_fullscreen_size(device_orientation, screen_size);
            // The window always fills the screen on Android, but it isn't
            // made an SDL "fullscreen" window: SDL leaves fullscreen when the
            // window is minimized (when going into the background), and on
            // Android that waits for the UI thread, which at that moment is
            // waiting for touchHLE to release the OpenGL ES surface. After
            // half a second both give up, the surface is destroyed while
            // still in use, and touchHLE crashed on returning. Instead,
            // MainActivity hides the system bars itself.
            let window = video_ctx
                .window(title, width, height)
                .opengl()
                .build()
                .unwrap();
            window
        } else if fullscreen {
            let (width, height) = video_ctx.display_bounds(0).unwrap().size();
            let window = video_ctx
                .window(title, width, height)
                .fullscreen_desktop()
                .opengl()
                .build()
                .unwrap();
            window
        } else {
            let (width, height) =
                size_for_orientation(display_size, device_orientation, scale_hack);
            let window = video_ctx
                .window(title, width, height)
                .position_centered()
                .opengl()
                .build()
                .unwrap();
            window
        };

        if env::consts::OS == "android" {
            // Sanity check
            let gl_attr = video_ctx.gl_attr();
            debug_assert_eq!(gl_attr.context_profile(), sdl2::video::GLProfile::GLES);
            debug_assert_eq!(gl_attr.context_version(), (1, 1));
        }

        if let Some(icon) = icon {
            window.set_icon(surface_from_image(&icon));
        }

        let event_pump = sdl_ctx.event_pump().unwrap();

        let controller_ctx = sdl_ctx.game_controller().unwrap();

        let sensor_ctx = sdl_ctx.sensor().unwrap();
        let mut accelerometer: Option<sdl2::sensor::Sensor> = None;
        if let Ok(num_sensors) = sensor_ctx.num_sensors() {
            for sensor_idx in 0..num_sensors {
                if let Ok(sensor) = sensor_ctx.open(sensor_idx) {
                    if sensor.sensor_type() == sdl2::sensor::SensorType::Accelerometer {
                        log!("Accelerometer detected: {}.", sensor.name());
                        accelerometer = Some(sensor);
                        break;
                    }
                }
            }
        }

        #[cfg(target_os = "macos")]
        let max_height = window.size().1;

        let mut window = Window {
            _sdl_ctx: sdl_ctx,
            video_ctx,
            window,
            event_pump,
            event_queue: VecDeque::new(),
            last_polled: Instant::now() - Duration::from_secs(1),
            high_priority_event: None,
            minimized: false,
            needs_redraw: std::cell::Cell::new(true),
            enable_event_polling: true,
            #[cfg(target_os = "macos")]
            max_height,
            #[cfg(target_os = "macos")]
            viewport_y_offset: 0,
            fullscreen,
            aspect_mode: options.aspect_mode,
            filter_mode: options.filter_mode,
            keep_screen_on: options.keep_screen_on,
            scale_hack,
            internal_gl_ins: None,
            screenshot_requested: std::cell::Cell::new(false),
            splash_image: launch_image,
            device_family,
            portrait_size,
            p2g_wide_width,
            touch_map_size: options.touch_map_size,
            device_orientation,
            controller_ctx,
            controllers: Vec::new(),
            dpad_state: DpadState {
                left: false,
                right: false,
                up: false,
                down: false,
                active: false,
            },
            stick_active: false,
            right_stick_active: false,
            right_swipe_position: None,
            native_controller_input: false,
            controller_input_focused: true,
            controller_touches: HashMap::new(),
            held_buttons: std::collections::HashSet::new(),
            _sensor_ctx: sensor_ctx,
            accelerometer,
            virtual_cursor_last: None,
            virtual_cursor_last_unsticky: None,
            virtual_accelerometer_last: None,
            on_main_stack: true,
        };

        // Include controllers connected before launch, before the first event
        // poll.
        // SDL provides automatic mappings for generic Android controllers.
        if let Ok(count) = window.controller_ctx.num_joysticks() {
            for index in 0..count {
                if window.controller_ctx.is_game_controller(index) {
                    window.controller_added(index);
                }
            }
        }

        // Set up OpenGL ES context used for splash screen and app UI rendering
        // (see src/frameworks/core_animation/composition.rs). OpenGL ES is used
        // because SDL2 won't let us use more than one graphics API in the same
        // window, and we also need OpenGL ES for the app's own rendering.
        let mut gl_ins = create_gles1_ctx_no_parent_stack(&mut window, options)
            .unwrap_or_else(|err| panic!("{}", err));
        {
            let gl_ctx = gl_ins.make_current(&mut window);
            log!("Driver info: {}", unsafe { gl_ctx.driver_description() });
        }
        window.internal_gl_ins = Some(gl_ins);

        if window.splash_image.is_some() {
            window.display_splash();
        }

        window
    }

    /// Poll for events from the OS. This needs to be done reasonably often
    /// (60Hz is probably fine) so that the host OS doesn't consider touchHLE
    /// to be unresponsive. Note that events are not returned by this function,
    /// since we often need to defer actually handling them.
    ///
    /// Since polling can be quite expensive, this function will skip it if it
    /// was called too recently.
    pub fn poll_for_events(&mut self, options: &Options) {
        assert!(self.on_main_stack);
        let now = Instant::now();
        // poll roughly twice per frame to try to avoid missing frames sometimes
        if now.duration_since(self.last_polled) < Duration::from_secs_f64(1.0 / 120.0) {
            return;
        }
        let controller_elapsed = now.saturating_duration_since(self.last_polled);
        self.last_polled = now;

        fn transform_input_coords(
            window: &Window,
            (in_x, in_y): (f32, f32),
            independent_of_viewport: bool,
        ) -> (f32, f32) {
            let (vx, vy, vw, vh) = if independent_of_viewport {
                let (width, height) = window.touch_map_size.unwrap_or_else(|| {
                    size_for_orientation(
                        window.device_family.portrait_size(),
                        window.device_orientation,
                        NonZeroU32::new(1).unwrap(),
                    )
                });
                (0, 0, width, height)
            } else {
                window.input_viewport()
            };
            transform_screen_coords(
                (in_x, in_y),
                (vx, vy, vw, vh),
                window.size_unrotated_unscaled(),
                window.rotation_matrix(),
            )
        }
        fn transform_virt_accel_coords(window: &Window, (in_x, in_y): (i32, i32)) -> (f32, f32) {
            let (_, _, vw, vh) = window.viewport();
            let out_x = ((in_x as f32 / vw as f32) * 2.0 - 1.0).clamp(-1.0, 1.0);
            let out_y = ((in_y as f32 / vh as f32) * 2.0 - 1.0).clamp(-1.0, 1.0);
            (out_x, out_y)
        }
        fn translate_button(button: sdl2::controller::Button) -> Option<crate::options::Button> {
            match button {
                sdl2::controller::Button::DPadLeft => Some(crate::options::Button::DPadLeft),
                sdl2::controller::Button::DPadUp => Some(crate::options::Button::DPadUp),
                sdl2::controller::Button::DPadRight => Some(crate::options::Button::DPadRight),
                sdl2::controller::Button::DPadDown => Some(crate::options::Button::DPadDown),
                sdl2::controller::Button::Start => Some(crate::options::Button::Start),
                sdl2::controller::Button::A => Some(crate::options::Button::A),
                sdl2::controller::Button::B => Some(crate::options::Button::B),
                sdl2::controller::Button::X => Some(crate::options::Button::X),
                sdl2::controller::Button::Y => Some(crate::options::Button::Y),
                sdl2::controller::Button::LeftShoulder => {
                    Some(crate::options::Button::LeftShoulder)
                }
                sdl2::controller::Button::RightShoulder => {
                    Some(crate::options::Button::RightShoulder)
                }
                sdl2::controller::Button::Back => Some(crate::options::Button::Back),
                sdl2::controller::Button::LeftStick => Some(crate::options::Button::LeftStick),
                sdl2::controller::Button::RightStick => Some(crate::options::Button::RightStick),
                _ => None,
            }
        }
        fn finger_absolute_coords(window: &Window, (x, y): (f32, f32)) -> (f32, f32) {
            let (screen_width, screen_height) = window.window.drawable_size();
            (screen_width as f32 * x, screen_height as f32 * y)
        }

        let mut controller_updated = false;
        // event_pump doesn't have a method to peek on events
        // so, we keep track of an unconsumed one from a previous loop iteration
        // FIXME: use peek_event() from even_subsystem
        let mut previous_event: Option<sdl2::event::Event> = None;
        while self.enable_event_polling {
            use sdl2::event::Event as E;
            let event = if let Some(e) = previous_event.take() {
                match e {
                    E::Unknown { .. } => (),
                    _ => log_dbg!("Consuming previous event: {:?}", e),
                }
                e
            } else if let Some(e) = self.event_pump.poll_event() {
                match e {
                    E::Unknown { .. } => (),
                    _ => log_dbg!("Consuming new event: {:?}", e),
                }
                e
            } else {
                break;
            };

            // Virtual accelerometer
            match event {
                E::MouseButtonDown {
                    x,
                    y,
                    mouse_btn: MouseButton::Right,
                    ..
                } => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, true));
                }
                E::MouseMotion {
                    x, y, mousestate, ..
                } if mousestate.right() => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, true));
                }
                E::MouseButtonUp {
                    x,
                    y,
                    mouse_btn: MouseButton::Right,
                    ..
                } => {
                    let (x, y) = transform_virt_accel_coords(self, (x, y));
                    self.virtual_accelerometer_last = Some((x, y, false));
                }
                _ => {}
            }

            let controller_event = matches!(
                event,
                E::ControllerButtonUp { .. }
                    | E::ControllerButtonDown { .. }
                    | E::ControllerAxisMotion { .. }
            );
            let translated_event = match event {
                E::Quit { .. } => Event::Quit,
                E::MouseButtonDown {
                    x,
                    y,
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    let coords = transform_input_coords(self, (x as f32, y as f32), false);
                    log_dbg!("MouseButtonDown x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesDown(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::MouseMotion {
                    x, y, mousestate, ..
                } if mousestate.left() => {
                    let coords = transform_input_coords(self, (x as f32, y as f32), false);
                    log_dbg!("MouseMotion x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesMove(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::MouseButtonUp {
                    x,
                    y,
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    let coords = transform_input_coords(self, (x as f32, y as f32), false);
                    log_dbg!("MouseButtonUp x {}, y {}, coords {:?}", x, y, coords);
                    Event::TouchesUp(HashMap::from([(FingerId::Mouse, coords)]))
                }
                E::ControllerDeviceAdded { which, .. } => {
                    self.controller_added(which);
                    continue;
                }
                E::ControllerDeviceRemoved { which, .. } => {
                    self.controller_removed(which);
                    continue;
                }
                // Note that accelerometer simulation with analog sticks is
                // handled with polling, rather than being event-based.
                E::ControllerButtonUp { button, which, .. }
                | E::ControllerButtonDown { button, which, .. } => {
                    controller_updated = true;
                    let Some(button) = translate_button(button) else {
                        continue;
                    };
                    if self.native_controller_input || !self.controller_input_focused {
                        continue;
                    }
                    let pressed = matches!(event, E::ControllerButtonDown { .. });
                    let was_pressed = self.held_buttons.iter().any(|&(_, b)| b == button);
                    if pressed {
                        self.held_buttons.insert((which, button));
                    } else {
                        self.held_buttons.remove(&(which, button));
                    }
                    let pressed = self.held_buttons.iter().any(|&(_, b)| b == button);
                    if pressed == was_pressed {
                        continue;
                    }
                    // Called whenever a DPad direction is pressed or released
                    if (button == crate::options::Button::DPadLeft
                        || button == crate::options::Button::DPadUp
                        || button == crate::options::Button::DPadRight
                        || button == crate::options::Button::DPadDown)
                        && options.dpad_to_touch.is_some()
                    {
                        let Some((x, y, w, h)) = options.dpad_to_touch else {
                            unreachable!();
                        };

                        // Update held state
                        match button {
                            crate::options::Button::DPadLeft => self.dpad_state.left = pressed,
                            crate::options::Button::DPadRight => self.dpad_state.right = pressed,
                            crate::options::Button::DPadUp => self.dpad_state.up = pressed,
                            crate::options::Button::DPadDown => self.dpad_state.down = pressed,
                            _ => unreachable!(),
                        }

                        // Compute center
                        let cx = x + w * 0.5;
                        let cy = y + h * 0.5;

                        // Compute combined delta
                        let mut dx = 0.0;
                        let mut dy = 0.0;

                        if self.dpad_state.left {
                            dx -= 0.5 * w;
                        }
                        if self.dpad_state.right {
                            dx += 0.5 * w;
                        }
                        if self.dpad_state.up {
                            dy -= 0.5 * h;
                        }
                        if self.dpad_state.down {
                            dy += 0.5 * h;
                        }

                        // Final coords: center + movement
                        let coords = transform_input_coords(self, (cx + dx, cy + dy), true);

                        // Send TouchDown if any dpad is held, TouchUp if none
                        let any_held = self.dpad_state.left
                            || self.dpad_state.right
                            || self.dpad_state.up
                            || self.dpad_state.down;

                        if !self.dpad_state.active && any_held {
                            // New touch
                            self.dpad_state.active = true;
                            Event::TouchesDown(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else if self.dpad_state.active && any_held {
                            // Move existing touch
                            Event::TouchesMove(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else if self.dpad_state.active && !any_held {
                            // Release touch
                            self.dpad_state.active = false;
                            Event::TouchesUp(HashMap::from([(FingerId::DpadToTouch, coords)]))
                        } else {
                            continue;
                        }
                    } else {
                        let Some(&(x, y)) = options.button_to_touch.get(&button) else {
                            continue;
                        };
                        let coords = transform_input_coords(self, (x, y), true);
                        let points = HashMap::from([(FingerId::ButtonToTouch(button), coords)]);
                        if pressed {
                            Event::TouchesDown(points)
                        } else {
                            Event::TouchesUp(points)
                        }
                    }
                }
                E::ControllerAxisMotion {
                    axis, value, which, ..
                } => {
                    controller_updated = true;
                    if self.native_controller_input || !self.controller_input_focused {
                        continue;
                    }
                    if matches!(
                        axis,
                        sdl2::controller::Axis::TriggerLeft | sdl2::controller::Axis::TriggerRight
                    ) {
                        let button = if axis == sdl2::controller::Axis::TriggerLeft {
                            crate::options::Button::LeftTrigger
                        } else {
                            crate::options::Button::RightTrigger
                        };
                        let held = self.held_buttons.contains(&(which, button));
                        let pressed = trigger_pressed(value, held);
                        if held == pressed {
                            continue;
                        }
                        let was_pressed = self.held_buttons.iter().any(|&(_, b)| b == button);
                        if pressed {
                            self.held_buttons.insert((which, button));
                        } else {
                            self.held_buttons.remove(&(which, button));
                        }
                        let pressed = self.held_buttons.iter().any(|&(_, b)| b == button);
                        if was_pressed == pressed {
                            continue;
                        }
                        let Some(&(x, y)) = options.button_to_touch.get(&button) else {
                            continue;
                        };
                        let coords = transform_input_coords(self, (x, y), true);
                        let points = HashMap::from([(FingerId::ButtonToTouch(button), coords)]);
                        if pressed {
                            Event::TouchesDown(points)
                        } else {
                            Event::TouchesUp(points)
                        }
                    } else {
                        let left = matches!(
                            axis,
                            sdl2::controller::Axis::LeftX | sdl2::controller::Axis::LeftY
                        );
                        let region = if left {
                            options.stick_to_touch
                        } else {
                            options.right_stick_to_touch
                        };
                        let finger = if left {
                            FingerId::StickToTouch
                        } else {
                            FingerId::RightStickToTouch
                        };
                        let mut active = if left {
                            self.stick_active
                        } else {
                            self.right_stick_active
                        };
                        let Some((x, y, w, h)) = region else {
                            continue;
                        };
                        if matches!(
                            axis,
                            sdl2::controller::Axis::LeftX
                                | sdl2::controller::Axis::LeftY
                                | sdl2::controller::Axis::RightX
                                | sdl2::controller::Axis::RightY
                        ) {
                            let (stick_x, stick_y, _) = self.get_controller_stick(options, left);
                            let coords = transform_input_coords(
                                self,
                                (
                                    x + ((stick_x + 1.0) / 2.0) * w,
                                    y + ((stick_y + 1.0) / 2.0) * h,
                                ),
                                true,
                            );
                            if stick_x == 0.0 && stick_y == 0.0 {
                                if !active {
                                    // Ignore deadzone events when stick is
                                    // inactive
                                    continue;
                                } else {
                                    // Release touch when stick returns to
                                    // deadzone
                                    active = false;
                                    if left {
                                        self.stick_active = active;
                                    } else {
                                        self.right_stick_active = active;
                                    }
                                    Event::TouchesUp(HashMap::from([(finger, coords)]))
                                }
                            } else if !active {
                                // New touch
                                active = true;
                                if left {
                                    self.stick_active = active;
                                } else {
                                    self.right_stick_active = active;
                                }
                                if left && options.stick_touch_start_at_center {
                                    let center = transform_input_coords(
                                        self,
                                        (x + w * 0.5, y + h * 0.5),
                                        true,
                                    );
                                    self.event_queue.push_back(Event::TouchesDown(HashMap::from(
                                        [(finger, center)],
                                    )));
                                    self.track_controller_touch();
                                    Event::TouchesMove(HashMap::from([(finger, coords)]))
                                } else {
                                    Event::TouchesDown(HashMap::from([(finger, coords)]))
                                }
                            } else {
                                // Move existing touch
                                if left {
                                    self.stick_active = active;
                                } else {
                                    self.right_stick_active = active;
                                }
                                Event::TouchesMove(HashMap::from([(finger, coords)]))
                            }
                        } else {
                            continue;
                        }
                    }
                }
                E::AppWillEnterBackground { .. } => {
                    self.release_controller_touches();
                    self.controller_input_focused = false;
                    log!("Received app-will-resign-active event.");
                    assert!(self.high_priority_event.is_none());
                    self.high_priority_event = Some(Event::AppWillResignActive);
                    // For some reason, if we don't pause event polling, we will
                    // never finish handling the event.
                    // TODO: Add a mechanism for re-enabling polling, if at some
                    // point we support returning touchHLE to the foreground.
                    self.enable_event_polling = false;
                    continue;
                }
                // SDL only sends a quit event when the last window is closed,
                // but ES 2.0 apps also have a hidden window used for rendering
                // (see create_offscreen_gl_context).
                E::Window {
                    win_event: sdl2::event::WindowEvent::Close,
                    window_id,
                    ..
                } if window_id == self.window.id() => Event::Quit,
                E::Window {
                    win_event,
                    window_id,
                    ..
                } => {
                    // Hidden ES2 windows must not change main-window state.
                    if window_id != self.window.id() {
                        continue;
                    }
                    use sdl2::event::WindowEvent as WE;
                    if matches!(win_event, WE::FocusLost | WE::Minimized | WE::Hidden) {
                        self.release_controller_touches();
                        self.controller_input_focused = false;
                    } else if matches!(win_event, WE::FocusGained) {
                        self.controller_input_focused = true;
                    }
                    match win_event {
                        WE::Minimized | WE::Hidden => self.minimized = true,
                        WE::Restored | WE::Shown | WE::Maximized => self.minimized = false,
                        _ => (),
                    }
                    if matches!(
                        win_event,
                        WE::Restored
                            | WE::Shown
                            | WE::Maximized
                            | WE::Exposed
                            | WE::Resized(..)
                            | WE::SizeChanged(..)
                            | WE::FocusGained
                    ) {
                        self.needs_redraw.set(true);
                    }
                    continue;
                }
                E::RenderDeviceReset { .. } => {
                    // On Android, SDL sends this if the OpenGL ES context
                    // couldn't be restored after touchHLE was in the
                    // background. touchHLE can't recover from that yet, but
                    // logging it makes such problems much easier to diagnose.
                    log!("Warning: the OpenGL ES context was lost (render device reset). Graphics may be broken from now on.");
                    continue;
                }
                E::AppDidEnterForeground { .. } => {
                    self.controller_input_focused = true;
                    log!("Received app-did-become-active event.");
                    Event::AppDidBecomeActive
                }
                E::AppTerminating { .. } => {
                    log!("Received app-will-terminate event.");
                    assert!(self.high_priority_event.is_none());
                    self.high_priority_event = Some(Event::AppWillTerminate);
                    self.enable_event_polling = false;
                    continue;
                }
                E::FingerUp {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                }
                | E::FingerMotion {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                }
                | E::FingerDown {
                    timestamp,
                    finger_id,
                    x,
                    y,
                    ..
                } => {
                    log_dbg!("Starting multi-touch for {:?}", event);
                    // To implement multi-touch we accumulate here same touch
                    // events at the same timestamp. This is consistent with
                    // UIKit, but could be broken if events come out of order.
                    // (in worst case we separate multi-touches in several ones)
                    // TODO: handle out of order touches
                    let curr_timestamp = timestamp;
                    let abs_coords = finger_absolute_coords(self, (x, y));
                    let coords = transform_input_coords(self, abs_coords, false);
                    log_dbg!("Finger event x {}, y {}, coords {:?}", x, y, coords);
                    let mut map = HashMap::from([(FingerId::Touch(finger_id), coords)]);
                    while let Some(next) = self.event_pump.poll_event() {
                        match next {
                            E::Unknown { .. } => (),
                            _ => log_dbg!("Next possible multi-touch event: {:?}", next),
                        }
                        match next {
                            E::FingerUp {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            }
                            | E::FingerMotion {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            }
                            | E::FingerDown {
                                timestamp,
                                finger_id,
                                x,
                                y,
                                ..
                            } if timestamp == curr_timestamp && next.is_same_kind_as(&event) => {
                                let abs_coords = finger_absolute_coords(self, (x, y));
                                let coords = transform_input_coords(self, abs_coords, false);
                                map.insert(FingerId::Touch(finger_id), coords);
                            }
                            E::MultiGesture { timestamp, .. } if timestamp == curr_timestamp => {
                                // TODO: handle gestures
                                continue;
                            }
                            _ => {
                                // event_pump doesn't have a method to peek on
                                // events, so we keep track of an unconsumed
                                // one from a previous loop iteration
                                assert!(previous_event.is_none());
                                previous_event = Some(next);
                                break;
                            }
                        }
                    }
                    log_dbg!("Finishing multi-touch for {:?} with {:?}", event, map);
                    match event {
                        E::FingerUp { .. } => Event::TouchesUp(map),
                        E::FingerMotion { .. } => Event::TouchesMove(map),
                        E::FingerDown { .. } => Event::TouchesDown(map),
                        _ => unreachable!(),
                    }
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F12),
                    ..
                } => {
                    // Log this so you can tell when touchHLE has received
                    // the event but it's stuck in the queue.
                    echo!("F12 pressed, EnterDebugger event queued.");
                    Event::EnterDebugger
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::F11),
                    ..
                } => {
                    self.screenshot_requested.set(true);
                    log!("F11 pressed, screenshot requested.");
                    Event::Noop
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Backspace),
                    ..
                } => {
                    log_dbg!("SDL TextInput Backspace");
                    Event::TextInput(TextInputEvent::Backspace)
                }
                E::KeyDown {
                    keycode: Some(sdl2::keyboard::Keycode::Return),
                    ..
                } => {
                    log_dbg!("SDL TextInput Return");
                    Event::TextInput(TextInputEvent::Return)
                }
                E::TextInput { text, .. } => {
                    log_dbg!("SDL TextInput {}", text);
                    Event::TextInput(TextInputEvent::Text(text))
                }
                _ => continue,
            };
            self.event_queue.push_back(translated_event);
            if controller_event {
                self.track_controller_touch();
            }
        }

        // Polling, rather than axis events, keeps the camera moving while held.
        if self.controller_input_focused && !self.native_controller_input {
            if let Some(region) = options.right_stick_swipe {
                let (x, y, _) = self.get_controller_stick(options, false);
                let (position, events) = step_controller_swipe(
                    self.right_swipe_position,
                    (x, y),
                    region,
                    controller_elapsed,
                );
                self.right_swipe_position = position;
                for (phase, coords) in events {
                    let coords = transform_input_coords(self, coords, true);
                    let points = HashMap::from([(FingerId::RightStickToTouch, coords)]);
                    let event = match phase {
                        ControllerTouchPhase::Down => Event::TouchesDown(points),
                        ControllerTouchPhase::Move => Event::TouchesMove(points),
                        ControllerTouchPhase::Up => Event::TouchesUp(points),
                    };
                    self.event_queue.push_back(event);
                    self.track_controller_touch();
                }
            }
        }

        if controller_updated
            && self.controller_input_focused
            && !self.native_controller_input
            && options.right_stick_to_touch.is_none()
            && options.right_stick_swipe.is_none()
            && !options
                .button_to_touch
                .contains_key(&crate::options::Button::RightStick)
            && !options
                .button_to_touch
                .contains_key(&crate::options::Button::RightShoulder)
        {
            let (new_x, new_y, pressed, pressed_changed, moved) =
                self.update_virtual_cursor(options);
            self.event_queue
                .push_back(match (pressed, pressed_changed, moved) {
                    (true, true, _) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesDown(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    (false, true, _) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesUp(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    (true, _, true) => {
                        let coords = transform_input_coords(self, (new_x, new_y), false);
                        Event::TouchesMove(HashMap::from([(FingerId::VirtualCursor, coords)]))
                    }
                    _ => return,
                });
            self.track_controller_touch();
        }
    }

    /// Pop an event from the queue (in FIFO order, except for high priority
    /// events)
    /// Called after [Event::AppWillResignActive] has been handled: blocks
    /// (so the app is frozen) until the OS brings touchHLE back to the
    /// foreground. Returns `false` if touchHLE should terminate instead; the
    /// corresponding event is left in the queue in that case.
    pub fn wait_until_active(&mut self, options: &Options) -> bool {
        self.enable_event_polling = true;
        loop {
            std::thread::sleep(Duration::from_millis(50));
            self.poll_for_events(options);
            // Another going-into-the-background event while already paused
            // (which shouldn't happen, but it would otherwise stop event
            // polling, so touchHLE would never notice it's back in the
            // foreground, or pause again right after resuming).
            if matches!(self.high_priority_event, Some(Event::AppWillResignActive)) {
                self.high_priority_event = None;
                self.enable_event_polling = true;
            }
            if matches!(self.high_priority_event, Some(Event::AppWillTerminate))
                || self.event_queue.iter().any(|e| matches!(e, Event::Quit))
            {
                return false;
            }
            if let Some(i) = self
                .event_queue
                .iter()
                .position(|e| matches!(e, Event::AppDidBecomeActive))
            {
                // Input from before the app was paused is stale.
                discard_paused_input(&mut self.event_queue, i + 1);
                return true;
            }
        }
    }

    pub fn pop_event(&mut self) -> Option<Event> {
        self.high_priority_event
            .take()
            .or_else(|| self.event_queue.pop_front())
    }

    fn controller_added(&mut self, joystick_idx: u32) {
        let Ok(controller) = self.controller_ctx.open(joystick_idx) else {
            log!("Warning: A new controller was connected, but it couldn't be accessed!");
            return;
        };

        // SDL also queues added events for controllers enumerated at startup.
        if self
            .controllers
            .iter()
            .any(|c| c.instance_id() == controller.instance_id())
        {
            return;
        }
        let controller_name = controller.name();
        if env::consts::OS == "android" && controller_name.starts_with("uinput-") {
            log!("ignoring fingerprint device: {}", controller_name);
            return;
        }
        log!(
            "New controller connected: {}. Left stick = device tilt. Right stick = touch input (press the stick or shoulder button to tap/hold).",
            controller_name
        );
        self.controllers.push(controller);
    }
    fn controller_removed(&mut self, instance_id: u32) {
        self.release_controller_touches();
        let Some(idx) = self
            .controllers
            .iter()
            .position(|controller| controller.instance_id() == instance_id)
        else {
            return;
        };
        let controller = self.controllers.remove(idx);
        log!("Warning: Controller disconnected: {}", controller.name());
    }
    pub fn print_accelerometer_notice(&self, options: &Options) {
        log!("This app uses the accelerometer.");

        if !self.controllers.is_empty() && options.analog_stick_tilt_controls {
            log!("Your connected controller's left analog stick will be used for accelerometer simulation.");
            if self.accelerometer.is_some() {
                log!("Disconnect the controller if you want to use your device's accelerometer.");
            }
        } else if self.accelerometer.is_some() {
            log!("Your device's accelerometer will be used for accelerometer simulation.");
            if options.analog_stick_tilt_controls {
                log!("Connect a controller if you would prefer to use an analog stick.");
            }
        } else if self.controllers.is_empty() && options.analog_stick_tilt_controls {
            log!("Connect a controller to get accelerometer simulation.");
        }

        if self.accelerometer.is_none() {
            log!(
                "You can {}hold right click and move the cursor to simulate the accelerometer.",
                if options.analog_stick_tilt_controls {
                    "also "
                } else {
                    ""
                }
            );
        }
    }

    /// Whether the real device's screen is rotated 180° from the emulated
    /// device's orientation. Only possible on Android, see
    /// [set_sdl2_orientation].
    fn is_physically_flipped(&self) -> bool {
        use sdl2::video::Orientation;
        if !Self::rotatable_fullscreen() {
            return false;
        }
        // The orientation SDL2 reports when the device is held the same way as
        // the emulated device. (SDL2's "landscape" has the right side up,
        // like UIDeviceOrientationLandscapeLeft.)
        let flipped = match self.device_orientation {
            DeviceOrientation::Portrait => Orientation::PortraitFlipped,
            DeviceOrientation::PortraitUpsideDown => Orientation::Portrait,
            DeviceOrientation::LandscapeLeft => Orientation::LandscapeFlipped,
            DeviceOrientation::LandscapeRight => Orientation::Landscape,
        };
        self.video_ctx.display_orientation(0) == flipped
    }

    /// Get the real or simulated accelerometer output.
    /// See also [crate::frameworks::uikit::ui_accelerometer].
    pub fn get_acceleration(&self, options: &Options) -> (f32, f32, f32) {
        if self.native_controller_input
            || !self.controller_input_focused
            || self.controllers.is_empty()
            || !options.analog_stick_tilt_controls
        {
            if let Some(ref accelerometer) = self.accelerometer {
                let data = accelerometer.get_data().unwrap();
                let sdl2::sensor::SensorData::Accel(data) = data else {
                    panic!();
                };
                let [x, y, z] = data;
                // UIAcceleration reports acceleration towards gravity, but SDL2
                // reports acceleration away from gravity.
                let (x, y, z) = (-x, -y, -z);
                // UIAcceleration reports acceleration in units of g-force, but
                // SDL2 reports acceleration in units of m/s^2.
                let gravity: f32 = 9.80665; // SDL_STANDARD_GRAVITY
                let (x, y, z) = (x / gravity, y / gravity, z / gravity);
                // If the real device is held the other way around from the
                // emulated device, the app would see tilting inverted.
                if self.is_physically_flipped() {
                    return (-x, -y, z);
                }
                return (x, y, z);
            }
        }

        let (x, y) = if self
            .virtual_accelerometer_last
            .is_some_and(|(_x, _y, right_click_hold)| right_click_hold)
        {
            self.virtual_accelerometer_last
                .map(|(x, y, _right_click_hold)| (x, y))
                .unwrap()
        } else if options.analog_stick_tilt_controls {
            // Get left analog stick input. The range is [-1, 1] on each axis.
            let (x, y, _) = self.get_controller_stick(options, true);
            (x, y)
        } else {
            (0.0, 0.0)
        };

        // Correct for window rotation
        let [x, y] = self.rotation_matrix().inverse().unwrap().transform([x, y]);
        let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)); // just in case

        // Let's simulate tilting the device based on the analog stick inputs.
        //
        // If an iPhone is lying flat on its back, level with the ground, and it
        // is on Earth, the accelerometer will report approximately (0, 0, -1).
        // The acceleration x and y axes are aligned with the screen's x and y
        // axes. +x points to the right of the screen, +y points to the top of
        // the screen, and +z points away from the screen. In the example
        // scenario, the z axis is parallel to gravity.

        let gravity: [f32; 3] = [0.0, 0.0, -1.0];

        let neutral_x = options.x_tilt_offset.to_radians();
        let neutral_y = options.y_tilt_offset.to_radians();
        let x_rotation_range = options.x_tilt_range.to_radians() / 2.0;
        let y_rotation_range = options.y_tilt_range.to_radians() / 2.0;
        // (x, y) are swapped because the controller Y axis usually corresponds
        // to forward/backward movement, but rotating about the Y axis means
        // tilting the device left/right.
        let x_rotation = neutral_x - x_rotation_range * y;
        let y_rotation = neutral_y - y_rotation_range * x;
        let matrix =
            Matrix::<3>::y_rotation(y_rotation).multiply(&Matrix::<3>::x_rotation(x_rotation));
        let [x, y, z] = matrix.transform(gravity);

        (x, y, z)
    }

    /// For use when redrawing the screen: Get the cached on-screen position and
    /// press state of the analog stick-controlled virtual cursor, if it is
    /// visible.
    pub fn virtual_cursor_visible_at(&self) -> Option<(f32, f32, bool)> {
        if self.native_controller_input || !self.controller_input_focused {
            return None;
        }
        let (x, y, pressed, visible) = self.virtual_cursor_last?;
        if visible {
            // When stickyness is in use, the visual cursor movement appears
            // uncomfortably choppy. Showing the un-sticky position is a bit
            // misleading but it *feels* better, and it is documented.
            if let Some((x_unsticky, y_unsticky, _time)) = self.virtual_cursor_last_unsticky {
                Some((x_unsticky, y_unsticky, pressed))
            } else {
                Some((x, y, pressed))
            }
        } else {
            None
        }
    }

    /// Update the virtual cursor's position, click state and visibility, then
    /// return the new position, pressed state, whether the press state changed
    /// and whether the cursor moved.
    fn update_virtual_cursor(&mut self, options: &Options) -> (f32, f32, bool, bool, bool) {
        // Get right analog stick input. The range is [-1, 1] on each axis.
        let (x, y, pressed) = self.get_controller_stick(options, false);

        // The cursor is intended to only show up once you move the analog stick
        // out of its deadzone, or while the button is held.
        let visible = pressed || x != 0.0 || y != 0.0;

        // Though the analog stick output fits within a square, its actual range
        // is usually a circle enclosed by the square. So we need to cut out the
        // rectangular shape of the screen from that circle within the square.
        let (vx, vy, vw, vh) = self.viewport();
        let (vx, vy, vw, vh) = (vx as f32, vy as f32, vw as f32, vh as f32);

        let (x, y) = {
            // Use Pythagoras's theorem to find the largest size the rectangle
            // can have within the circle.
            let ratio = vw / vh;
            let rect_height = (ratio * ratio + 1.0).powf(-0.5);
            let rect_width = ratio * rect_height;

            let x_abs = x.abs().min(rect_width) / rect_width;
            let y_abs = y.abs().min(rect_height) / rect_height;
            (x_abs.copysign(x), y_abs.copysign(y))
        };

        // Convert to on-screen window co-ordinates
        let x = (x / 2.0 + 0.5) * vw + vx;
        let y = (y / 2.0 + 0.5) * vh + vy;

        let (old_x, old_y, old_pressed, _old_visible) =
            self.virtual_cursor_last.unwrap_or_default();

        let (x, y) = if let Some((smoothing_strength, sticky_radius)) =
            options.stabilize_virtual_cursor
        {
            let new_time = Instant::now();

            let (old_x_unsticky, old_y_unsticky, old_time) = self
                .virtual_cursor_last_unsticky
                .unwrap_or((0.0, 0.0, new_time));

            let delta_t = new_time.saturating_duration_since(old_time).as_secs_f32();

            // Apply a feedback-based smoothing with exponential decay, to try
            // to dampen shakiness in the stick movement.

            let smooth = |old: f32, new: f32| -> f32 {
                if smoothing_strength != 0.0 {
                    let lerp_factor = 1.0 - (0.5_f32).powf(delta_t * (1.0 / smoothing_strength));
                    old + (new - old) * lerp_factor
                } else {
                    new
                }
            };

            let new_x_unsticky = smooth(old_x_unsticky, x);
            let new_y_unsticky = smooth(old_y_unsticky, y);

            self.virtual_cursor_last_unsticky = Some((new_x_unsticky, new_y_unsticky, new_time));

            // Make the reported position "sticky" within a certain radius, i.e.
            // if the new position's distance from the old one is within the
            // radius, report no change in position.

            if (new_x_unsticky - old_x).hypot(new_y_unsticky - old_y) < sticky_radius {
                (old_x, old_y)
            } else {
                (new_x_unsticky, new_y_unsticky)
            }
        } else {
            (x, y)
        };

        self.virtual_cursor_last = Some((x, y, pressed, visible));

        (
            x,
            y,
            pressed,
            pressed != old_pressed,
            x != old_x || y != old_y,
        )
    }

    /// Get the summed X and Y positions and button state of the left or right
    /// analog stick of the game controllers. Each axis value is in the range
    /// [-1, 1].
    fn get_controller_stick(&self, options: &Options, left: bool) -> (f32, f32, bool) {
        // Guard the accelerometer fallback when no real sensor is present.
        if self.native_controller_input || !self.controller_input_focused {
            return (0.0, 0.0, false);
        }
        let (mut x, mut y) = (0.0, 0.0);
        let mut pressed = false;
        for controller in &self.controllers {
            use sdl2::controller::{Axis, Button};
            let (x_axis, y_axis, button1, button2) = if left {
                (
                    Axis::LeftX,
                    Axis::LeftY,
                    Button::LeftStick,
                    Button::LeftShoulder,
                )
            } else {
                (
                    Axis::RightX,
                    Axis::RightY,
                    Button::RightStick,
                    Button::RightShoulder,
                )
            };
            x += normalize_controller_axis(controller.axis(x_axis), options.deadzone);
            y += normalize_controller_axis(controller.axis(y_axis), options.deadzone);
            let mapped = |button| match button {
                Button::RightStick => options
                    .button_to_touch
                    .contains_key(&crate::options::Button::RightStick),
                Button::RightShoulder => options
                    .button_to_touch
                    .contains_key(&crate::options::Button::RightShoulder),
                _ => false,
            };
            pressed |= !mapped(button1) && controller.button(button1);
            pressed |= !mapped(button2) && controller.button(button2);
        }
        let (x, y) = (x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0));

        (x, y, pressed)
    }

    pub fn create_offscreen_gl_context(&self) -> Result<(GLContext, sdl2::video::Window), String> {
        let attr = self.video_ctx.gl_attr();
        attr.set_context_version(2, 1);
        attr.set_context_profile(sdl2::video::GLProfile::Compatibility);
        let window = self
            .video_ctx
            .window("touchHLE rendering", 1, 1)
            .hidden()
            .opengl()
            .build()
            .map_err(|e| e.to_string())?;
        let context = window.gl_create_context()?;
        Ok((GLContext(context), window))
    }

    pub fn create_gl_context(&self, version: GLVersion) -> Result<GLContext, String> {
        let attr = self.video_ctx.gl_attr();
        match version {
            GLVersion::GLES11 => {
                attr.set_context_version(1, 1);
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
            }
            GLVersion::GLES20 => {
                attr.set_context_version(2, 0);
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
            }
            GLVersion::GLES30 => {
                attr.set_context_version(3, 0);
                attr.set_context_profile(sdl2::video::GLProfile::GLES);
            }
            GLVersion::GL21Compat => {
                attr.set_context_version(2, 1);
                attr.set_context_profile(sdl2::video::GLProfile::Compatibility);
            }
            GLVersion::GL33Core => {
                attr.set_context_version(3, 3);
                attr.set_context_profile(sdl2::video::GLProfile::Core);
            }
        }

        let gl_ctx = self.window.gl_create_context()?;

        Ok(GLContext(gl_ctx))
    }

    pub fn gl_get_proc_address(&self, procname: &str) -> *const std::ffi::c_void {
        // For some reason, rust-sdl2 uses *const (), but () is not meant to be
        // used for void pointees (just void results), so let's fix that.
        self.video_ctx.gl_get_proc_address(procname) as *const _
    }

    pub fn set_share_with_current_context(&self, value: bool) {
        self.video_ctx
            .gl_attr()
            .set_share_with_current_context(value)
    }

    pub unsafe fn make_gl_context_current(&self, gl_ctx: &GLContext) {
        make_current_or_warn(&self.window, gl_ctx);
    }

    /// Whether drawing should be skipped because the window is minimized or
    /// touchHLE is in the background.
    pub fn is_minimized(&self) -> bool {
        self.minimized
    }

    /// Whether the window's contents need to be fully redrawn even if the
    /// frame to be drawn hasn't changed (e.g. because the window was exposed
    /// or resized). Resets the flag.
    pub fn take_needs_redraw(&self) -> bool {
        self.needs_redraw.replace(false)
    }

    /// Make the internal OpenGL ES context (for splash screen and UI rendering)
    /// current.
    #[must_use]
    pub fn make_internal_gl_ctx_current<'win>(&'win mut self) -> Box<dyn GLES + 'win> {
        // The invariant is held up here - since the instance we return is
        // bound to the lifetime of window, it can't outlive the internal GL
        // context and can't outlive the window.
        let gl_ins = unsafe {
            self.internal_gl_ins
                .as_mut()
                .unwrap()
                .make_current_unchecked_for_window(
                    &mut |gl_ctx| make_current_or_warn(&self.window, gl_ctx),
                    &mut |s| self.video_ctx.gl_get_proc_address(s) as *const _,
                )
        };
        gl_ins
    }

    fn display_splash(&mut self) {
        assert!(self.splash_image.is_some());

        // OpenGL ES expects bottom-to-top row order for image data, but our
        // image data will be top-to-bottom. A reflection transform compensates.
        let matrix = self.rotation_matrix().multiply(&Matrix::y_flip());
        let (vx, vy, vw, vh) = self.viewport();
        let viewport = (vx, vy + self.viewport_y_offset() as i32, vw, vh);

        let image = self.splash_image.as_ref().unwrap();

        unsafe {
            let mut gl_ctx = self
                .internal_gl_ins
                .as_mut()
                .unwrap()
                .make_current_unchecked_for_window(
                    &mut |gl_ctx| make_current_or_warn(&self.window, gl_ctx),
                    &mut |s| self.video_ctx.gl_get_proc_address(s) as *const _,
                );

            use crate::gles::gles11_raw as gles11; // constants only

            let mut texture = 0;
            gl_ctx.GenTextures(1, &mut texture);
            gl_ctx.BindTexture(gles11::TEXTURE_2D, texture);
            let (width, height) = image.dimensions();
            gl_ctx.TexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGBA as _,
                width as _,
                height as _,
                0,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                image.pixels().as_ptr() as *const _,
            );
            gl_ctx.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MIN_FILTER,
                gles11::LINEAR as _,
            );
            gl_ctx.TexParameteri(
                gles11::TEXTURE_2D,
                gles11::TEXTURE_MAG_FILTER,
                gles11::LINEAR as _,
            );

            present_frame(
                gl_ctx.as_mut(),
                viewport,
                matrix,
                /* virtual_cursor_visible_at: */ None,
                /* clear: */ true,
            );

            gl_ctx.DeleteTextures(1, &texture);
        };

        self.window.gl_swap_window();

        // hold onto GL context so the image doesn't disappear, and hold
        // onto image so we can rotate later if necessary
    }

    /// Swap front-buffer and back-buffer so the result of OpenGL rendering is
    /// presented.
    pub fn swap_window(&mut self) {
        if self.minimized {
            return;
        }
        self.window.gl_swap_window();

        if self.screenshot_requested.replace(false) {
            if let Some(gl_ins) = self.internal_gl_ins.as_mut() {
                unsafe {
                    let mut ctx = gl_ins.make_current_unchecked_for_window(
                        &mut |gl_ctx| make_current_or_warn(&self.window, gl_ctx),
                        &mut |s| self.video_ctx.gl_get_proc_address(s) as *const _,
                    );
                    use crate::gles::gles11_raw as gles11;
                    let mut vp = [0i32; 4];
                    ctx.GetIntegerv(gles11::VIEWPORT, vp.as_mut_ptr());
                    let (w, h) = (vp[2] as usize, vp[3] as usize);
                    if w > 0 && h > 0 {
                        let mut buf = vec![0u8; w * h * 4];
                        ctx.ReadPixels(
                            0,
                            0,
                            vp[2],
                            vp[3],
                            gles11::RGBA,
                            gles11::UNSIGNED_BYTE,
                            buf.as_mut_ptr().cast(),
                        );
                        let result =
                            std::fs::write("/tmp/touchhle_shot.raw", &buf).and_then(|_| {
                                std::fs::write("/tmp/touchhle_shot.dim", format!("{} {}", w, h))
                            });
                        match result {
                            Ok(()) => {
                                log!("[SHOT] saved screenshot {}x{}", w, h);
                            }
                            Err(e) => {
                                log!("[SHOT] couldn't save screenshot: {}", e);
                            }
                        }
                    }
                }
            }
        }
    }

    /// Consider the emulated device to be rotated to a particular orientation.
    ///
    /// On a PC or laptop, this will make the window be rotated so the app
    /// content appears upright. On a mobile device, this might do something
    /// else, because the user can physically rotate the screen.
    pub fn rotate_device(&mut self, new_orientation: DeviceOrientation) {
        assert!(self.on_main_stack);
        if new_orientation == self.device_orientation {
            return;
        }

        if !self.fullscreen && !Self::rotatable_fullscreen() {
            let (width, height) = if Self::rotatable_fullscreen() {
                set_sdl2_orientation(new_orientation);
                rotate_fullscreen_size(new_orientation, self.window.size())
            } else {
                size_for_orientation(
                    self.p2g_wide_width
                        .map(|w| (320, w))
                        .unwrap_or(self.portrait_size),
                    new_orientation,
                    self.scale_hack,
                )
            };

            // macOS quirk: when resizing the window, the new framebuffer's size
            // is apparently max(new_size, old_size) in each dimension, but the
            // viewport is positioned wrong on the y axis for some reason, so we
            // need to apply an offset.
            // Recreating the OpenGL context was an alternative workaround, but
            // that apparently stops other OpenGL contexts drawing to the
            // framebuffer!
            #[cfg(target_os = "macos")]
            {
                let (_old_width, old_height) = self.window.size();
                self.max_height = self.max_height.max(old_height).max(height);
                self.viewport_y_offset = self.max_height - height;
            }

            self.window.set_size(width, height).unwrap();
        }

        if Self::rotatable_fullscreen() {
            set_sdl2_orientation(new_orientation);
            // Hack: from reading SDL2's source code, it seems that SDL2 will
            // only re-do the orientation when changing whether a window is
            // "resizeable" (can be rotated). (The window isn't an SDL
            // fullscreen window on Android, see Window::new, otherwise this
            // wouldn't be allowed.)
            unsafe {
                let window_raw = self.window.raw();
                sdl2_sys::SDL_SetWindowResizable(window_raw, sdl2_sys::SDL_bool::SDL_FALSE);
                sdl2_sys::SDL_SetWindowResizable(window_raw, sdl2_sys::SDL_bool::SDL_TRUE);
            }
        }

        self.device_orientation = new_orientation;

        if self.splash_image.is_some() {
            self.display_splash();
        }
    }

    pub fn device_family(&self) -> DeviceFamily {
        self.device_family
    }

    /// Returns the current device orientation
    pub fn current_rotation(&self) -> DeviceOrientation {
        self.device_orientation
    }

    /// Get the size in pixels of the window without rotation or scaling.
    ///
    /// The aspect ratio, scale and orientation reflect the guest app's view of
    /// the world.
    pub fn size_unrotated_unscaled(&self) -> (u32, u32) {
        size_for_orientation(
            self.portrait_size,
            DeviceOrientation::Portrait,
            NonZeroU32::new(1).unwrap(),
        )
    }

    /// Get the region of the on-screen window (x, y, width, height) used to
    /// display the app content.
    ///
    /// The aspect ratio of this region always reflects the guest app's view of
    /// the world, but the scale and orientation might not.
    /// The area of the window the app's screen is drawn to, as `(x, y, width,
    /// height)` in pixels. `x` and `y` can be negative when the screen is
    /// cropped ([crate::options::AspectMode::Fill]).
    pub fn viewport(&self) -> (i32, i32, u32, u32) {
        let display_size = self
            .p2g_wide_width
            .map(|w| (320, w))
            .unwrap_or(self.portrait_size);
        let (app_width, app_height) =
            size_for_orientation(display_size, self.device_orientation, self.scale_hack);
        if !self.fullscreen && !Self::rotatable_fullscreen() {
            return (0, 0, app_width, app_height);
        }

        let (screen_width, screen_height) = self.window.drawable_size();
        // The scale hack only increases the internal resolution, so the
        // unscaled size is what matters for whole-number scaling.
        let (base_width, base_height) = size_for_orientation(
            display_size,
            self.device_orientation,
            NonZeroU32::new(1).unwrap(),
        );

        compute_viewport(
            if self.p2g_wide_width.is_some() {
                crate::options::AspectMode::Fit
            } else {
                self.aspect_mode
            },
            (app_width, app_height),
            (base_width, base_height),
            (screen_width, screen_height),
        )
    }

    /// Original 16:9 input canvas centered inside the expanded scene.
    fn input_viewport(&self) -> (i32, i32, u32, u32) {
        let viewport = self.viewport();
        if self.p2g_wide_width.is_none() {
            return viewport;
        }
        p2g_input_viewport(viewport)
    }

    /// For [crate::options::FilterMode::Sharp]: the whole number the app's
    /// screen is scaled up by (with nearest-neighbour filtering) before the
    /// final bilinear scaling to the viewport size. 1 for other modes.
    pub fn sharp_upscale_factor(&self) -> u32 {
        use crate::options::FilterMode;
        if !matches!(self.filter_mode, FilterMode::Sharp | FilterMode::Hq) {
            return 1;
        }
        let display_size = self
            .p2g_wide_width
            .map(|w| (320, w))
            .unwrap_or(self.portrait_size);
        let (app_width, app_height) =
            size_for_orientation(display_size, self.device_orientation, self.scale_hack);
        let (_, _, vw, vh) = self.viewport();
        let factor = (vw / app_width).min(vh / app_height).max(1);
        // With the hq filter, some layers' textures are twice the size, so the
        // factor must be even for their pixels to be scaled evenly. Round up
        // rather than down, since the final scaling handles shrinking better.
        let factor = if self.filter_mode == FilterMode::Hq {
            factor.next_multiple_of(2)
        } else {
            factor
        };
        // Cap it to keep the intermediate texture a reasonable size.
        factor.clamp(1, 8)
    }

    /// The configured [crate::options::FilterMode].
    pub fn filter_mode(&self) -> crate::options::FilterMode {
        self.filter_mode
    }

    /// Special offset to add to y co-ordinates, only when drawing to screen.
    pub fn viewport_y_offset(&self) -> u32 {
        #[cfg(target_os = "macos")]
        return self.viewport_y_offset;
        #[cfg(not(target_os = "macos"))]
        return 0;
    }

    /// Transformation matrix for transforming between the window's co-ordinate
    /// space and the app's original co-ordinate space when rotation is in use
    /// (see [Self::rotate_device]). This returns a matrix appropriate for
    /// rotating texture co-ordinates to display the image in the window; when
    /// rotating input co-ordinates, invert the matrix.
    pub fn rotation_matrix(&self) -> Matrix<2> {
        match self.device_orientation {
            DeviceOrientation::Portrait => Matrix::identity(),
            DeviceOrientation::PortraitUpsideDown => Matrix::z_rotation(PI),
            DeviceOrientation::LandscapeLeft => Matrix::z_rotation(-FRAC_PI_2),
            DeviceOrientation::LandscapeRight => Matrix::z_rotation(FRAC_PI_2),
        }
    }

    pub fn is_screen_saver_enabled(&self) -> bool {
        self.video_ctx.is_screen_saver_enabled()
    }
    pub fn set_screen_saver_enabled(&mut self, enabled: bool) {
        assert!(self.on_main_stack);
        match enabled && !self.keep_screen_on {
            true => self.video_ctx.enable_screen_saver(),
            false => self.video_ctx.disable_screen_saver(),
        }
    }

    pub fn start_text_input(&self) {
        assert!(self.on_main_stack);
        unsafe {
            sdl2_sys::SDL_StartTextInput();
        }
    }
    /// Hide the window, e.g. while the app is quitting, which may take a few
    /// seconds.
    pub fn hide(&mut self) {
        assert!(self.on_main_stack);
        self.window.hide();
        // Let the OS process this now: the app might not return to the event
        // loop for a while.
        unsafe { sdl2_sys::SDL_PumpEvents() };
    }

    pub fn stop_text_input(&self) {
        assert!(self.on_main_stack);
        unsafe {
            sdl2_sys::SDL_StopTextInput();
        }
    }

    pub fn on_main_stack(&self) -> bool {
        self.on_main_stack
    }
}

pub fn open_url(env: &mut Environment, url: &str) -> Result<(), String> {
    env.on_parent_stack_in_coroutine(|_, _| sdl2::url::open_url(url).map_err(|e| e.to_string()))
}

/// Show an SDL messagebox for an error (typically after a panic).
///
/// The window argument allows for passing in the parent window for the
/// messagebox, which is not required but should be done if possible.
pub fn show_error_messagebox(window: Option<&Window>, error_message: &str) {
    assert!(window.is_none_or(|win| win.on_main_stack));
    use sdl2::messagebox;
    let mbox = [
        messagebox::ButtonData {
            flags: messagebox::MessageBoxButtonFlag::NOTHING,
            button_id: 0,
            text: "Open touchHLE directory",
        },
        messagebox::ButtonData {
            flags: messagebox::MessageBoxButtonFlag::NOTHING,
            button_id: 1,
            text: "Close",
        },
    ];

    let Ok(clicked_button) = messagebox::show_message_box(
        messagebox::MessageBoxFlag::ERROR,
        &mbox,
        "touchHLE crashed!",
        &format!("touchHLE crashed with the following error: {error_message}"),
        window.map(|win| &win.window),
        None,
    ) else {
        panic!("Failed to show message box!");
    };

    match clicked_button {
        messagebox::ClickedButton::CloseButton => {}
        messagebox::ClickedButton::CustomButton(button) => {
            match button.button_id {
                // Open data directory (contains log file on android)
                0 => match crate::paths::url_for_opening_user_data_dir() {
                    Ok(url) => {
                        if let Err(e) = sdl2::url::open_url(&url).map_err(|e| e.to_string()) {
                            echo!("Couldn't open file manager at {:?}: {}", url, e);
                        } else {
                            echo!("Opened file manager at {:?}, exiting.", url);
                        }
                    }
                    Err(e) => echo!("Couldn't open file manager: {}", e),
                },
                // Close
                1 => {}
                _ => unreachable!(),
            }
        }
    }
}

/// Get current battery state from SDL2.
///
/// Returns:
/// - pct: i32 - percentage of battery remaining.
/// - status: [BatteryState] - the current status of the battery
///   (unplugged, charging, full, etc.)
pub fn get_battery_status() -> (i32, BatteryState) {
    if env::consts::OS == "android" {
        log_once!(
            "Warning: get_battery_status on Android, returning fully charged to avoid SDL crash"
        );
        // Android_JNI_GetPowerInfo is crashing with `JNI DETECTED ERROR IN
        // APPLICATION: JNI ERROR (app bug): jobject is an invalid JNI
        // transition frame reference: 0x7b1bc0c7a0 (use of invalid jobject)`
        // TODO: See if updating SDL fixes that
        return (100, BatteryState::Full);
    }
    let mut pct = 0;
    // Unfortunately, Rust-SDL2 does not expose this function yet.
    // iPhoneOS does not measure the battery in seconds remaining,
    // so we discard this argument.
    let status = unsafe { sdl2_sys::SDL_GetPowerInfo(null_mut(), &mut pct) };
    (
        pct,
        match status {
            SDL_PowerState::SDL_POWERSTATE_UNKNOWN => BatteryState::Unknown,
            SDL_PowerState::SDL_POWERSTATE_ON_BATTERY => BatteryState::OnBattery,
            SDL_PowerState::SDL_POWERSTATE_NO_BATTERY => BatteryState::NoBattery,
            SDL_PowerState::SDL_POWERSTATE_CHARGING => BatteryState::Charging,
            SDL_PowerState::SDL_POWERSTATE_CHARGED => BatteryState::Full,
        },
    )
}

/// Like [get_preferred_language_codes], but usable without an [Environment]
/// (e.g. before one exists).
pub fn system_language_codes() -> Vec<String> {
    sdl2::locale::get_preferred_locales()
        .map(ios_language_code)
        .collect()
}

pub fn get_preferred_language_codes(env: &mut Environment) -> Vec<String> {
    env.on_parent_stack_in_coroutine(|_, _| {
        sdl2::locale::get_preferred_locales()
            .map(ios_language_code)
            .collect()
    })
}

/// The language code iPhone OS would use for a locale. This matters for
/// Chinese: iPhone OS reports "zh-Hans" or "zh-Hant", and apps (e.g. Inotia 2)
/// compare against those, but on some systems (e.g. Android) SDL only gives
/// "zh" and a region.
fn ios_language_code(locale: sdl2::locale::Locale) -> String {
    if locale.lang != "zh" {
        return locale.lang;
    }
    let traditional = locale
        .country
        .as_deref()
        .is_some_and(|country| ["TW", "HK", "MO"].contains(&country));
    if traditional {
        "zh-Hant".to_string()
    } else {
        "zh-Hans".to_string()
    }
}

pub fn get_preferred_country_codes(env: &mut Environment) -> Vec<String> {
    env.on_parent_stack_in_coroutine(|_, _| {
        sdl2::locale::get_preferred_locales()
            .filter_map(|loc| loc.country)
            .collect()
    })
}

fn p2g_input_viewport((x, y, w, h): (i32, i32, u32, u32)) -> (i32, i32, u32, u32) {
    if w >= h {
        let safe_w = ((h as u64 * 568 / 320) as u32).min(w);
        (x + ((w - safe_w) / 2) as i32, y, safe_w, h)
    } else {
        let safe_h = ((w as u64 * 568 / 320) as u32).min(h);
        (x, y + ((h - safe_h) / 2) as i32, w, safe_h)
    }
}

#[cfg(test)]
mod controller_input_tests {
    use super::{normalize_controller_axis, trigger_pressed, ControllerSnapshot};

    #[test]
    fn camera_swipe_seeds_moves_continuously_and_releases() {
        use super::{
            step_controller_swipe,
            ControllerTouchPhase::{Down, Move, Up},
        };
        use std::time::Duration;
        let region = (180.0, 100.0, 130.0, 100.0);
        let (position, events) =
            step_controller_swipe(None, (1.0, -0.5), region, Duration::from_millis(25));
        assert_eq!(events, [(Down, (245.0, 150.0)), (Move, (249.0, 148.0))]);
        let (position, events) =
            step_controller_swipe(position, (1.0, -0.5), region, Duration::from_millis(25));
        assert_eq!(events, [(Move, (253.0, 146.0))]);
        let (position, events) =
            step_controller_swipe(position, (0.0, 0.0), region, Duration::from_millis(25));
        assert_eq!(position, None);
        assert_eq!(events, [(Up, (253.0, 146.0))]);
        assert!(
            step_controller_swipe(None, (0.0, 0.0), region, Duration::ZERO)
                .1
                .is_empty()
        );
    }

    #[test]
    fn camera_swipe_caps_time_and_reseeds_without_backwards_move() {
        use super::{
            step_controller_swipe,
            ControllerTouchPhase::{Down, Move, Up},
        };
        use std::time::Duration;
        let region = (180.0, 100.0, 130.0, 100.0);
        let start = Some((245.0, 150.0));
        let capped = step_controller_swipe(start, (1.0, 1.0), region, Duration::from_millis(50));
        assert_eq!(capped.0, Some((253.0, 158.0)));
        assert_eq!(
            step_controller_swipe(start, (1.0, 1.0), region, Duration::from_secs(5)),
            capped
        );
        assert_eq!(
            step_controller_swipe(start, (1.0, 1.0), region, Duration::ZERO).0,
            start
        );
        for (position, stick, edge) in [
            ((309.0, 150.0), (1.0, 0.0), (310.0, 150.0)),
            ((181.0, 150.0), (-1.0, 0.0), (180.0, 150.0)),
            ((245.0, 199.0), (0.0, 1.0), (245.0, 200.0)),
            ((245.0, 101.0), (0.0, -1.0), (245.0, 100.0)),
        ] {
            let (next, events) =
                step_controller_swipe(Some(position), stick, region, Duration::from_millis(50));
            assert_eq!(next, start);
            assert_eq!(events, [(Move, edge), (Up, edge), (Down, (245.0, 150.0))]);
        }
    }

    #[test]
    fn resume_keeps_controller_releases_before_new_input() {
        use super::{discard_paused_input, Event, FingerId};
        use std::collections::{HashMap, VecDeque};
        let finger = FingerId::ButtonToTouch(crate::options::Button::RightTrigger);
        let points = HashMap::from([(finger, (10.0, 20.0))]);
        let mut queue = VecDeque::from([
            Event::TouchesMove(points.clone()),
            Event::TouchesUp(points.clone()),
            Event::TouchesUp(HashMap::from([(FingerId::Mouse, (1.0, 2.0))])),
            Event::AppDidBecomeActive,
            Event::TouchesDown(points),
        ]);
        discard_paused_input(&mut queue, 4);
        let Some(Event::TouchesUp(releases)) = queue.pop_front() else {
            panic!("controller release lost during resume");
        };
        assert_eq!(releases.len(), 1);
        assert_eq!(releases.get(&finger), Some(&(10.0, 20.0)));
        assert!(matches!(queue.pop_front(), Some(Event::TouchesDown(_))));
        assert!(queue.is_empty());
    }

    #[test]
    fn focus_loss_neutralizes_input_without_disconnect() {
        let snapshot = ControllerSnapshot {
            instance_id: 42,
            name: "Host controller".into(),
            axes: [0.5, -0.5, 1.0, -1.0, 0.7, 0.9],
            buttons: vec![sdl2::controller::Button::A],
        };
        let focused = snapshot.clone().with_focus(true);
        assert_eq!(focused.axes, snapshot.axes);
        assert_eq!(focused.buttons, snapshot.buttons);
        let unfocused = snapshot.with_focus(false);
        assert_eq!(unfocused.instance_id, 42);
        assert_eq!(unfocused.name, "Host controller");
        assert_eq!(unfocused.axes, [0.0; 6]);
        assert!(unfocused.buttons.is_empty());
    }

    #[test]
    fn axes_have_bounded_endpoints_and_rescaled_deadzone() {
        assert_eq!(normalize_controller_axis(i16::MIN, 0.2), -1.0);
        assert_eq!(normalize_controller_axis(i16::MAX, 0.2), 1.0);
        assert_eq!(normalize_controller_axis(6000, 0.2), 0.0);
        assert!((normalize_controller_axis(19660, 0.2) - 0.5).abs() < 0.001);
        assert_eq!(normalize_controller_axis(0, 0.0), 0.0);
        for deadzone in [1.0, 2.0, f32::NAN] {
            assert!(normalize_controller_axis(i16::MAX, deadzone).is_finite());
        }
    }

    #[test]
    fn trigger_hysteresis_releases_without_repeated_edges() {
        let mut held = false;
        let mut transitions = Vec::new();
        for value in [0, 15000, 17000, 16000, 14000, 12000, 0] {
            let pressed = trigger_pressed(value, held);
            if held != pressed {
                transitions.push(pressed);
            }
            held = pressed;
        }
        assert_eq!(transitions, [true, false]);
        assert!(!trigger_pressed(i16::MIN, true));
        assert!(trigger_pressed(i16::MAX, false));
    }
}

#[cfg(test)]
mod selected_screen_geometry_tests {
    use super::*;

    #[test]
    fn selected_portrait_size_drives_rotation_and_scale() {
        let scale = NonZeroU32::new(2).unwrap();
        for portrait in [(320, 480), (320, 568), (375, 667), (414, 736), (768, 1024)] {
            for orientation in [
                DeviceOrientation::Portrait,
                DeviceOrientation::PortraitUpsideDown,
            ] {
                assert_eq!(
                    size_for_orientation(portrait, orientation, scale),
                    (portrait.0 * 2, portrait.1 * 2)
                );
            }
            for orientation in [
                DeviceOrientation::LandscapeLeft,
                DeviceOrientation::LandscapeRight,
            ] {
                assert_eq!(
                    size_for_orientation(portrait, orientation, scale),
                    (portrait.1 * 2, portrait.0 * 2)
                );
            }
        }
    }

    #[test]
    fn legacy_mapping_and_physical_input_reach_same_tall_screen_points() {
        let selected = (320, 568);
        for rotation in [
            Matrix::identity(),
            Matrix::z_rotation(PI),
            Matrix::z_rotation(-FRAC_PI_2),
            Matrix::z_rotation(FRAC_PI_2),
        ] {
            for (x, y) in [(0.25, 0.25), (0.5, 0.5), (0.75, 0.75)] {
                let mapped = transform_screen_coords(
                    (480.0 * x, 320.0 * y),
                    (0, 0, 480, 320),
                    selected,
                    rotation,
                );
                let mouse = transform_screen_coords(
                    (100.0 + 1136.0 * x, 50.0 + 640.0 * y),
                    (100, 50, 1136, 640),
                    selected,
                    rotation,
                );
                assert_eq!(mapped, mouse);
            }
        }
        let left = Matrix::z_rotation(-FRAC_PI_2);
        assert_eq!(
            transform_screen_coords((240.0, 160.0), (0, 0, 480, 320), selected, left),
            (160.0, 284.0)
        );
        assert_eq!(
            transform_screen_coords((120.0, 80.0), (0, 0, 480, 320), selected, left),
            (240.0, 142.0)
        );
    }

    #[test]
    fn tall_viewport_uses_selected_aspect_instead_of_legacy_aspect() {
        use crate::options::AspectMode;
        assert_eq!(
            compute_viewport(AspectMode::Fit, (568, 320), (568, 320), (1136, 640)),
            (0, 0, 1136, 640)
        );
        assert_eq!(
            compute_viewport(AspectMode::Fit, (480, 320), (480, 320), (1136, 640)),
            (88, 0, 960, 640)
        );
    }
    #[test]
    fn expanded_scene_preserves_original_button_touch_positions() {
        use std::f32::consts::FRAC_PI_2;
        let base = (0, 0, 1136, 640);
        let safe = p2g_input_viewport((0, 0, 1388, 640));
        assert_eq!(safe, (126, 0, 1136, 640));
        let rotation = Matrix::z_rotation(-FRAC_PI_2);
        for p in [(80.0, 540.0), (1060.0, 580.0), (568.0, 320.0)] {
            let original = transform_screen_coords(p, base, (320, 568), rotation);
            let expanded = transform_screen_coords((p.0 + 126.0, p.1), safe, (320, 568), rotation);
            assert_eq!(original, expanded);
        }
        assert_eq!(p2g_input_viewport((0, 0, 640, 1388)), (0, 126, 640, 1136));
    }
}
