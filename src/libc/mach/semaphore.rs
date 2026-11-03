/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach/semaphore.h`
//!
//! Implemented as a wrapper around libc semaphore

#![allow(non_camel_case_types)]

use std::time::{Duration, Instant};

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::mach::arm::task::task_t;
use crate::libc::mach::init::MACH_TASK_SELF;
use crate::libc::mach::thread_info::{kern_return_t, KERN_SUCCESS};
use crate::libc::semaphore::{sem_destroy, sem_init, sem_post, sem_t, sem_wait};
use crate::mem::MutPtr;

// Opaque type. Reusing sem_t for convenience
// TODO: `semaphore_t` should be `mach_port_t`
type semaphore = sem_t;
type semaphore_t = MutPtr<semaphore>;
const KERN_INVALID_ARGUMENT: kern_return_t = 4;
const KERN_OPERATION_TIMED_OUT: kern_return_t = 49;

fn semaphore_create(
    env: &mut Environment,
    task: task_t,
    semaphore: MutPtr<semaphore_t>,
    policy: i32,
    value: i32,
) -> kern_return_t {
    assert_eq!(task, MACH_TASK_SELF);
    assert_eq!(policy, 0);

    let open_semaphore: semaphore_t = env.mem.alloc_and_write(0);
    let res = sem_init(env, open_semaphore, 0, value.try_into().unwrap());
    assert_eq!(res, 0);

    env.mem.write(semaphore, open_semaphore);
    let result = KERN_SUCCESS;
    log_dbg!(
        "semaphore_create({:?}, {:?}, {:?}, {:?}) -> {:?}",
        task,
        semaphore,
        policy,
        value,
        result
    );
    result
}

fn semaphore_signal(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    let result = if sem_post(env, semaphore) == 0 {
        KERN_SUCCESS
    } else {
        KERN_INVALID_ARGUMENT
    };
    log_dbg!("semaphore_signal({:?}) -> {:?}", semaphore, result);
    result
}

fn semaphore_wait(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    let result = if sem_wait(env, semaphore) == 0 {
        KERN_SUCCESS
    } else {
        KERN_INVALID_ARGUMENT
    };
    log_dbg!("semaphore_wait({:?}) -> {:?}", semaphore, result);
    result
}

fn semaphore_trywait(env: &mut Environment, semaphore: semaphore_t) -> kern_return_t {
    if !env
        .libc_state
        .semaphore
        .open_semaphores
        .contains_key(&semaphore)
    {
        return KERN_INVALID_ARGUMENT;
    }
    if env.sem_decrement(semaphore, false) {
        KERN_SUCCESS
    } else {
        KERN_OPERATION_TIMED_OUT
    }
}

// The 32-bit guest ABI passes mach_timespec_t as consecutive u32 words.
fn semaphore_timedwait(
    env: &mut Environment,
    semaphore: semaphore_t,
    seconds: u32,
    nanos: u32,
) -> kern_return_t {
    if nanos >= 1_000_000_000 {
        return KERN_INVALID_ARGUMENT;
    }
    let timeout = Duration::new(u64::from(seconds), nanos);
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return KERN_INVALID_ARGUMENT;
    };
    loop {
        let result = semaphore_trywait(env, semaphore);
        if result != KERN_OPERATION_TIMED_OUT {
            return result;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return KERN_OPERATION_TIMED_OUT;
        };
        if remaining.is_zero() {
            return KERN_OPERATION_TIMED_OUT;
        }
        env.sleep(remaining.min(Duration::from_millis(1)));
    }
}

fn semaphore_destroy(env: &mut Environment, task: task_t, semaphore: semaphore_t) -> kern_return_t {
    assert_eq!(task, MACH_TASK_SELF);

    sem_destroy(env, semaphore);
    env.mem.free(semaphore.cast());
    let result = KERN_SUCCESS;
    log_dbg!("semaphore_destroy({:?}) -> {:?}", semaphore, result);
    result
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(semaphore_create(_, _, _, _)),
    export_c_func!(semaphore_signal(_)),
    export_c_func!(semaphore_wait(_)),
    export_c_func!(semaphore_trywait(_)),
    export_c_func!(semaphore_timedwait(_, _, _)),
    export_c_func!(semaphore_destroy(_, _)),
];
