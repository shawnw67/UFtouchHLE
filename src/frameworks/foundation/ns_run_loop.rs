/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSRunLoop`.
//!
//! Resources:
//! - Apple's [Threading Programming Guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/Introduction/Introduction.html)

use super::{ns_string, ns_timer, NSTimeInterval};
use crate::dyld::{ConstantExports, HostConstant};
use crate::environment::{ThreadBlock, ThreadId};
use crate::frameworks::audio_toolbox::audio_queue::{handle_audio_queue, AudioQueueRef};
use crate::frameworks::audio_toolbox::audio_unit::{render_audio_unit, AudioUnit};
use crate::frameworks::core_animation::ca_transaction;
use crate::frameworks::core_foundation::cf_run_loop::{
    kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoopRef,
};
use crate::frameworks::{core_animation, media_player, uikit};
use crate::libc::semaphore::{host_create_semaphore, sem_post, sem_t};
use crate::mem::MutPtr;
use crate::objc::{
    id, msg, msg_send, nil, objc_classes, release, retain, Class, ClassExports, HostObject, SEL,
};
use crate::Environment;
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// `NSString*`
pub type NSRunLoopMode = id;
// FIXME: Maybe this shouldn't be the same value? See: https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/Multithreading/RunLoopManagement/RunLoopManagement.html
pub const NSRunLoopCommonModes: &str = kCFRunLoopCommonModes;
pub const NSDefaultRunLoopMode: &str = kCFRunLoopDefaultMode;

pub const CONSTANTS: ConstantExports = &[
    (
        "_NSRunLoopCommonModes",
        HostConstant::NSString(NSRunLoopCommonModes),
    ),
    (
        "_NSDefaultRunLoopMode",
        HostConstant::NSString(NSDefaultRunLoopMode),
    ),
];

#[derive(Default)]
pub struct ThreadLocalState {
    run_loop: id,
}

struct NSRunLoopHostObject {
    audio_units: Vec<AudioUnit>,
    /// Weak reference. Audio queue must remove itself when destroyed (TODO).
    /// They are in no particular order.
    audio_queues: Vec<AudioQueueRef>,
    /// Objects to run for performSelector:onThread:(afterDelay:/waitUntilDone:)
    selector_objects: VecDeque<ObjectSelectorSource>,
    /// Strong references to `NSTimer*` in no particular order. Timers are owned
    /// by the run loop. The timer must remove itself when invalidated.
    timers: Vec<id>,
    cf_sources: Vec<id>,
    cf_streams: Vec<(id, bool)>,
    owner: ThreadId,
    waiting: bool,
    stop_flags: Vec<bool>,
}
impl HostObject for NSRunLoopHostObject {}

pub fn add_cf_stream(env: &mut Environment, run_loop: id, stream: id, is_write: bool) {
    env.objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .cf_streams
        .push((stream, is_write));
}

pub fn remove_cf_stream(env: &mut Environment, run_loop: id, stream: id, is_write: bool) {
    let streams = &mut env
        .objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .cf_streams;
    if let Some(index) = streams.iter().position(|&item| item == (stream, is_write)) {
        streams.remove(index);
    }
}

fn handle_cf_streams(env: &mut Environment, run_loop: id) {
    let streams = env
        .objc
        .borrow::<NSRunLoopHostObject>(run_loop)
        .cf_streams
        .clone();
    for &(stream, _) in &streams {
        retain(env, stream);
    }
    for (stream, is_write) in streams {
        crate::frameworks::core_foundation::cf_stream::perform_events(
            env, run_loop, stream, is_write,
        );
        release(env, stream);
    }
}

pub fn cf_run_loop_is_waiting(env: &mut Environment, run_loop: id) -> bool {
    env.objc.borrow::<NSRunLoopHostObject>(run_loop).waiting
}

pub fn wake_cf_run_loop(env: &mut Environment, run_loop: id) {
    let host = env.objc.borrow::<NSRunLoopHostObject>(run_loop);
    let (waiting, owner) = (host.waiting, host.owner);
    if waiting && matches!(env.threads[owner].blocked_by, ThreadBlock::Sleeping(_)) {
        env.threads[owner].blocked_by = ThreadBlock::NotBlocked;
    }
}

pub fn stop_cf_run_loop(env: &mut Environment, run_loop: id) {
    if let Some(flag) = env
        .objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .stop_flags
        .last_mut()
    {
        *flag = true;
        wake_cf_run_loop(env, run_loop);
    }
}

pub fn add_cf_source(env: &mut Environment, run_loop: id, source: id) {
    env.objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .cf_sources
        .push(source);
}

pub fn remove_cf_source(env: &mut Environment, run_loop: id, source: id) {
    let sources = &mut env
        .objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .cf_sources;
    if let Some(index) = sources.iter().position(|&item| item == source) {
        sources.remove(index);
    }
}

pub fn cf_sources(env: &mut Environment, run_loop: id) -> Vec<id> {
    env.objc
        .borrow::<NSRunLoopHostObject>(run_loop)
        .cf_sources
        .clone()
}

#[derive(Clone, Debug)]
struct ObjectSelectorSource {
    target: id,
    selector: SEL,
    argument: id,
    due_by: Option<Instant>,
    // Used for waitUntilDone:, (uses NULL if not waiting)
    semaphore: MutPtr<sem_t>,
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSRunLoop: NSObject

+ (id)mainRunLoop {
    run_loop_for_thread(env, this, 0)
}

+ (id)currentRunLoop {
    run_loop_for_thread(env, this, env.current_thread)
}

// TODO: more accessors

- (id) retain { this }
- (()) release {}
- (id) autorelease { this }

- (CFRunLoopRef)getCFRunLoop {
    // In our implementation these are the same type (they aren't in Apple's).
    this
}

- (bool)runMode:(NSRunLoopMode)mode beforeDate:(id)limit_date {
    assert!(!mode.is_null());
    let limit = if limit_date == nil {
        None
    } else {
        Some(msg![env; limit_date timeIntervalSince1970])
    };
    let _ = run_run_loop(env, this, /* single_iteration: */ true, limit);
    // Modes are not yet tracked separately, so source availability is unknown.
    true
}

- (())addTimer:(id)timer // NSTimer*
       forMode:(NSRunLoopMode)mode {
    // Modes share the same timer list until run loop modes are implemented.
    assert!(!mode.is_null());

    log_dbg!(
        "Adding timer {:?} to run loop {:?} with mode {:?}",
        timer,
        this,
        ns_string::to_rust_string(env, mode),
    );

    retain(env, timer);

    let host_object = env.objc.borrow_mut::<NSRunLoopHostObject>(this);
    assert!(!host_object.timers.contains(&timer)); // TODO: what do we do here?
    host_object.timers.push(timer);
    ns_timer::set_run_loop(env, timer, this);
}

- (())run {
    let _ = run_run_loop(env, this, /* single_iteration: */ false, None);
}

- (())runUntilDate:(id)date {
    let time_limit: NSTimeInterval = msg![env; date timeIntervalSince1970];
    let _ = run_run_loop(env, this, /* single_iteration: */ false, Some(time_limit));
}

// TODO: other run methods

@end

};

/// For use by Audio Toolbox.
pub fn add_audio_unit(env: &mut Environment, run_loop: id, unit: AudioUnit) {
    env.objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .audio_units
        .push(unit);
}

/// For use by Audio Toolbox.
pub fn remove_audio_unit(env: &mut Environment, run_loop: id, unit: AudioUnit) -> Result<(), ()> {
    let units = &mut env
        .objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .audio_units;
    if let Some(unit_idx) = units.iter().position(|&item| item == unit) {
        units.remove(unit_idx);
        Ok(())
    } else {
        Err(())
    }
}

/// For use by Audio Toolbox.
/// TODO: Maybe replace this with a `CFRunLoopObserver` or some other generic
/// mechanism?
/// TODO: Handle run loop modes. Currently assumes the common modes.
pub fn add_audio_queue(env: &mut Environment, run_loop: id, queue: AudioQueueRef) {
    env.objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .audio_queues
        .push(queue);
}

/// For use by Audio Toolbox.
pub fn remove_audio_queue(env: &mut Environment, run_loop: id, queue: AudioQueueRef) {
    let queues = &mut env
        .objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .audio_queues;
    let queue_idx = queues.iter().position(|&item| item == queue).unwrap();
    queues.remove(queue_idx);
}

/// For use by NSTimer so it can remove itself once it's invalidated.
/// Like [remove_timer], but does nothing if the timer isn't in the run loop.
/// For `CFRunLoopRemoveTimer`.
pub fn remove_timer_if_present(env: &mut Environment, run_loop: id, timer: id) {
    if env
        .objc
        .borrow::<NSRunLoopHostObject>(run_loop)
        .timers
        .contains(&timer)
    {
        remove_timer(env, run_loop, timer);
    }
}

pub(super) fn remove_timer(env: &mut Environment, run_loop: id, timer: id) {
    log_dbg!("Removing timer {:?} from run loop {:?}", timer, run_loop,);
    let NSRunLoopHostObject { timers, .. } = env.objc.borrow_mut(run_loop);

    let mut i = 0;
    let mut release_count = 0;
    while i < timers.len() {
        if timers[i] == timer {
            timers.swap_remove(i);
            release_count += 1;
        } else {
            i += 1;
        }
    }
    assert!(release_count == 1); // TODO?
    for _ in 0..release_count {
        release(env, timer);
    }
}

/// Adds a selector to perform on the target run loop from
/// performSelector:withObject:onThread:(afterDelay:/waitUntilDone:). The delay
/// arg corrseponds to the afterDelay: arg and should_sync corresponds to
/// waitUntilDone: arg.
///
/// If should_sync is set to true, a semaphore that should
/// be waited on by the calling thread is returned. Otherwise, a null value is
/// returned.
pub(super) fn add_perform_request(
    env: &mut Environment,
    run_loop: id,
    target: id,
    selector: SEL,
    argument: id,
    delay: Option<f64>,
    should_sync: bool,
) -> MutPtr<sem_t> {
    log_dbg!(
        "Adding object selector request {target:?} {:?} {argument:?} on run loop {run_loop:?}",
        selector.as_str(env.mem.as_mut())
    );
    let semaphore = if should_sync {
        host_create_semaphore(env, 0)
    } else {
        MutPtr::null()
    };
    retain(env, target);
    retain(env, argument);

    let NSRunLoopHostObject {
        selector_objects, ..
    } = &mut env.objc.borrow_mut::<NSRunLoopHostObject>(run_loop);
    let due_by = delay.map(|dur| {
        Instant::now()
            .checked_add(Duration::from_secs_f64(dur))
            .unwrap()
    });
    selector_objects.push_back(ObjectSelectorSource {
        target,
        selector,
        argument,
        due_by,
        semaphore,
    });
    semaphore
}

/// Cancels a selector that was previously requested by [add_perform_request].
/// The argument arg is compared via isEqual, [as documented by Apple]
/// (<https://developer.apple.com/documentation/objectivec/nsobject/1410849-cancelpreviousperformrequestswit?language=objc>)
pub(super) fn cancel_perform_requests(
    env: &mut Environment,
    run_loop: id,
    target: id,
    selector: SEL,
    argument: id,
) {
    log_dbg!(
        "Removing object selector request {target:?} {:?} {argument:?} on run loop {run_loop:?}",
        selector.as_str(env.mem.as_mut())
    );
    let mut new_selector_objects = VecDeque::new();
    let host_object = env.objc.borrow_mut::<NSRunLoopHostObject>(run_loop);
    let mut selector_objects = std::mem::take(&mut host_object.selector_objects);
    while let Some(obj) = selector_objects.pop_front() {
        if obj.target != target || obj.selector != selector {
            new_selector_objects.push_back(obj);
            continue;
        }
        let curr_arg = obj.argument;
        let arg_equal = if curr_arg.is_null() {
            argument.is_null()
        } else {
            msg![env; curr_arg isEqual:argument]
        };
        if arg_equal {
            let ObjectSelectorSource {
                target,
                argument,
                semaphore,
                ..
            } = obj;
            release(env, target);
            release(env, argument);
            if !semaphore.is_null() {
                sem_post(env, semaphore);
            }
        } else {
            new_selector_objects.push_back(obj);
        }
    }
    env.objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .selector_objects = new_selector_objects;
}

/// Run the run loop for just a single iteration. This is a special mode just
/// for the app picker, since we don't have `runMode:beforeDate:` yet.
/// (TODO: implement those to replace this.)
pub fn run_run_loop_single_iteration(env: &mut Environment, run_loop: id) {
    let _ = run_run_loop(env, run_loop, /* single_iteration: */ true, None);
}

pub fn run_run_loop(
    env: &mut Environment,
    run_loop: id,
    single_iteration: bool,
    unix_time_limit: Option<f64>,
) -> bool {
    if single_iteration {
        log_dbg!(
            "Entering run loop {:?} (single iteration), limit {:?}",
            run_loop,
            unix_time_limit
        );
    } else {
        log_dbg!(
            "Entering run loop {:?} (indefinitely), limit {:?}",
            run_loop,
            unix_time_limit
        );
    }

    env.objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .stop_flags
        .push(false);

    // Temporary vectors used to track things without needing a reference to the
    // environment or to lock the object. Re-used each iteration for efficiency.
    let mut timers_tmp = Vec::new();
    let mut audio_queues_tmp = Vec::new();
    let mut audio_units_tmp = Vec::new();

    fn limit_sleep_time(current: &mut Option<Instant>, new: Option<Instant>) {
        if let Some(new) = new {
            *current = Some(current.map_or(new, |i| i.min(new)));
        }
    }

    let is_main_run_loop = env.current_thread == 0;

    loop {
        let mut sleep_until = None;

        // Commit implicit CATransactions
        // From the CATransaction docs:
        //  "Implicit transactions are created automatically when the layer
        //  tree is modified by a thread without an active transaction and are
        //  committed automatically when the thread’s runloop next iterates."
        ca_transaction::ThreadLocalState::commit_implicit_transaction(env);

        // We want to process those only on the main run loop
        if is_main_run_loop {
            crate::frameworks::game_controller::handle_controllers(env);
            let next_due = uikit::handle_events(env);
            limit_sleep_time(&mut sleep_until, next_due);

            let next_due = core_animation::recomposite_if_necessary(env, false);
            limit_sleep_time(&mut sleep_until, next_due);
        }

        crate::frameworks::core_foundation::cf_run_loop_source::perform_signaled_sources(
            env, run_loop,
        );
        handle_cf_streams(env, run_loop);

        assert!(timers_tmp.is_empty());
        timers_tmp.extend_from_slice(&env.objc.borrow::<NSRunLoopHostObject>(run_loop).timers);
        // Retain the timers in case a timer cancels another timer
        // (which releases it)
        for timer in timers_tmp.iter() {
            retain(env, *timer);
        }

        for timer in timers_tmp.drain(..) {
            let next_due = ns_timer::handle_timer(env, timer);
            limit_sleep_time(&mut sleep_until, next_due);
            release(env, timer);
        }

        crate::frameworks::openal::capture_audio_marker(env);

        // TODO: We currently don't properly handle if an audio queue or audio
        // unit gets deleted while inside another queue's handler. Fixing this
        // would be best done by implementing a more general run loop source
        // system that can handle invalidation.
        assert!(audio_queues_tmp.is_empty());
        audio_queues_tmp.extend_from_slice(
            &env.objc
                .borrow::<NSRunLoopHostObject>(run_loop)
                .audio_queues,
        );

        for audio_queue in audio_queues_tmp.drain(..) {
            handle_audio_queue(env, audio_queue);
        }

        // TODO: not clear if audio units should be processed in the run loop
        assert!(audio_units_tmp.is_empty());
        audio_units_tmp
            .extend_from_slice(&env.objc.borrow::<NSRunLoopHostObject>(run_loop).audio_units);

        for audio_unit in audio_units_tmp.drain(..) {
            render_audio_unit(env, audio_unit);
        }

        loop {
            let selector_objects = &mut env
                .objc
                .borrow_mut::<NSRunLoopHostObject>(run_loop)
                .selector_objects;
            let to_run = selector_objects
                .iter()
                .enumerate()
                .find(|(_, oss)| oss.due_by.is_none_or(|due_by| Instant::now() >= due_by))
                .map(|(index, _)| index);

            match to_run {
                Some(index) => {
                    // TODO: remove() is linear here
                    let ObjectSelectorSource {
                        target,
                        selector,
                        argument,
                        due_by: _,
                        semaphore,
                    } = selector_objects.remove(index).unwrap();
                    log_dbg!("Running object selector request {target:?} {:?} {argument:?} on run loop {run_loop:?}", selector.as_str(env.mem.as_mut()));

                    if selector.as_str(&env.mem).ends_with(':') {
                        () = msg_send(env, (target, selector, argument));
                    } else {
                        assert!(argument.is_null());
                        () = msg_send(env, (target, selector));
                    }

                    release(env, target);
                    release(env, argument);

                    if !semaphore.is_null() {
                        sem_post(env, semaphore);
                    }
                }
                None => {
                    for oss in selector_objects {
                        limit_sleep_time(&mut sleep_until, oss.due_by);
                    }
                    break;
                }
            }
        }

        if is_main_run_loop {
            crate::libc::dispatch::drain_main_queue(env);
            media_player::handle_players(env);
            crate::frameworks::avfoundation::handle_players(env);
            crate::frameworks::openal::handle_notifications(env);
        }

        // Unfortunately, touchHLE has to poll for certain things repeatedly;
        // it can't just wait until the next event appears.
        //
        // For optimal responsiveness we could poll as often as possible, but
        // this results in 100% usage of a CPU core and excessive energy use.
        // On the other hand, for optimal energy use we could always sleep until
        // the next scheduled event (e.g. the next timer), but this would lead
        // to late handling of unscheduled events (e.g. a finger movement) and
        // events that are scheduled but we can't get the time for currently
        // (audio queue buffer exhaustion).
        //
        // The compromise used here is that we will wait for a 60th of a second,
        // or until the next scheduled event, whichever is sooner. iPhone OS
        // apps can't do more than 60fps so this should be fine.
        if *env
            .objc
            .borrow::<NSRunLoopHostObject>(run_loop)
            .stop_flags
            .last()
            .unwrap()
        {
            break;
        }
        let limit = Duration::from_millis(1000 / 60);
        env.objc.borrow_mut::<NSRunLoopHostObject>(run_loop).waiting = true;
        env.sleep(sleep_until.map_or(limit, |i| i.duration_since(Instant::now()).min(limit)));
        env.objc.borrow_mut::<NSRunLoopHostObject>(run_loop).waiting = false;

        if *env
            .objc
            .borrow::<NSRunLoopHostObject>(run_loop)
            .stop_flags
            .last()
            .unwrap()
        {
            break;
        }

        if single_iteration {
            break;
        }

        if let Some(limit) = unix_time_limit {
            // We use Unix epoch as a convenience reference date.
            // (Apple's epoch is less convenient in Rust. And "pure"
            // Rust approach with Duration/Instant is just too troublesome
            // and not worthy to convert back and forth)
            if SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs_f64()
                >= limit
            {
                break;
            }
        }
    }
    env.objc
        .borrow_mut::<NSRunLoopHostObject>(run_loop)
        .stop_flags
        .pop()
        .unwrap()
}

/// Helper method for `mainRunLoop` and `currentRunLoop` NSThread class methods
fn run_loop_for_thread(env: &mut Environment, this: Class, thread_id: ThreadId) -> id {
    if env.threads[thread_id]
        .framework_state
        .foundation
        .ns_run_loop
        .run_loop
        == nil
    {
        let host_object = Box::new(NSRunLoopHostObject {
            audio_units: Vec::new(),
            audio_queues: Vec::new(),
            selector_objects: VecDeque::new(),
            timers: Vec::new(),
            cf_sources: Vec::new(),
            cf_streams: Vec::new(),
            owner: thread_id,
            waiting: false,
            stop_flags: Vec::new(),
        });
        // TODO: is it OK to allocate static object for all threads,
        // not only main one?
        let new = env
            .objc
            .alloc_static_object(this, host_object, &mut env.mem);
        env.threads[thread_id]
            .framework_state
            .foundation
            .ns_run_loop
            .run_loop = new;
    }
    env.threads[thread_id]
        .framework_state
        .foundation
        .ns_run_loop
        .run_loop
}
