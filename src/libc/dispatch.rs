/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Grand Central Dispatch (`dispatch/dispatch.h`) and the Blocks runtime used
//! to enqueue callbacks.
//! Block layout: <https://clang.llvm.org/docs/Block-ABI-Apple.html>
//!
//! This is a simplified implementation: all work runs on the main thread,
//! either when the main run loop next runs (`dispatch_async`, `dispatch_after`,
//! timer sources) or immediately (`dispatch_sync`, `dispatch_group_async`).
//! Global queues are treated like the main queue. Semaphores do block other
//! threads.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::libc::semaphore::{host_create_semaphore, sem_t};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{id, msg_class, objc_classes, ClassExports};
use crate::Environment;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[allow(non_camel_case_types)]
type dispatch_time_t = u64;
const DISPATCH_TIME_NOW: dispatch_time_t = 0;
const DISPATCH_TIME_FOREVER: dispatch_time_t = !0;
/// `KERN_OPERATION_TIMED_OUT`, returned by waits that time out.
const TIMED_OUT: i32 = 49;

const HAS_COPY_DISPOSE: u32 = 1 << 25;
const IS_GLOBAL: u32 = 1 << 28;

#[derive(Default)]
pub struct State {
    block_class: id,
    main_queue: MutVoidPtr,
    global_queue: MutVoidPtr,
    source_type_timer: MutVoidPtr,
    pending: VecDeque<MutVoidPtr>,
    /// Blocks from `dispatch_after`, and when to run them.
    delayed: Vec<(Instant, MutVoidPtr)>,
    /// Host state for dispatch objects other than queues.
    objects: HashMap<MutVoidPtr, DispatchObject>,
    /// Reference point for `dispatch_time_t` values.
    epoch: Option<Instant>,
    copies: HashMap<MutVoidPtr, u32>,
    /// Heap-promoted __block cells: one stack owner plus each copied Block.
    byrefs: HashMap<MutVoidPtr, u32>,
    draining: bool,
}

enum DispatchObject {
    Group,
    Semaphore(MutPtr<sem_t>),
    TimerSource(TimerSource),
}

struct TimerSource {
    /// Copied blocks, or null.
    event_handler: MutVoidPtr,
    cancel_handler: MutVoidPtr,
    next_fire: Option<Instant>,
    interval: Option<Duration>,
    /// Sources are created suspended.
    suspend_count: i32,
    cancelled: bool,
}

/// Turn a `dispatch_time_t` into an [Instant] ([None] if it's "forever").
fn time_to_instant(env: &mut Environment, time: dispatch_time_t) -> Option<Instant> {
    let epoch = *env
        .libc_state
        .dispatch
        .epoch
        .get_or_insert_with(Instant::now);
    match time {
        DISPATCH_TIME_FOREVER => None,
        DISPATCH_TIME_NOW => Some(Instant::now()),
        _ => Some(epoch + Duration::from_nanos(time)),
    }
}

fn dispatch_time(env: &mut Environment, when: dispatch_time_t, delta: i64) -> dispatch_time_t {
    let epoch = *env
        .libc_state
        .dispatch
        .epoch
        .get_or_insert_with(Instant::now);
    let base = match when {
        DISPATCH_TIME_FOREVER => return DISPATCH_TIME_FOREVER,
        DISPATCH_TIME_NOW => epoch.elapsed().as_nanos() as i64,
        _ => when as i64,
    };
    // Avoid returning DISPATCH_TIME_NOW for a time that isn't "now".
    base.saturating_add(delta).max(1) as dispatch_time_t
}

fn new_object(env: &mut Environment, object: DispatchObject) -> MutVoidPtr {
    let ptr = env.mem.calloc(64);
    env.libc_state.dispatch.objects.insert(ptr, object);
    ptr
}

/// The function that runs a block. It takes the block as its first argument,
/// followed by the block's own arguments.
pub fn block_invoke_function(env: &Environment, block: ConstVoidPtr) -> GuestFunction {
    GuestFunction::from_addr_with_thumb_bit(block_word(env, block, 3))
}

/// Call a block that takes no arguments.
fn invoke_block(env: &mut Environment, block: ConstVoidPtr) {
    let invoke = GuestFunction::from_addr_with_thumb_bit(block_word(env, block, 3));
    let trace = crate::network_trace::callback_begin(env, block.to_bits());
    let _: () = invoke.call_from_host(env, (block,));
    crate::network_trace::end(env, trace);
}

fn main_queue(env: &mut Environment) -> ConstVoidPtr {
    if env.libc_state.dispatch.main_queue.is_null() {
        env.libc_state.dispatch.main_queue = env.mem.calloc(64);
    }
    env.libc_state.dispatch.main_queue.cast_const()
}

/// `DISPATCH_SOURCE_TYPE_TIMER` is the address of this symbol.
fn source_type_timer(env: &mut Environment) -> ConstVoidPtr {
    if env.libc_state.dispatch.source_type_timer.is_null() {
        env.libc_state.dispatch.source_type_timer = env.mem.calloc(16);
    }
    env.libc_state.dispatch.source_type_timer.cast_const()
}

fn block_class(env: &mut Environment) -> ConstVoidPtr {
    let class = env.objc.get_known_class("_touchHLE_NSBlock", &mut env.mem);
    env.libc_state.dispatch.block_class = class;
    class.cast_void().cast_const()
}

/// Block literals live in guest stack/static memory, outside ObjC's object map.
pub(crate) fn is_block(env: &Environment, object: id) -> bool {
    let class = env.libc_state.dispatch.block_class;
    !class.is_null() && env.mem.read(object.cast::<id>()) == class
}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation _touchHLE_NSBlock: NSObject
- (id)copyWithZone:(MutVoidPtr)_zone {
    _Block_copy(env, this.cast_void().cast_const()).cast()
}
- (id)copy {
    _Block_copy(env, this.cast_void().cast_const()).cast()
}
- (id)retain {
    let block = this.cast_void();
    if let Some(count) = env.libc_state.dispatch.copies.get_mut(&block) {
        *count = count.checked_add(1).unwrap();
    }
    this
}
- (())release { _Block_release(env, this.cast_void()); }
- (id)autorelease {
    let _: () = msg_class![env; NSAutoreleasePool addObject:this];
    this
}
@end
};

fn block_word(env: &Environment, block: ConstVoidPtr, word: u32) -> u32 {
    env.mem.read(block.cast::<u32>() + word)
}

pub(crate) fn _Block_copy(env: &mut Environment, block: ConstVoidPtr) -> MutVoidPtr {
    if block.is_null() || block_word(env, block, 1) & IS_GLOBAL != 0 {
        return block.cast_mut();
    }
    if let Some(count) = env.libc_state.dispatch.copies.get_mut(&block.cast_mut()) {
        *count = count.checked_add(1).unwrap();
        return block.cast_mut();
    }
    let descriptor = ConstPtr::<u32>::from_bits(block_word(env, block, 4));
    let size = env.mem.read(descriptor + 1);
    assert!(size >= 20);
    let copy = env.mem.alloc(size);
    env.mem.memmove(copy, block, size);
    env.libc_state.dispatch.copies.insert(copy, 1);
    if block_word(env, block, 1) & HAS_COPY_DISPOSE != 0 {
        let helper = GuestFunction::from_addr_with_thumb_bit(env.mem.read(descriptor + 2));
        let _: () = helper.call_from_host(env, (copy, block));
    }
    copy
}

pub(crate) fn _Block_release(env: &mut Environment, block: MutVoidPtr) {
    let Some(count) = env.libc_state.dispatch.copies.get_mut(&block) else {
        return;
    };
    *count -= 1;
    if *count != 0 {
        return;
    }
    env.libc_state.dispatch.copies.remove(&block);
    crate::network_trace::release_callback(env, block.to_bits());
    if block_word(env, block.cast_const(), 1) & HAS_COPY_DISPOSE != 0 {
        let descriptor = ConstPtr::<u32>::from_bits(block_word(env, block.cast_const(), 4));
        let helper = GuestFunction::from_addr_with_thumb_bit(env.mem.read(descriptor + 3));
        let _: () = helper.call_from_host(env, (block,));
    }
    env.mem.free(block);
}

fn _Block_object_assign(
    env: &mut Environment,
    dest: MutPtr<MutVoidPtr>,
    object: MutVoidPtr,
    flags: i32,
) {
    // MRC __block object helpers use BYREF_CALLER; ownership of their payload
    // is unchanged. ARC strong helpers instead use the ordinary object path.
    if flags & 128 != 0 {
        env.mem.write(dest, object);
        return;
    }
    let object = match flags & 0xf {
        3 => {
            if flags & 16 == 0 {
                crate::objc::retain(env, object.cast());
            }
            object
        }
        7 => _Block_copy(env, object.cast_const()),
        8 => copy_byref(env, object),
        _ => unimplemented!("Block capture flags {flags:#x}"),
    };
    env.mem.write(dest, object);
}

fn _Block_object_dispose(env: &mut Environment, object: MutVoidPtr, flags: i32) {
    if flags & 128 != 0 {
        return;
    }
    match flags & 0xf {
        3 => {
            if flags & 16 == 0 {
                crate::objc::release(env, object.cast());
            }
        }
        7 => _Block_release(env, object),
        8 => release_byref(env, object),
        _ => unimplemented!("Block capture flags {flags:#x}"),
    }
}

// Clang's 32-bit Block ABI: isa, forwarding, flags, size, optional
// keep/destroy.
// The forwarding cell is installed before calling guest helpers (which may
// recursively capture the same cell). Never keep a State borrow across helpers.
fn copy_byref(env: &mut Environment, original: MutVoidPtr) -> MutVoidPtr {
    let words = original.cast::<u32>();
    let forwarded = MutVoidPtr::from_bits(env.mem.read(words + 1));
    if let Some(count) = env.libc_state.dispatch.byrefs.get_mut(&forwarded) {
        *count = count.checked_add(1).unwrap();
        return forwarded;
    }
    let size = env.mem.read(words + 3);
    let flags = env.mem.read(words + 2);
    assert!(
        size >= if flags & HAS_COPY_DISPOSE != 0 {
            24
        } else {
            16
        }
    );
    let copy = env.mem.alloc(size);
    env.mem.memmove(copy, original.cast_const(), size);
    env.mem.write(words + 1, copy.to_bits());
    env.mem.write(copy.cast::<u32>() + 1, copy.to_bits());
    env.libc_state.dispatch.byrefs.insert(copy, 2);
    if flags & HAS_COPY_DISPOSE != 0 {
        let helper = GuestFunction::from_addr_with_thumb_bit(env.mem.read(words + 4));
        let _: () = helper.call_from_host(env, (copy, original));
    }
    copy
}

fn release_byref(env: &mut Environment, original: MutVoidPtr) {
    let forwarded = MutVoidPtr::from_bits(env.mem.read(original.cast::<u32>() + 1));
    let Some(count) = env.libc_state.dispatch.byrefs.get_mut(&forwarded) else {
        // A stack cell never copied to the heap has no owned allocation.
        return;
    };
    *count -= 1;
    if *count != 0 {
        return;
    }
    env.libc_state.dispatch.byrefs.remove(&forwarded);
    let words = forwarded.cast::<u32>();
    if env.mem.read(words + 2) & HAS_COPY_DISPOSE != 0 {
        let helper = GuestFunction::from_addr_with_thumb_bit(env.mem.read(words + 5));
        let _: () = helper.call_from_host(env, (forwarded,));
    }
    env.mem.free(forwarded);
}

fn dispatch_get_global_queue(env: &mut Environment, _identifier: i32, _flags: u32) -> ConstVoidPtr {
    if env.libc_state.dispatch.global_queue.is_null() {
        env.libc_state.dispatch.global_queue = env.mem.calloc(64);
    }
    env.libc_state.dispatch.global_queue.cast_const()
}

fn dispatch_async(env: &mut Environment, _queue: ConstVoidPtr, block: ConstVoidPtr) {
    let block = _Block_copy(env, block);
    env.libc_state.dispatch.pending.push_back(block);
    crate::network_trace::scheduled(env, block.to_bits(), "dispatch_async");
}

fn dispatch_sync(env: &mut Environment, _queue: ConstVoidPtr, block: ConstVoidPtr) {
    invoke_block(env, block);
}

fn dispatch_after(
    env: &mut Environment,
    when: dispatch_time_t,
    _queue: ConstVoidPtr,
    block: ConstVoidPtr,
) {
    let Some(when) = time_to_instant(env, when) else {
        return;
    };
    let block = _Block_copy(env, block);
    env.libc_state.dispatch.delayed.push((when, block));
    crate::network_trace::scheduled(env, block.to_bits(), "dispatch_after");
}

/// `dispatch_once_t` is a `long`, which is ~0 once the block has run.
fn dispatch_once(env: &mut Environment, predicate: MutPtr<i32>, block: ConstVoidPtr) {
    if env.mem.read(predicate) == !0 {
        return;
    }
    env.mem.write(predicate, !0);
    invoke_block(env, block);
}

// Dispatch objects are never freed.
fn dispatch_retain(_env: &mut Environment, _object: MutVoidPtr) {}
fn dispatch_release(_env: &mut Environment, _object: MutVoidPtr) {}

fn dispatch_group_create(env: &mut Environment) -> MutVoidPtr {
    new_object(env, DispatchObject::Group)
}
fn dispatch_group_async(
    env: &mut Environment,
    _group: MutVoidPtr,
    _queue: ConstVoidPtr,
    block: ConstVoidPtr,
) {
    // Run immediately, so the group is never waiting for anything.
    invoke_block(env, block);
}
fn dispatch_group_wait(
    _env: &mut Environment,
    _group: MutVoidPtr,
    _timeout: dispatch_time_t,
) -> i32 {
    0
}

fn dispatch_semaphore_create(env: &mut Environment, value: i32) -> MutVoidPtr {
    if value < 0 {
        return MutVoidPtr::null();
    }
    let sem = host_create_semaphore(env, value as u32);
    new_object(env, DispatchObject::Semaphore(sem))
}
fn semaphore_for(env: &Environment, object: MutVoidPtr) -> MutPtr<sem_t> {
    match env.libc_state.dispatch.objects.get(&object) {
        Some(&DispatchObject::Semaphore(sem)) => sem,
        _ => panic!("{object:?} is not a dispatch semaphore"),
    }
}
fn dispatch_semaphore_signal(env: &mut Environment, semaphore: MutVoidPtr) -> i32 {
    let sem = semaphore_for(env, semaphore);
    env.sem_increment(sem)
        .expect("dispatch semaphore signal overflow or invalid object");
    0
}
fn dispatch_semaphore_wait(
    env: &mut Environment,
    semaphore: MutVoidPtr,
    timeout: dispatch_time_t,
) -> i32 {
    let sem = semaphore_for(env, semaphore);
    if env.sem_decrement(sem, false) {
        return 0;
    }
    // The work that signals the semaphore may be waiting to run on the main
    // thread, which is where everything runs.
    if env.current_thread == 0 {
        drain_main_queue(env);
        if env.sem_decrement(sem, false) {
            return 0;
        }
    }
    if timeout != DISPATCH_TIME_FOREVER {
        // TODO: actually wait until the timeout
        return TIMED_OUT;
    }
    env.sem_decrement(sem, true);
    0
}

fn timer_source(env: &mut Environment, source: MutVoidPtr) -> &mut TimerSource {
    match env.libc_state.dispatch.objects.get_mut(&source) {
        Some(DispatchObject::TimerSource(timer)) => timer,
        _ => panic!("{source:?} is not a dispatch timer source"),
    }
}

fn dispatch_source_create(
    env: &mut Environment,
    type_: ConstVoidPtr,
    _handle: u32,
    _mask: u32,
    _queue: ConstVoidPtr,
) -> MutVoidPtr {
    assert_eq!(
        type_,
        source_type_timer(env),
        "Only timer dispatch sources are implemented"
    );
    new_object(
        env,
        DispatchObject::TimerSource(TimerSource {
            event_handler: MutVoidPtr::null(),
            cancel_handler: MutVoidPtr::null(),
            next_fire: None,
            interval: None,
            suspend_count: 1,
            cancelled: false,
        }),
    )
}
fn dispatch_source_set_timer(
    env: &mut Environment,
    source: MutVoidPtr,
    start: dispatch_time_t,
    interval: u64,
    _leeway: u64,
) {
    let start = time_to_instant(env, start);
    let timer = timer_source(env, source);
    timer.next_fire = start;
    timer.interval = (interval != 0 && interval != DISPATCH_TIME_FOREVER)
        .then(|| Duration::from_nanos(interval));
}
fn dispatch_source_set_event_handler(
    env: &mut Environment,
    source: MutVoidPtr,
    block: ConstVoidPtr,
) {
    let block = _Block_copy(env, block);
    let old = std::mem::replace(&mut timer_source(env, source).event_handler, block);
    _Block_release(env, old);
}
fn dispatch_source_set_cancel_handler(
    env: &mut Environment,
    source: MutVoidPtr,
    block: ConstVoidPtr,
) {
    let block = _Block_copy(env, block);
    let old = std::mem::replace(&mut timer_source(env, source).cancel_handler, block);
    _Block_release(env, old);
}
fn dispatch_source_cancel(env: &mut Environment, source: MutVoidPtr) {
    let timer = timer_source(env, source);
    if timer.cancelled {
        return;
    }
    timer.cancelled = true;
    let cancel_handler = std::mem::take(&mut timer.cancel_handler);
    if !cancel_handler.is_null() {
        env.libc_state.dispatch.pending.push_back(cancel_handler);
    }
}

fn dispatch_suspend(env: &mut Environment, object: MutVoidPtr) {
    if let Some(DispatchObject::TimerSource(timer)) =
        env.libc_state.dispatch.objects.get_mut(&object)
    {
        timer.suspend_count += 1;
    } else {
        log!("TODO: dispatch_suspend({:?}) (ignored)", object);
    }
}
fn dispatch_resume(env: &mut Environment, object: MutVoidPtr) {
    if let Some(DispatchObject::TimerSource(timer)) =
        env.libc_state.dispatch.objects.get_mut(&object)
    {
        timer.suspend_count -= 1;
    } else {
        log!("TODO: dispatch_resume({:?}) (ignored)", object);
    }
}

/// Move `dispatch_after` blocks and timer events that are due to the queue.
fn queue_due_work(env: &mut Environment) {
    let now = Instant::now();
    let state = &mut env.libc_state.dispatch;
    let mut i = 0;
    while i < state.delayed.len() {
        if state.delayed[i].0 <= now {
            let (_, block) = state.delayed.remove(i);
            state.pending.push_back(block);
        } else {
            i += 1;
        }
    }
    let mut handlers = Vec::new();
    for object in state.objects.values_mut() {
        let DispatchObject::TimerSource(timer) = object else {
            continue;
        };
        if timer.cancelled || timer.suspend_count > 0 || timer.event_handler.is_null() {
            continue;
        }
        let Some(next_fire) = timer.next_fire else {
            continue;
        };
        if next_fire > now {
            continue;
        }
        // Coalesce missed events, like GCD does.
        timer.next_fire = timer.interval.map(|interval| {
            let mut next = next_fire + interval;
            while next <= now {
                next += interval;
            }
            next
        });
        handlers.push(timer.event_handler);
    }
    for handler in handlers {
        let handler = _Block_copy(env, handler.cast_const());
        env.libc_state.dispatch.pending.push_back(handler);
    }
}

/// Run the work submitted before this main run-loop iteration. Work enqueued
/// by a callback waits until the next iteration, preserving asynchronous use.
pub fn drain_main_queue(env: &mut Environment) {
    assert_eq!(env.current_thread, 0);
    if env.libc_state.dispatch.draining {
        return;
    }
    env.libc_state.dispatch.draining = true;
    crate::frameworks::game_kit::pump(env);
    crate::frameworks::foundation::ns_url_connection::pump(env);
    queue_due_work(env);
    let pending = std::mem::take(&mut env.libc_state.dispatch.pending);
    for block in pending {
        let invoke =
            GuestFunction::from_addr_with_thumb_bit(block_word(env, block.cast_const(), 3));
        let trace = crate::network_trace::callback_begin(env, block.to_bits());
        let _: () = invoke.call_from_host(env, (block,));
        crate::network_trace::end(env, trace);
        _Block_release(env, block);
    }
    env.libc_state.dispatch.draining = false;
}

pub const CONSTANTS: ConstantExports = &[
    ("__dispatch_main_q", HostConstant::Custom(main_queue)),
    ("__NSConcreteStackBlock", HostConstant::Custom(block_class)),
    ("__NSConcreteGlobalBlock", HostConstant::Custom(block_class)),
    ("__NSConcreteMallocBlock", HostConstant::Custom(block_class)),
    (
        "__dispatch_source_type_timer",
        HostConstant::Custom(source_type_timer),
    ),
];
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(dispatch_get_global_queue(_, _)),
    export_c_func!(dispatch_async(_, _)),
    export_c_func!(dispatch_sync(_, _)),
    export_c_func!(dispatch_after(_, _, _)),
    export_c_func!(dispatch_once(_, _)),
    export_c_func!(dispatch_time(_, _)),
    export_c_func!(dispatch_retain(_)),
    export_c_func!(dispatch_release(_)),
    export_c_func!(dispatch_group_create()),
    export_c_func!(dispatch_group_async(_, _, _)),
    export_c_func!(dispatch_group_wait(_, _)),
    export_c_func!(dispatch_semaphore_create(_)),
    export_c_func!(dispatch_semaphore_signal(_)),
    export_c_func!(dispatch_semaphore_wait(_, _)),
    export_c_func!(dispatch_source_create(_, _, _, _)),
    export_c_func!(dispatch_source_set_timer(_, _, _, _)),
    export_c_func!(dispatch_source_set_event_handler(_, _)),
    export_c_func!(dispatch_source_set_cancel_handler(_, _)),
    export_c_func!(dispatch_source_cancel(_)),
    export_c_func!(dispatch_suspend(_)),
    export_c_func!(dispatch_resume(_)),
    export_c_func!(_Block_copy(_)),
    export_c_func!(_Block_release(_)),
    export_c_func!(_Block_object_assign(_, _, _)),
    export_c_func!(_Block_object_dispose(_, _)),
];
