/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Version-zero Core Foundation run loop sources.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_run_loop::{CFRunLoopMode, CFRunLoopRef};
use super::{CFIndex, CFTypeRef};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::ns_run_loop;
use crate::mem::{MutPtr, MutVoidPtr, SafeRead};
use crate::objc::{id, nil, objc_classes, release, retain, ClassExports, HostObject};
use crate::Environment;

type CFRunLoopSourceRef = CFTypeRef;

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct SourceContext {
    version: CFIndex,
    info: MutVoidPtr,
    retain: GuestFunction,
    release: GuestFunction,
    copy_description: GuestFunction,
    equal: GuestFunction,
    hash: GuestFunction,
    schedule: GuestFunction,
    cancel: GuestFunction,
    perform: GuestFunction,
}
unsafe impl SafeRead for SourceContext {}

struct Source {
    context: SourceContext,
    order: CFIndex,
    valid: bool,
    signaled: bool,
    // Run loops are static objects. Modes are retained until removal.
    registrations: Vec<(CFRunLoopRef, CFRunLoopMode)>,
}
impl HostObject for Source {}

fn CFRunLoopSourceCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    order: CFIndex,
    context_ptr: MutPtr<SourceContext>,
) -> CFRunLoopSourceRef {
    if context_ptr.is_null()
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return nil;
    }
    let mut context = env.mem.read(context_ptr);
    if context.version != 0 {
        return nil;
    }
    let callback = context.retain;
    if !callback.to_ptr().is_null() {
        context.info = callback.call_from_host(env, (context.info,));
    }
    let class = env
        .objc
        .get_known_class("_Podroid_CFRunLoopSource", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(Source {
            context,
            order,
            valid: true,
            signaled: false,
            registrations: Vec::new(),
        }),
        &mut env.mem,
    )
}

fn CFRunLoopAddSource(
    env: &mut Environment,
    run_loop: CFRunLoopRef,
    source: CFRunLoopSourceRef,
    mode: CFRunLoopMode,
) {
    if run_loop.is_null() || source.is_null() || mode.is_null() {
        return;
    }
    let host = env.objc.borrow::<Source>(source);
    // Our run loops currently share all modes; keep one registration per loop.
    if !host.valid || host.registrations.iter().any(|&(rl, _)| rl == run_loop) {
        return;
    }
    retain(env, source);
    retain(env, mode);
    let host = env.objc.borrow_mut::<Source>(source);
    host.registrations.push((run_loop, mode));
    let context = host.context;
    ns_run_loop::add_cf_source(env, run_loop, source);
    let callback = context.schedule;
    if !callback.to_ptr().is_null() {
        let _: () = callback.call_from_host(env, (context.info, run_loop, mode));
    }
}

fn remove_source(env: &mut Environment, run_loop: CFRunLoopRef, source: CFRunLoopSourceRef) {
    let host = env.objc.borrow_mut::<Source>(source);
    let Some(index) = host
        .registrations
        .iter()
        .position(|&(rl, _)| rl == run_loop)
    else {
        return;
    };
    let (_, mode) = host.registrations.remove(index);
    let context = host.context;
    ns_run_loop::remove_cf_source(env, run_loop, source);
    let callback = context.cancel;
    if !callback.to_ptr().is_null() {
        let _: () = callback.call_from_host(env, (context.info, run_loop, mode));
    }
    release(env, mode);
    release(env, source);
}

fn CFRunLoopRemoveSource(
    env: &mut Environment,
    run_loop: CFRunLoopRef,
    source: CFRunLoopSourceRef,
    _mode: CFRunLoopMode,
) {
    if !run_loop.is_null() && !source.is_null() {
        remove_source(env, run_loop, source);
    }
}

fn CFRunLoopContainsSource(
    env: &mut Environment,
    run_loop: CFRunLoopRef,
    source: CFRunLoopSourceRef,
    _mode: CFRunLoopMode,
) -> bool {
    !run_loop.is_null()
        && !source.is_null()
        && env
            .objc
            .borrow::<Source>(source)
            .registrations
            .iter()
            .any(|&(rl, _)| rl == run_loop)
}

fn CFRunLoopSourceInvalidate(env: &mut Environment, source: CFRunLoopSourceRef) {
    if source.is_null() || !env.objc.borrow::<Source>(source).valid {
        return;
    }
    retain(env, source);
    env.objc.borrow_mut::<Source>(source).valid = false;
    while let Some((run_loop, _)) = env
        .objc
        .borrow::<Source>(source)
        .registrations
        .last()
        .copied()
    {
        remove_source(env, run_loop, source);
    }
    release(env, source);
}
fn CFRunLoopSourceIsValid(env: &mut Environment, source: CFRunLoopSourceRef) -> bool {
    !source.is_null() && env.objc.borrow::<Source>(source).valid
}
fn CFRunLoopSourceSignal(env: &mut Environment, source: CFRunLoopSourceRef) {
    if !source.is_null() {
        let host = env.objc.borrow_mut::<Source>(source);
        if host.valid {
            host.signaled = true;
        }
    }
}
fn CFRunLoopSourceGetOrder(env: &mut Environment, source: CFRunLoopSourceRef) -> CFIndex {
    env.objc.borrow::<Source>(source).order
}
fn CFRunLoopSourceGetContext(
    env: &mut Environment,
    source: CFRunLoopSourceRef,
    out: MutPtr<SourceContext>,
) {
    let context = env.objc.borrow::<Source>(source).context;
    env.mem.write(out, context);
}
fn CFRunLoopSourceRetain(env: &mut Environment, source: CFRunLoopSourceRef) -> CFRunLoopSourceRef {
    if source.is_null() {
        source
    } else {
        retain(env, source)
    }
}
fn CFRunLoopSourceRelease(env: &mut Environment, source: CFRunLoopSourceRef) {
    if !source.is_null() {
        release(env, source);
    }
}

pub fn perform_signaled_sources(env: &mut Environment, run_loop: id) {
    let mut sources = ns_run_loop::cf_sources(env, run_loop);
    for &source in &sources {
        retain(env, source);
    }
    sources.sort_by_key(|&source| env.objc.borrow::<Source>(source).order);
    for source in sources {
        let host = env.objc.borrow_mut::<Source>(source);
        let perform =
            host.valid && host.signaled && host.registrations.iter().any(|&(rl, _)| rl == run_loop);
        let context = host.context;
        if perform {
            host.signaled = false;
        }
        let callback = context.perform;
        if perform && !callback.to_ptr().is_null() {
            let _: () = callback.call_from_host(env, (context.info,));
        }
        release(env, source);
    }
}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation _Podroid_CFRunLoopSource: NSObject
- (())dealloc {
    let context = env.objc.borrow::<Source>(this).context;
    let callback = context.release;
    if !callback.to_ptr().is_null() {
        let _: () = callback.call_from_host(env, (context.info,));
    }
    env.objc.dealloc_object(this, &mut env.mem);
}
@end
};

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFRunLoopSourceCreate(_, _, _)),
    export_c_func!(CFRunLoopAddSource(_, _, _)),
    export_c_func!(CFRunLoopRemoveSource(_, _, _)),
    export_c_func!(CFRunLoopContainsSource(_, _, _)),
    export_c_func!(CFRunLoopSourceInvalidate(_)),
    export_c_func!(CFRunLoopSourceIsValid(_)),
    export_c_func!(CFRunLoopSourceSignal(_)),
    export_c_func!(CFRunLoopSourceGetOrder(_)),
    export_c_func!(CFRunLoopSourceGetContext(_, _)),
    export_c_func!(CFRunLoopSourceRetain(_)),
    export_c_func!(CFRunLoopSourceRelease(_)),
];
