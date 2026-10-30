/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `malloc.h` memory management zones

use std::collections::HashMap;

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::errno::{set_errno, EINVAL, ENOMEM};
use crate::libc::stdlib::allocation_result;
use crate::mem::{
    guest_size_of, ConstPtr, ConstVoidPtr, GuestUSize, HeapAllocator, Mem, MutPtr, MutVoidPtr, Ptr,
    SafeRead,
};

#[derive(Default)]
pub struct State {
    default_zone: Option<MutPtr<malloc_zone_t>>,
    zone_to_heap: HashMap<MutPtr<malloc_zone_t>, HeapAllocator>,
    owned_names: HashMap<MutPtr<malloc_zone_t>, MutPtr<u8>>,
}

#[repr(C, packed)]
#[allow(non_camel_case_types)]
pub struct malloc_zone_t {
    reserved1: MutVoidPtr,
    reserved2: MutVoidPtr,
    size: MutVoidPtr,
    malloc: MutVoidPtr,
    calloc: MutVoidPtr,
    valloc: MutVoidPtr,
    free: MutVoidPtr,
    realloc: MutVoidPtr,
    destroy: MutVoidPtr,
    zone_name: ConstPtr<u8>,
    batch_malloc: MutVoidPtr,
    batch_free: MutVoidPtr,
    introspect: MutVoidPtr,
    version: u32,
    memalign: MutVoidPtr,
}
unsafe impl SafeRead for malloc_zone_t {}

impl malloc_zone_t {
    pub fn new(env: &mut Environment) -> malloc_zone_t {
        malloc_zone_t {
            reserved1: Ptr::null(),
            reserved2: Ptr::null(),
            size: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_zone_size")
                .unwrap(),
            malloc: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_zone_malloc")
                .unwrap(),
            calloc: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_zone_calloc")
                .unwrap(),
            valloc: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_zone_valloc")
                .unwrap(),
            free: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_zone_free")
                .unwrap(),
            realloc: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_zone_realloc")
                .unwrap(),
            destroy: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_destroy_zone")
                .unwrap(),
            zone_name: Ptr::null(),
            batch_malloc: Ptr::null(),
            batch_free: Ptr::null(),
            introspect: Ptr::null(),
            version: 5,
            memalign: env
                .dyld
                .create_function_address(&mut env.mem, &mut env.cpu, "malloc_zone_memalign")
                .unwrap(),
        }
    }
}

fn malloc_default_zone(env: &mut Environment) -> MutPtr<malloc_zone_t> {
    if env.libc_state.malloc.default_zone.is_none() {
        let zone_data = malloc_zone_t::new(env);
        let zone = env.mem.alloc_and_write(zone_data);
        env.libc_state.malloc.default_zone = Some(zone);
    }

    env.libc_state.malloc.default_zone.unwrap()
}

fn malloc_create_zone(
    env: &mut Environment,
    start_size: GuestUSize,
    flags: u32,
) -> MutPtr<malloc_zone_t> {
    assert_eq!(flags, 0);
    if start_size.checked_add(crate::mem::PAGE_SIZE - 1).is_none() {
        set_errno(env, ENOMEM);
        return Ptr::null();
    }
    let zone_data = malloc_zone_t::new(env);
    let allocation = env.mem.try_alloc(guest_size_of::<malloc_zone_t>());
    let zone: MutPtr<malloc_zone_t> = allocation_result(env, allocation).cast();
    if zone.is_null() {
        return zone;
    }
    let Some(heap) = env.mem.try_create_heap(start_size) else {
        env.mem.free(zone.cast());
        set_errno(env, ENOMEM);
        return Ptr::null();
    };
    env.mem.write(zone, zone_data);
    assert!(env
        .libc_state
        .malloc
        .zone_to_heap
        .insert(zone, heap)
        .is_none());
    zone
}

fn malloc_destroy_zone(env: &mut Environment, zone: MutPtr<malloc_zone_t>) {
    if Some(zone) == env.libc_state.malloc.default_zone {
        panic!("Attempted to destroy default zone");
    } else {
        let heap = env
            .libc_state
            .malloc
            .zone_to_heap
            .remove(&zone)
            .unwrap_or_else(|| panic!("Zone {zone:?} does not map to an allocator"));
        env.mem.destroy_heap(heap);
        if let Some(name) = env.libc_state.malloc.owned_names.remove(&zone) {
            env.mem.free(name.cast());
        }
        env.mem.free(zone.cast());
    }
}

fn malloc_zone_free(env: &mut Environment, zone: MutPtr<malloc_zone_t>, ptr: MutVoidPtr) {
    with_zone(env, zone, |mem, heap| {
        if !ptr.is_null() {
            mem.free_in_heap(heap, ptr);
        }
    })
}

fn malloc_zone_malloc(
    env: &mut Environment,
    zone: MutPtr<malloc_zone_t>,
    size: GuestUSize,
) -> MutVoidPtr {
    let allocation = with_zone(env, zone, |mem, heap| mem.try_alloc_in_heap(heap, size));
    allocation_result(env, allocation)
}

fn malloc_zone_calloc(
    env: &mut Environment,
    zone: MutPtr<malloc_zone_t>,
    count: GuestUSize,
    size: GuestUSize,
) -> MutVoidPtr {
    let allocation = with_zone(env, zone, |mem, heap| {
        let total = count.checked_mul(size)?;
        let ptr = mem.try_alloc_in_heap(heap, total)?;
        mem.bytes_at_mut(ptr.cast(), total).fill(0);
        Some(ptr)
    });
    allocation_result(env, allocation)
}

fn malloc_zone_valloc(
    env: &mut Environment,
    zone: MutPtr<malloc_zone_t>,
    size: GuestUSize,
) -> MutVoidPtr {
    let allocation = with_zone(env, zone, |mem, heap| mem.try_valloc_in_heap(heap, size));
    allocation_result(env, allocation)
}

fn malloc_zone_memalign(
    env: &mut Environment,
    zone: MutPtr<malloc_zone_t>,
    alignment: GuestUSize,
    size: GuestUSize,
) -> MutVoidPtr {
    if !alignment.is_power_of_two() || alignment < 4 {
        set_errno(env, EINVAL);
        return Ptr::null();
    }
    let allocation = with_zone(env, zone, |mem, heap| {
        mem.try_memalign_in_heap(heap, alignment, size)
    });
    allocation_result(env, allocation)
}

fn malloc_zone_realloc(
    env: &mut Environment,
    zone: MutPtr<malloc_zone_t>,
    ptr: MutVoidPtr,
    size: GuestUSize,
) -> MutVoidPtr {
    let allocation = with_zone(env, zone, |mem, heap| resize_in_heap(mem, heap, ptr, size));
    allocation_result(env, allocation)
}

fn malloc_zone_size(
    env: &mut Environment,
    zone: MutPtr<malloc_zone_t>,
    ptr: ConstVoidPtr,
) -> GuestUSize {
    with_zone(env, zone, |mem, heap| match heap {
        Some(heap) => heap.allocated_size(ptr.to_bits()),
        None => mem.default_allocation_size(ptr),
    })
    .unwrap_or(0)
}

fn custom_zone_for_ptr(env: &Environment, ptr: ConstVoidPtr) -> Option<MutPtr<malloc_zone_t>> {
    env.libc_state
        .malloc
        .zone_to_heap
        .iter()
        .find(|(_, heap)| heap.allocated_size(ptr.to_bits()).is_some())
        .map(|(&zone, _)| zone)
}

fn malloc_zone_from_ptr(env: &mut Environment, ptr: ConstVoidPtr) -> MutPtr<malloc_zone_t> {
    if let Some(zone) = custom_zone_for_ptr(env, ptr) {
        return zone;
    }
    if env.mem.default_allocation_size(ptr).is_some() {
        malloc_default_zone(env)
    } else {
        Ptr::null()
    }
}

pub(super) fn allocation_size(env: &mut Environment, ptr: ConstVoidPtr) -> GuestUSize {
    if let Some(zone) = custom_zone_for_ptr(env, ptr) {
        malloc_zone_size(env, zone, ptr)
    } else {
        env.mem.malloc_size(ptr)
    }
}

pub(super) fn free_allocation(env: &mut Environment, ptr: MutVoidPtr) {
    if let Some(zone) = custom_zone_for_ptr(env, ptr.cast_const()) {
        malloc_zone_free(env, zone, ptr);
    } else {
        env.mem.free(ptr);
    }
}

fn resize_in_heap(
    mem: &mut Mem,
    mut heap: Option<&mut HeapAllocator>,
    ptr: MutVoidPtr,
    size: GuestUSize,
) -> Option<MutVoidPtr> {
    if !ptr.is_null() && size == 0 {
        let new = mem.try_alloc_in_heap(heap.as_deref_mut(), 0)?;
        mem.free_in_heap(heap, ptr);
        Some(new)
    } else if heap.is_none() {
        mem.try_realloc(ptr, size)
    } else {
        mem.try_realloc_in_heap(heap, ptr, size)
    }
}

pub(super) fn reallocate(
    env: &mut Environment,
    ptr: MutVoidPtr,
    size: GuestUSize,
) -> Option<MutVoidPtr> {
    if let Some(zone) = custom_zone_for_ptr(env, ptr.cast_const()) {
        with_zone(env, zone, |mem, heap| resize_in_heap(mem, heap, ptr, size))
    } else {
        resize_in_heap(&mut env.mem, None, ptr, size)
    }
}

fn malloc_set_zone_name(env: &mut Environment, zone: MutPtr<malloc_zone_t>, name: ConstPtr<u8>) {
    with_zone(env, zone, |_, _| ());
    let name = if name.is_null() {
        None
    } else {
        let bytes = env.mem.cstr_at(name).to_vec();
        let size = GuestUSize::try_from(bytes.len())
            .ok()
            .and_then(|len| len.checked_add(1));
        let allocation = size.and_then(|size| env.mem.try_alloc(size));
        let ptr: MutPtr<u8> = allocation_result(env, allocation).cast();
        if ptr.is_null() {
            return;
        }
        env.mem
            .bytes_at_mut(ptr, bytes.len().try_into().unwrap())
            .copy_from_slice(&bytes);
        env.mem
            .write(ptr + GuestUSize::try_from(bytes.len()).unwrap(), 0u8);
        Some(ptr)
    };
    if let Some(old) = env.libc_state.malloc.owned_names.remove(&zone) {
        env.mem.free(old.cast());
    }
    let mut zone_data = env.mem.read(zone);
    zone_data.zone_name = name.unwrap_or_default().cast_const();
    env.mem.write(zone, zone_data);
    if let Some(name) = name {
        env.libc_state.malloc.owned_names.insert(zone, name);
    }
}

fn malloc_get_zone_name(env: &mut Environment, zone: MutPtr<malloc_zone_t>) -> ConstPtr<u8> {
    with_zone(env, zone, |_, _| ());
    env.mem.read(zone).zone_name
}

/// Call a function using the heap corresponding to `zone`. Zone helper
/// not a part of the API.
fn with_zone<R>(
    env: &mut Environment,
    zone: MutPtr<malloc_zone_t>,
    f: impl FnOnce(&mut Mem, Option<&mut HeapAllocator>) -> R,
) -> R {
    let heap = if Some(zone) == env.libc_state.malloc.default_zone {
        None
    } else {
        Some(
            env.libc_state
                .malloc
                .zone_to_heap
                .get_mut(&zone)
                .unwrap_or_else(|| panic!("Zone {zone:?} does not map to an allocator")),
        )
    };
    f(&mut env.mem, heap)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(malloc_create_zone(_, _)),
    export_c_func!(malloc_default_zone()),
    export_c_func!(malloc_destroy_zone(_)),
    export_c_func!(malloc_zone_free(_, _)),
    export_c_func!(malloc_zone_malloc(_, _)),
    export_c_func!(malloc_zone_calloc(_, _, _)),
    export_c_func!(malloc_zone_valloc(_, _)),
    export_c_func!(malloc_zone_memalign(_, _, _)),
    export_c_func!(malloc_zone_from_ptr(_)),
    export_c_func!(malloc_zone_realloc(_, _, _)),
    export_c_func!(malloc_zone_size(_, _)),
    export_c_func!(malloc_set_zone_name(_, _)),
    export_c_func!(malloc_get_zone_name(_)),
];
