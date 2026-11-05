/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSProcessInfo`.

use super::{NSTimeInterval, NSUInteger};
use crate::frameworks::foundation::ns_string;
use crate::libc::mach::host::PHYSICAL_MEMORY;
use crate::objc::{autorelease, id, msg, msg_class, objc_classes, ClassExports};
use crate::Environment;
use std::time::Instant;

#[derive(Default)]
pub struct State {
    /// `NSProcessInfo*`
    process_info: Option<id>,
}

fn assert_process_info_singleton(env: &mut Environment, this: id) {
    assert_eq!(
        this,
        env.framework_state
            .foundation
            .ns_process_info
            .process_info
            .unwrap()
    );
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSProcessInfo: NSObject

+ (id)processInfo {
    if let Some(existing) = env.framework_state.foundation.ns_process_info.process_info {
        existing
    } else {
        let process_info: id = msg![env; this new];
        env.framework_state.foundation.ns_process_info.process_info = Some(process_info);
        process_info
    }
}

- (id)environment {
    // Snapshot the emulated process environment, not the host's variables.
    let vars: Vec<(String, String)> = env.env_vars.iter().map(|(key, value)| {
        (String::from_utf8_lossy(key).into_owned(),
         env.mem.cstr_at_utf8(*value).unwrap().to_string())
    }).collect();
    let dictionary: id = msg_class![env; NSMutableDictionary new];
    for (key, value) in vars {
        let key = ns_string::from_rust_string(env, key);
        let value = ns_string::from_rust_string(env, value);
        () = msg![env; dictionary setObject:value forKey:key];
        crate::objc::release(env, key);
        crate::objc::release(env, value);
    }
    let snapshot: id = msg![env; dictionary copy];
    crate::objc::release(env, dictionary);
    autorelease(env, snapshot)
}

- (NSTimeInterval)systemUptime {
    assert_process_info_singleton(env, this); // TODO
    Instant::now().duration_since(env.startup_time).as_secs_f64()
}

- (NSUInteger)processorCount {
    assert_process_info_singleton(env, this);
    crate::libc::sysctl::GUEST_PROCESSOR_COUNT as NSUInteger
}

- (NSUInteger)activeProcessorCount {
    msg![env; this processorCount]
}

- (u64)physicalMemory {
    assert_process_info_singleton(env, this); // TODO
    PHYSICAL_MEMORY.into()
}

- (id)processName {
    // This function probably just needs to return a unique value
    // Testing on macOS appears CFBundleName is used
    assert_process_info_singleton(env, this); // TODO
    let main_bundle: id = msg_class![env; NSBundle mainBundle];
    let name_key: id = ns_string::get_static_str(env, "CFBundleName");
    msg![env; main_bundle objectForInfoDictionaryKey:name_key]
}

@end

};
