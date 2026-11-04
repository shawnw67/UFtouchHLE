/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal ReplayKit support for apps that probe screen-recording availability.

use crate::dyld::HostDylib;
use crate::objc::{id, nil, objc_classes, ClassExports, HostObject, NSZonePtr};

#[derive(Default)]
pub struct State {
    shared_recorder: id,
}

#[derive(Default)]
struct RPScreenRecorderHostObject {
    microphone_enabled: bool,
    camera_enabled: bool,
}
impl HostObject for RPScreenRecorderHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation RPScreenRecorder: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<RPScreenRecorderHostObject>::default(),
        &mut env.mem,
    )
}

+ (id)sharedRecorder {
    let existing = env.framework_state.replay_kit.shared_recorder;
    if existing != nil {
        return existing;
    }
    let recorder = env.objc.alloc_static_object(
        this,
        Box::<RPScreenRecorderHostObject>::default(),
        &mut env.mem,
    );
    env.framework_state.replay_kit.shared_recorder = recorder;
    recorder
}

- (bool)isAvailable {
    false
}

- (bool)isRecording {
    false
}

- (bool)isMicrophoneEnabled {
    env.objc
        .borrow::<RPScreenRecorderHostObject>(this)
        .microphone_enabled
}

- (())setMicrophoneEnabled:(bool)enabled {
    env.objc
        .borrow_mut::<RPScreenRecorderHostObject>(this)
        .microphone_enabled = enabled;
}

- (bool)isCameraEnabled {
    env.objc.borrow::<RPScreenRecorderHostObject>(this).camera_enabled
}

- (())setCameraEnabled:(bool)enabled {
    env.objc
        .borrow_mut::<RPScreenRecorderHostObject>(this)
        .camera_enabled = enabled;
}

@end

};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/ReplayKit.framework/ReplayKit",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};
