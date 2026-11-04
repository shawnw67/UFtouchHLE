/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Parsing and management of user-configurable options, e.g. for input methods.

use crate::gles::GLESImplementation;
use crate::window::{DeviceFamily, DeviceOrientation};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::net::{SocketAddr, ToSocketAddrs};
use std::num::NonZeroU32;
use std::path::PathBuf;

pub const OPTIONS_HELP: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/OPTIONS_HELP.txt"));

/// Game controller button for `--button-to-touch=` option.
#[derive(Copy, Clone, Hash, PartialEq, Eq, Debug)]
pub enum Button {
    DPadLeft,
    DPadUp,
    DPadRight,
    DPadDown,
    Start,
    A,
    B,
    X,
    Y,
    LeftShoulder,
    RightShoulder,
    Back,
    LeftStick,
    RightStick,
    LeftTrigger,
    RightTrigger,
}

/// How the app's screen is fitted into a fullscreen window.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum AspectMode {
    /// Keep the aspect ratio, add black bars (letterbox/pillarbox).
    #[default]
    Fit,
    /// Fill the whole window, distorting the image.
    Stretch,
    /// Keep the aspect ratio and fill the whole window, cropping the edges.
    Fill,
    /// Keep the aspect ratio and only scale by whole numbers, so every pixel
    /// of the app is the same size.
    Integer,
}

/// How the app's screen is filtered when scaled up.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FilterMode {
    /// Bilinear filtering: smooth, but blurry for pixel art.
    Smooth,
    /// "Sharp bilinear": scale up by a whole number with nearest-neighbour
    /// filtering, then bilinear for the rest. Crisp pixels, no uneven pixel
    /// sizes.
    Sharp,
    /// Nearest-neighbour filtering only.
    Pixel,
    /// Like [FilterMode::Sharp], but images drawn by the app's CPU code (e.g.
    /// 2D games that use Core Graphics, and UIKit views) are first scaled up
    /// with a pixel art scaling algorithm, rounding off jagged edges.
    Hq,
}

impl FilterMode {
    /// The default. Phone screens are typically scaled up by 3× or more,
    /// where plain bilinear filtering makes iPhone OS apps (especially pixel
    /// art) look very blurry, so sharp filtering is the default there.
    pub const DEFAULT: FilterMode = if cfg!(target_os = "android") {
        FilterMode::Sharp
    } else {
        FilterMode::Smooth
    };

    /// The name used for this mode in `--filter=`.
    pub fn name(self) -> &'static str {
        match self {
            FilterMode::Smooth => "smooth",
            FilterMode::Sharp => "sharp",
            FilterMode::Pixel => "pixel",
            FilterMode::Hq => "hq",
        }
    }
}

/// Default for [Options::keep_screen_on]. On Android, if the screen turns off
/// during play (e.g. in a cutscene, or in a game that's mostly watched), the
/// game is paused in the background, which players don't expect from a game.
pub const KEEP_SCREEN_ON_DEFAULT: bool = cfg!(target_os = "android");

/// Struct containing all user-configurable options.
#[derive(Clone)]
pub struct Options {
    pub fullscreen: bool,
    pub aspect_mode: AspectMode,
    pub filter_mode: FilterMode,
    pub keep_screen_on: bool,
    pub device_family: Option<DeviceFamily>,
    /// Device model reported to the app (`hw.machine`), e.g. `iPhone2,1`.
    /// [None] means the original iPhone (`iPhone1,1`).
    pub device_model: Option<&'static str>,
    /// P2G expanded 3D view, retaining its original 16:9 controls.
    pub p2g_wide_screen: bool,
    /// Resolved landscape width in points; the guest remains 568x320.
    pub p2g_wide_width: Option<u32>,
    pub device_profile: &'static str,
    /// Resolved logical portrait dimensions; populated at runtime only.
    pub device_screen_size: Option<(u32, u32)>,
    /// Runtime device pixel multiplier (3 for iPhone 6 Plus).
    pub device_screen_scale: Option<u32>,
    /// Orientation-relative coordinate reference for controller touch mappings.
    pub touch_map_size: Option<(u32, u32)>,
    /// Operating system version reported by UIDevice.
    pub system_version: String,
    /// Report a Retina display (`[UIScreen scale]` of 2).
    pub retina: bool,
    /// Shift for everything the app draws, in pixels (see
    /// `draw_with_subpixel_offset` in `gles_guest`).
    pub subpixel_offset: Option<f32>,
    pub initial_orientation: DeviceOrientation,
    pub scale_hack: NonZeroU32,
    pub deadzone: f32,
    pub analog_stick_tilt_controls: bool,
    pub x_tilt_range: f32,
    pub y_tilt_range: f32,
    pub x_tilt_offset: f32,
    pub y_tilt_offset: f32,
    pub button_to_touch: HashMap<Button, (f32, f32)>,
    pub dpad_to_touch: Option<(f32, f32, f32, f32)>,
    pub stick_touch_start_at_center: bool,
    pub right_stick_swipe: Option<(f32, f32, f32, f32)>,
    pub right_stick_to_touch: Option<(f32, f32, f32, f32)>,
    pub stick_to_touch: Option<(f32, f32, f32, f32)>,
    pub stabilize_virtual_cursor: Option<(f32, f32)>,
    pub gles1_implementation: Option<GLESImplementation>,
    pub direct_memory_access: bool,
    pub gdb_listen_addrs: Option<Vec<SocketAddr>>,
    pub preferred_languages: Option<Vec<String>>,
    pub headless: bool,
    pub print_fps: bool,
    pub performance_test: Option<crate::performance::Mode>,
    pub audio_test: Option<crate::audio::sound_test::Mode>,
    pub fps_limit: Option<f64>,
    pub force_composition: bool,
    pub network_access: bool,
    pub network_trace: bool,
    pub network_trace_limit: u64,
    pub mhp2g_save_research: bool,
    pub gamekit_role: Option<crate::transport::Role>,
    pub gamekit_bind: std::net::SocketAddrV4,
    pub gamekit_player_id: Option<crate::transport::PlayerId>,
    pub gamekit_timeout: std::time::Duration,
    /// See `--gamekit-auto`. An explicit role or discovery overrides it.
    pub gamekit_auto: bool,
    pub gamekit_room: Option<String>,
    pub gamekit_discover: Option<String>,
    pub gamekit_discovery_bind: std::net::SocketAddrV4,
    pub gamekit_broadcasts: Vec<std::net::SocketAddrV4>,
    pub popup_errors: bool,
    pub dumping_options: DumpingOptions,
    pub dumping_file: PathBuf,
    pub ignore_gl_errors: bool,
    /// Check for an OpenGL ES error after every guest call and log where it
    /// came from. Diagnostic only: it hides errors from the app.
    pub trace_gl_errors: bool,
    /// Log every OpenGL ES call made by the guest. Diagnostic only.
    pub verbose_gles: bool,
    /// Create an OpenGL ES 2.0 context when an app asks for ES 1.1, for apps
    /// that ask for ES 1.1 but use the ES 2.0 shader entry points.
    pub prefer_gles2_context: bool,
    /// Create an OpenGL ES 1.1 context when an app asks for ES 2.0 or 3.0,
    /// for engines that have an ES 1.1 fallback.
    pub force_gles1_context: bool,
    /// After uploading level 0 of a texture, use `GL_LINEAR` minification if
    /// the app never set a filter, so textures without mipmaps aren't
    /// incomplete. On by default on Android.
    pub fix_texture_min_filter: bool,
    /// Give a linked ES 2.0 program's attributes the locations 0 to N-1 in the
    /// reverse of the order the host driver chose, if the app binds none
    /// itself.
    pub reverse_attrib_locations: bool,
    pub zero_stack_after_guest_to_host_call: Option<u32>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            fullscreen: false,
            aspect_mode: AspectMode::Fit,
            filter_mode: FilterMode::DEFAULT,
            keep_screen_on: KEEP_SCREEN_ON_DEFAULT,
            device_family: None,
            device_model: None,
            p2g_wide_screen: false,
            p2g_wide_width: None,
            device_profile: "auto",
            device_screen_size: None,
            device_screen_scale: None,
            touch_map_size: None,
            system_version: "2.0".to_string(),
            retina: false,
            subpixel_offset: None,
            initial_orientation: DeviceOrientation::Portrait,
            scale_hack: NonZeroU32::new(1).unwrap(),
            analog_stick_tilt_controls: true,
            deadzone: 0.1,
            x_tilt_range: 60.0,
            y_tilt_range: 60.0,
            x_tilt_offset: 0.0,
            y_tilt_offset: 0.0,
            button_to_touch: HashMap::new(),
            dpad_to_touch: None,
            stick_to_touch: None,
            right_stick_to_touch: None,
            right_stick_swipe: None,
            stick_touch_start_at_center: false,
            stabilize_virtual_cursor: None,
            gles1_implementation: None,
            direct_memory_access: true,
            gdb_listen_addrs: None,
            preferred_languages: None,
            headless: false,
            print_fps: false,
            performance_test: None,
            audio_test: None,
            fps_limit: Some(60.0), // Original iPhone is 60Hz and uses v-sync,
            force_composition: false,
            network_access: false,
            network_trace: false,
            network_trace_limit: 50_000,
            mhp2g_save_research: false,
            gamekit_role: None,
            gamekit_bind: "0.0.0.0:0".parse().unwrap(),
            gamekit_player_id: None,
            gamekit_timeout: std::time::Duration::from_secs(30),
            gamekit_auto: false,
            gamekit_room: None,
            gamekit_discover: None,
            gamekit_discovery_bind: "0.0.0.0:42019".parse().unwrap(),
            gamekit_broadcasts: Vec::new(),
            popup_errors: true,
            dumping_options: Default::default(),
            dumping_file: crate::paths::user_data_base_path().join("DUMP.txt"),
            ignore_gl_errors: false,
            trace_gl_errors: false,
            verbose_gles: false,
            prefer_gles2_context: false,
            force_gles1_context: false,
            fix_texture_min_filter: cfg!(target_os = "android"),
            reverse_attrib_locations: false,
            zero_stack_after_guest_to_host_call: None,
        }
    }
}

impl Options {
    pub fn screen_scale(&self) -> u32 {
        self.device_screen_scale
            .unwrap_or(if self.retina { 2 } else { 1 })
    }

    /// Parse the command-line argument syntax for an option. Returns `Ok(true)`
    /// if the option was valid and has been applied, or `Ok(false)` if the
    /// option was not recognized.
    /// Whether the local GameKit backend has a connection mode.
    pub fn gamekit_enabled(&self) -> bool {
        self.gamekit_role.is_some() || self.gamekit_discover.is_some() || self.gamekit_auto
    }

    pub fn parse_argument(&mut self, arg: &str) -> Result<bool, String> {
        fn parse_degrees(arg: &str, name: &str) -> Result<f32, String> {
            let arg: f32 = arg
                .parse()
                .map_err(|_| format!("Value for {name} is invalid"))?;
            if !arg.is_finite() || !(-360.0..=360.0).contains(&arg) {
                return Err(format!("Value for {name} is out of range"));
            }
            Ok(arg)
        }

        if arg == "--fullscreen" {
            self.fullscreen = true;
        } else if let Some(value) = arg.strip_prefix("--aspect=") {
            self.aspect_mode = match value {
                "fit" => AspectMode::Fit,
                "stretch" => AspectMode::Stretch,
                "fill" => AspectMode::Fill,
                "integer" => AspectMode::Integer,
                _ => return Err("Invalid value for --aspect".to_string()),
            };
        } else if arg == "--keep-screen-on" {
            self.keep_screen_on = true;
        } else if arg == "--no-keep-screen-on" {
            self.keep_screen_on = false;
        } else if let Some(value) = arg.strip_prefix("--filter=") {
            self.filter_mode = match value {
                "smooth" => FilterMode::Smooth,
                "sharp" => FilterMode::Sharp,
                "pixel" => FilterMode::Pixel,
                "hq" => FilterMode::Hq,
                _ => return Err("Invalid value for --filter".to_string()),
            };
        } else if arg == "--upside-down" {
            self.initial_orientation = DeviceOrientation::PortraitUpsideDown;
        } else if arg == "--landscape-left" {
            self.initial_orientation = DeviceOrientation::LandscapeLeft;
        } else if arg == "--landscape-right" {
            self.initial_orientation = DeviceOrientation::LandscapeRight;
        } else if arg == "--p2g-wide-screen" {
            self.p2g_wide_screen = true;
        } else if let Some(value) = arg.strip_prefix("--device-profile=") {
            self.device_profile = if value == "auto" {
                "auto"
            } else {
                crate::device_profiles::profile(value)
                    .ok_or_else(|| format!("Unknown device profile {value:?}"))?
                    .machine
            };
        } else if let Some(value) = arg.strip_prefix("--touch-map-size=") {
            let (width, height) = value
                .split_once(',')
                .ok_or_else(|| "--touch-map-size requires W,H".to_string())?;
            let dimension = |value: &str| -> Result<u32, String> {
                if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("--touch-map-size requires positive integer dimensions".to_string());
                }
                value
                    .parse::<u32>()
                    .ok()
                    .filter(|&v| v > 0)
                    .ok_or_else(|| "--touch-map-size requires positive u32 dimensions".to_string())
            };
            self.touch_map_size = Some((dimension(width)?, dimension(height)?));
        } else if let Some(value) = arg.strip_prefix("--system-version=") {
            let parts: Vec<_> = value.split('.').collect();
            if !(1..=3).contains(&parts.len())
                || parts.iter().any(|part| {
                    part.is_empty()
                        || !part.bytes().all(|byte| byte.is_ascii_digit())
                        || part.parse::<u16>().is_err()
                })
            {
                return Err(
                    "--system-version requires 1 to 3 dot-separated u16 numbers".to_string()
                );
            }
            self.system_version = value.to_string();
        } else if let Some(value) = arg.strip_prefix("--device-family=") {
            let parsed =
                DeviceFamily::try_from(value).map_err(|_| "Invalid device family".to_string())?;
            self.device_family = Some(parsed);
        } else if let Some(value) = arg.strip_prefix("--device-model=") {
            if crate::libc::sysctl::model_for_machine(value).is_none() {
                return Err(format!("Unknown device model {value:?}"));
            }
            // Options are parsed only a few times, so leaking is fine.
            self.device_model = Some(Box::leak(value.to_string().into_boxed_str()));
        } else if arg == "--retina" {
            self.retina = true;
        } else if let Some(value) = arg.strip_prefix("--subpixel-offset=") {
            let offset: f32 = value
                .parse()
                .map_err(|_| "Invalid subpixel offset".to_string())?;
            if !(offset.is_finite() && offset.abs() < 1.0) {
                return Err("Subpixel offset must be less than a pixel".to_string());
            }
            self.subpixel_offset = Some(offset);
        } else if let Some(value) = arg.strip_prefix("--scale-hack=") {
            self.scale_hack = value
                .parse()
                .map_err(|_| "Invalid scale hack factor".to_string())?;
        } else if arg == "--disable-analog-stick-tilt-controls" {
            self.analog_stick_tilt_controls = false;
        } else if let Some(value) = arg.strip_prefix("--deadzone=") {
            self.deadzone = parse_degrees(value, "deadzone")?;
        } else if let Some(value) = arg.strip_prefix("--x-tilt-range=") {
            self.x_tilt_range = parse_degrees(value, "X tilt range")?;
        } else if let Some(value) = arg.strip_prefix("--y-tilt-range=") {
            self.y_tilt_range = parse_degrees(value, "Y tilt range")?;
        } else if let Some(value) = arg.strip_prefix("--x-tilt-offset=") {
            self.x_tilt_offset = parse_degrees(value, "X tilt offset")?;
        } else if let Some(value) = arg.strip_prefix("--y-tilt-offset=") {
            self.y_tilt_offset = parse_degrees(value, "Y tilt offset")?;
        } else if let Some(values) = arg.strip_prefix("--button-to-touch=") {
            let (button, coords) = values
                .split_once(',')
                .ok_or_else(|| "--button-to-touch= requires three values".to_string())?;
            let (x, y) = coords
                .split_once(',')
                .ok_or_else(|| "--button-to-touch= requires three values".to_string())?;
            let button = match button {
                "DPadLeft" => Ok(Button::DPadLeft),
                "DPadUp" => Ok(Button::DPadUp),
                "DPadRight" => Ok(Button::DPadRight),
                "DPadDown" => Ok(Button::DPadDown),
                "Start" => Ok(Button::Start),
                "A" => Ok(Button::A),
                "B" => Ok(Button::B),
                "X" => Ok(Button::X),
                "Y" => Ok(Button::Y),
                "LeftShoulder" => Ok(Button::LeftShoulder),
                "RightShoulder" => Ok(Button::RightShoulder),
                "Back" => Ok(Button::Back),
                "LeftStick" => Ok(Button::LeftStick),
                "RightStick" => Ok(Button::RightStick),
                "LeftTrigger" => Ok(Button::LeftTrigger),
                "RightTrigger" => Ok(Button::RightTrigger),
                _ => Err("Invalid button for --button-to-touch=".to_string()),
            }?;
            let x: f32 = x
                .parse()
                .map_err(|_| "Invalid X co-ordinate for --button-to-touch=".to_string())?;
            let y: f32 = y
                .parse()
                .map_err(|_| "Invalid Y co-ordinate for --button-to-touch=".to_string())?;
            self.button_to_touch.insert(button, (x, y));
        } else if arg == "--stick-touch-start-at-center" {
            self.stick_touch_start_at_center = true;
        } else if let Some(values) = arg
            .strip_prefix("--right-stick-to-touch=")
            .or_else(|| arg.strip_prefix("--right-stick-swipe="))
        {
            let swipe = arg.starts_with("--right-stick-swipe=");
            let option_name = if swipe {
                "--right-stick-swipe"
            } else {
                "--right-stick-to-touch"
            };
            let nums = values
                .split(',')
                .map(str::parse::<f32>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| format!("invalid {option_name}"))?;
            if nums.len() != 4
                || nums.iter().any(|v| !v.is_finite())
                || nums[2] <= 0.0
                || nums[3] <= 0.0
            {
                return Err(format!(
                    "{option_name} requires X,Y,positive width,positive height"
                ));
            }
            let region = Some((nums[0], nums[1], nums[2], nums[3]));
            if swipe {
                self.right_stick_swipe = region;
                self.right_stick_to_touch = None;
            } else {
                self.right_stick_to_touch = region;
                self.right_stick_swipe = None;
            }
        } else if let Some(values) = arg.strip_prefix("--stick-to-touch=") {
            let nums: [f32; 4] = values
                .split(',')
                .map(|s| s.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid --stick-to-touch".to_string())?
                .try_into()
                .map_err(|_| "--stick-to-touch= requires four values".to_string())?;

            self.stick_to_touch = Some((nums[0], nums[1], nums[2], nums[3]));
        } else if let Some(values) = arg.strip_prefix("--dpad-to-touch=") {
            let nums: [f32; 4] = values
                .split(',')
                .map(|s| s.parse::<f32>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "invalid --dpad-to-touch".to_string())?
                .try_into()
                .map_err(|_| "--dpad-to-touch= requires four values".to_string())?;

            self.dpad_to_touch = Some((nums[0], nums[1], nums[2], nums[3]));
        } else if let Some(value) = arg.strip_prefix("--stabilize-virtual-cursor=") {
            let (smoothing_strength, sticky_radius) = value
                .split_once(',')
                .ok_or_else(|| "--stabilize-virtual-cursor= requires two values".to_string())?;
            let smoothing_strength: f32 = smoothing_strength
                .parse()
                .ok()
                .filter(|&s| s >= 0.0)
                .ok_or_else(|| {
                    "Invalid smoothing strength for --stabilize-virtual-cursor=".to_string()
                })?;
            let sticky_radius: f32 = sticky_radius
                .parse()
                .ok()
                .filter(|&s| s >= 0.0)
                .ok_or_else(|| {
                    "Invalid sticky radius for --stabilize-virtual-cursor=".to_string()
                })?;
            self.stabilize_virtual_cursor = Some((smoothing_strength, sticky_radius));
        } else if let Some(value) = arg.strip_prefix("--gles1=") {
            self.gles1_implementation = Some(
                GLESImplementation::from_short_name(value)
                    .map_err(|_| "Unrecognized --gles1= value".to_string())?,
            );
        } else if arg == "--disable-direct-memory-access" {
            self.direct_memory_access = false;
        } else if let Some(address) = arg.strip_prefix("--gdb=") {
            let addrs = address
                .to_socket_addrs()
                .map_err(|e| format!("Could not resolve GDB server listen address: {e}"))?
                .collect();
            self.gdb_listen_addrs = Some(addrs);
        } else if let Some(value) = arg.strip_prefix("--preferred-languages=") {
            self.preferred_languages = Some(value.split(',').map(ToOwned::to_owned).collect());
        } else if arg == "--headless" {
            self.headless = true;
            // Can't show the dialog box when headless!
            self.popup_errors = false;
        } else if let Some(mode) = arg.strip_prefix("--audio-test=") {
            self.audio_test = Some(crate::audio::sound_test::Mode::parse(mode)?);
        } else if let Some(mode) = arg.strip_prefix("--performance-test=") {
            self.performance_test = Some(crate::performance::Mode::parse(mode)?);
        } else if arg == "--print-fps" {
            self.print_fps = true;
        } else if let Some(value) = arg.strip_prefix("--fps-limit=") {
            if value == "off" {
                self.fps_limit = None;
            } else {
                let limit: f64 = value
                    .parse()
                    .ok()
                    .filter(|&v| v > 0.0)
                    .ok_or_else(|| "Invalid value for --fps-limit=".to_string())?;
                self.fps_limit = Some(limit);
            }
        } else if arg == "--force-composition" {
            self.force_composition = true;
        } else if arg == "--mhp2g-save-research" {
            self.mhp2g_save_research = true;
        } else if arg == "--network-trace" || arg == "--network-trace=on" {
            self.network_trace = true;
        } else if arg == "--network-trace=off" {
            self.network_trace = false;
        } else if arg.starts_with("--network-trace=") {
            return Err("Invalid --network-trace= (expected on or off)".to_string());
        } else if let Some(value) = arg.strip_prefix("--network-trace-limit=") {
            self.network_trace_limit = value
                .parse()
                .ok()
                .filter(|v| *v > 0)
                .ok_or_else(|| "Invalid --network-trace-limit=".to_string())?;
        } else if arg == "--allow-network-access" {
            self.network_access = true;
        } else if let Some(value) = arg.strip_prefix("--gamekit-host=") {
            if self.gamekit_discover.is_some()
                || matches!(self.gamekit_role, Some(crate::transport::Role::Client(_)))
            {
                return Err("Conflicting GameKit roles".into());
            }
            self.gamekit_bind = value.parse().map_err(|_| "GameKit Host needs IPv4:port")?;
            self.gamekit_role = Some(crate::transport::Role::Host);
            self.gamekit_auto = false;
        } else if let Some(value) = arg.strip_prefix("--gamekit-join=") {
            if self.gamekit_discover.is_some()
                || matches!(self.gamekit_role, Some(crate::transport::Role::Host))
            {
                return Err("Conflicting GameKit roles".into());
            }
            self.gamekit_role = Some(crate::transport::Role::Client(crate::lan::ipv4_endpoint(
                value,
            )?));
            self.gamekit_auto = false;
        } else if let Some(value) = arg.strip_prefix("--gamekit-room=") {
            if value.is_empty() || value.len() > 96 || value.chars().any(char::is_control) {
                return Err(
                    "GameKit room name must be 1..96 UTF-8 bytes without control characters".into(),
                );
            }
            self.gamekit_room = Some(value.into());
        } else if let Some(value) = arg.strip_prefix("--gamekit-discover=") {
            if self.gamekit_role.is_some() {
                return Err("Conflicting GameKit roles".into());
            }
            if value != "auto"
                && uuid::Uuid::parse_str(value)
                    .ok()
                    .is_none_or(|id| id.is_nil())
            {
                return Err("GameKit discovery needs auto or a nonzero room UUID".into());
            }
            self.gamekit_discover = Some(value.into());
            self.gamekit_auto = false;
        } else if arg == "--gamekit-auto" {
            // Options from files are applied before and after command-line
            // ones, so an explicit role must win whichever comes first.
            self.gamekit_auto = self.gamekit_role.is_none() && self.gamekit_discover.is_none();
        } else if let Some(value) = arg.strip_prefix("--gamekit-discovery-bind=") {
            let address: std::net::SocketAddrV4 = value
                .parse()
                .map_err(|_| "GameKit discovery needs IPv4:port")?;
            if address.port() == 0 {
                return Err("Discovery port cannot be zero".into());
            }
            self.gamekit_discovery_bind = address;
        } else if let Some(value) = arg.strip_prefix("--gamekit-broadcast=") {
            let address: std::net::SocketAddrV4 = value
                .parse()
                .map_err(|_| "GameKit broadcast needs IPv4:port")?;
            if address.port() == 0 || self.gamekit_broadcasts.len() >= 16 {
                return Err("Invalid GameKit broadcast target or too many targets".into());
            }
            self.gamekit_broadcasts.push(address);
        } else if let Some(value) = arg.strip_prefix("--gamekit-bind=") {
            self.gamekit_bind = value.parse().map_err(|_| "GameKit bind needs IPv4:port")?;
        } else if let Some(value) = arg.strip_prefix("--gamekit-player-id=") {
            let uuid = uuid::Uuid::parse_str(value).map_err(|_| "Invalid GameKit player UUID")?;
            if uuid.is_nil() {
                return Err("Zero GameKit identity".into());
            }
            self.gamekit_player_id = Some(crate::transport::PlayerId(*uuid.as_bytes()));
        } else if let Some(value) = arg.strip_prefix("--gamekit-timeout-ms=") {
            let ms = value
                .parse::<u64>()
                .ok()
                .filter(|v| (100..=300_000).contains(v))
                .ok_or("GameKit timeout must be 100..300000 ms")?;
            self.gamekit_timeout = std::time::Duration::from_millis(ms);
        } else if arg == "--no-error-popup" {
            self.popup_errors = false;
        } else if let Some(values) = arg.strip_prefix("--dump=") {
            self.dumping_options = parse_dump_options(values)?;
        } else if let Some(path) = arg.strip_prefix("--dump-file=") {
            self.dumping_file = crate::paths::user_data_base_path().join(path);
        } else if arg == "--ignore-gl-errors" {
            self.ignore_gl_errors = true;
        } else if arg == "--trace-gl-errors" {
            self.trace_gl_errors = true;
        } else if arg == "--verbose-gles" {
            self.verbose_gles = true;
        } else if arg == "--prefer-gles2-context" {
            self.prefer_gles2_context = true;
        } else if arg == "--force-gles1-context" {
            self.force_gles1_context = true;
        } else if arg == "--fix-texture-min-filter" {
            self.fix_texture_min_filter = true;
            // The ES 1.1 backend reads this, as it has no access to the
            // options.
            std::env::set_var("TOUCHHLE_FIX_TEXTURE_MIN_FILTER", "1");
        } else if arg == "--no-fix-texture-min-filter" {
            self.fix_texture_min_filter = false;
            std::env::set_var("TOUCHHLE_FIX_TEXTURE_MIN_FILTER", "0");
        } else if arg == "--reverse-attrib-locations" {
            self.reverse_attrib_locations = true;
        } else if let Some(value) = arg.strip_prefix("--zero-stack-after-guest-to-host-call=") {
            self.zero_stack_after_guest_to_host_call = Some(value.parse().map_err(|_| {
                "Invalid value for --zero-stack-after-guest-to-host-call=".to_string()
            })?);
        } else {
            return Ok(false);
        };
        Ok(true)
    }
}

/// Try to get app-specific options from a file.
///
/// Returns [Ok] if there is no error when reading the file, otherwise [Err].
/// The [Ok] value is a [Some] with the options if they could be found, or
/// [None] if no options were found for this app.
pub fn get_options_from_file<F: Read>(file: F, app_id: &str) -> Result<Option<String>, String> {
    let file = BufReader::new(file);
    for (line_no, line) in BufRead::lines(file).enumerate() {
        // Line numbering usually starts from 1
        let line_no = line_no + 1;

        let line = line.map_err(|e| format!("Error while reading line {line_no}: {e}"))?;

        // # for single-line comments
        let line = if let Some((rest, _)) = line.split_once('#') {
            rest
        } else {
            &line
        };

        // Empty/all-comment lines ignored
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let (line_app_id, line_options) = line.split_once(':').ok_or_else(|| format!("Line {line_no} is not a comment and is missing a colon (:) to separate the app ID from the options"))?;
        let line_app_id = line_app_id.trim();

        if line_app_id != app_id {
            continue;
        }

        let line_options = line_options.trim();
        if line_options.is_empty() {
            return Ok(None);
        } else {
            return Ok(Some(line_options.to_string()));
        }
    }
    Ok(None)
}

#[derive(Default, Clone)]
pub struct DumpingOptions {
    pub linking_info: bool,
    pub symbols: bool,
}

impl DumpingOptions {
    /// Check if any of the dumping options are active.
    pub fn any(&self) -> bool {
        self.linking_info || self.symbols
    }
}

fn parse_dump_options(options: &str) -> Result<DumpingOptions, String> {
    let mut dumping_options = DumpingOptions::default();
    for opt in options.split(",") {
        if opt == "linking-info" {
            // Dumps linked symbols, classes and selectors for the given app
            dumping_options.linking_info = true;
        } else if opt == "symbols" {
            // Dumps touchHLE provided symbols and exits
            dumping_options.symbols = true;
        } else {
            return Err(format!("Unrecognized option {opt} for --dump=..."));
        }
    }
    Ok(dumping_options)
}

#[cfg(test)]
mod controller_mapping_tests {
    use super::{Button, Options};

    #[test]
    fn local_gamekit_is_explicit_and_never_enables_network_permission() {
        let mut options = Options::default();
        assert!(options.gamekit_role.is_none());
        options
            .parse_argument("--gamekit-host=127.0.0.1:0")
            .unwrap();
        assert!(!options.network_access);
        assert!(matches!(
            options.gamekit_role,
            Some(crate::transport::Role::Host)
        ));
        assert!(options
            .parse_argument("--gamekit-join=127.0.0.1:1234")
            .is_err());
        assert!(options.parse_argument("--gamekit-timeout-ms=0").is_err());
        assert!(options
            .parse_argument("--gamekit-player-id=00000000000000000000000000000000")
            .is_err());
    }

    #[test]
    fn gamekit_room_modes_are_explicit_and_mutually_exclusive() {
        let mut options = Options::default();
        options.parse_argument("--gamekit-discover=auto").unwrap();
        assert!(!options.network_access);
        assert!(options.gamekit_role.is_none());
        assert!(options
            .parse_argument("--gamekit-host=127.0.0.1:0")
            .is_err());
        assert!(options
            .parse_argument("--gamekit-join=127.0.0.1:42018")
            .is_err());
        assert!(options.parse_argument("--gamekit-discover=bad").is_err());
        assert!(options.parse_argument("--gamekit-room=").is_err());
        assert!(options
            .parse_argument("--gamekit-discovery-bind=127.0.0.1:0")
            .is_err());
        options
            .parse_argument("--gamekit-broadcast=127.0.0.1:42019")
            .unwrap();
        options.parse_argument("--gamekit-room=Test Room").unwrap();
        let mut host = Options::default();
        host.parse_argument("--gamekit-host=127.0.0.1:0").unwrap();
        assert!(host.parse_argument("--gamekit-discover=auto").is_err());
    }

    #[test]
    fn network_trace_is_opt_in_and_independent_from_network_access() {
        let mut options = Options::default();
        assert!(!options.network_trace);
        options.parse_argument("--network-trace").unwrap();
        assert!(options.network_trace);
        assert!(!options.network_access);
        options.parse_argument("--network-trace=off").unwrap();
        assert!(!options.network_trace);
        options.parse_argument("--network-trace=on").unwrap();
        options.parse_argument("--network-trace-limit=7").unwrap();
        assert_eq!(options.network_trace_limit, 7);
        for argument in [
            "--network-trace-limit=0",
            "--network-trace-limit=-1",
            "--network-trace-limit=abc",
            "--network-trace=maybe",
        ] {
            assert!(options.parse_argument(argument).is_err());
        }
    }

    #[test]
    fn gta_gestures_are_opt_in_and_camera_modes_are_exclusive() {
        let mut options = Options::default();
        assert!(!options.stick_touch_start_at_center);
        assert_eq!(options.right_stick_swipe, None);
        options
            .parse_argument("--stick-touch-start-at-center")
            .unwrap();
        assert!(options.stick_touch_start_at_center);
        options
            .parse_argument("--right-stick-to-touch=1,2,3,4")
            .unwrap();
        options
            .parse_argument("--right-stick-swipe=180,100,130,100")
            .unwrap();
        assert_eq!(
            options.right_stick_swipe,
            Some((180.0, 100.0, 130.0, 100.0))
        );
        assert_eq!(options.right_stick_to_touch, None);
        options
            .parse_argument("--right-stick-to-touch=1,2,3,4")
            .unwrap();
        assert_eq!(options.right_stick_swipe, None);
        for invalid in [
            "0,0,0,10",
            "0,0,10,-1",
            "NaN,0,10,10",
            "0,0,10,inf",
            "0,0,10",
        ] {
            assert!(options
                .parse_argument(&format!("--right-stick-swipe={invalid}"))
                .is_err());
        }
    }

    #[test]
    fn extended_buttons_and_camera_region_parse() {
        let mut options = Options::default();
        for (name, button) in [
            ("RightShoulder", Button::RightShoulder),
            ("Back", Button::Back),
            ("LeftStick", Button::LeftStick),
            ("RightStick", Button::RightStick),
            ("LeftTrigger", Button::LeftTrigger),
            ("RightTrigger", Button::RightTrigger),
        ] {
            options
                .parse_argument(&format!("--button-to-touch={name},10,20"))
                .unwrap();
            assert_eq!(options.button_to_touch.get(&button), Some(&(10.0, 20.0)));
        }
        options
            .parse_argument("--right-stick-to-touch=200,50,100,80")
            .unwrap();
        assert_eq!(
            options.right_stick_to_touch,
            Some((200.0, 50.0, 100.0, 80.0))
        );
        for invalid in ["0,0,0,10", "0,0,10,-1", "NaN,0,10,10", "0,0,10"] {
            assert!(options
                .parse_argument(&format!("--right-stick-to-touch={invalid}"))
                .is_err());
        }
    }
}

#[cfg(test)]
mod system_version_tests {
    use super::Options;

    #[test]
    fn accepts_one_to_three_u16_components() {
        let mut options = Options::default();
        assert_eq!(options.system_version, "2.0");
        for version in ["7", "7.0", "7.0.1", "0", "65535.65535.65535", "07.00"] {
            assert!(options
                .parse_argument(&format!("--system-version={version}"))
                .unwrap());
            assert_eq!(options.system_version, version);
        }
    }

    #[test]
    fn rejects_invalid_versions_without_changing_previous_value() {
        let mut options = Options::default();
        options.parse_argument("--system-version=7.0").unwrap();
        for version in [
            "", ".7", "7.", "7..0", "1.2.3.4", "65536", "7.65536", "-1", "+7", "7.a", "7. 0", " 7",
            "7\n", "７.０",
        ] {
            assert!(
                options
                    .parse_argument(&format!("--system-version={version}"))
                    .is_err(),
                "accepted {version:?}"
            );
            assert_eq!(options.system_version, "7.0");
        }
    }
}

#[cfg(test)]
mod device_geometry_options_tests {
    use super::Options;

    #[test]
    fn profile_selection_does_not_resolve_runtime_geometry() {
        let mut options = Options::default();
        assert_eq!(options.device_profile, "auto");
        assert_eq!(options.device_screen_size, None);
        options
            .parse_argument("--device-profile=iPhone2,1")
            .unwrap();
        assert_eq!(options.device_profile, "iPhone2,1");
        assert_eq!(options.device_screen_size, None);
        assert!(options.parse_argument("--device-profile=unknown").is_err());
        assert_eq!(options.device_profile, "iPhone2,1");
        options.parse_argument("--device-profile=auto").unwrap();
        assert_eq!(options.device_profile, "auto");
    }

    #[test]
    fn mapping_reference_requires_positive_integer_pair() {
        let mut options = Options::default();
        assert_eq!(options.touch_map_size, None);
        options.parse_argument("--touch-map-size=480,320").unwrap();
        assert_eq!(options.touch_map_size, Some((480, 320)));
        for invalid in [
            "",
            "480",
            "0,320",
            "480,-1",
            "480,320.5",
            "NaN,320",
            "480,inf",
            "480,320,1",
            "+480,320",
            "4294967296,320",
        ] {
            assert!(options
                .parse_argument(&format!("--touch-map-size={invalid}"))
                .is_err());
            assert_eq!(options.touch_map_size, Some((480, 320)));
        }
    }
}
/// Highest iOS version currently exposed by the emulator compatibility layer.
#[allow(dead_code)]
pub const LATEST_IOS_VERSION: (i32, i32, i32) = (12, 0, 0);
