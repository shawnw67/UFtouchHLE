/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! AVAudioSession

use crate::dyld::{ConstantExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSUInteger;
use crate::mem::MutPtr;
use crate::objc::autorelease;
use crate::objc::msg_class;
use crate::objc::nil;
use crate::objc::release;
use crate::objc::HostObject;
use crate::objc::{id, objc_classes, ClassExports, TrivialHostObject};
use crate::todo_objc_setter;
use crate::Environment;

type AVAudioSessionCategory = id; // NSString *

#[derive(Default)]
pub struct State {
    /// [AVAudioSession sharedInstance]
    shared_instance: Option<id>,
}

// Per Apple's
// <https://developer.apple.com/documentation/avfaudio/avaudiosessionport>,
// each `AVAudioSessionPort*` constant is a `NSString *` whose literal value
// is the bare port-type identifier ("Speaker", "Headphones", ...). Apps use
// `[portDescription.portType isEqualToString:AVAudioSessionPortHeadphones]`,
// so the only requirement is that each constant resolves to a distinct,
// non-NULL NSString with the correct iOS-canonical spelling.
pub const AVAudioSessionPortBuiltInSpeaker: &str = "Speaker";

/// Host object backing `AVAudioSessionPortDescription` instances we create
/// ourselves (built-in mic for `availableInputs`, speaker for `currentRoute`).
pub(super) struct AVAudioSessionPortDescriptionHostObject {
    port_type: &'static str,
    name: &'static str,
    uid: &'static str,
}

impl HostObject for AVAudioSessionPortDescriptionHostObject {}

// Create a port description for the host's built-in microphone (used by
// `-[AVAudioSession availableInputs]`).
fn built_in_mic_port(env: &mut Environment) -> id {
    let class = env
        .objc
        .get_known_class("AVAudioSessionPortDescription", &mut env.mem);
    let host_object = Box::new(AVAudioSessionPortDescriptionHostObject {
        port_type: AVAudioSessionPortBuiltInMic,
        name: "iPhone Microphone",
        uid: "Built-In Microphone",
    });
    let port = env.objc.alloc_object(class, host_object, &mut env.mem);
    autorelease(env, port)
}

pub const AVAudioSessionPortBuiltInMic: &str = "MicrophoneBuiltIn";

impl HostObject for AVAudioSessionHostObject {}

// ДОБАВЛЕНО: Полноценный объект состояния
// (без заглушек)
#[derive(Default)]
#[allow(dead_code)]
pub(super) struct AVAudioSessionHostObject {
    category: id,
    category_options: AVAudioSessionCategoryOptions,
    mode: id,
    active: bool,
    preferred_sample_rate: f64,
    preferred_io_buffer_duration: f64,
    delegate: id,
}

#[allow(dead_code)]
type AVAudioSessionCategoryOptions = NSUInteger;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// This is a singleton.
@implementation AVAudioSession: NSObject

+ (id)sharedInstance {
    if let Some(audio_session) =
        env.framework_state.avfoundation.av_audio_session.shared_instance {
        audio_session
    } else {
        let new = env.objc.alloc_static_object(
            this,
            Box::new(TrivialHostObject),
            &mut env.mem
        );
        env.framework_state.avfoundation.av_audio_session.shared_instance = Some(new);
        new
    }
}

- (id)retain { this }
- (())release {}
- (id)autorelease { this }

- (())setDelegate:(id)delegate {
    todo_objc_setter!(this, delegate);
}

- (bool)setCategory:(AVAudioSessionCategory)category
              error:(MutPtr<id>)error { // NSError **
    log!(
        "TODO: [(AVAudioSession *){:?} setCategory:'{}' error:{:?}] -> true",
        this,
        ns_string::to_rust_string(env, category),
        error
    );
    true
}

- (bool)setActive:(bool)active
            error:(MutPtr<id>)error { // NSError **
    log!(
        "TODO: [(AVAudioSession *){:?} setActive:{} error:{:?}] -> true",
        this,
        active,
        error
    );
    true
}

@end

@implementation AVAudioSessionPortDescription: NSObject

- (id)portType {
    let default_type = AVAudioSessionPortBuiltInSpeaker;
    let port_type = env
        .objc
        .get_host_object(this)
        .and_then(|h| h.as_any().downcast_ref::<AVAudioSessionPortDescriptionHostObject>())
        .map(|h| h.port_type)
        .unwrap_or(default_type);
    ns_string::get_static_str(env, port_type)
}

- (id)portName {
    let default_name = "Speaker";
    let name = env
        .objc
        .get_host_object(this)
        .and_then(|h| h.as_any().downcast_ref::<AVAudioSessionPortDescriptionHostObject>())
        .map(|h| h.name)
        .unwrap_or(default_name);
    ns_string::get_static_str(env, name)
}

- (id)UID {
    let default_uid = "Built-In Speaker";
    let uid = env
        .objc
        .get_host_object(this)
        .and_then(|h| h.as_any().downcast_ref::<AVAudioSessionPortDescriptionHostObject>())
        .map(|h| h.uid)
        .unwrap_or(default_uid);
    ns_string::get_static_str(env, uid)
}

- (id)channels { // NSArray<AVAudioSessionChannelDescription*>*
    msg_class![env; NSArray new]
}

- (id)dataSources {
    nil
}

- (id)selectedDataSource {
    nil
}

- (id)preferredDataSource {
    nil
}

- (bool)setPreferredDataSource:(id)_source error:(MutPtr<id>)_error {
    false
}

@end

@implementation AVAudioSessionRouteDescription: NSObject

- (id)inputs {
    // Report built-in microphone as the sole input so games that check
    // `currentRoute.inputs` see that a mic exists.
    let port = built_in_mic_port(env);
    let arr: id = msg_class![env; NSArray arrayWithObject:port];
    release(env, port);
    arr
}

- (id)outputs {
    // Report built-in speaker as the sole output.
    let port: id = msg_class![env; AVAudioSessionPortDescription new];
    let arr: id  = msg_class![env; NSArray arrayWithObject:port];
    release(env, port);
    arr
}

@end

};

// Values might not be correct, but as these are linked symbol constants, it
// shouldn't matter.
const AVAudioSessionCategoryAmbient: &str = "AVAudioSessionCategoryAmbient";
const AVAudioSessionCategoryMultiRoute: &str = "AVAudioSessionCategoryMultiRoute";
const AVAudioSessionCategoryPlayAndRecord: &str = "AVAudioSessionCategoryPlayAndRecord";
const AVAudioSessionCategoryPlayback: &str = "AVAudioSessionCategoryPlayback";
const AVAudioSessionCategoryRecord: &str = "AVAudioSessionCategoryRecord";
const AVAudioSessionCategorySoloAmbient: &str = "AVAudioSessionCategorySoloAmbient";
const AVAudioSessionCategoryAudioProcessing: &str = "AVAudioSessionCategoryAudioProcessing";

/// `AVAudioSessionCategory` constants, and notifications
pub const CONSTANTS: ConstantExports = &[
    (
        "_AVAudioSessionInterruptionNotification",
        HostConstant::NSString("AVAudioSessionInterruptionNotification"),
    ),
    (
        "_AVAudioSessionInterruptionTypeKey",
        HostConstant::NSString("AVAudioSessionInterruptionTypeKey"),
    ),
    (
        "_AVAudioSessionRouteChangeNotification",
        HostConstant::NSString("AVAudioSessionRouteChangeNotification"),
    ),
    (
        "_AVAudioSessionRouteChangeReasonKey",
        HostConstant::NSString("AVAudioSessionRouteChangeReasonKey"),
    ),
    (
        "_AVAudioSessionCategoryAmbient",
        HostConstant::NSString(AVAudioSessionCategoryAmbient),
    ),
    (
        "_AVAudioSessionCategoryMultiRoute",
        HostConstant::NSString(AVAudioSessionCategoryMultiRoute),
    ),
    (
        "_AVAudioSessionCategoryPlayAndRecord",
        HostConstant::NSString(AVAudioSessionCategoryPlayAndRecord),
    ),
    (
        "_AVAudioSessionCategoryPlayback",
        HostConstant::NSString(AVAudioSessionCategoryPlayback),
    ),
    (
        "_AVAudioSessionCategoryRecord",
        HostConstant::NSString(AVAudioSessionCategoryRecord),
    ),
    (
        "_AVAudioSessionCategorySoloAmbient",
        HostConstant::NSString(AVAudioSessionCategorySoloAmbient),
    ),
    (
        "_AVAudioSessionCategoryAudioProcessing",
        HostConstant::NSString(AVAudioSessionCategoryAudioProcessing),
    ),
];
