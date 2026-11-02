/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSOperation` and `NSOperationQueue`.
//!
//! Operations added to a queue run one at a time on the main thread, when its
//! run loop next runs, rather than on other threads.

use crate::frameworks::foundation::NSInteger;
use crate::objc::{
    id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};

#[derive(Default)]
struct NSOperationHostObject {
    cancelled: bool,
    executing: bool,
    finished: bool,
}
impl HostObject for NSOperationHostObject {}

#[derive(Default)]
struct NSOperationQueueHostObject {
    /// Operations waiting to run, retained.
    pending: Vec<id>,
    max_concurrent_operation_count: NSInteger,
    suspended: bool,
}
impl HostObject for NSOperationQueueHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSOperation: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<NSOperationHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())start {
    if env.objc.borrow::<NSOperationHostObject>(this).cancelled {
        env.objc.borrow_mut::<NSOperationHostObject>(this).finished = true;
        return;
    }
    env.objc.borrow_mut::<NSOperationHostObject>(this).executing = true;
    () = msg![env; this main];
    let host_object = env.objc.borrow_mut::<NSOperationHostObject>(this);
    host_object.executing = false;
    host_object.finished = true;
}
- (())main {
}
- (())cancel {
    env.objc.borrow_mut::<NSOperationHostObject>(this).cancelled = true;
}
- (bool)isCancelled {
    env.objc.borrow::<NSOperationHostObject>(this).cancelled
}
- (bool)isExecuting {
    env.objc.borrow::<NSOperationHostObject>(this).executing
}
- (bool)isFinished {
    env.objc.borrow::<NSOperationHostObject>(this).finished
}
- (bool)isConcurrent {
    false
}
- (bool)isReady {
    true
}

@end

@implementation NSOperationQueue: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSOperationQueueHostObject {
        max_concurrent_operation_count: -1,
        ..Default::default()
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    let pending = std::mem::take(&mut env.objc.borrow_mut::<NSOperationQueueHostObject>(this).pending);
    for operation in pending {
        release(env, operation);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (())addOperation:(id)operation {
    retain(env, operation);
    env.objc.borrow_mut::<NSOperationQueueHostObject>(this).pending.push(operation);
    // Retained until _Podroid_runNextOperation runs.
    retain(env, this);
    let sel = env.objc.lookup_selector("_Podroid_runNextOperation").unwrap();
    () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0f64];
}

- (())_Podroid_runNextOperation {
    let host_object = env.objc.borrow_mut::<NSOperationQueueHostObject>(this);
    if !host_object.suspended && !host_object.pending.is_empty() {
        let operation = host_object.pending.remove(0);
        () = msg![env; operation start];
        release(env, operation);
    }
    release(env, this);
}

- (NSInteger)operationCount {
    env.objc.borrow::<NSOperationQueueHostObject>(this).pending.len() as NSInteger
}
- (id)operations {
    let pending = env.objc.borrow::<NSOperationQueueHostObject>(this).pending.clone();
    for &operation in &pending {
        retain(env, operation);
    }
    let array = crate::frameworks::foundation::ns_array::from_vec(env, pending);
    crate::objc::autorelease(env, array)
}
- (())cancelAllOperations {
    let pending = env.objc.borrow::<NSOperationQueueHostObject>(this).pending.clone();
    for operation in pending {
        () = msg![env; operation cancel];
    }
}
- (())waitUntilAllOperationsAreFinished {
    loop {
        let host_object = env.objc.borrow_mut::<NSOperationQueueHostObject>(this);
        if host_object.pending.is_empty() {
            break;
        }
        let operation = host_object.pending.remove(0);
        () = msg![env; operation start];
        release(env, operation);
    }
}
- (NSInteger)maxConcurrentOperationCount {
    env.objc.borrow::<NSOperationQueueHostObject>(this).max_concurrent_operation_count
}
- (())setMaxConcurrentOperationCount:(NSInteger)count {
    env.objc.borrow_mut::<NSOperationQueueHostObject>(this).max_concurrent_operation_count = count;
}
- (bool)isSuspended {
    env.objc.borrow::<NSOperationQueueHostObject>(this).suspended
}
- (())setSuspended:(bool)suspended {
    env.objc.borrow_mut::<NSOperationQueueHostObject>(this).suspended = suspended;
}
- (())setName:(id)_name {
}

@end

};
