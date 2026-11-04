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
//! timer sources and groups) or immediately (`dispatch_sync`).
//! Global queues are treated like the main queue. Semaphores do block other
//! threads.

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::libc::semaphore::{host_create_semaphore, sem_t};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{id, msg_class, objc_classes, ClassExports};
use crate::Environment;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    source_type_data_add: MutVoidPtr,
    source_type_data_or: MutVoidPtr,
    source_type_data_replace: MutVoidPtr,
    pending: VecDeque<DispatchWork>,
    /// Blocks from `dispatch_after`, and when to run them.
    delayed: Vec<(Instant, DispatchWork)>,
    /// Host state for dispatch objects other than queues.
    objects: HashMap<MutVoidPtr, DispatchObject>,
    /// Reference point for `dispatch_time_t` values.
    epoch: Option<Instant>,
    copies: HashMap<MutVoidPtr, u32>,
    /// Heap-promoted __block cells: one stack owner plus each copied Block.
    byrefs: HashMap<MutVoidPtr, u32>,
    queue_labels: HashMap<ConstVoidPtr, ConstPtr<u8>>,
    queue_specifics: HashMap<(ConstVoidPtr, ConstVoidPtr), (MutVoidPtr, GuestFunction)>,
    contexts: HashMap<MutVoidPtr, MutVoidPtr>,
    current_queues: HashMap<usize, Vec<ConstVoidPtr>>,
    once_in_progress: HashSet<MutPtr<i32>>,
    draining: bool,
}

enum DispatchWork {
    Block {
        block: MutVoidPtr,
        queue: ConstVoidPtr,
    },
    Function {
        context: MutVoidPtr,
        function: GuestFunction,
        queue: ConstVoidPtr,
    },
    Group {
        group: MutVoidPtr,
        work: Box<DispatchWork>,
    },
    SourceEvent {
        source: MutVoidPtr,
        data: u32,
        work: Box<DispatchWork>,
    },
    BlockBoolObject {
        block: MutVoidPtr,
        queue: ConstVoidPtr,
        value: bool,
        object: id,
    },
}

enum DispatchObject {
    Group {
        count: u32,
        notifications: Vec<DispatchWork>,
    },
    Semaphore(MutPtr<sem_t>),
    TimerSource(TimerSource),
}

struct TimerSource {
    event_handler: Option<SourceHandler>,
    cancel_handler: Option<SourceHandler>,
    next_fire: Option<Instant>,
    interval: Option<Duration>,
    /// Sources are created suspended.
    suspend_count: i32,
    cancelled: bool,
    cancel_queued: bool,
    queue: ConstVoidPtr,
    handle: u32,
    mask: u32,
    kind: SourceKind,
    pending_data: u32,
    pending_event: bool,
    event_queued: bool,
    delivering_data: Option<u32>,
}

#[derive(Clone, Copy)]
enum SourceKind {
    Timer,
    DataAdd,
    DataOr,
    DataReplace,
}

#[derive(Clone, Copy)]
enum SourceHandler {
    Block(MutVoidPtr),
    Function(GuestFunction),
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

fn dispatch_walltime(
    env: &mut Environment,
    spec: ConstPtr<crate::libc::time::timespec>,
    delta: i64,
) -> dispatch_time_t {
    let epoch = *env
        .libc_state
        .dispatch
        .epoch
        .get_or_insert_with(Instant::now);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as i128;
    let base = if spec.is_null() {
        now
    } else {
        let spec = env.mem.read(spec);
        i128::from(spec.tv_sec) * 1_000_000_000 + i128::from(spec.tv_nsec)
    };
    let monotonic = epoch.elapsed().as_nanos() as i128 + base - now + i128::from(delta);
    monotonic.clamp(1, i128::from(u64::MAX - 1)) as u64
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

pub(crate) fn enqueue_bool_object_block(
    env: &mut Environment,
    block: ConstVoidPtr,
    value: bool,
    object: id,
) {
    if block.is_null() {
        return;
    }
    let block = _Block_copy(env, block);
    if !object.is_null() {
        crate::objc::retain(env, object);
    }
    let queue = main_queue(env);
    env.libc_state
        .dispatch
        .pending
        .push_back(DispatchWork::BlockBoolObject {
            block,
            queue,
            value,
            object,
        });
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
fn source_type_data_add(env: &mut Environment) -> ConstVoidPtr {
    if env.libc_state.dispatch.source_type_data_add.is_null() {
        env.libc_state.dispatch.source_type_data_add = env.mem.calloc(16);
    }
    env.libc_state.dispatch.source_type_data_add.cast_const()
}
fn source_type_data_or(env: &mut Environment) -> ConstVoidPtr {
    if env.libc_state.dispatch.source_type_data_or.is_null() {
        env.libc_state.dispatch.source_type_data_or = env.mem.calloc(16);
    }
    env.libc_state.dispatch.source_type_data_or.cast_const()
}
fn source_type_data_replace(env: &mut Environment) -> ConstVoidPtr {
    if env.libc_state.dispatch.source_type_data_replace.is_null() {
        env.libc_state.dispatch.source_type_data_replace = env.mem.calloc(16);
    }
    env.libc_state
        .dispatch
        .source_type_data_replace
        .cast_const()
}

fn block_class(env: &mut Environment) -> ConstVoidPtr {
    let class = env.objc.get_known_class("_Podroid_NSBlock", &mut env.mem);
    env.libc_state.dispatch.block_class = class;
    class.cast_void().cast_const()
}

/// Block literals live in guest stack/static memory, outside ObjC's object map.
pub(crate) fn is_block(env: &Environment, object: id) -> bool {
    let class = env.libc_state.dispatch.block_class;
    !class.is_null() && env.mem.read(object.cast::<id>()) == class
}

/// Opaque queue handle — we use a small integer sentinel.
#[allow(non_camel_case_types)]
pub type dispatch_queue_t = MutVoidPtr;

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation _Podroid_NSBlock: NSObject
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
// `-[NSBlock invoke]` calls a block that takes no arguments, which lets code
// run a block through `performSelector:@selector(invoke)` or NSInvocation
// (Crashlytics does). The invoke pointer is the fourth word of the literal.
// https://clang.llvm.org/docs/Block-ABI-Apple.html
- (())invoke {
    let invoke = block_word(env, this.cast_void().cast_const(), 3);
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke);
    let (): () = invoke.call_from_host(env, (this,));
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

fn dispatch_get_main_queue(env: &mut Environment) -> ConstVoidPtr {
    main_queue(env)
}

fn dispatch_get_current_queue(env: &mut Environment) -> ConstVoidPtr {
    if let Some(queue) = env
        .libc_state
        .dispatch
        .current_queues
        .get(&env.current_thread)
        .and_then(|stack| stack.last())
        .copied()
    {
        return queue;
    }
    if env.current_thread == 0 {
        main_queue(env)
    } else {
        dispatch_get_global_queue(env, 0, 0)
    }
}
fn push_queue(env: &mut Environment, queue: ConstVoidPtr) {
    env.libc_state
        .dispatch
        .current_queues
        .entry(env.current_thread)
        .or_default()
        .push(queue);
}
fn pop_queue(env: &mut Environment) {
    env.libc_state
        .dispatch
        .current_queues
        .get_mut(&env.current_thread)
        .unwrap()
        .pop();
}
fn dispatch_get_specific(env: &mut Environment, key: ConstVoidPtr) -> MutVoidPtr {
    let queue = dispatch_get_current_queue(env);
    dispatch_queue_get_specific(env, queue, key)
}

fn dispatch_queue_create(
    env: &mut Environment,
    label: ConstPtr<u8>,
    _attr: ConstVoidPtr,
) -> ConstVoidPtr {
    let name = if label.is_null() {
        Vec::new()
    } else {
        env.mem
            .cstr_at_utf8(label)
            .unwrap_or_default()
            .as_bytes()
            .to_vec()
    };
    let stored = env.mem.alloc_and_write_cstr(&name).cast_const();
    let queue = env.mem.calloc(64).cast_const();
    env.libc_state.dispatch.queue_labels.insert(queue, stored);
    queue
}
fn dispatch_queue_get_label(env: &mut Environment, queue: ConstVoidPtr) -> ConstPtr<u8> {
    let queue = if queue.is_null() {
        dispatch_get_current_queue(env)
    } else {
        queue
    };
    if let Some(&label) = env.libc_state.dispatch.queue_labels.get(&queue) {
        return label;
    }
    let main = main_queue(env);
    let global = dispatch_get_global_queue(env, 0, 0);
    let label: &[u8] = if queue == main {
        b"com.apple.main-thread"
    } else if queue == global {
        b"com.apple.root.default-priority"
    } else {
        b""
    };
    let label = env.mem.alloc_and_write_cstr(label).cast_const();
    env.libc_state.dispatch.queue_labels.insert(queue, label);
    label
}
fn dispatch_queue_set_specific(
    env: &mut Environment,
    queue: ConstVoidPtr,
    key: ConstVoidPtr,
    context: MutVoidPtr,
    destructor: GuestFunction,
) {
    if key.is_null() {
        return;
    }
    let old = if context.is_null() {
        env.libc_state
            .dispatch
            .queue_specifics
            .remove(&(queue, key))
    } else {
        env.libc_state
            .dispatch
            .queue_specifics
            .insert((queue, key), (context, destructor))
    };
    if let Some((context, function)) = old {
        if !function.to_ptr().is_null() {
            let queue = dispatch_get_global_queue(env, 0, 0);
            env.libc_state
                .dispatch
                .pending
                .push_back(DispatchWork::Function {
                    context,
                    function,
                    queue,
                });
        }
    }
}
fn dispatch_queue_get_specific(
    env: &mut Environment,
    queue: ConstVoidPtr,
    key: ConstVoidPtr,
) -> MutVoidPtr {
    env.libc_state
        .dispatch
        .queue_specifics
        .get(&(queue, key))
        .map_or(MutVoidPtr::null(), |(context, _)| *context)
}
fn dispatch_set_context(env: &mut Environment, object: MutVoidPtr, context: MutVoidPtr) {
    env.libc_state.dispatch.contexts.insert(object, context);
}
fn dispatch_get_context(env: &mut Environment, object: MutVoidPtr) -> MutVoidPtr {
    env.libc_state
        .dispatch
        .contexts
        .get(&object)
        .copied()
        .unwrap_or(MutVoidPtr::null())
}
fn dispatch_queue_retain(env: &mut Environment, queue: MutVoidPtr) -> MutVoidPtr {
    dispatch_retain(env, queue);
    queue
}
fn dispatch_queue_release(env: &mut Environment, queue: MutVoidPtr) {
    dispatch_release(env, queue);
}
fn dispatch_queue_attr_make_with_qos_class(
    _env: &mut Environment,
    attr: ConstVoidPtr,
    _qos: u32,
    _priority: i32,
) -> ConstVoidPtr {
    attr
}

fn dispatch_async_f(
    env: &mut Environment,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    env.libc_state
        .dispatch
        .pending
        .push_back(DispatchWork::Function {
            context,
            function,
            queue,
        });
}
fn dispatch_sync_f(
    env: &mut Environment,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    push_queue(env, queue);
    let _: () = function.call_from_host(env, (context,));
    pop_queue(env);
}
fn dispatch_after_f(
    env: &mut Environment,
    when: dispatch_time_t,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    if let Some(when) = time_to_instant(env, when) {
        env.libc_state.dispatch.delayed.push((
            when,
            DispatchWork::Function {
                context,
                function,
                queue,
            },
        ));
    }
}
fn dispatch_once_f(
    env: &mut Environment,
    predicate: MutPtr<i32>,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    if !begin_once(env, predicate) {
        return;
    }
    let _: () = function.call_from_host(env, (context,));
    finish_once(env, predicate);
}
fn dispatch_apply_f(
    env: &mut Environment,
    iterations: u32,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    push_queue(env, queue);
    for index in 0..iterations {
        let _: () = function.call_from_host(env, (context, index));
    }
    pop_queue(env);
}
fn dispatch_apply(
    env: &mut Environment,
    iterations: u32,
    queue: ConstVoidPtr,
    block: ConstVoidPtr,
) {
    let function = block_invoke_function(env, block);
    push_queue(env, queue);
    for index in 0..iterations {
        let _: () = function.call_from_host(env, (block, index));
    }
    pop_queue(env);
}
fn dispatch_group_async_f(
    env: &mut Environment,
    group: MutVoidPtr,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    dispatch_group_enter(env, group);
    env.libc_state
        .dispatch
        .pending
        .push_back(DispatchWork::Group {
            group,
            work: Box::new(DispatchWork::Function {
                context,
                function,
                queue,
            }),
        });
}
fn dispatch_barrier_async(env: &mut Environment, queue: ConstVoidPtr, block: ConstVoidPtr) {
    dispatch_async(env, queue, block);
}
fn dispatch_barrier_sync(env: &mut Environment, queue: ConstVoidPtr, block: ConstVoidPtr) {
    dispatch_sync(env, queue, block);
}
fn dispatch_barrier_async_f(
    env: &mut Environment,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    dispatch_async_f(env, queue, context, function);
}
fn dispatch_barrier_sync_f(
    env: &mut Environment,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    dispatch_sync_f(env, queue, context, function);
}

fn dispatch_async(env: &mut Environment, queue: ConstVoidPtr, block: ConstVoidPtr) {
    let block = _Block_copy(env, block);
    env.libc_state
        .dispatch
        .pending
        .push_back(DispatchWork::Block { block, queue });
    crate::network_trace::scheduled(env, block.to_bits(), "dispatch_async");
}

fn dispatch_sync(env: &mut Environment, queue: ConstVoidPtr, block: ConstVoidPtr) {
    push_queue(env, queue);
    invoke_block(env, block);
    pop_queue(env);
}

fn dispatch_after(
    env: &mut Environment,
    when: dispatch_time_t,
    queue: ConstVoidPtr,
    block: ConstVoidPtr,
) {
    let Some(when) = time_to_instant(env, when) else {
        return;
    };
    let block = _Block_copy(env, block);
    env.libc_state
        .dispatch
        .delayed
        .push((when, DispatchWork::Block { block, queue }));
    crate::network_trace::scheduled(env, block.to_bits(), "dispatch_after");
}

/// `dispatch_once_t` is a `long`, which is ~0 once the block has run.
fn dispatch_once(env: &mut Environment, predicate: MutPtr<i32>, block: ConstVoidPtr) {
    if !begin_once(env, predicate) {
        return;
    }
    invoke_block(env, block);
    finish_once(env, predicate);
}

fn begin_once(env: &mut Environment, predicate: MutPtr<i32>) -> bool {
    loop {
        if env.mem.read(predicate) == !0 {
            return false;
        }
        if env.libc_state.dispatch.once_in_progress.insert(predicate) {
            return true;
        }
        env.sleep(Duration::from_millis(1));
    }
}
fn finish_once(env: &mut Environment, predicate: MutPtr<i32>) {
    env.mem.write(predicate, !0);
    env.libc_state.dispatch.once_in_progress.remove(&predicate);
}

// Dispatch objects are never freed.
fn dispatch_retain(_env: &mut Environment, _object: MutVoidPtr) {}
fn dispatch_release(_env: &mut Environment, _object: MutVoidPtr) {}

fn dispatch_group_create(env: &mut Environment) -> MutVoidPtr {
    new_object(
        env,
        DispatchObject::Group {
            count: 0,
            notifications: Vec::new(),
        },
    )
}
fn dispatch_group_async(
    env: &mut Environment,
    group: MutVoidPtr,
    queue: ConstVoidPtr,
    block: ConstVoidPtr,
) {
    let block = _Block_copy(env, block);
    dispatch_group_enter(env, group);
    env.libc_state
        .dispatch
        .pending
        .push_back(DispatchWork::Group {
            group,
            work: Box::new(DispatchWork::Block { block, queue }),
        });
}
fn dispatch_group_enter(env: &mut Environment, group: MutVoidPtr) {
    let Some(DispatchObject::Group { count, .. }) = env.libc_state.dispatch.objects.get_mut(&group)
    else {
        return;
    };
    *count = count.checked_add(1).expect("dispatch group count overflow");
}
fn dispatch_group_leave(env: &mut Environment, group: MutVoidPtr) {
    let Some(DispatchObject::Group {
        count,
        notifications,
    }) = env.libc_state.dispatch.objects.get_mut(&group)
    else {
        return;
    };
    if *count == 0 {
        return;
    }
    *count -= 1;
    if *count == 0 {
        let ready = std::mem::take(notifications);
        env.libc_state.dispatch.pending.extend(ready);
    }
}
fn group_notify(env: &mut Environment, group: MutVoidPtr, work: DispatchWork) {
    let Some(DispatchObject::Group {
        count,
        notifications,
    }) = env.libc_state.dispatch.objects.get_mut(&group)
    else {
        return;
    };
    if *count == 0 {
        env.libc_state.dispatch.pending.push_back(work);
    } else {
        notifications.push(work);
    }
}
fn dispatch_group_notify(
    env: &mut Environment,
    group: MutVoidPtr,
    queue: ConstVoidPtr,
    block: ConstVoidPtr,
) {
    let block = _Block_copy(env, block);
    group_notify(env, group, DispatchWork::Block { block, queue });
}
fn dispatch_group_notify_f(
    env: &mut Environment,
    group: MutVoidPtr,
    queue: ConstVoidPtr,
    context: MutVoidPtr,
    function: GuestFunction,
) {
    group_notify(
        env,
        group,
        DispatchWork::Function {
            context,
            function,
            queue,
        },
    );
}
fn dispatch_group_wait(env: &mut Environment, group: MutVoidPtr, timeout: dispatch_time_t) -> i32 {
    let deadline = time_to_instant(env, timeout);
    loop {
        if matches!(
            env.libc_state.dispatch.objects.get(&group),
            Some(DispatchObject::Group { count: 0, .. })
        ) {
            return 0;
        }
        if !matches!(
            env.libc_state.dispatch.objects.get(&group),
            Some(DispatchObject::Group { .. })
        ) {
            return TIMED_OUT;
        }
        if env.current_thread == 0 && !env.libc_state.dispatch.draining {
            drain_main_queue(env);
        }
        if matches!(
            env.libc_state.dispatch.objects.get(&group),
            Some(DispatchObject::Group { count: 0, .. })
        ) {
            return 0;
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return TIMED_OUT;
        }
        env.sleep(Duration::from_millis(1));
    }
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
    handle: u32,
    mask: u32,
    queue: ConstVoidPtr,
) -> MutVoidPtr {
    let kind = if type_ == source_type_timer(env) {
        SourceKind::Timer
    } else if type_ == source_type_data_add(env) {
        SourceKind::DataAdd
    } else if type_ == source_type_data_or(env) {
        SourceKind::DataOr
    } else if type_ == source_type_data_replace(env) {
        SourceKind::DataReplace
    } else {
        return MutVoidPtr::null();
    };
    new_object(
        env,
        DispatchObject::TimerSource(TimerSource {
            event_handler: None,
            cancel_handler: None,
            next_fire: None,
            interval: None,
            suspend_count: 1,
            cancelled: false,
            cancel_queued: false,
            queue,
            handle,
            mask,
            kind,
            pending_data: 0,
            pending_event: false,
            event_queued: false,
            delivering_data: None,
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
    if !matches!(timer_source(env, source).kind, SourceKind::Timer) {
        return;
    }
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
    let old = std::mem::replace(
        &mut timer_source(env, source).event_handler,
        (!block.is_null()).then_some(SourceHandler::Block(block)),
    );
    if let Some(SourceHandler::Block(old)) = old {
        _Block_release(env, old);
    }
}
fn dispatch_source_set_cancel_handler(
    env: &mut Environment,
    source: MutVoidPtr,
    block: ConstVoidPtr,
) {
    let block = _Block_copy(env, block);
    let old = std::mem::replace(
        &mut timer_source(env, source).cancel_handler,
        (!block.is_null()).then_some(SourceHandler::Block(block)),
    );
    if let Some(SourceHandler::Block(old)) = old {
        _Block_release(env, old);
    }
}
fn dispatch_source_set_event_handler_f(
    env: &mut Environment,
    source: MutVoidPtr,
    function: GuestFunction,
) {
    let handler = (!function.to_ptr().is_null()).then_some(SourceHandler::Function(function));
    let old = std::mem::replace(&mut timer_source(env, source).event_handler, handler);
    if let Some(SourceHandler::Block(old)) = old {
        _Block_release(env, old);
    }
}
fn dispatch_source_set_cancel_handler_f(
    env: &mut Environment,
    source: MutVoidPtr,
    function: GuestFunction,
) {
    let handler = (!function.to_ptr().is_null()).then_some(SourceHandler::Function(function));
    let old = std::mem::replace(&mut timer_source(env, source).cancel_handler, handler);
    if let Some(SourceHandler::Block(old)) = old {
        _Block_release(env, old);
    }
}
fn dispatch_source_testcancel(env: &mut Environment, source: MutVoidPtr) -> i32 {
    i32::from(timer_source(env, source).cancelled)
}
fn dispatch_source_get_handle(env: &mut Environment, source: MutVoidPtr) -> u32 {
    timer_source(env, source).handle
}
fn dispatch_source_get_mask(env: &mut Environment, source: MutVoidPtr) -> u32 {
    timer_source(env, source).mask
}
fn dispatch_source_get_data(env: &mut Environment, source: MutVoidPtr) -> u32 {
    let source = timer_source(env, source);
    source.delivering_data.unwrap_or(source.pending_data)
}
fn dispatch_source_merge_data(env: &mut Environment, source: MutVoidPtr, value: u32) {
    let source = timer_source(env, source);
    if source.cancelled {
        return;
    }
    match source.kind {
        SourceKind::DataAdd => source.pending_data = source.pending_data.saturating_add(value),
        SourceKind::DataOr => source.pending_data |= value,
        SourceKind::DataReplace => source.pending_data = value,
        SourceKind::Timer => return,
    }
    source.pending_event = true;
}
fn dispatch_source_cancel(env: &mut Environment, source: MutVoidPtr) {
    let timer = timer_source(env, source);
    if timer.cancelled {
        return;
    }
    timer.cancelled = true;
    queue_source_cancel(env, source);
}

fn queue_source_cancel(env: &mut Environment, source: MutVoidPtr) {
    let timer = timer_source(env, source);
    if !timer.cancelled || timer.cancel_queued || timer.suspend_count > 0 || timer.event_queued {
        return;
    }
    timer.cancel_queued = true;
    let cancel_handler = timer.cancel_handler.take();
    let queue = timer.queue;
    if let Some(handler) = cancel_handler {
        let context = dispatch_get_context(env, source);
        let work = match handler {
            SourceHandler::Block(block) => DispatchWork::Block { block, queue },
            SourceHandler::Function(function) => DispatchWork::Function {
                context,
                function,
                queue,
            },
        };
        env.libc_state.dispatch.pending.push_back(work);
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
        if timer.suspend_count > 0 {
            timer.suspend_count -= 1;
        }
        if timer.cancelled {
            queue_source_cancel(env, object);
        }
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
    for (&source, object) in state.objects.iter_mut() {
        let DispatchObject::TimerSource(timer) = object else {
            continue;
        };
        if timer.cancelled
            || timer.suspend_count > 0
            || timer.event_handler.is_none()
            || timer.event_queued
        {
            continue;
        }
        if let Some(next_fire) = timer.next_fire {
            if next_fire <= now {
                let count = timer.interval.map_or(1, |interval| {
                    let elapsed = now.duration_since(next_fire).as_nanos();
                    (1 + elapsed / interval.as_nanos()).min(u32::MAX as u128) as u32
                });
                timer.pending_data = timer.pending_data.saturating_add(count);
                timer.pending_event = true;
                timer.next_fire = timer
                    .interval
                    .and_then(|interval| now.checked_add(interval));
            }
        }
        if !timer.pending_event {
            continue;
        }
        let data = std::mem::take(&mut timer.pending_data);
        timer.pending_event = false;
        timer.event_queued = true;
        handlers.push((source, timer.event_handler.unwrap(), timer.queue, data));
    }
    for (source, handler, queue, data) in handlers {
        let context = dispatch_get_context(env, source);
        let work = match handler {
            SourceHandler::Block(block) => DispatchWork::Block {
                block: _Block_copy(env, block.cast_const()),
                queue,
            },
            SourceHandler::Function(function) => DispatchWork::Function {
                context,
                function,
                queue,
            },
        };
        env.libc_state
            .dispatch
            .pending
            .push_back(DispatchWork::SourceEvent {
                source,
                data,
                work: Box::new(work),
            });
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
    for work in pending {
        run_work(env, work);
    }
    env.libc_state.dispatch.draining = false;
}

fn run_work(env: &mut Environment, work: DispatchWork) {
    match work {
        DispatchWork::Block { block, queue } => {
            push_queue(env, queue);
            let invoke =
                GuestFunction::from_addr_with_thumb_bit(block_word(env, block.cast_const(), 3));
            let trace = crate::network_trace::callback_begin(env, block.to_bits());
            let _: () = invoke.call_from_host(env, (block,));
            crate::network_trace::end(env, trace);
            _Block_release(env, block);
            pop_queue(env);
        }
        DispatchWork::Function {
            context,
            function,
            queue,
        } => {
            push_queue(env, queue);
            let _: () = function.call_from_host(env, (context,));
            pop_queue(env);
        }
        DispatchWork::Group { group, work } => {
            run_work(env, *work);
            dispatch_group_leave(env, group);
        }
        DispatchWork::SourceEvent { source, data, work } => {
            timer_source(env, source).delivering_data = Some(data);
            run_work(env, *work);
            let timer = timer_source(env, source);
            timer.delivering_data = None;
            timer.event_queued = false;
            queue_source_cancel(env, source);
        }
        DispatchWork::BlockBoolObject {
            block,
            queue,
            value,
            object,
        } => {
            push_queue(env, queue);
            let invoke =
                GuestFunction::from_addr_with_thumb_bit(block_word(env, block.cast_const(), 3));
            let _: () = invoke.call_from_host(env, (block, value, object));
            _Block_release(env, block);
            if !object.is_null() {
                crate::objc::release(env, object);
            }
            pop_queue(env);
        }
    }
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
    (
        "__dispatch_source_type_data_add",
        HostConstant::Custom(source_type_data_add),
    ),
    (
        "__dispatch_source_type_data_or",
        HostConstant::Custom(source_type_data_or),
    ),
    (
        "__dispatch_source_type_data_replace",
        HostConstant::Custom(source_type_data_replace),
    ),
];
/// `void dispatch_debug(dispatch_object_t object, const char *message, ...)`
///
/// Per Apple docs: "Programmatically logs debug information about a dispatch
/// object." This is a debugging aid that apps rarely call in production,
/// but some Unity-based games (Frontline Commando 2) reference it. Since
/// we don't have real dispatch object internals to inspect, we simply no-op.
fn dispatch_debug(_env: &mut Environment, _object: MutVoidPtr, _message: ConstVoidPtr) {
    // No-op. The message is a printf-style format string with varargs that
    // we intentionally ignore.
}

fn dispatch_main(_env: &mut Environment) {
    log!("dispatch_main: stubbed (returning immediately)");
    // Cannot spin forever here since GuestRet requires (). Just return.
}

fn dispatch_set_finalizer_f(
    _env: &mut Environment,
    _object: MutVoidPtr,
    _finalizer: GuestFunction,
) {
}

fn dispatch_set_target_queue(
    _env: &mut Environment,
    _object: MutVoidPtr,
    _queue: dispatch_queue_t,
) {
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(dispatch_get_current_queue()),
    export_c_func!(dispatch_get_specific(_)),
    export_c_func!(dispatch_queue_create(_, _)),
    export_c_func!(dispatch_queue_get_label(_)),
    export_c_func!(dispatch_queue_set_specific(_, _, _, _)),
    export_c_func!(dispatch_queue_get_specific(_, _)),
    export_c_func!(dispatch_queue_retain(_)),
    export_c_func!(dispatch_queue_release(_)),
    export_c_func!(dispatch_queue_attr_make_with_qos_class(_, _, _)),
    export_c_func!(dispatch_set_context(_, _)),
    export_c_func!(dispatch_get_context(_)),
    export_c_func!(dispatch_get_main_queue()),
    export_c_func!(dispatch_async_f(_, _, _)),
    export_c_func!(dispatch_sync_f(_, _, _)),
    export_c_func!(dispatch_after_f(_, _, _, _)),
    export_c_func!(dispatch_once_f(_, _, _)),
    export_c_func!(dispatch_apply_f(_, _, _, _)),
    export_c_func!(dispatch_apply(_, _, _)),
    export_c_func!(dispatch_group_async_f(_, _, _, _)),
    export_c_func!(dispatch_barrier_async(_, _)),
    export_c_func!(dispatch_barrier_sync(_, _)),
    export_c_func!(dispatch_barrier_async_f(_, _, _)),
    export_c_func!(dispatch_barrier_sync_f(_, _, _)),
    export_c_func!(dispatch_get_global_queue(_, _)),
    export_c_func!(dispatch_async(_, _)),
    export_c_func!(dispatch_sync(_, _)),
    export_c_func!(dispatch_after(_, _, _)),
    export_c_func!(dispatch_once(_, _)),
    export_c_func!(dispatch_time(_, _)),
    export_c_func!(dispatch_walltime(_, _)),
    export_c_func!(dispatch_retain(_)),
    export_c_func!(dispatch_release(_)),
    export_c_func!(dispatch_group_create()),
    export_c_func!(dispatch_group_enter(_)),
    export_c_func!(dispatch_group_leave(_)),
    export_c_func!(dispatch_group_notify(_, _, _)),
    export_c_func!(dispatch_group_notify_f(_, _, _, _)),
    export_c_func!(dispatch_group_async(_, _, _)),
    export_c_func!(dispatch_group_wait(_, _)),
    export_c_func!(dispatch_semaphore_create(_)),
    export_c_func!(dispatch_semaphore_signal(_)),
    export_c_func!(dispatch_semaphore_wait(_, _)),
    export_c_func!(dispatch_source_create(_, _, _, _)),
    export_c_func!(dispatch_source_set_timer(_, _, _, _)),
    export_c_func!(dispatch_source_set_event_handler(_, _)),
    export_c_func!(dispatch_source_set_event_handler_f(_, _)),
    export_c_func!(dispatch_source_set_cancel_handler(_, _)),
    export_c_func!(dispatch_source_set_cancel_handler_f(_, _)),
    export_c_func!(dispatch_source_cancel(_)),
    export_c_func!(dispatch_source_testcancel(_)),
    export_c_func!(dispatch_source_get_handle(_)),
    export_c_func!(dispatch_source_get_mask(_)),
    export_c_func!(dispatch_source_get_data(_)),
    export_c_func!(dispatch_source_merge_data(_, _)),
    export_c_func!(dispatch_suspend(_)),
    export_c_func!(dispatch_resume(_)),
    export_c_func!(_Block_copy(_)),
    export_c_func!(_Block_release(_)),
    export_c_func!(_Block_object_assign(_, _, _)),
    export_c_func!(_Block_object_dispose(_, _)),
    export_c_func!(dispatch_debug(_, _)),
    export_c_func!(dispatch_main()),
    export_c_func!(dispatch_set_finalizer_f(_, _)),
    export_c_func!(dispatch_set_target_queue(_, _)),
];
