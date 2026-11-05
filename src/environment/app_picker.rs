//! App picker GUI.
//!
//! This also includes a license text viewer. The license text viewer is needed
//! on Android, where the command-line way to view license text doesn't exist.

use crate::bundle::Bundle;
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextCreateImage,
};
use crate::frameworks::core_graphics::cg_color_space::CGColorSpaceCreateDeviceRGB;
use crate::frameworks::core_graphics::cg_context::{
    CGContextFillRect, CGContextRelease, CGContextScaleCTM, CGContextSetRGBFillColor,
    CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{self, kCGImageAlphaPremultipliedLast};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::uikit::ui_font::{
    UITextAlignment, UITextAlignmentCenter, UITextAlignmentLeft,
};
use crate::frameworks::uikit::ui_graphics::{UIGraphicsPopContext, UIGraphicsPushContext};
use crate::frameworks::uikit::ui_view::ui_control::ui_button::UIButtonTypeCustom;
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchUpInside, UIControlEventValueChanged, UIControlStateNormal,
};
use crate::fs::BundleData;
use crate::image::Image;
use crate::mem::Ptr;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject};
use crate::options::{FilterMode, Options};
use crate::paths;
use crate::window::DeviceOrientation;
use crate::Environment;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

struct AppInfo {
    path: PathBuf,
    display_name: String,
    icon: Option<Image>,
    /// `NSString*`
    display_name_ns_string: Option<id>,
    /// `UIImage*`
    icon_ui_image: Option<id>,
}

pub fn app_picker(options: Options) -> Result<(PathBuf, Vec<String>), String> {
    let apps = load_apps(&options);
    show_app_picker_gui(options, apps)
}

/// Find the apps in the apps directory. The error is a message to display.
fn load_apps(options: &Options) -> Result<Vec<AppInfo>, String> {
    // For localized app names.
    let languages = options
        .preferred_languages
        .clone()
        .unwrap_or_else(crate::window::system_language_codes);
    let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);

    if !apps_dir.is_dir() {
        Err(format!(
            "找不到 {} 文件夹。请确认 Podroid 是从正确的位置启动的。",
            apps_dir.display()
        ))
    } else {
        enumerate_apps(&apps_dir, &languages)
            .map_err(|err| {
                format!(
                    "无法读取 {} 文件夹里的应用列表：{}",
                    apps_dir.display(),
                    err
                )
            })
            .and_then(|apps| {
                if apps.is_empty() {
                    Err(format!(
                        "{} 文件夹里还没有应用。\n\n点击下方的「文件管理」，把 .ipa 文件或 .app 文件夹放进 Podroid_apps 文件夹即可。",
                        apps_dir.display()
                    ))
                } else {
                    Ok(apps)
                }
            })
    }
}

fn enumerate_apps(apps_dir: &Path, languages: &[String]) -> Result<Vec<AppInfo>, std::io::Error> {
    let mut apps = Vec::new();
    for app in std::fs::read_dir(apps_dir)? {
        let app_path = app?.path();
        if app_path.extension() != Some(OsStr::new("app")) {
            continue;
        }

        // TODO: avoid loading the whole FS somehow?
        let (bundle, fs) = match BundleData::open_any(&app_path).and_then(|bundle_data| {
            Bundle::new_bundle_and_fs_from_host_path(bundle_data, /* read_only_mode: */ true)
        }) {
            Ok(ok) => ok,
            Err(e) => {
                log!(
                    "Warning: couldn't open app bundle {}: {} (skipping)",
                    app_path.display(),
                    e
                );
                continue;
            }
        };

        // TODO: what if this crashes?
        let display_name = bundle.localized_display_name(&fs, languages);

        let icon = match bundle.load_icon(&fs) {
            Ok(icon) => Some(icon),
            Err(e) => {
                log!("Warning: couldn't load icon for app bundle {}: {} (displaying placeholder instead)", app_path.display(), e);
                None
            }
        };

        apps.push(AppInfo {
            path: app_path,
            display_name,
            icon,
            display_name_ns_string: None,
            icon_ui_image: None,
        });
    }

    apps.sort_by_key(|app| app.display_name.to_uppercase());

    Ok(apps)
}

#[derive(Default)]
struct AppPickerDelegateHostObject {
    icon_tapped: id,
    copyright_show: bool,
    copyright_hide: bool,
    copyright_prev: bool,
    copyright_next: bool,
    quick_options_show: bool,
    quick_options_hide: bool,
    scale_hack_default: bool,
    scale_hack1: bool,
    scale_hack2: bool,
    scale_hack3: bool,
    scale_hack4: bool,
    orientation_default: bool,
    orientation_portrait_upside_down: bool,
    orientation_landscape_left: bool,
    orientation_landscape_right: bool,
    analog_stick_tilt_controls: Option<bool>,
    network: Option<bool>,
    keep_screen_on: Option<bool>,
    fullscreen: Option<bool>,
    /// Button tapped among the quick options that use `quickOptionTapped:`.
    option_tapped: id,
    /// Set when touchHLE returns from the background (e.g. after using the
    /// file manager), so the app list can be reloaded.
    became_active: bool,
}
impl HostObject for AppPickerDelegateHostObject {}

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    // Not a real iOS dylib obviously. This shouldn't really be in the list of
    // dylibs if we can avoid it somehow (TODO?).
    path: "/.touchHLE/AppPickerHelpers.dylib",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

/// Be careful! These classes go in the normal class list, just like everything
/// else, so an app could try to instantiate them. Don't give them special
/// powers that could be exploited!
const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _Podroid_AppPickerDelegate: NSObject

- (())iconTapped:(id)sender {
    // There is no allocWithZone: that creates AppPickerDelegateHostObject, so
    // this downcast effectively acts as an assertion that this class is being
    // used within the app picker, so it can't be abused. :)
    let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    host_obj.icon_tapped = sender;
}

- (())copyrightInfoShow {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_show = true;
}
- (())copyrightInfoHide {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_hide = true;
}
- (())copyrightInfoPrevPage {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_prev = true;
}
- (())copyrightInfoNextPage {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_next = true;
}

- (())quickOptionsShow {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).quick_options_show = true;
}
- (())quickOptionsHide {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).quick_options_hide = true;
}
- (())scaleHackDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack_default = true;
}
- (())scaleHack1 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack1 = true;
}
- (())scaleHack2 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack2 = true;
}
- (())scaleHack3 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack3 = true;
}
- (())scaleHack4 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack4 = true;
}
- (())orientationDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_default = true;
}
- (())orientationPortraitUpsideDown {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_portrait_upside_down = true;
}
- (())orientationLandscapeLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_left = true;
}
- (())orientationLandscapeRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_right = true;
}
- (())analogStickTiltControls:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).analog_stick_tilt_controls = Some(switch_state);
}
- (())network:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).network = Some(switch_state);
}
- (())keepScreenOn:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).keep_screen_on = Some(switch_state);
}
- (())fullscreen:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fullscreen = Some(switch_state);
}

- (())quickOptionTapped:(id)sender { // UIButton*
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).option_tapped = sender;
}

- (())applicationDidBecomeActive:(id)_application {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).became_active = true;
}

- (())openFileManager {
    // Assert (see above).
    let _ = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);

    match paths::url_for_opening_user_data_dir() {
        Ok(url) => {
            // Our `openURL:` implementation is bypassed because it doesn't
            // allow non-web URLs.
            let url_res = crate::window::open_url(env, &url);
            if let Err(e) = url_res {
                echo!("Couldn't open file manager at {:?}: {}", url, e);
            } else if crate::window::Window::rotatable_fullscreen() {
                // On Android, touchHLE is paused while the file manager is
                // open, and the app list is reloaded when it returns.
                echo!("Opened file manager at {:?}.", url);
            } else {
                // Elsewhere touchHLE stays in the foreground, so there's no
                // good moment to reload the app list; exit so it's fresh
                // next time.
                echo!("Opened file manager at {:?}, exiting.", url);
                std::process::exit(0);
            }
        },
        Err(e) => echo!("Couldn't open file manager: {}", e),
    }
}

- (())visitWebsite {
    // Assert (see above).
    let _ = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);

    let url = ns_string::get_static_str(env, "https://touchhle.org/");
    let url: id = msg_class![env; NSURL URLWithString:url];
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    assert!(msg![env; ui_application openURL:url]);
}

@end

};

fn show_app_picker_gui(
    options: Options,
    apps: Result<Vec<AppInfo>, String>,
) -> Result<(PathBuf, Vec<String>), String> {
    let icon = {
        let bytes: &[u8] = match crate::branding() {
            "" => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon.png")),
            "UF" => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_uf.png")),
            "PREVIEW" => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_preview.png"))
            }
            _ => panic!(),
        };
        let mut image = Image::from_bytes(bytes).unwrap();
        // should match Bundle::load_icon()
        image.round_corners(
            (10.0 / 57.0) * (image.dimensions().0 as f32),
            /* four_corners: */ true,
            /* add_sheen: */ true,
        );
        image
    };
    let environment = Environment::new_without_app(options, icon)?;
    Ok(environment.run_app_picker(|env| app_picker_inner(env, apps)))
}

fn app_picker_inner(
    env: &mut Environment,
    mut apps: Result<Vec<AppInfo>, String>,
) -> (PathBuf, Vec<String>) {
    let mut option_args = Vec::new();
    // Note that objects are generally not released in this code, because they
    // don't need to be: the entire Environment is thrown away at the end.

    // Bypassing UIApplicationMain!
    let ui_application: id = msg_class![env; UIApplication new];
    let delegate = env
        .objc
        .get_known_class("_Podroid_AppPickerDelegate", &mut env.mem);
    let delegate = env.objc.alloc_object(
        delegate,
        Box::<AppPickerDelegateHostObject>::default(),
        &mut env.mem,
    );
    () = msg![env; ui_application setDelegate:delegate];

    // Use the whole screen, the status bar would just be an empty black bar.
    () = msg![env; ui_application setStatusBarHidden:true];

    let screen: id = msg_class![env; UIScreen mainScreen];
    let bounds: CGRect = msg![env; screen bounds];

    let window: id = msg_class![env; UIWindow alloc];
    let window: id = msg![env; window initWithFrame:bounds];

    let app_frame: CGRect = msg![env; screen applicationFrame];
    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:app_frame];
    () = msg![env; window addSubview:main_view];

    // Wallpaper: the user's own if there is one, otherwise the built-in
    // background.
    let mut user_wallpaper = None;
    for candidate in paths::WALLPAPER_FILES {
        let candidate = paths::user_data_base_path().join(candidate);
        if !candidate.exists() {
            continue;
        }
        match std::fs::read(&candidate)
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                Image::from_bytes(&bytes).map_err(|e| format!("couldn't decode: {}", e))
            }) {
            Ok(image) => user_wallpaper = Some(image),
            Err(e) => {
                log!("Warning: couldn't read {}: {}", candidate.display(), e);
            }
        }
        break;
    }
    if user_wallpaper.is_none() {
        let CGSize { width, height } = app_frame.size;
        log!(
            "No wallpaper found; filename can be one of: {}; ideal size is {}×{} pixels",
            paths::WALLPAPER_FILES.join(", "),
            width,
            height,
        );
    }
    {
        let is_user_wallpaper = user_wallpaper.is_some();
        let image = user_wallpaper.unwrap_or_else(|| {
            Image::from_bytes(include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/res/picker_background.png"
            )))
            .unwrap()
        });
        let image = cg_image::from_image(env, image);
        let image: id = msg_class![env; UIImage imageWithCGImage:image];
        let wallpaper: id = msg_class![env; UIImageView alloc];
        let wallpaper: id = msg![env; wallpaper initWithImage:image];
        () = msg![env; wallpaper setFrame:(CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: app_frame.size,
        })];
        if is_user_wallpaper {
            () = msg![env; wallpaper setAlpha:(0.5 as CGFloat)];
        }
        () = msg![env; main_view addSubview:wallpaper];
    }
    let have_wallpaper = true;

    // Header: icon, name and a hint
    {
        let icon_bytes: &[u8] = match crate::branding() {
            "UF" => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_uf.png")),
            "PREVIEW" => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_preview.png"))
            }
            _ => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon.png")),
        };
        let mut icon = Image::from_bytes(icon_bytes).unwrap();
        icon.round_corners(
            (10.0 / 57.0) * (icon.dimensions().0 as f32),
            /* four_corners: */ true,
            /* add_sheen: */ false,
        );
        let icon = cg_image::from_image(env, icon);
        let icon: id = msg_class![env; UIImage imageWithCGImage:icon];
        let icon_view: id = msg_class![env; UIImageView alloc];
        let icon_view: id = msg![env; icon_view initWithImage:icon];
        () = msg![env; icon_view setFrame:(CGRect {
            origin: CGPoint { x: 14.0, y: 16.0 },
            size: CGSize { width: 36.0, height: 36.0 },
        })];
        () = msg![env; main_view addSubview:icon_view];

        let title = "Podroid".to_string();
        add_label(
            env,
            main_view,
            (58.0, 14.0, 248.0, 24.0),
            &title,
            19.0,
            true,
            (1.0, 1.0, 1.0, 1.0),
            UITextAlignmentLeft,
        );
        add_label(
            env,
            main_view,
            (58.0, 38.0, 248.0, 16.0),
            "点击图标启动游戏",
            11.0,
            false,
            SUBTLE_TEXT_COLOR,
            UITextAlignmentLeft,
        );

        let divider: id = msg_class![env; UIView alloc];
        let divider: id = msg![env; divider initWithFrame:(CGRect {
            origin: CGPoint { x: 14.0, y: 64.0 },
            size: CGSize { width: app_frame.size.width - 28.0, height: 1.0 },
        })];
        let color: id =
            msg_class![env; UIColor colorWithWhite:(1.0 as CGFloat) alpha:(0.2 as CGFloat)];
        () = msg![env; divider setBackgroundColor:color];
        () = msg![env; main_view addSubview:divider];
    }

    // If touchHLE crashed last time, say where to find the log, which is
    // needed to find out why (and is easily lost by running touchHLE again).
    if previous_run_crashed() {
        // Between the header and the icons, where it can't overlap anything.
        add_label(
            env,
            main_view,
            (8.0, 66.0, app_frame.size.width - 16.0, 13.0),
            "上次运行出错退出 · 日志：文件管理 › Podroid_log_previous.txt",
            9.0,
            false,
            WARNING_TEXT_COLOR,
            UITextAlignmentCenter,
        );
    }

    // Version label
    {
        let edition = match crate::branding() {
            "UF" => " · 非官方版本",
            "PREVIEW" => " · 预览版",
            _ => "",
        };
        let text = format!("{}{}", crate::VERSION, edition);
        add_label(
            env,
            main_view,
            (
                0.0,
                app_frame.size.height - 20.0,
                app_frame.size.width,
                14.0,
            ),
            &text,
            10.0,
            false,
            SUBTLE_TEXT_COLOR,
            UITextAlignmentCenter,
        );
    }

    let mut icon_grid_stuff = make_icon_grid(
        env,
        delegate,
        main_view,
        app_frame,
        apps.as_ref().map_or(0, |apps| apps.len()),
        have_wallpaper,
    );
    let message_label = {
        let label_frame = CGRect {
            origin: CGPoint { x: 20.0, y: 80.0 },
            size: CGSize {
                width: app_frame.size.width - 40.0,
                height: app_frame.size.height - 128.0 - 90.0,
            },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        () = msg![env; label setNumberOfLines:0]; // unlimited
        let text_color: id = msg_class![env; UIColor whiteColor];
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];
        label
    };
    show_apps(env, &mut icon_grid_stuff, message_label, &mut apps);

    // Buttons, in a translucent card at the bottom
    {
        let card_frame = CGRect {
            origin: CGPoint {
                x: 10.0,
                y: app_frame.size.height - 128.0,
            },
            size: CGSize {
                width: app_frame.size.width - 20.0,
                height: 100.0,
            },
        };
        let card: id = msg_class![env; UIView alloc];
        let card: id = msg![env; card initWithFrame:card_frame];
        let color: id =
            msg_class![env; UIColor colorWithWhite:(0.0 as CGFloat) alpha:(0.27 as CGFloat)];
        () = msg![env; card setBackgroundColor:color];
        let layer: id = msg![env; card layer];
        () = msg![env; layer setCornerRadius:(14.0 as CGFloat)];
        () = msg![env; main_view addSubview:card];

        let buttons = [
            ("文件管理", "openFileManager"),
            ("快捷设置", "quickOptionsShow"),
            ("版权信息", "copyrightInfoShow"),
            ("官方网站", "visitWebsite"),
        ];
        let gap = 8.0;
        let size = CGSize {
            width: (card_frame.size.width - gap * 3.0) / 2.0,
            height: (card_frame.size.height - gap * 3.0) / 2.0,
        };
        for (i, (title, selector)) in buttons.into_iter().enumerate() {
            let frame = CGRect {
                origin: CGPoint {
                    x: card_frame.origin.x + gap + (i % 2) as CGFloat * (size.width + gap),
                    y: card_frame.origin.y + gap + (i / 2) as CGFloat * (size.height + gap),
                },
                size,
            };
            make_styled_button(env, delegate, main_view, frame, title, selector, 14.0);
        }
    }

    let copyright_info_text = crate::licenses::get_text();
    let mut copyright_info_stuff = setup_copyright_info(env, delegate, main_view, app_frame);
    let mut copyright_info_page_idx = 0;

    let quick_options_stuff = setup_quick_options(env, delegate, main_view, app_frame);
    let mut quick_options_scale_hack: Option<NonZeroU32> = None;
    let mut quick_options_fullscreen: Option<()> = None;
    let mut quick_options_orientation: Option<DeviceOrientation> = None;
    let mut quick_options_aspect: usize = 0;
    let mut quick_options_filter: usize = QUICK_FILTER_VALUES
        .iter()
        .position(|&x| x == Some(FilterMode::DEFAULT.name()))
        .unwrap();
    let mut quick_options_analog_stick_tilt_controls = true;
    let mut quick_options_network = false;
    let mut quick_options_keep_screen_on = crate::options::KEEP_SCREEN_ON_DEFAULT;

    // Restore the quick options chosen last time (saved when an app is
    // launched), so they don't have to be chosen again every time.
    let quick_options_file = paths::user_data_base_path().join(QUICK_OPTIONS_FILE);
    if let Ok(saved) = std::fs::read_to_string(&quick_options_file) {
        for arg in saved.lines().map(str::trim) {
            if let Some(v) = arg.strip_prefix("--scale-hack=") {
                quick_options_scale_hack = v.parse().ok().filter(|v: &NonZeroU32| v.get() <= 4);
            } else if let Some(v) = arg.strip_prefix("--aspect=") {
                quick_options_aspect = QUICK_ASPECT_VALUES
                    .iter()
                    .position(|&x| x == Some(v))
                    .unwrap_or(0);
            } else if let Some(v) = arg.strip_prefix("--filter=") {
                quick_options_filter = QUICK_FILTER_VALUES
                    .iter()
                    .position(|&x| x == Some(v))
                    .unwrap_or(0);
            } else {
                match arg {
                    "--landscape-left" => {
                        quick_options_orientation = Some(DeviceOrientation::LandscapeLeft)
                    }
                    "--landscape-right" => {
                        quick_options_orientation = Some(DeviceOrientation::LandscapeRight)
                    }
                    "--upside-down" => {
                        quick_options_orientation = Some(DeviceOrientation::PortraitUpsideDown)
                    }
                    "--fullscreen" => quick_options_fullscreen = Some(()),
                    "--disable-analog-stick-tilt-controls" => {
                        quick_options_analog_stick_tilt_controls = false
                    }
                    "--allow-network-access" => quick_options_network = true,
                    "--keep-screen-on" => quick_options_keep_screen_on = true,
                    "--no-keep-screen-on" => quick_options_keep_screen_on = false,
                    _ => (),
                }
            }
        }
    }
    for &(selector, switch) in &quick_options_stuff.switches {
        let on = match selector {
            "network:" => quick_options_network,
            "keepScreenOn:" => quick_options_keep_screen_on,
            "analogStickTiltControls:" => quick_options_analog_stick_tilt_controls,
            "fullscreen:" => quick_options_fullscreen.is_some(),
            _ => continue,
        };
        () = msg![env; switch setOn:on];
    }

    fn update_quick_option_buttons(env: &mut Environment, buttons: &[id], selected_idx: usize) {
        for (idx, &button) in buttons.iter().enumerate() {
            let color: id = if idx == selected_idx {
                msg_class![env; UIColor colorWithRed:(0.2 as CGFloat)
                                               green:(0.62 as CGFloat)
                                                blue:(0.86 as CGFloat)
                                               alpha:(1.0 as CGFloat)]
            } else {
                msg_class![env; UIColor colorWithWhite:(1.0 as CGFloat) alpha:(0.15 as CGFloat)]
            };
            () = msg![env; button setBackgroundColor:color];
        }
    }
    fn update_scale_hack_buttons(env: &mut Environment, buttons: &[id], value: Option<NonZeroU32>) {
        update_quick_option_buttons(env, buttons, value.map_or(0, |v| v.get() as usize));
    }
    fn update_orientation_buttons(
        env: &mut Environment,
        buttons: &[id],
        value: Option<DeviceOrientation>,
    ) {
        update_quick_option_buttons(
            env,
            buttons,
            value.map_or(0, |v| match v {
                DeviceOrientation::LandscapeLeft => 1,
                DeviceOrientation::LandscapeRight => 2,
                DeviceOrientation::PortraitUpsideDown => 3,
                _ => panic!(),
            }),
        );
    }
    update_scale_hack_buttons(
        env,
        &quick_options_stuff.scale_hack_buttons,
        quick_options_scale_hack,
    );
    update_orientation_buttons(
        env,
        &quick_options_stuff.orientation_buttons,
        quick_options_orientation,
    );

    update_quick_option_buttons(
        env,
        &quick_options_stuff.aspect_buttons,
        quick_options_aspect,
    );
    update_quick_option_buttons(
        env,
        &quick_options_stuff.filter_buttons,
        quick_options_filter,
    );

    () = msg![env; window makeKeyAndVisible];

    let main_run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    // If an app is picked, this loop returns. If the user quits touchHLE, the
    // process exits.
    let app_path = loop {
        run_run_loop_single_iteration(env, main_run_loop);
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        if std::mem::take(&mut host_obj.became_active) {
            log!("Reloading app list.");
            // This uses Android APIs via JNI (e.g. to get the system
            // languages), which crashes if done on a coroutine's stack.
            apps = env.on_parent_stack_in_coroutine(|_, options| load_apps(options));
            show_apps(env, &mut icon_grid_stuff, message_label, &mut apps);
            continue;
        }
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        let option_tapped = std::mem::take(&mut host_obj.option_tapped);
        if option_tapped != nil {
            if let Some(i) = quick_options_stuff
                .aspect_buttons
                .iter()
                .position(|&b| b == option_tapped)
            {
                quick_options_aspect = i;
                update_quick_option_buttons(env, &quick_options_stuff.aspect_buttons, i);
            } else if let Some(i) = quick_options_stuff
                .filter_buttons
                .iter()
                .position(|&b| b == option_tapped)
            {
                quick_options_filter = i;
                update_quick_option_buttons(env, &quick_options_stuff.filter_buttons, i);
            }
            continue;
        }
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        let icon_tapped = std::mem::take(&mut host_obj.icon_tapped);
        if icon_tapped != nil {
            match icon_grid_stuff.icon_map.get(&icon_tapped) {
                Some(&TappedIcon::App(app_idx)) => {
                    // Provide visual feedback that the app has been picked
                    // (it may take a while for the splash screen to appear etc)
                    () = msg![env; icon_tapped setAlpha:(0.5 as CGFloat)];
                    // Redraw screen, even if this makes the next frame early
                    // (the app picker will never be redrawn after this).
                    crate::frameworks::core_animation::recomposite_if_necessary(
                        env, /* force: */ true,
                    );
                    // Ensure touchHLE is responsive from the OS perspective,
                    // otherwise screen redraw might not show up? (Unclear if
                    // this explanation is correct.)
                    run_run_loop_single_iteration(env, main_run_loop);

                    let app_path = &apps.as_ref().unwrap()[app_idx].path;
                    echo!("Picked: {}", app_path.display());
                    break app_path.clone();
                }
                Some(&TappedIcon::ChangePage(page_idx)) => {
                    update_icon_grid(env, &mut icon_grid_stuff, apps.as_mut().unwrap(), page_idx);
                }
                None => (), // Tapped on a black space
            }
            continue;
        }
        if std::mem::take(&mut host_obj.copyright_show) {
            copyright_info_page_idx = 0;
            change_copyright_page(
                env,
                &mut copyright_info_stuff,
                &copyright_info_text,
                copyright_info_page_idx,
            );
            () = msg![env; (copyright_info_stuff.main_view) setHidden:false];
        } else if std::mem::take(&mut host_obj.copyright_hide) {
            () = msg![env; (copyright_info_stuff.main_view) setHidden:true];
        } else if std::mem::take(&mut host_obj.copyright_prev) && copyright_info_page_idx != 0 {
            copyright_info_page_idx -= 1;
            change_copyright_page(
                env,
                &mut copyright_info_stuff,
                &copyright_info_text,
                copyright_info_page_idx,
            );
        } else if std::mem::take(&mut host_obj.copyright_next)
            && Some(copyright_info_page_idx) != copyright_info_stuff.last_page_idx
        {
            copyright_info_page_idx += 1;
            change_copyright_page(
                env,
                &mut copyright_info_stuff,
                &copyright_info_text,
                copyright_info_page_idx,
            );
        } else if std::mem::take(&mut host_obj.quick_options_show) {
            () = msg![env; (quick_options_stuff.main_view) setHidden:false];
        } else if std::mem::take(&mut host_obj.quick_options_hide) {
            () = msg![env; (quick_options_stuff.main_view) setHidden:true];
        } else if std::mem::take(&mut host_obj.scale_hack_default) {
            quick_options_scale_hack = None;
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack1) {
            quick_options_scale_hack = Some(NonZeroU32::new(1).unwrap());
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack2) {
            quick_options_scale_hack = Some(NonZeroU32::new(2).unwrap());
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack3) {
            quick_options_scale_hack = Some(NonZeroU32::new(3).unwrap());
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack4) {
            quick_options_scale_hack = Some(NonZeroU32::new(4).unwrap());
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.orientation_default) {
            quick_options_orientation = None;
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_portrait_upside_down) {
            quick_options_orientation = Some(DeviceOrientation::PortraitUpsideDown);
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_left) {
            quick_options_orientation = Some(DeviceOrientation::LandscapeLeft);
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_right) {
            quick_options_orientation = Some(DeviceOrientation::LandscapeRight);
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if let Some(enabled) = std::mem::take(&mut host_obj.analog_stick_tilt_controls) {
            quick_options_analog_stick_tilt_controls = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.network) {
            quick_options_network = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.keep_screen_on) {
            quick_options_keep_screen_on = enabled;
        } else if let Some(fullscreen) = std::mem::take(&mut host_obj.fullscreen) {
            quick_options_fullscreen = match fullscreen {
                false => None,
                true => Some(()),
            };
        }
    };

    // Apply user-specified overrides
    if let Some(scale_hack) = quick_options_scale_hack {
        option_args.push(format!("--scale-hack={}", scale_hack.get()));
    }
    if let Some(orientation) = quick_options_orientation {
        option_args.push(
            match orientation {
                DeviceOrientation::LandscapeLeft => "--landscape-left",
                DeviceOrientation::LandscapeRight => "--landscape-right",
                DeviceOrientation::PortraitUpsideDown => "--upside-down",
                _ => todo!(),
            }
            .to_string(),
        );
    }
    if let Some(value) = QUICK_ASPECT_VALUES[quick_options_aspect] {
        option_args.push(format!("--aspect={}", value));
    }
    if let Some(value) = QUICK_FILTER_VALUES[quick_options_filter] {
        option_args.push(format!("--filter={}", value));
    }
    if let Some(()) = quick_options_fullscreen {
        option_args.push("--fullscreen".to_string());
    }
    if !quick_options_analog_stick_tilt_controls {
        option_args.push("--disable-analog-stick-tilt-controls".to_string());
    }
    if quick_options_network {
        option_args.push("--allow-network-access".to_string());
    }
    option_args.push(
        if quick_options_keep_screen_on {
            "--keep-screen-on"
        } else {
            "--no-keep-screen-on"
        }
        .to_string(),
    );

    if let Err(e) = std::fs::write(&quick_options_file, option_args.join("\n")) {
        log!("Warning: couldn't save quick options: {}", e);
    }

    // Return the environment so some parts of it can be salvaged.
    (app_path, option_args)
}

const ICON_SIZE: CGSize = CGSize {
    width: 57.0,
    height: 57.0,
};

enum TappedIcon {
    App(usize),
    ChangePage(usize),
}

struct IconGridStuff {
    icon_buttons_and_labels: Vec<(id, id)>,
    placeholder_icon: Option<id>,
    prev_icon: Option<id>,
    next_icon: Option<id>,
    pages: Vec<std::ops::Range<usize>>,
    icon_map: HashMap<id, TappedIcon>,
}

fn make_icon_grid(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    app_frame: CGRect,
    total_app_count: usize,
    have_wallpaper: bool,
) -> IconGridStuff {
    let num_cols = 4;
    let num_cols_f = num_cols as CGFloat;
    let num_rows = 3;
    let label_size = CGSize {
        width: 74.0,
        height: 13.0,
    };
    let icon_gap_x: CGFloat = 19.0;
    let icon_gap_y: CGFloat = 4.0 + label_size.height + 14.0;
    let icon_grid_width = (ICON_SIZE.width * num_cols_f) + icon_gap_x * (num_cols_f - 1.0);
    let icon_grid_origin = CGPoint {
        x: (app_frame.size.width - icon_grid_width) / 2.0,
        y: 80.0,
    };

    let icon_tapped_sel = env.objc.lookup_selector("iconTapped:").unwrap();

    let mut icon_buttons_and_labels = Vec::new();

    for i in 0..(num_cols * num_rows) {
        let col = i % num_cols;
        let row = i / num_cols;

        // Rounding is needed here to avoid a blurry or offset image.
        let icon_frame = CGRect {
            origin: CGPoint {
                x: (icon_grid_origin.x + (col as CGFloat) * (ICON_SIZE.width + icon_gap_x)).round(),
                y: (icon_grid_origin.y + (row as CGFloat) * (ICON_SIZE.height + icon_gap_y))
                    .round(),
            },
            size: ICON_SIZE,
        };
        let icon_button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; icon_button setFrame:icon_frame];
        let image_view: id = msg![env; icon_button imageView];
        let bounds: CGRect = msg![env; icon_button bounds];
        () = msg![env; image_view setFrame:bounds];
        () = msg![env; icon_button addTarget:delegate
                                      action:icon_tapped_sel
                            forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; main_view addSubview:icon_button];

        // Rounding is needed here to avoid blurry text.
        let label_frame = CGRect {
            origin: CGPoint {
                x: (icon_frame.origin.x - (label_size.width - ICON_SIZE.width) / 2.0).round(),
                y: (icon_frame.origin.y + ICON_SIZE.height + 4.0).round(),
            },
            size: label_size,
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        let font_size: CGFloat = label_size.height - 2.0;
        let font: id = if have_wallpaper {
            msg_class![env; UIFont systemFontOfSize:font_size]
        } else {
            msg_class![env; UIFont boldSystemFontOfSize:font_size]
        };
        () = msg![env; label setFont:font];
        let text_color: id = if have_wallpaper {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor lightGrayColor]
        };
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];

        icon_buttons_and_labels.push((icon_button, label));
    }

    // TODO: Use UIScrollView pagination and UIPageControl once available.
    let pages = compute_pages(icon_buttons_and_labels.len(), total_app_count);

    IconGridStuff {
        icon_buttons_and_labels,
        placeholder_icon: None,
        prev_icon: None,
        next_icon: None,
        pages,
        icon_map: HashMap::new(),
    }
}

/// Show the app list in the icon grid, or the error message if there are no
/// apps.
fn show_apps(
    env: &mut Environment,
    icon_grid_stuff: &mut IconGridStuff,
    message_label: id,
    apps: &mut Result<Vec<AppInfo>, String>,
) {
    let slots = icon_grid_stuff.icon_buttons_and_labels.len();
    match apps {
        Ok(apps) => {
            icon_grid_stuff.pages = compute_pages(slots, apps.len());
            update_icon_grid(env, icon_grid_stuff, apps, 0);
            () = msg![env; message_label setHidden:true];
        }
        Err(e) => {
            // A single empty page, so the grid is blanked.
            #[allow(clippy::single_range_in_vec_init)]
            {
                icon_grid_stuff.pages = vec![0..0];
            }
            update_icon_grid(env, icon_grid_stuff, &mut [], 0);
            let text = ns_string::from_rust_string(env, e.clone());
            () = msg![env; message_label setText:text];
            () = msg![env; message_label setHidden:false];
        }
    }
}

/// Split the apps into pages of icons, leaving space for the
/// "previous"/"next" page icons.
fn compute_pages(slots: usize, total_app_count: usize) -> Vec<std::ops::Range<usize>> {
    let mut pages = Vec::new();
    let mut start = 0;
    while start < total_app_count {
        let mut end = start + slots;
        if start > 0 {
            end -= 1; // one icon space taken by "previous" button
        }
        if end < total_app_count {
            end -= 1; // one icon space taken by "next" button
        } else {
            end = total_app_count;
        }
        pages.push(start..end);
        start = end;
    }
    pages
}

fn make_icon_from_glyph(
    env: &mut Environment,
    glyph: char,
    font_size: CGFloat,
    baseline_offset: CGFloat,
    bg_color: (CGFloat, CGFloat, CGFloat, CGFloat),
) -> id {
    let color_space = CGColorSpaceCreateDeviceRGB(env);
    let context = CGBitmapContextCreate(
        env,
        Ptr::null(),
        ICON_SIZE.width as u32,
        ICON_SIZE.height as u32,
        8,
        4 * (ICON_SIZE.width as u32),
        color_space,
        kCGImageAlphaPremultipliedLast,
    );
    UIGraphicsPushContext(env, context);

    // Compensate for row order inversion
    CGContextTranslateCTM(env, context, 0.0, ICON_SIZE.height);
    CGContextScaleCTM(env, context, 1.0, -1.0);

    let (r, g, b, a) = bg_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    CGContextFillRect(
        env,
        context,
        CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: ICON_SIZE,
        },
    );

    let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
    let glyph_string: id = ns_string::from_rust_string(env, [glyph].into_iter().collect());
    let glyph_size: CGSize = msg![env; glyph_string sizeWithFont:font];
    CGContextSetRGBFillColor(env, context, 1.0, 1.0, 1.0, 1.0); // white
    let glyph_origin = CGPoint {
        x: ICON_SIZE.width / 2.0 - glyph_size.width / 2.0,
        y: ICON_SIZE.height / 2.0 - glyph_size.height / 2.0 + baseline_offset,
    };
    let _: CGSize = msg![env; glyph_string drawAtPoint:glyph_origin withFont:font];
    release(env, glyph_string);

    UIGraphicsPopContext(env);

    let cg_image = CGBitmapContextCreateImage(env, context);
    // This radius should match the one in src/bundle.rs.
    cg_image::borrow_image_mut(&mut env.objc, cg_image).round_corners(
        (10.0 / 57.0) * ICON_SIZE.width,
        /* four_corners: */ true,
        /* add_sheen: */ true,
    );
    CGContextRelease(env, context);

    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);

    ui_image
}

fn update_icon_grid(
    env: &mut Environment,
    icon_grid_stuff: &mut IconGridStuff,
    apps: &mut [AppInfo],
    page_idx: usize,
) {
    icon_grid_stuff.icon_map.clear();

    let app_idx_range = icon_grid_stuff.pages[page_idx].clone();
    let have_prev_icon = page_idx != 0;
    let have_next_icon = app_idx_range.end != apps.len();

    let mut icon_iter = icon_grid_stuff.icon_buttons_and_labels.iter();

    if have_prev_icon {
        let &(icon_button, label) = icon_iter.next().unwrap();
        let image = *icon_grid_stuff.prev_icon.get_or_insert_with(|| {
            make_icon_from_glyph(env, '←', 50.0, -9.0, (0.25, 0.25, 0.25, 1.0))
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::ChangePage(page_idx - 1));
    }

    for app_idx in app_idx_range.clone() {
        let app = &mut apps[app_idx];

        let &(icon_button, label) = icon_iter.next().unwrap();

        if let Some(icon) = app.icon.take() {
            let image = cg_image::from_image(env, icon);
            let image: id = msg_class![env; UIImage imageWithCGImage:image];
            app.icon_ui_image = Some(image);
        }

        let image = app.icon_ui_image.unwrap_or_else(|| {
            *icon_grid_stuff.placeholder_icon.get_or_insert_with(|| {
                make_icon_from_glyph(env, '?', 40.0, 0.0, (0.5, 0.5, 0.5, 1.0))
            })
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];

        let text = *app
            .display_name_ns_string
            .get_or_insert_with(|| ns_string::from_rust_string(env, app.display_name.clone()));
        () = msg![env; label setText:text];

        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::App(app_idx));
    }

    if have_next_icon {
        let &(icon_button, label) = icon_iter.next().unwrap();
        let image = *icon_grid_stuff.next_icon.get_or_insert_with(|| {
            make_icon_from_glyph(env, '→', 50.0, -9.0, (0.25, 0.25, 0.25, 1.0))
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::ChangePage(page_idx + 1));
    }

    // There may be remaining spaces might need to be blanked.
    for &(icon_button, label) in icon_iter {
        () = msg![env; icon_button setImage:nil forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
    }
}

const SUBTLE_TEXT_COLOR: (CGFloat, CGFloat, CGFloat, CGFloat) = (0.75, 0.8, 0.9, 1.0);
const WARNING_TEXT_COLOR: (CGFloat, CGFloat, CGFloat, CGFloat) = (1.0, 0.78, 0.4, 1.0);

/// Whether the log of the previous run (see [crate::log::get_log_file]) shows
/// that touchHLE crashed.
fn previous_run_crashed() -> bool {
    std::fs::read_to_string(paths::user_data_base_path().join("Podroid_log_previous.txt"))
        .is_ok_and(|log| {
            // Written by the panic hook on Android, when a panic happens
            // while running an app, and when one happens in the app picker.
            log.contains("Panic at ")
                || log.contains("Register state immediately after panic")
                || log.contains("Crash in app picker!")
        })
}

#[allow(clippy::too_many_arguments)]
fn add_label(
    env: &mut Environment,
    super_view: id,
    (x, y, width, height): (CGFloat, CGFloat, CGFloat, CGFloat),
    text: &str,
    font_size: CGFloat,
    bold: bool,
    (r, g, b, a): (CGFloat, CGFloat, CGFloat, CGFloat),
    alignment: UITextAlignment,
) -> id {
    let label: id = msg_class![env; UILabel alloc];
    let label: id = msg![env; label initWithFrame:(CGRect {
        origin: CGPoint { x, y },
        size: CGSize { width, height },
    })];
    let text = ns_string::from_rust_string(env, text.to_string());
    () = msg![env; label setText:text];
    let font: id = if bold {
        msg_class![env; UIFont boldSystemFontOfSize:font_size]
    } else {
        msg_class![env; UIFont systemFontOfSize:font_size]
    };
    () = msg![env; label setFont:font];
    let color: id = msg_class![env; UIColor colorWithRed:r green:g blue:b alpha:a];
    () = msg![env; label setTextColor:color];
    let bg_color: id = msg_class![env; UIColor clearColor];
    () = msg![env; label setBackgroundColor:bg_color];
    () = msg![env; label setTextAlignment:alignment];
    () = msg![env; super_view addSubview:label];
    label
}

/// A translucent rounded button with white text, for use on the dark
/// background.
fn make_styled_button(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    frame: CGRect,
    title: &str,
    selector: &str,
    font_size: CGFloat,
) -> id {
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    () = msg![env; button setFrame:frame];
    let bg: id = msg_class![env; UIColor colorWithWhite:(1.0 as CGFloat) alpha:(0.15 as CGFloat)];
    () = msg![env; button setBackgroundColor:bg];
    let layer: id = msg![env; button layer];
    () = msg![env; layer setCornerRadius:(10.0 as CGFloat)];
    let text = ns_string::from_rust_string(env, title.to_string());
    () = msg![env; button setTitle:text forState:UIControlStateNormal];
    let white: id = msg_class![env; UIColor whiteColor];
    () = msg![env; button setTitleColor:white forState:UIControlStateNormal];
    // FIXME: manually calling layoutSubviews shouldn't be needed?
    () = msg![env; button layoutSubviews];
    let label: id = msg![env; button titleLabel];
    let font: id = msg_class![env; UIFont boldSystemFontOfSize:font_size];
    () = msg![env; label setFont:font];
    let selector = env.objc.lookup_selector(selector).unwrap();
    () = msg![env; button addTarget:delegate
                             action:selector
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; super_view addSubview:button];
    button
}

fn make_button_row(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    super_view_size: CGSize,
    buttons_row_center: CGFloat,
    buttons: &[(&'static str, &'static str)],
    font_size: Option<CGFloat>,
) -> Vec<id> {
    let margin = 10.0;

    let button_size = CGSize {
        width: (super_view_size.width - margin) / (buttons.len() as CGFloat) - margin,
        height: 30.0,
    };
    let mut button_frame = CGRect {
        origin: CGPoint {
            x: margin,
            y: buttons_row_center - button_size.height / 2.0,
        },
        size: button_size,
    };

    let mut ui_buttons = Vec::new();
    for &(title_text, selector) in buttons {
        let button = make_styled_button(
            env,
            delegate,
            super_view,
            button_frame,
            title_text,
            selector,
            font_size.unwrap_or(14.0),
        );
        button_frame.origin.x += button_size.width + margin;
        ui_buttons.push(button);
    }
    ui_buttons
}

struct CopyrightInfoStuff {
    main_view: id,
    text_frame: CGRect,
    text_label: id,
    font: id,
    pages: Vec<(std::ops::Range<usize>, CGFloat)>,
    last_page_idx: Option<usize>,
    prev_page_button: id,
    next_page_button: id,
}

fn setup_copyright_info(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    app_frame: CGRect,
) -> CopyrightInfoStuff {
    let main_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: app_frame.size,
    };

    let divider = main_frame.size.height - 40.0;

    // Container for all the other stuff

    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:main_frame];
    let bg_color: id = msg_class![env; UIColor colorWithRed:(0.11 as CGFloat)
                                                      green:(0.12 as CGFloat)
                                                       blue:(0.28 as CGFloat)
                                                      alpha:(1.0 as CGFloat)];
    () = msg![env; main_view setBackgroundColor:bg_color];
    // This main_view is hidden until the copyright info button is tapped.
    () = msg![env; main_view setHidden:true];
    () = msg![env; super_view addSubview:main_view];

    // UILabel that will display part of the copyright text

    let padding = 10.0;
    let text_frame = CGRect {
        origin: CGPoint {
            x: padding,
            y: padding,
        },
        size: CGSize {
            width: app_frame.size.width - padding * 2.0,
            height: divider - padding * 2.0,
        },
    };

    let text_label: id = msg_class![env; UILabel alloc];
    let text_label: id = msg![env; text_label initWithFrame:text_frame];
    () = msg![env; text_label setNumberOfLines:0]; // unlimited
    let text_color: id =
        msg_class![env; UIColor colorWithWhite:(0.92 as CGFloat) alpha:(1.0 as CGFloat)];
    () = msg![env; text_label setTextColor:text_color];
    let bg_color: id = msg_class![env; UIColor clearColor];
    () = msg![env; text_label setBackgroundColor:bg_color];
    let font_size: CGFloat = 16.0;
    let font: id = msg_class![env; UIFont systemFontOfSize:font_size];
    () = msg![env; text_label setFont:font];
    () = msg![env; main_view addSubview:text_label];

    // Navigation

    let buttons_row_center = (main_frame.size.height + divider) / 2.0;
    let buttons = make_button_row(
        env,
        delegate,
        main_view,
        main_frame.size,
        buttons_row_center,
        &[
            ("上一页", "copyrightInfoPrevPage"),
            ("下一页", "copyrightInfoNextPage"),
            ("关闭", "copyrightInfoHide"),
        ],
        None,
    );

    CopyrightInfoStuff {
        main_view,
        text_frame,
        text_label,
        font,
        pages: Vec::new(),
        last_page_idx: None,
        prev_page_button: buttons[0],
        next_page_button: buttons[1],
    }
}

fn change_copyright_page(
    env: &mut Environment,
    copyright_info_stuff: &mut CopyrightInfoStuff,
    copyright_info_text: &str,
    page_idx: usize,
) {
    // TODO: Eventually this should be ripped out and replaced with a scrolling
    // UITextView, once that's implemented.

    let &mut CopyrightInfoStuff {
        text_frame,
        text_label,
        font,
        ref mut pages,
        ref mut last_page_idx,
        prev_page_button,
        next_page_button,
        ..
    } = copyright_info_stuff;

    // Lazily lay out pages of text as needed.

    if page_idx == pages.len() {
        let mut page_start = pages.last().map_or(0, |page| page.0.end);
        while copyright_info_text[page_start..].starts_with([' ', '\n', '\r']) {
            page_start += 1;
        }
        let mut page_height = 0.0;
        let page_end = loop {
            let mut line_start = page_start;
            while line_start < copyright_info_text.len() {
                let is_first_line = line_start == page_start;

                let line_end = if let Some(i) = copyright_info_text[line_start..].find('\n') {
                    line_start + i + 1
                } else {
                    copyright_info_text.len()
                };

                let line = &copyright_info_text[line_start..line_end];

                // Force pagination before headings (in Dynarmic's license text)
                if !is_first_line && line.starts_with("###") {
                    break;
                }

                let line_temp = ns_string::from_rust_string(env, line.to_string());
                let line_size: CGSize = msg![env; line_temp sizeWithFont:font
                                                       constrainedToSize:(text_frame.size)];
                // Avoid accumulation of old line strings.
                release(env, line_temp);

                if page_height + line_size.height > text_frame.size.height {
                    break;
                }

                page_height += line_size.height;
                line_start = line_end;

                // Force pagination after dividers
                if !is_first_line && line.starts_with("---") {
                    break;
                }
            }
            let page_end = line_start;
            assert!(page_start != page_end);

            // Avoid entirely blank pages
            if copyright_info_text[page_start..page_end].trim() == "" {
                page_start = page_end;
            } else {
                break page_end;
            }
        };
        assert!(page_start != page_end);
        pages.push((page_start..page_end, page_height));
        if page_end == copyright_info_text.len() {
            *last_page_idx = Some(page_idx);
        }
    }

    // Actually display the page

    let (page, page_height) = pages[page_idx].clone();
    let page = &copyright_info_text[page];

    let page: id = ns_string::from_rust_string(env, page.to_string());
    () = msg![env; text_label setText:page];
    // Avoid accumulation of old page strings.
    release(env, page);

    // UILabel always vertically centers text. Work around that by resizing it.
    let label_frame = CGRect {
        origin: text_frame.origin,
        size: CGSize {
            width: text_frame.size.width,
            // The page height is slightly off, a little padding is needed.
            height: page_height + 10.0,
        },
    };
    () = msg![env; text_label setFrame:label_frame];

    () = msg![env; prev_page_button setHidden:(page_idx == 0)];
    () = msg![env; next_page_button setHidden:(Some(page_idx) == *last_page_idx)];
}

/// Where the quick options are remembered, in the user data directory.
const QUICK_OPTIONS_FILE: &str = "Podroid_quick_options.txt";

struct QuickOptionsStuff {
    main_view: id,
    /// `UISwitch*`es and their action selectors.
    switches: Vec<(&'static str, id)>,
    scale_hack_buttons: [id; 5],
    orientation_buttons: [id; 4],
    aspect_buttons: [id; 4],
    filter_buttons: [id; 4],
}

/// `--aspect=` values for the quick options buttons, in order.
const QUICK_ASPECT_VALUES: [Option<&str>; 4] =
    [None, Some("stretch"), Some("fill"), Some("integer")];
/// `--filter=` values for the quick options buttons, in order.
const QUICK_FILTER_VALUES: [Option<&str>; 4] =
    [Some("smooth"), Some("sharp"), Some("pixel"), Some("hq")];

fn setup_quick_options(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    app_frame: CGRect,
) -> QuickOptionsStuff {
    // UIView*
    let main_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: app_frame.size,
    };

    // Container for all the other stuff

    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:main_frame];
    let bg_color: id = msg_class![env; UIColor colorWithRed:(0.11 as CGFloat)
                                                      green:(0.12 as CGFloat)
                                                       blue:(0.28 as CGFloat)
                                                      alpha:(1.0 as CGFloat)];
    () = msg![env; main_view setBackgroundColor:bg_color];
    // This main_view is hidden until the copyright info button is tapped.
    () = msg![env; main_view setHidden:true];
    () = msg![env; super_view addSubview:main_view];

    let divider = 40.0;

    // Title and close button
    add_label(
        env,
        main_view,
        (14.0, 10.0, 200.0, 24.0),
        "快捷设置",
        17.0,
        true,
        (1.0, 1.0, 1.0, 1.0),
        UITextAlignmentLeft,
    );
    make_styled_button(
        env,
        delegate,
        main_view,
        CGRect {
            origin: CGPoint {
                x: main_frame.size.width - 74.0,
                y: 8.0,
            },
            size: CGSize {
                width: 64.0,
                height: 28.0,
            },
        },
        "关闭",
        "quickOptionsHide",
        13.0,
    );

    enum RowKind {
        Label(&'static str),
        Buttons(&'static [(&'static str, &'static str)]),
        Switch(&'static str, bool),
    }
    let rows = [
        RowKind::Label("放大渲染（实验性）"),
        RowKind::Buttons(&[
            ("默认", "scaleHackDefault"),
            ("关", "scaleHack1"),
            ("2×", "scaleHack2"),
            ("3×", "scaleHack3"),
            ("4×", "scaleHack4"),
        ]),
        RowKind::Label("屏幕方向"),
        RowKind::Buttons(&[
            ("默认", "orientationDefault"),
            ("←", "orientationLandscapeLeft"),
            ("→", "orientationLandscapeRight"),
            ("↓", "orientationPortraitUpsideDown"),
        ]),
        RowKind::Label("画面比例"),
        RowKind::Buttons(&[
            ("原始", "quickOptionTapped:"),
            ("拉伸", "quickOptionTapped:"),
            ("填满", "quickOptionTapped:"),
            ("整数倍", "quickOptionTapped:"),
        ]),
        RowKind::Label("画面滤镜"),
        RowKind::Buttons(&[
            ("平滑", "quickOptionTapped:"),
            ("锐利", "quickOptionTapped:"),
            ("像素", "quickOptionTapped:"),
            ("高清", "quickOptionTapped:"),
        ]),
        RowKind::Label("游戏时保持亮屏"),
        RowKind::Switch("keepScreenOn:", false),
        RowKind::Label("允许联网"),
        RowKind::Switch("network:", false),
        RowKind::Label("用摇杆模拟重力感应"),
        RowKind::Switch("analogStickTiltControls:", true),
        // ---- (divider for stuff skipped below)
        RowKind::Label("强制全屏"),
        RowKind::Switch("fullscreen:", false),
    ];
    let rows_len_full = rows.len();
    let rows = if crate::window::Window::rotatable_fullscreen() {
        // Fullscreen option doesn't make sense on always-fullscreen platforms
        &rows[..rows.len() - 2]
    } else {
        &rows[..]
    };

    let mut button_rows = Vec::new();
    let mut switches = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let row_center = divider
            + ((1 + i) as CGFloat)
                * ((main_frame.size.height - divider) / ((rows_len_full + 1) as CGFloat));

        match *row {
            RowKind::Label(text) => {
                let frame = CGRect {
                    origin: CGPoint {
                        x: 0.0,
                        y: row_center - 30.0 / 2.0,
                    },
                    size: CGSize {
                        width: main_frame.size.width,
                        height: 30.0,
                    },
                };

                let CGRect { origin, size } = frame;
                add_label(
                    env,
                    main_view,
                    (origin.x, origin.y, size.width, size.height),
                    text,
                    15.0,
                    false,
                    SUBTLE_TEXT_COLOR,
                    UITextAlignmentCenter,
                );
            }
            RowKind::Buttons(buttons) => {
                button_rows.push(make_button_row(
                    env,
                    delegate,
                    main_view,
                    main_frame.size,
                    row_center,
                    buttons,
                    /* font_size: */ None,
                ));
            }
            RowKind::Switch(selector, default_state) => {
                let switch_frame = CGRect {
                    origin: CGPoint {
                        x: main_frame.size.width / 2.0 - 94.0 / 2.0,
                        y: row_center - 27.0 / 2.0,
                    },
                    size: Default::default(),
                };

                let switch: id = msg_class![env; UISwitch alloc];
                let switch: id = msg![env; switch initWithFrame:switch_frame];
                () = msg![env; switch setOn:default_state];
                switches.push((selector, switch));
                let selector = env.objc.lookup_selector(selector).unwrap();
                () = msg![env; switch addTarget:delegate
                                         action:selector
                               forControlEvents:UIControlEventValueChanged];
                () = msg![env; main_view addSubview:switch];
            }
        }
    }

    QuickOptionsStuff {
        main_view,
        switches,
        scale_hack_buttons: button_rows[0][..].try_into().unwrap(),
        orientation_buttons: button_rows[1][..].try_into().unwrap(),
        aspect_buttons: button_rows[2][..].try_into().unwrap(),
        filter_buttons: button_rows[3][..].try_into().unwrap(),
    }
}
