/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIScreen`.

use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_geometry::UIEdgeInsets;
use crate::objc::{id, msg, msg_class, nil, objc_classes, ClassExports, TrivialHostObject, SEL};

#[derive(Default)]
pub struct State {
    main_screen: Option<id>,
    current_mode: Option<id>,
    overscan_compensation: NSInteger,
}

/// Convert screen bounds from points to the physical pixel grid.
fn native_bounds(bounds: CGRect, scale: CGFloat) -> CGRect {
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: bounds.size.width * scale,
            height: bounds.size.height * scale,
        },
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// For now this is a singleton (the only instance is returned by mainScreen),
// so there are hardcoded assumptions related to that.
@implementation UIScreen: NSObject

+ (id)mainScreen {
    if let Some(screen) = env.framework_state.uikit.ui_screen.main_screen {
        screen
    } else {
        let new = env.objc.alloc_static_object(
            this,
            Box::new(TrivialHostObject),
            &mut env.mem
        );
        env.framework_state.uikit.ui_screen.main_screen = Some(new);
        new
   }
}

// We only model just one screen (main screen).
+ (id)screens {
    let main_screen: id = msg![env; this mainScreen];
    msg_class![env; NSArray arrayWithObject:main_screen]
}

- (id)retain { this }
- (())release {}
- (id)autorelease { this }

// TODO: more accessors

- (CGRect)bounds {
    // While Apple's documentation says this changes with the interface
    // orientation, https://useyourloaf.com/blog/uiscreen-bounds-in-ios-8/ says
    // ths wasn't the case prior to iOS 8.
    let (width, height) = env.window().size_unrotated_unscaled();
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: width as f32, height: height as f32 },
    }
}

- (CGRect)applicationFrame {
    // FIXME: Does this change depending on the status bar orientation?
    let mut bounds: CGRect = msg![env; this bounds];
    const STATUS_BAR_HEIGHT: f32 = 20.0;
    if !env.framework_state.uikit.ui_application.status_bar_hidden {
        bounds.origin.y += STATUS_BAR_HEIGHT;
        bounds.size.height -= STATUS_BAR_HEIGHT;
    }
    bounds
}

- (CGRect)nativeBounds {
    let bounds: CGRect = msg![env; this bounds];
    native_bounds(bounds, env.options.screen_scale() as CGFloat)
}

- (CGFloat)scale {
    env.options.screen_scale() as CGFloat
}

- (CGFloat)nativeScale {
    env.options.screen_scale() as CGFloat
}

- (id)currentMode {
    if let Some(mode) = env.framework_state.uikit.ui_screen.current_mode {
        mode
    } else {
        let class = env.objc.get_known_class("UIScreenMode", &mut env.mem);
        let mode = env.objc.alloc_static_object(class, Box::new(TrivialHostObject), &mut env.mem);
        env.framework_state.uikit.ui_screen.current_mode = Some(mode);
        mode
    }
}

- (id)availableModes {
    let mode: id = msg![env; this currentMode];
    msg_class![env; NSArray arrayWithObject:mode]
}

- (id)preferredMode { nil }

- (())setCurrentMode:(id)_mode {
    // The built-in display has a single fixed mode.
}

- (id)mirroredScreen { nil }

- (bool)isCaptured { false }

- (CGRect)convertRect:(CGRect)rect toScreen:(id)_other_screen { rect }
- (CGRect)convertRect:(CGRect)rect fromScreen:(id)_other_screen { rect }
- (CGPoint)convertPoint:(CGPoint)point toScreen:(id)_other_screen { point }
- (CGPoint)convertPoint:(CGPoint)point fromScreen:(id)_other_screen { point }

- (id)displayLinkWithTarget:(id)target selector:(SEL)selector {
    msg_class![env; CADisplayLink displayLinkWithTarget:target selector:selector]
}

- (NSInteger)overscanCompensation {
    env.framework_state.uikit.ui_screen.overscan_compensation
}

- (())setOverscanCompensation:(NSInteger)compensation {
    // Only an external display can overscan; record the documented setting.
    if (0..=2).contains(&compensation) {
        env.framework_state.uikit.ui_screen.overscan_compensation = compensation;
    } else {
        log!("Ignoring unsupported overscan compensation: {}", compensation);
    }
}

- (UIEdgeInsets)overscanCompensationInsets {
    UIEdgeInsets::default()
}

@end

@implementation UIScreenMode: NSObject

- (id)retain { this }
- (())release {}
- (id)autorelease { this }

- (CGSize)size {
    let (width, height) = env.window().size_unrotated_unscaled();
    let scale: CGFloat = env.options.screen_scale() as CGFloat;
    CGSize { width: width as f32 * scale, height: height as f32 * scale }
}

- (CGFloat)pixelAspectRatio { 1.0 }

@end

};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_screen_bounds_are_in_pixels() {
        let bounds = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 320.0,
                height: 480.0,
            },
        };
        assert_eq!(
            native_bounds(bounds, 2.0).size,
            CGSize {
                width: 640.0,
                height: 960.0
            }
        );
        assert_eq!(native_bounds(bounds, 1.0), bounds);
    }
}
