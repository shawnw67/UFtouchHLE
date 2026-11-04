/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Separate module just for the dylib list, so it gets its own git history.

use crate::frameworks;
use crate::libc;
use crate::objc;

// Social: `SLComposeViewController`, whose services are all unavailable.
const SOCIAL: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/Social.framework/Social",
    aliases: &[],
    class_exports: &[frameworks::social::CLASSES],
    constant_exports: &[frameworks::social::CONSTANTS],
    function_exports: &[frameworks::social::FUNCTIONS],
};

const CORE_IMAGE: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/CoreImage.framework/CoreImage",
    aliases: &[],
    class_exports: &[
        frameworks::core_image::CLASSES,
        frameworks::core_image::pipeline::CLASSES,
    ],
    constant_exports: &[frameworks::core_image::CONSTANTS],
    function_exports: &[frameworks::core_image::FUNCTIONS],
};

// The CaptiveNetwork part of SystemConfiguration.
const CAPTIVE_NETWORK: super::HostDylib = super::HostDylib {
    path: "/System/Library/Frameworks/SystemConfiguration.framework/CaptiveNetwork",
    aliases: &["/System/Library/SystemConfiguration/CaptiveNetwork.bundle/CaptiveNetwork"],
    class_exports: &[],
    constant_exports: &[frameworks::captive_network::CONSTANTS],
    function_exports: &[frameworks::captive_network::FUNCTIONS],
};

/// The single list of host dylibs that the linker (and Objective-C runtime)
/// searches through.
pub const DYLIB_LIST: &[&super::HostDylib] = &[
    &frameworks::accelerate::DYLIB,
    &frameworks::address_book::DYLIB,
    &libc::DYLIB,
    &objc::DYLIB,
    &crate::environment::app_picker::DYLIB, // Not a real library; special internal classes.
    &frameworks::audio_toolbox::DYLIB,
    &frameworks::avfoundation::DYLIB,
    &frameworks::cf_network::DYLIB,
    &frameworks::core_animation::DYLIB,
    &frameworks::core_foundation::DYLIB,
    &frameworks::core_graphics::DYLIB,
    &frameworks::core_location::DYLIB,
    &frameworks::core_media::DYLIB,
    &frameworks::core_motion::DYLIB,
    &frameworks::core_telephony::DYLIB,
    &frameworks::core_text::DYLIB,
    &frameworks::foundation::DYLIB,
    &frameworks::game_controller::DYLIB,
    &frameworks::game_kit::DYLIB,
    &frameworks::image_io::DYLIB,
    &frameworks::media_player::DYLIB,
    &frameworks::message_ui::DYLIB,
    &frameworks::openal::DYLIB,
    &frameworks::opengles::DYLIB,
    &frameworks::security::DYLIB,
    &frameworks::store_kit::DYLIB,
    &frameworks::system_configuration::DYLIB,
    &frameworks::uikit::DYLIB,
    &frameworks::libicucore::DYLIB,
    &frameworks::libsqlite3::DYLIB,
    &frameworks::libxml2::DYLIB,
    &frameworks::libbz2::DYLIB,
    &frameworks::metal::DYLIB,
    &frameworks::pass_kit::DYLIB,
    &frameworks::watch_connectivity::DYLIB,
    &frameworks::gl_kit::DYLIB,
    &frameworks::accounts::DYLIB,
    &frameworks::contacts::DYLIB,
    &frameworks::media_toolbox::DYLIB,
    &frameworks::replay_kit::DYLIB,
    &frameworks::safari_services::DYLIB,
    &SOCIAL,
    &CORE_IMAGE,
    &CAPTIVE_NETWORK,
];

#[cfg(test)]
mod tests {
    use crate::objc::ClassTemplate;

    use super::*;
    use std::collections::HashSet;

    #[test]
    fn no_duplicate_classes() {
        let mut seen_classes = HashSet::new();

        for (class_name, template) in DYLIB_LIST
            .iter()
            .flat_map(|dylib| dylib.class_exports)
            .copied()
            .flatten()
        {
            if !seen_classes.insert(class_name) {
                panic!("Found duplicate class export {class_name}");
            }
            let ClassTemplate {
                class_methods,
                instance_methods,
                ..
            } = template;

            let mut seen_class_methods = HashSet::with_capacity(class_methods.len());

            for (method_name, _) in *class_methods {
                if !seen_class_methods.insert(method_name) {
                    panic!("Found duplicate class method {method_name} for class {class_name}")
                }
            }

            let mut seen_instance_methods = HashSet::with_capacity(instance_methods.len());

            for (method_name, _) in *instance_methods {
                if !seen_instance_methods.insert(method_name) {
                    panic!("Found duplicate instance method {method_name} for class {class_name}")
                }
            }
        }
    }

    #[test]
    fn no_duplicate_functions() {
        let mut seen = HashSet::new();

        for (function_name, _) in DYLIB_LIST
            .iter()
            .flat_map(|dylib| dylib.function_exports)
            .copied()
            .flatten()
        {
            if !seen.insert(function_name) {
                panic!("Found duplicate function export {function_name}");
            }
        }
    }

    #[test]
    fn no_duplicate_constants() {
        let mut seen = HashSet::new();

        for (constant_name, _) in DYLIB_LIST
            .iter()
            .flat_map(|dylib| dylib.constant_exports)
            .copied()
            .flatten()
        {
            if !seen.insert(constant_name) {
                panic!("Found duplicate constant export {constant_name}");
            }
        }
    }
}
