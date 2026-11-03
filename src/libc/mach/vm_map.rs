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

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(vm_allocate(_, _, _, _)),
    export_c_func!(vm_deallocate(_, _, _)),
    export_c_func!(vm_read_overwrite(_, _, _, _, _)),
    export_c_func!(vm_purgable_control(_, _, _, _)),
];
