/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::abi::DotDotDot;
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::errno::{set_errno, EINVAL, ENOTSUP};
use crate::libc::posix_io;
use crate::libc::posix_io::{off_t, FileDescriptor, SEEK_SET};
use crate::mem::VMAllocError;
use crate::mem::{ConstPtr, GuestUSize, MutVoidPtr, PAGE_SIZE, PAGE_SIZE_ALIGN_MASK};
use std::collections::HashMap;

#[allow(dead_code)]
const MAP_FILE: i32 = 0x0000;
const MAP_FIXED: i32 = 0x0010;
const MAP_ANON: i32 = 0x1000;

#[derive(Default)]
pub struct State {
    /// Keeping track of `mmap` allocations
    mmap_allocations: HashMap<MutVoidPtr, GuestUSize>,
}

/// For files, our implementation of mmap is really simple:
/// it's just load entirety of file in memory!
fn mmap(
    env: &mut Environment,
    addr: MutVoidPtr,
    len: GuestUSize,
    prot: i32,
    flags: i32,
    fd: FileDescriptor,
    offset: off_t,
) -> MutVoidPtr {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!(
        "mmap({:?}, {}, {}, {}, {}, {})",
        addr,
        len,
        prot,
        flags,
        fd,
        offset
    );

    assert_eq!(offset, 0);
    let ptr = if addr.is_null() {
        env.mem.vm_alloc(None, len).unwrap()
    } else {
        match env.mem.vm_alloc(Some(addr.to_bits()), len) {
            Err(VMAllocError::AddressUnavailable) if flags & MAP_FIXED == 0 => {
                let ptr = env.mem.vm_alloc(None, len).unwrap();
                log!("Warning: mmap could not allocate at hint {addr:?}, allocated at {ptr:?}",);
                ptr
            }
            result => result.unwrap(),
        }
    };

    assert!(ptr.to_bits() & PAGE_SIZE_ALIGN_MASK == 0);

    if (flags & MAP_ANON) != 0 {
        assert_eq!(fd, -1);
    } else {
        let new_offset = posix_io::lseek(env, fd, offset, SEEK_SET);
        assert_eq!(new_offset, offset);

        let read = posix_io::read(env, fd, ptr, len);
        assert_eq!(read as u32, len);
    };

    assert!(!env.libc_state.mman.mmap_allocations.contains_key(&ptr));
    env.libc_state
        .mman
        .mmap_allocations
        .insert(ptr, len.max(1).next_multiple_of(PAGE_SIZE));

    ptr
}

fn munmap(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!("munmap({:?}, {})", addr, len);

    let start = u64::from(addr.to_bits());
    let end = start + u64::from(len).next_multiple_of(u64::from(PAGE_SIZE));
    if len == 0 || addr.to_bits() & PAGE_SIZE_ALIGN_MASK != 0 || end > (1u64 << 32) {
        set_errno(env, EINVAL);
        log!("Warning: munmap({:?}, {}) failed, returning -1", addr, len);
        return -1;
    }

    // Only free pages owned by mmap. Other guest VM allocations may share
    // the requested range. Keep any part of a mapping outside the range.
    let mappings = &mut env.libc_state.mman.mmap_allocations;
    let overlaps: Vec<_> = mappings
        .iter()
        .filter_map(|(&base, &size)| {
            let low = u64::from(base.to_bits());
            let high = low + u64::from(size);
            (low < end && start < high).then_some((base, low, high))
        })
        .collect();
    for (base, low, high) in overlaps {
        let first = low.max(start);
        let last = high.min(end);
        mappings.remove(&base);
        if low < first {
            mappings.insert(base, (first - low) as GuestUSize);
        }
        if last < high {
            mappings.insert(
                MutVoidPtr::from_bits(last as u32),
                (high - last) as GuestUSize,
            );
        }
        env.mem.vm_free(
            MutVoidPtr::from_bits(first as u32),
            (last - first) as GuestUSize,
        );
    }
    0
}

fn madvise(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize, advice: i32) -> i32 {
    log!("TODO: madvise({:?}, {}, {}) -> -1", addr, len, advice);
    set_errno(env, ENOTSUP);
    -1
}

fn shm_open(env: &mut Environment, name: ConstPtr<u8>, oflag: i32, _dots: DotDotDot) -> i32 {
    log!(
        "TODO: shm_open({:?} '{:?}', {}, ...) -> -1",
        name,
        env.mem.cstr_at_utf8(name),
        oflag
    );
    set_errno(env, EINVAL);
    -1
}

fn mprotect(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize, prot: i32) -> i32 {
    log!("TODO: mprotect({:?}, {}, {}) -> -1", addr, len, prot);
    set_errno(env, ENOTSUP);
    -1
}

/// `int mlock(const void *addr, size_t len)` — lock a region of memory
/// so it stays resident in physical RAM. touchHLE keeps the entire
/// guest address space resident in the host process at all times, so
/// there is nothing to pin: the memory is already non-pageable from the
/// guest's perspective. Real Darwin returns 0 on success, so we report
/// success as a no-op (returning -1 here would make guest code that
/// relies on mlock — e.g. crypto/keychain libraries protecting secrets,
/// or audio engines pinning buffers — treat initialization as failed).
///
/// Reference: POSIX/Darwin mlock(2) — returns 0 on success, -1 with
/// errno on failure.
fn mlock(env: &mut Environment, addr: ConstPtr<u8>, len: GuestUSize) -> i32 {
    log_dbg!(
        "mlock({:?}, {}) -> 0 (no-op; guest memory is always resident)",
        addr,
        len
    );
    set_errno(env, 0);
    0
}

/// `int msync(void *addr, size_t len, int flags)` — synchronize a mapped
/// region with the file that backs it. touchHLE's `mmap` copies file
/// content into a guest allocation and never writes it back, and guest
/// memory is not paged, so there is nothing to flush: report success as
/// a no-op (Darwin returns 0 on success; returning -1 would make guests
/// that treat msync failures as fatal — e.g. engines persisting save
/// data through mmap — abort their write path).
///
/// Reference: POSIX msync(2) — returns 0 on success, -1 with errno on
/// failure.
fn msync(env: &mut Environment, addr: MutVoidPtr, len: GuestUSize, flags: i32) -> i32 {
    log_dbg!("msync({:?}, {}, {:#x}) -> 0 (no-op)", addr, len, flags);
    set_errno(env, 0);
    0
}

/// `int munlock(const void *addr, size_t len)` — unlock a region
/// previously locked with `mlock`. Mirrors [mlock]: a no-op that
/// succeeds.
///
/// Reference: POSIX/Darwin munlock(2) — returns 0 on success.
fn munlock(env: &mut Environment, addr: ConstPtr<u8>, len: GuestUSize) -> i32 {
    log_dbg!(
        "munlock({:?}, {}) -> 0 (no-op; guest memory is always resident)",
        addr,
        len
    );
    set_errno(env, 0);
    0
}

/// `int shm_unlink(const char *name)`
///
/// POSIX shared-memory unlink: removes the named shared-memory object.
/// Regions opened via `shm_open` are backed by our fs layer, where `unlink`
/// already implements remove-on-last-close semantics. Missing regions return
/// -1/ENOENT per POSIX.
fn shm_unlink(env: &mut Environment, name: ConstPtr<u8>) -> i32 {
    crate::libc::unistd::unlink(env, name)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(mmap(_, _, _, _, _, _)),
    export_c_func!(munmap(_, _)),
    export_c_func!(madvise(_, _, _)),
    export_c_func!(shm_open(_, _, _)),
    export_c_func!(mprotect(_, _, _)),
    export_c_func!(mlock(_, _)),
    export_c_func!(msync(_, _, _)),
    export_c_func!(munlock(_, _)),
    export_c_func!(shm_unlink(_)),
];
