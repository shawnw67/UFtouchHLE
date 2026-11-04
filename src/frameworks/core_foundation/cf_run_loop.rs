/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFRunLoop`.
//!
//! This is not even toll-free bridged to `NSRunLoop` in Apple's implementation,
//! but here it is the same type.

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_foundation::time::CFTimeInterval;
use crate::frameworks::core_foundation::CFRelease;
use crate::frameworks::core_foundation::CFRetain;
use crate::frameworks::core_foundation::CFTypeRef;
use crate::frameworks::foundation::ns_run_loop;
use crate::frameworks::foundation::ns_string;
use crate::mem::MutVoidPtr;
use crate::objc::nil;
use crate::objc::{id, msg, msg_class, release, retain};
use crate::Environment;

pub type CFRunLoopRef = super::CFTypeRef;
pub type CFRunLoopMode = super::cf_string::CFStringRef;

fn CFRunLoopGetCurrent(env: &mut Environment) -> CFRunLoopRef {
    msg_class![env; NSRunLoop currentRunLoop]
}

pub fn CFRunLoopGetMain(env: &mut Environment) -> CFRunLoopRef {
    msg_class![env; NSRunLoop mainRunLoop]
}

fn CFRunLoopRetain(env: &mut Environment, run_loop: CFRunLoopRef) -> CFRunLoopRef {
    if run_loop.is_null() {
        run_loop
    } else {
        retain(env, run_loop)
    }
}

fn CFRunLoopRelease(env: &mut Environment, run_loop: CFRunLoopRef) {
    if !run_loop.is_null() {
        release(env, run_loop);
    }
}

fn CFRunLoopRun(env: &mut Environment) {
    let run_loop = CFRunLoopGetCurrent(env);
    () = msg![env; run_loop run];
}

fn CFRunLoopStop(env: &mut Environment, run_loop: CFRunLoopRef) {
    if !run_loop.is_null() {
        ns_run_loop::stop_cf_run_loop(env, run_loop);
    }
}

fn CFRunLoopWakeUp(env: &mut Environment, run_loop: CFRunLoopRef) {
    if !run_loop.is_null() {
        ns_run_loop::wake_cf_run_loop(env, run_loop);
    }
}

fn CFRunLoopIsWaiting(env: &mut Environment, run_loop: CFRunLoopRef) -> bool {
    !run_loop.is_null() && ns_run_loop::cf_run_loop_is_waiting(env, run_loop)
}

fn CFRunLoopRunInMode(
    env: &mut Environment,
    mode: CFRunLoopMode,
    seconds: CFTimeInterval,
    _return_something: bool,
) -> i32 {
    // The underlying NSRunLoop currently keeps one shared set of sources and
    // timers. Treat custom modes like the default until modes are separated.
    assert!(!mode.is_null());
    let current_run_loop = CFRunLoopGetCurrent(env);
    let stopped = if seconds <= 0.0 {
        ns_run_loop::run_run_loop(env, current_run_loop, true, None)
    } else {
        let limit_date: id = msg_class![env; NSDate dateWithTimeIntervalSinceNow:seconds];
        let limit: f64 = msg![env; limit_date timeIntervalSince1970];
        ns_run_loop::run_run_loop(env, current_run_loop, false, Some(limit))
    };
    if stopped {
        2
    } else {
        1
    } // stopped or finished
}

pub const kCFRunLoopCommonModes: &str = "kCFRunLoopCommonModes";
pub const kCFRunLoopDefaultMode: &str = "kCFRunLoopDefaultMode";

// CFRunLoopActivity flags
type CFRunLoopActivity = u32;

pub type CFRunLoopObserverRef = CFTypeRef;

pub type CFRunLoopTimerRef = CFTypeRef;

const kCFRunLoopAllActivities: CFRunLoopActivity = 0x0FFFFFFF;

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFRunLoopCommonModes",
        HostConstant::NSString(kCFRunLoopCommonModes),
    ),
    (
        "_kCFRunLoopDefaultMode",
        HostConstant::NSString(kCFRunLoopDefaultMode),
    ),
];

fn CFRunLoopAddObserver(
    _env: &mut Environment,
    _rl: CFRunLoopRef,
    _observer: CFRunLoopObserverRef,
    _mode: CFRunLoopMode,
) {
    log!("CFRunLoopAddObserver: stubbed");
}

fn CFRunLoopContainsObserver(
    _env: &mut Environment,
    _rl: CFRunLoopRef,
    _observer: CFRunLoopObserverRef,
    _mode: CFRunLoopMode,
) -> bool {
    false
}

fn CFRunLoopContainsTimer(
    _env: &mut Environment,
    _rl: CFRunLoopRef,
    _timer: CFRunLoopTimerRef,
    _mode: CFRunLoopMode,
) -> bool {
    false
}

fn CFRunLoopCopyAllModes(env: &mut Environment, _rl: CFRunLoopRef) -> CFTypeRef {
    // Return an array containing only the default mode.
    let mode = ns_string::get_static_str(env, kCFRunLoopDefaultMode);
    let arr: id = msg_class![env; NSArray arrayWithObject:mode];
    arr
}

fn CFRunLoopCopyCurrentMode(env: &mut Environment, _rl: CFRunLoopRef) -> CFRunLoopMode {
    // We only support the default mode.
    let s = ns_string::from_rust_string(env, kCFRunLoopDefaultMode.to_string());
    crate::objc::autorelease(env, s)
}

fn CFRunLoopObserverCreate(
    _env: &mut Environment,
    _allocator: CFTypeRef,
    _activities: CFRunLoopActivity,
    _repeats: bool,
    _order: i32,
    _callout: MutVoidPtr, // CFRunLoopObserverCallBack
    _context: MutVoidPtr, // CFRunLoopObserverContext*
) -> CFRunLoopObserverRef {
    log!("CFRunLoopObserverCreate: stubbed, returning null");
    nil
}

fn CFRunLoopObserverDoesRepeat(_env: &mut Environment, _observer: CFRunLoopObserverRef) -> bool {
    false
}

fn CFRunLoopObserverGetActivities(
    _env: &mut Environment,
    _observer: CFRunLoopObserverRef,
) -> CFRunLoopActivity {
    kCFRunLoopAllActivities
}

fn CFRunLoopObserverGetOrder(_env: &mut Environment, _observer: CFRunLoopObserverRef) -> i32 {
    0
}

fn CFRunLoopObserverInvalidate(_env: &mut Environment, _observer: CFRunLoopObserverRef) {
    log_dbg!("CFRunLoopObserverInvalidate: stubbed");
}

fn CFRunLoopObserverIsValid(_env: &mut Environment, observer: CFRunLoopObserverRef) -> bool {
    !observer.is_null()
}

fn CFRunLoopObserverRelease(env: &mut Environment, observer: CFRunLoopObserverRef) {
    if !observer.is_null() {
        CFRelease(env, observer);
    }
}

fn CFRunLoopObserverRetain(
    env: &mut Environment,
    observer: CFRunLoopObserverRef,
) -> CFRunLoopObserverRef {
    if !observer.is_null() {
        CFRetain(env, observer)
    } else {
        observer
    }
}

fn CFRunLoopRemoveObserver(
    _env: &mut Environment,
    _rl: CFRunLoopRef,
    _observer: CFRunLoopObserverRef,
    _mode: CFRunLoopMode,
) {
    log!("CFRunLoopRemoveObserver: stubbed");
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFRunLoopGetCurrent()),
    export_c_func!(CFRunLoopGetMain()),
    export_c_func!(CFRunLoopRetain(_)),
    export_c_func!(CFRunLoopRelease(_)),
    export_c_func!(CFRunLoopRun()),
    export_c_func!(CFRunLoopStop(_)),
    export_c_func!(CFRunLoopWakeUp(_)),
    export_c_func!(CFRunLoopIsWaiting(_)),
    export_c_func!(CFRunLoopRunInMode(_, _, _)),
    export_c_func!(CFRunLoopAddObserver(_, _, _)),
    export_c_func!(CFRunLoopContainsObserver(_, _, _)),
    export_c_func!(CFRunLoopContainsTimer(_, _, _)),
    export_c_func!(CFRunLoopCopyAllModes(_)),
    export_c_func!(CFRunLoopCopyCurrentMode(_)),
    export_c_func!(CFRunLoopObserverCreate(_, _, _, _, _, _)),
    export_c_func!(CFRunLoopObserverDoesRepeat(_)),
    export_c_func!(CFRunLoopObserverGetActivities(_)),
    export_c_func!(CFRunLoopObserverGetOrder(_)),
    export_c_func!(CFRunLoopObserverInvalidate(_)),
    export_c_func!(CFRunLoopObserverIsValid(_)),
    export_c_func!(CFRunLoopObserverRelease(_)),
    export_c_func!(CFRunLoopObserverRetain(_)),
    export_c_func!(CFRunLoopRemoveObserver(_, _, _)),
];
