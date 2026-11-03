/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `cxxabi.h`
//!
//! Resources:
//! - [Itanium C++ ABI specification](https://itanium-cxx-abi.github.io/cxx-abi/abi.html#dso-dtor-runtime-api)

use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{MutPtr, MutVoidPtr};
use crate::Environment;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Default)]
pub struct State {
    guards: HashMap<MutPtr<u8>, usize>,
}

fn __cxa_guard_acquire(env: &mut Environment, guard: MutPtr<u8>) -> i32 {
    if guard.is_null() {
        return 0;
    }
    loop {
        if env.mem.read(guard) & 1 != 0 {
            return 0;
        }
        match env.libc_state.cxxabi.guards.get(&guard) {
            None => {
                env.libc_state
                    .cxxabi
                    .guards
                    .insert(guard, env.current_thread);
                return 1;
            }
            Some(&owner) if owner == env.current_thread => {
                // Recursive initialization is undefined; do not deadlock.
                log_once!("Recursive C++ local static initialization");
                return 0;
            }
            Some(_) => env.sleep(Duration::from_millis(1)),
        }
    }
}

fn __cxa_guard_release(env: &mut Environment, guard: MutPtr<u8>) {
    if env.libc_state.cxxabi.guards.get(&guard) == Some(&env.current_thread) {
        let value = env.mem.read(guard);
        env.mem.write(guard, value | 1);
        env.libc_state.cxxabi.guards.remove(&guard);
    }
}

fn __cxa_guard_abort(env: &mut Environment, guard: MutPtr<u8>) {
    if env.libc_state.cxxabi.guards.get(&guard) == Some(&env.current_thread) {
        env.libc_state.cxxabi.guards.remove(&guard);
    }
}

fn __cxa_atexit(
    _env: &mut Environment,
    func: GuestFunction, // void (*func)(void *)
    p: MutVoidPtr,
    d: MutVoidPtr,
) -> i32 {
    // TODO: when this is implemented, make sure it's properly compatible with
    // C atexit.
    log!(
        "TODO: __cxa_atexit({:?}, {:?}, {:?}) (unimplemented)",
        func,
        p,
        d
    );
    0 // success
}

fn __cxa_finalize(_env: &mut Environment, d: MutVoidPtr) {
    log!("TODO: __cxa_finalize({:?}) (unimplemented)", d);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__cxa_atexit(_, _, _)),
    export_c_func!(__cxa_finalize(_)),
    export_c_func!(__cxa_guard_acquire(_)),
    export_c_func!(__cxa_guard_release(_)),
    export_c_func!(__cxa_guard_abort(_)),
];
