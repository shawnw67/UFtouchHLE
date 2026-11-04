/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Mach VM functions

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::mach::init::MACH_TASK_SELF;
use crate::libc::mach::port::mach_port_t;
use crate::libc::mach::thread_info::{kern_return_t, KERN_SUCCESS};
use crate::mem::ConstPtr;
use crate::mem::SafeRead;
use crate::mem::{MutPtr, Ptr, PAGE_SIZE, PAGE_SIZE_ALIGN_MASK};
use crate::Environment;
use std::collections::HashMap;

type vm_map_t = mach_port_t;
type vm_purgable_t = i32;
type mach_vm_address_t = u32;
type mach_vm_size_t = u32;

const VM_FLAGS_ANYWHERE: i32 = 0x1;
const KERN_INVALID_ADDRESS: kern_return_t = 1;
const KERN_NO_SPACE: kern_return_t = 3;
const KERN_INVALID_ARGUMENT: kern_return_t = 4;

#[derive(Default)]
pub struct State {
    /// Keeping track of `vm_allocate` allocations
    allocations: HashMap<mach_vm_address_t, mach_vm_size_t>,
}

pub fn vm_allocate(
    env: &mut Environment,
    target_task: vm_map_t,
    address_ptr: MutPtr<mach_vm_address_t>,
    size: mach_vm_size_t,
    flags: i32,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF || address_ptr.is_null() || flags & !VM_FLAGS_ANYWHERE != 0 {
        return KERN_INVALID_ARGUMENT;
    }
    let Some(rounded_size) = size
        .checked_next_multiple_of(PAGE_SIZE)
        .filter(|&size| size != 0)
    else {
        return KERN_NO_SPACE;
    };

    let address = (flags & VM_FLAGS_ANYWHERE == 0).then(|| env.mem.read(address_ptr));

    let Ok(allocated) = env.mem.vm_alloc(address, rounded_size) else {
        return KERN_NO_SPACE;
    };
    let address = allocated.to_bits();
    assert!(address & PAGE_SIZE_ALIGN_MASK == 0);
    env.mem.write(address_ptr, address);

    assert!(!env.libc_state.mach_vm.allocations.contains_key(&address));
    env.libc_state
        .mach_vm
        .allocations
        .insert(address, rounded_size);

    KERN_SUCCESS
}

fn vm_deallocate(
    env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    size: mach_vm_size_t,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF {
        return KERN_INVALID_ARGUMENT;
    }
    if size == 0 {
        return KERN_SUCCESS;
    }
    let start = address & !PAGE_SIZE_ALIGN_MASK;
    let end = (u64::from(address) + u64::from(size) + u64::from(PAGE_SIZE_ALIGN_MASK))
        & !u64::from(PAGE_SIZE_ALIGN_MASK);
    if end > (1u64 << 32) {
        return KERN_INVALID_ADDRESS;
    }
    let mut regions: Vec<(u32, u64)> = env
        .libc_state
        .mach_vm
        .allocations
        .iter()
        .filter_map(|(&base, &length)| {
            let region_end = u64::from(base) + u64::from(length);
            (u64::from(base) < end && region_end > u64::from(start)).then_some((base, region_end))
        })
        .collect();
    regions.sort_unstable();
    let mut covered = u64::from(start);
    for &(base, region_end) in &regions {
        if u64::from(base) > covered {
            break;
        }
        covered = covered.max(region_end);
    }
    if covered < end {
        return KERN_INVALID_ADDRESS;
    }
    let Ok(length) = u32::try_from(end - u64::from(start)) else {
        return KERN_INVALID_ADDRESS;
    };
    env.mem.vm_free(Ptr::from_bits(start), length);
    let allocations = &mut env.libc_state.mach_vm.allocations;
    for (base, region_end) in regions {
        allocations.remove(&base);
        if start > base {
            allocations.insert(base, start - base);
        }
        if end < region_end {
            allocations.insert(end as u32, (region_end - end) as u32);
        }
    }

    KERN_SUCCESS
}

fn vm_read_overwrite(
    env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    size: mach_vm_size_t,
    data: mach_vm_address_t,
    outsize: MutPtr<mach_vm_size_t>,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF || outsize.is_null() || size >= 0x8000_0000 {
        return KERN_INVALID_ARGUMENT;
    }
    if env
        .mem
        .get_bytes_fallible(outsize.cast_const().cast(), 4)
        .is_none()
    {
        return KERN_INVALID_ADDRESS;
    }
    if size != 0 {
        if env
            .mem
            .get_bytes_fallible(Ptr::from_bits(address), size)
            .is_none()
            || env
                .mem
                .get_bytes_fallible(Ptr::from_bits(data), size)
                .is_none()
        {
            return KERN_INVALID_ADDRESS;
        }
        env.mem
            .memmove(Ptr::from_bits(data), Ptr::from_bits(address), size);
    }
    env.mem.write(outsize, size);
    KERN_SUCCESS
}

fn vm_purgable_control(
    _env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    control: vm_purgable_t,
    state: MutPtr<vm_purgable_t>,
) -> kern_return_t {
    assert_eq!(target_task, MACH_TASK_SELF);
    log!("TODO: vm_purgable_control({target_task:#x}, {address:#x}, {control:#x}, {state:?})");
    KERN_SUCCESS
}

/// `kern_return_t vm_protect(vm_map_t target_task, vm_address_t address,
/// vm_size_t size, boolean_t set_maximum, vm_prot_t new_protection)`
///
/// Guest memory has no per-page protection enforcement, so this is a no-op
/// that reports success (a real kernel would return KERN_INVALID_ADDRESS for
/// unmapped ranges; callers like Chrome's sandbox setup ignore the result).
fn vm_protect(
    _env: &mut Environment,
    target_task: vm_map_t,
    address: mach_vm_address_t,
    size: mach_vm_size_t,
    set_maximum: i32,
    new_protection: vm_prot_t,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF {
        return KERN_INVALID_ADDRESS;
    }
    log_dbg!(
        "vm_protect({:#x}, {:#x}, set_max={}, prot={:#x}) accepted (no-op)",
        address,
        size,
        set_maximum != 0,
        new_protection
    );
    KERN_SUCCESS
}

/// `kern_return_t vm_region_recurse(vm_map_t target_task,
/// vm_address_t *address, vm_size_t *size, uint32_t *nesting_depth,
/// vm_region_recurse_info_t info, mach_msg_type_number_t *info_count)`
///
/// Reports tracked heap regions. Apps like Chrome use this to probe mappings
/// and tolerate failure, but returning success for tracked regions is closer
/// to a real kernel.
fn vm_region_recurse(
    env: &mut Environment,
    target_task: vm_map_t,
    address_ptr: MutPtr<mach_vm_address_t>,
    size_ptr: MutPtr<mach_vm_size_t>,
    nesting_depth_ptr: MutPtr<u32>,
    info_ptr: MutPtr<u8>,
    info_count_ptr: MutPtr<u32>,
) -> kern_return_t {
    if target_task != MACH_TASK_SELF || address_ptr.is_null() || size_ptr.is_null() {
        return KERN_INVALID_ADDRESS;
    }
    let address = env.mem.read(address_ptr);
    let Some(tracked) = env.libc_state.mach_vm.allocations.get(&address) else {
        log_dbg!(
            "vm_region_recurse({:#x}): untracked region; returning KERN_INVALID_ADDRESS",
            address
        );
        return KERN_INVALID_ADDRESS;
    };
    env.mem.write(size_ptr, *tracked);
    if !nesting_depth_ptr.is_null() {
        env.mem.write(nesting_depth_ptr, 0);
    }
    if !info_ptr.is_null() && !info_count_ptr.is_null() {
        let info_count = env.mem.read(info_count_ptr);
        if info_count >= 9 {
            // vm_region_basic_info_data_t: five u32 fields in our layout
            let info: MutPtr<vm_region_basic_info_data_t> = Ptr::from_bits(info_ptr.to_bits());
            env.mem.write(
                info,
                vm_region_basic_info_data_t {
                    protection: VM_PROT_READ | VM_PROT_WRITE,
                    max_protection: VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE,
                    inheritance: 1, // VM_INHERIT_COPY
                    reserved: 0,
                    offset: 0,
                },
            );
        }
    }
    KERN_SUCCESS
}

/// `kern_return_t vm_remap(vm_map_t target_task, vm_address_t *target_address,
/// vm_size_t size, vm_address_t mask, int flags, vm_map_t src_task,
/// vm_address_t src_address, boolean_t copy, vm_prot_t *cur_protection,
/// vm_prot_t *max_protection, vm_inherit_t inheritance)`
///
/// Real Mach `vm_remap` maps the physical pages backing a region of one task's
/// address space into another (or the same) task, optionally sharing them.
/// touchHLE has a single flat guest address space and no notion of shared
/// physical pages, so we approximate it: allocate a fresh page-aligned region
/// and copy the source bytes into it. This is what callers like the Mono/Boehm
/// GC actually need to keep running — a valid, readable mapping of the same
/// contents — even though writes won't be reflected back into the source.
#[allow(clippy::too_many_arguments)]
fn vm_remap(
    env: &mut Environment,
    target_task: vm_map_t,
    target_address: MutPtr<mach_vm_address_t>,
    size: mach_vm_size_t,
    _mask: mach_vm_address_t,
    _flags: i32,
    _src_task: vm_map_t,
    src_address: mach_vm_address_t,
    copy: i32,
    cur_protection: MutPtr<vm_prot_t>,
    max_protection: MutPtr<vm_prot_t>,
    _inheritance: vm_inherit_t,
) -> kern_return_t {
    assert_eq!(target_task, MACH_TASK_SELF);

    if size == 0 || src_address == 0 {
        log!(
            "Warning: vm_remap(src={:#x}, size={:#x}): invalid argument; returning KERN_INVALID_ADDRESS.",
            src_address,
            size
        );
        return KERN_INVALID_ADDRESS;
    }

    // `size` is always rounded up to an integral number of pages.
    let new_size = round_up_to_page(size);

    log_dbg!(
        "vm_remap(src={:#x}, size={:#x}, copy={}) approximated by allocate + copy",
        src_address,
        size,
        copy
    );

    let allocated = env.mem.alloc(new_size);
    let address = allocated.to_bits();
    assert!(address & PAGE_SIZE_ALIGN_MASK == 0);

    // Copy the source region's bytes so the new mapping reads back the same
    // data. We copy the page-rounded length so the trailing partial page is
    // also valid.
    let copy_len = new_size;
    let src: ConstPtr<u8> = Ptr::from_bits(src_address);
    let dst: MutPtr<u8> = Ptr::from_bits(address);
    let bytes: Vec<u8> = env.mem.bytes_at(src, copy_len).to_vec();
    env.mem.bytes_at_mut(dst, copy_len).copy_from_slice(&bytes);

    env.mem.write(target_address, address);
    if !cur_protection.is_null() {
        env.mem.write(
            cur_protection,
            VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE,
        );
    }
    if !max_protection.is_null() {
        env.mem.write(
            max_protection,
            VM_PROT_READ | VM_PROT_WRITE | VM_PROT_EXECUTE,
        );
    }

    assert!(!env.libc_state.mach_vm.allocations.contains_key(&address));
    env.libc_state.mach_vm.allocations.insert(address, new_size);

    KERN_SUCCESS
}

const VM_PROT_EXECUTE: vm_prot_t = 4;

const VM_PROT_READ: vm_prot_t = 1;

const VM_PROT_WRITE: vm_prot_t = 2;

/// Round a byte count up to a whole number of pages, as Mach does for every
/// region size it hands out or accepts.
fn round_up_to_page(size: mach_vm_size_t) -> mach_vm_size_t {
    if !size.is_multiple_of(PAGE_SIZE) {
        size + PAGE_SIZE - (size % PAGE_SIZE)
    } else {
        size
    }
}

type vm_inherit_t = u32;

type vm_prot_t = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct vm_region_basic_info_data_t {
    protection: vm_prot_t,
    max_protection: vm_prot_t,
    inheritance: vm_inherit_t,
    reserved: u32,
    offset: u32,
}

unsafe impl SafeRead for vm_region_basic_info_data_t {}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(vm_allocate(_, _, _, _)),
    export_c_func!(vm_deallocate(_, _, _)),
    export_c_func!(vm_read_overwrite(_, _, _, _, _)),
    export_c_func!(vm_purgable_control(_, _, _, _)),
    export_c_func!(vm_protect(_, _, _, _, _)),
    export_c_func!(vm_region_recurse(_, _, _, _, _, _)),
    export_c_func!(vm_remap(_, _, _, _, _, _, _, _, _, _, _)),
];
