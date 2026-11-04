/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The UIKit framework.
//!
//! For the time being the focus of this project is on running games, which are
//! likely to use UIKit in very simple and limited ways, so this implementation
//! will probably take a lot of shortcuts.

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::{msg, Environment};
use std::time::Instant;

/// Constants for classes touchHLE doesn't implement (yet), or that don't fit
/// elsewhere.
const CONSTANTS: ConstantExports = &[
    // `UIBackgroundTaskIdentifier`, which is an `NSUInteger`.
    (
        "_UIBackgroundTaskInvalid",
        HostConstant::Custom(|env| env.mem.alloc_and_write(0u32).cast_void().cast_const()),
    ),
    // `CGFloat`
    (
        "_UIScrollViewDecelerationRateNormal",
        HostConstant::Custom(|env| env.mem.alloc_and_write(0.998f32).cast_void().cast_const()),
    ),
    (
        "_UITextViewTextDidChangeNotification",
        HostConstant::NSString("UITextViewTextDidChangeNotification"),
    ),
    (
        "_UIImagePickerControllerOriginalImage",
        HostConstant::NSString("UIImagePickerControllerOriginalImage"),
    ),
    // From UIKit's additions to `NSAttributedString`.
    ("_NSFontAttributeName", HostConstant::NSString("NSFont")),
];

/// Podroid doesn't emulate an active Guided Access session.
fn UIAccessibilityIsGuidedAccessEnabled(_env: &mut Environment) -> bool {
    false
}

const FUNCTIONS: FunctionExports = &[export_c_func!(UIAccessibilityIsGuidedAccessEnabled())];

pub mod ns_paragraph_style;
pub mod ui_accelerometer;
pub mod ui_action_sheet;
pub mod ui_activity;
pub mod ui_activity_indicator_view;
pub mod ui_alert_controller;
pub mod ui_application;
pub mod ui_bar_button_item;
pub mod ui_bezier_path;
pub mod ui_color;
pub mod ui_custom_object;
pub mod ui_device;
pub mod ui_document;
pub mod ui_event;
pub mod ui_font;
pub mod ui_geometry;
pub mod ui_gesture_recognizer;
pub mod ui_graphics;
pub mod ui_image;
pub mod ui_image_picker_controller;
pub mod ui_keyboard;
pub mod ui_launch_delegate;
pub mod ui_layout_placeholders;
pub mod ui_local_notification;
pub mod ui_navigation_bar;
pub mod ui_nib;
pub mod ui_page_view_controller;
pub mod ui_pasteboard;
pub mod ui_pinch_gesture_recognizer;
pub mod ui_popover_controller;
pub mod ui_responder;
pub mod ui_rotation_gesture_recognizer;
pub mod ui_screen;
pub mod ui_search_bar;
pub mod ui_split_view_controller;
pub mod ui_storyboard;
pub mod ui_tab_bar_controller;
pub mod ui_tab_bar_item;
pub mod ui_text_input;
pub mod ui_touch;
pub mod ui_view;
pub mod ui_view_controller;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/UIKit.framework/UIKit",
    aliases: &[],
    class_exports: &[
        ui_view::ui_text_selection_view::CLASSES,
        ui_view::ui_table_view::CLASSES,
        ui_view::ui_refresh_control::CLASSES,
        ui_view::ui_collection_view::CLASSES,
        ui_text_input::CLASSES,
        ui_tab_bar_item::CLASSES,
        ui_tab_bar_controller::CLASSES,
        ui_storyboard::CLASSES,
        ui_split_view_controller::CLASSES,
        ui_search_bar::CLASSES,
        ui_rotation_gesture_recognizer::CLASSES,
        ui_popover_controller::CLASSES,
        ui_pinch_gesture_recognizer::CLASSES,
        ui_page_view_controller::CLASSES,
        ui_navigation_bar::CLASSES,
        ui_layout_placeholders::CLASSES,
        ui_launch_delegate::CLASSES,
        ui_keyboard::CLASSES,
        ui_gesture_recognizer::CLASSES,
        ui_document::CLASSES,
        ui_custom_object::CLASSES,
        ui_alert_controller::CLASSES,
        ui_activity::CLASSES,
        ui_action_sheet::CLASSES,
        ns_paragraph_style::CLASSES,
        ui_accelerometer::CLASSES,
        ui_activity_indicator_view::CLASSES,
        ui_application::CLASSES,
        ui_bar_button_item::CLASSES,
        ui_bezier_path::CLASSES,
        ui_color::CLASSES,
        ui_device::CLASSES,
        ui_event::CLASSES,
        ui_font::CLASSES,
        ui_image::CLASSES,
        ui_image_picker_controller::CLASSES,
        ui_local_notification::CLASSES,
        ui_nib::CLASSES,
        ui_pasteboard::CLASSES,
        ui_responder::CLASSES,
        ui_screen::CLASSES,
        ui_touch::CLASSES,
        ui_view::CLASSES,
        ui_view::ui_alert_view::CLASSES,
        ui_view::ui_control::CLASSES,
        ui_view::ui_control::ui_button::CLASSES,
        ui_view::ui_control::ui_segmented_control::CLASSES,
        ui_view::ui_control::ui_slider::CLASSES,
        ui_view::ui_control::ui_text_field::CLASSES,
        ui_view::ui_control::ui_switch::CLASSES,
        ui_view::ui_image_view::CLASSES,
        ui_view::ui_label::CLASSES,
        ui_view::ui_page_control::CLASSES,
        ui_view::ui_picker_view::CLASSES,
        ui_view::ui_scroll_view::CLASSES,
        ui_view::ui_scroll_view::ui_text_view::CLASSES,
        ui_view::ui_toolbar::CLASSES,
        ui_view::ui_web_view::CLASSES,
        ui_view::ui_window::CLASSES,
        ui_view_controller::CLASSES,
        ui_view_controller::ui_navigation_controller::CLASSES,
    ],
    constant_exports: &[
        ui_storyboard::CONSTANTS,
        ui_keyboard::CONSTANTS,
        CONSTANTS,
        ui_application::CONSTANTS,
        ui_device::CONSTANTS,
        ui_view::ui_control::ui_text_field::CONSTANTS,
        ui_view::ui_window::CONSTANTS,
    ],
    function_exports: &[
        FUNCTIONS,
        ui_application::FUNCTIONS,
        ui_image::FUNCTIONS,
        ui_image_picker_controller::FUNCTIONS,
        ui_geometry::FUNCTIONS,
        ui_graphics::FUNCTIONS,
    ],
};

#[derive(Default)]
pub struct State {
    ui_accelerometer: ui_accelerometer::State,
    ui_geometry: ui_geometry::State,
    ui_keyboard: ui_keyboard::State,
    ui_launch_delegate: ui_launch_delegate::State,
    ui_application: ui_application::State,
    ui_color: ui_color::State,
    ui_device: ui_device::State,
    ui_font: ui_font::State,
    ui_graphics: ui_graphics::State,
    ui_image: ui_image::State,
    ui_screen: ui_screen::State,
    ui_touch: ui_touch::State,
    pub ui_view: ui_view::State,
    ui_responder: ui_responder::State,
}

/// For use by `NSRunLoop`: handles any events that have queued up.
///
/// Returns the next time this function must be called, if any, e.g. the next
/// time an accelerometer input is due.
pub fn handle_events(env: &mut Environment) -> Option<Instant> {
    use crate::window::Event;
    use crate::window::TextInputEvent;

    // NSRunLoop will never call this function in headless mode.
    while let Some(event) = env.window_mut().pop_event() {
        match event {
            Event::Quit => {
                echo!("User requested quit, exiting.");
                // Some apps take a while to quit, or never finish (see
                // ui_application::exit), so close the window right away.
                if !cfg!(target_os = "android") {
                    env.on_parent_stack_in_coroutine(|window, _| window.hide());
                }
                ui_application::exit(env);
            }
            Event::TouchesDown(..) | Event::TouchesMove(..) | Event::TouchesUp(..) => {
                ui_touch::handle_event(env, event)
            }
            Event::AppWillResignActive => {
                // Getting this event means touchHLE is becoming inactive, e.g.
                // due to switching apps or the screen turning off. Rather than
                // terminating the app (which loses unsaved progress), freeze
                // it with audio paused until it's back in the foreground, like
                // a suspended app on later iOS versions.
                log!("Handling app-will-resign-active event: pausing app.");
                ui_application::will_resign_active(env);
                crate::audio::openal::pause_all_devices();
                let resumed = env.on_parent_stack_in_coroutine(|window, options| {
                    window.wait_until_active(options)
                });
                crate::audio::openal::resume_all_devices();
                if resumed {
                    log!("Resuming app.");
                    ui_application::did_become_active(env);
                }
                // Otherwise, the terminate/quit event is handled next.
            }
            Event::AppDidBecomeActive => {
                // Handled by wait_until_active().
            }
            Event::AppWillTerminate => {
                log!("Handling app-will-terminate event.");
                ui_application::exit(env);
            }
            Event::EnterDebugger => {
                if env.is_debugging_enabled() {
                    log!("Handling EnterDebugger event: entering debugger.");
                    env.enter_debugger(/* reason: */ None);
                } else {
                    log!("Ignoring EnterDebugger event: no debugger connected.");
                }
            }
            Event::TextInput(text_event) => {
                let responder = env.framework_state.uikit.ui_responder.first_responder;
                let class = msg![env; responder class];
                let ui_text_field_class = env.objc.get_known_class("UITextField", &mut env.mem);
                let ui_text_view_class = env.objc.get_known_class("UITextView", &mut env.mem);
                if !responder.is_null() && env.objc.class_is_subclass_of(class, ui_text_view_class)
                {
                    use ui_view::ui_scroll_view::ui_text_view;
                    match text_event {
                        TextInputEvent::Text(text) => {
                            ui_text_view::handle_text(env, responder, text)
                        }
                        TextInputEvent::Backspace => ui_text_view::handle_backspace(env, responder),
                        TextInputEvent::Return => ui_text_view::handle_return(env, responder),
                    }
                } else if !responder.is_null()
                    && env.objc.class_is_subclass_of(class, ui_text_field_class)
                {
                    match text_event {
                        TextInputEvent::Text(text) => {
                            ui_view::ui_control::ui_text_field::handle_text(env, responder, text)
                        }
                        TextInputEvent::Backspace => {
                            ui_view::ui_control::ui_text_field::handle_backspace(env, responder)
                        }
                        TextInputEvent::Return => {
                            ui_view::ui_control::ui_text_field::handle_return(env, responder)
                        }
                    }
                }
            }
            Event::Noop => (),
        }
    }

    ui_accelerometer::handle_accelerometer(env)
}
