/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `semaphore.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::set_errno;
use crate::libc::posix_io::stat::mode_t;
use crate::libc::posix_io::{O_CREAT, O_EXCL};
use crate::mem::{ConstPtr, MutPtr};
use crate::{Environment, ThreadId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::errno::{EAGAIN, EBUSY, EEXIST, EFAULT, EINVAL, ENAMETOOLONG, ENOENT, ENOTSUP};

// Darwin's sys/semaphore.h, not the host's POSIX semaphore implementation.
pub const SEM_VALUE_MAX: i32 = 32767;

// SEM_FAILED is defined as -1 while having a type of sem_t *
pub const SEM_FAILED: MutPtr<sem_t> = MutPtr::from_bits(u32::MAX);

#[derive(Default)]
pub struct State {
    named_semaphores: HashMap<Vec<u8>, Rc<RefCell<SemaphoreHostObject>>>,
    pub open_semaphores: HashMap<MutPtr<sem_t>, Rc<RefCell<SemaphoreHostObject>>>,
}
impl State {
    fn get(env: &Environment) -> &Self {
        &env.libc_state.semaphore
    }
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.libc_state.semaphore
    }
}

#[allow(non_camel_case_types)]
pub type sem_t = i32;

pub struct SemaphoreHostObject {
    pub value: i32,
    pub max_value: i32,
    pub waiting: HashSet<ThreadId>,
    guest_sem: Option<MutPtr<sem_t>>,
    named: bool,
}

pub fn sem_init(env: &mut Environment, sem: MutPtr<sem_t>, pshared: i32, value: u32) -> i32 {
    if pshared != 0 {
        return failure(env, ENOTSUP);
    }
    if sem.is_null()
        || sem == SEM_FAILED
        || value > SEM_VALUE_MAX as u32
        || env
            .mem
            .get_bytes_fallible(sem.cast_const().cast(), 4)
            .is_none()
    {
        return failure(env, EINVAL);
    }
    if env.libc_state.semaphore.open_semaphores.contains_key(&sem) {
        return failure(env, EBUSY);
    }
    init_unnamed(env, sem, value, SEM_VALUE_MAX);
    0
}

fn init_unnamed(env: &mut Environment, sem: MutPtr<sem_t>, value: u32, max_value: i32) {
    let state = State::get_mut(env);
    let host_sem_rc = Rc::new(RefCell::new(SemaphoreHostObject {
        value: value as i32,
        max_value,
        waiting: HashSet::new(),
        guest_sem: Some(sem),
        named: false,
    }));

    state.open_semaphores.insert(sem, host_sem_rc);
}

pub fn sem_destroy(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    let Some(object) = State::get(env).open_semaphores.get(&sem).cloned() else {
        return failure(env, EINVAL);
    };
    if object.borrow().named {
        return failure(env, EINVAL);
    }
    if !object.borrow().waiting.is_empty() {
        return failure(env, EBUSY);
    }
    State::get_mut(env).open_semaphores.remove(&sem);
    // An unnamed semaphore's storage belongs to the guest.
    0
}

fn failure(env: &mut Environment, errno: i32) -> i32 {
    set_errno(env, errno);
    -1
}

fn read_name(mem: &crate::mem::Mem, name: ConstPtr<u8>) -> Result<Vec<u8>, i32> {
    if name.is_null() {
        return Err(EFAULT);
    }
    let mut bytes = Vec::new();
    // XNU PSEMNAMLEN is 31 bytes including the terminating NUL.
    for index in 0..31 {
        let address = name.to_bits().checked_add(index).ok_or(EFAULT)?;
        let byte = *mem
            .get_bytes_fallible(ConstPtr::<u8>::from_bits(address).cast(), 1)
            .ok_or(EFAULT)?
            .first()
            .ok_or(EFAULT)?;
        if byte == 0 {
            return if bytes.is_empty() {
                Err(EINVAL)
            } else {
                Ok(bytes)
            };
        }
        bytes.push(byte);
    }
    Err(ENAMETOOLONG)
}

pub fn sem_open(
    env: &mut Environment,
    name: ConstPtr<u8>,
    oflag: i32,
    _mode: mode_t,
    value: u32,
) -> MutPtr<sem_t> {
    let sem_name = match read_name(&env.mem, name) {
        Ok(name) => name,
        Err(error) => {
            failure(env, error);
            return SEM_FAILED;
        }
    };
    let host_sem_rc =
        if let Some(existing_host_sem_rc) = State::get(env).named_semaphores.get(&sem_name) {
            // HyperHLE-Reborn 099b81cd fixes the inverted O_EXCL check.
            // O_EXCL only has defined meaning together with O_CREAT.
            if (oflag & (O_CREAT | O_EXCL)) == (O_CREAT | O_EXCL) {
                set_errno(env, EEXIST);
                return SEM_FAILED;
            }
            let existing_host_sem = (*existing_host_sem_rc).borrow();
            if let Some(existing_sem) = existing_host_sem.guest_sem {
                return existing_sem;
            }
            existing_host_sem_rc.clone()
        } else {
            if (oflag & O_CREAT) == 0 {
                set_errno(env, ENOENT);
                return SEM_FAILED;
            }
            if value > SEM_VALUE_MAX as u32 {
                failure(env, EINVAL);
                return SEM_FAILED;
            }
            let host_sem_rc = Rc::new(RefCell::new(SemaphoreHostObject {
                value: value as i32,
                max_value: SEM_VALUE_MAX,
                waiting: HashSet::new(),
                guest_sem: None,
                named: true,
            }));
            State::get_mut(env)
                .named_semaphores
                .insert(sem_name, Rc::clone(&host_sem_rc));
            host_sem_rc
        };

    let sem = env.mem.alloc_and_write(0);
    (*host_sem_rc).borrow_mut().guest_sem = Some(sem);
    State::get_mut(env).open_semaphores.insert(sem, host_sem_rc);

    sem
}

pub fn sem_post(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    match env.sem_increment(sem) {
        Ok(()) => 0,
        Err(error) => failure(env, error),
    }
}

pub fn sem_wait(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    if !State::get(env).open_semaphores.contains_key(&sem) {
        return failure(env, EINVAL);
    }
    env.sem_decrement(sem, true);
    0 // success
}

fn sem_trywait(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    if !State::get(env).open_semaphores.contains_key(&sem) {
        return failure(env, EINVAL);
    }
    if env.sem_decrement(sem, false) {
        0 // success
    } else {
        failure(env, EAGAIN)
    }
}

pub fn sem_close(env: &mut Environment, sem: MutPtr<sem_t>) -> i32 {
    let Some(host_sem_rc) = State::get(env).open_semaphores.get(&sem).cloned() else {
        return failure(env, EINVAL);
    };
    let mut host_sem = (*host_sem_rc).borrow_mut();
    if !host_sem.named {
        return failure(env, EINVAL);
    }
    // Closing with blocked users is undefined by POSIX. Keep the scheduler's
    // live object and report misuse rather than leaving a dangling wait key.
    if !host_sem.waiting.is_empty() {
        return failure(env, EBUSY);
    }
    State::get_mut(env).open_semaphores.remove(&sem);
    env.mem.free(host_sem.guest_sem.unwrap().cast());
    host_sem.guest_sem = None;
    0 // success
}

pub fn sem_unlink(env: &mut Environment, name: ConstPtr<u8>) -> i32 {
    let name = match read_name(&env.mem, name) {
        Ok(name) => name,
        Err(error) => return failure(env, error),
    };
    if State::get_mut(env).named_semaphores.remove(&name).is_some() {
        0
    } else {
        failure(env, ENOENT)
    }
}

/// Shortcut for host code to make an unnamed semaphore. Destroy with
/// host_destroy_semaphore, not sem_destroy
pub fn host_create_semaphore(env: &mut Environment, value: u32) -> MutPtr<sem_t> {
    let sem: MutPtr<sem_t> = env.mem.alloc_and_write(0);
    // GCD uses long counters, independent of Darwin's POSIX SEM_VALUE_MAX.
    assert!(value <= i32::MAX as u32);
    init_unnamed(env, sem, value, i32::MAX);
    sem
}
pub fn host_destroy_semaphore(env: &mut Environment, sem: MutPtr<sem_t>) {
    assert_eq!(sem_destroy(env, sem), 0, "destroying a live host semaphore");
    env.mem.free(sem.cast());
}

// POSIX compatibility extension, like our existing unnamed sem_init support.
// This reads actual emulator state, not a fabricated success or waiter count.
fn sem_getvalue(env: &mut Environment, sem: MutPtr<sem_t>, output: MutPtr<i32>) -> i32 {
    let Some(object) = State::get(env).open_semaphores.get(&sem) else {
        return failure(env, EINVAL);
    };
    let value = object.borrow().value;
    if output.is_null() {
        return failure(env, EFAULT);
    }
    let Some(bytes) = env
        .mem
        .get_bytes_fallible_mut(output.cast_const().cast(), 4)
    else {
        return failure(env, EFAULT);
    };
    bytes.copy_from_slice(&value.to_le_bytes());
    0
}

#[cfg(test)]
mod name_tests {
    use super::*;
    use crate::mem::Mem;

    #[test]
    fn names_are_byte_strings_with_darwin_length_bounds() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(4096);
        for name in [vec![b'a'; 30], vec![0xff, 0x80], b"without_slash".to_vec()] {
            let pointer = mem.alloc_and_write_cstr(&name);
            assert_eq!(read_name(&mem, pointer.cast_const()), Ok(name));
        }
        let long = mem.alloc_and_write_cstr(&[b'a'; 31]);
        assert_eq!(read_name(&mem, long.cast_const()), Err(ENAMETOOLONG));
        let empty = mem.alloc_and_write_cstr(b"");
        assert_eq!(read_name(&mem, empty.cast_const()), Err(EINVAL));
        assert_eq!(read_name(&mem, ConstPtr::null()), Err(EFAULT));
        // Use a genuinely protected address, rather than assuming a high
        // address is invalid in the guest's 4 GiB virtual address space.
        let mut protected = Mem::new();
        protected.set_null_segment_size(4096);
        assert_eq!(read_name(&protected, ConstPtr::from_bits(1)), Err(EFAULT));
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sem_init(_, _, _)),
    export_c_func!(sem_destroy(_)),
    export_c_func!(sem_open(_, _, _, _)),
    export_c_func!(sem_post(_)),
    export_c_func!(sem_wait(_)),
    export_c_func!(sem_trywait(_)),
    export_c_func!(sem_getvalue(_, _)),
    export_c_func!(sem_close(_)),
    export_c_func!(sem_unlink(_)),
];
