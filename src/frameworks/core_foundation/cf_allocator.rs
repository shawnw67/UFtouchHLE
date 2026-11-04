/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFAllocator`.
//!
//! Only a small subset of allocators is currently supported.

use super::CFIndex;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::mem::{ConstPtr, ConstVoidPtr, MutVoidPtr, Ptr, SafeRead};
use crate::Environment;

// CFAllocator is an opaque struct.
// It doesn't need to be a CFTypeRef compatible (seems so?),
// so it's just a struct without Obj-C counterpart.
pub struct CFAllocatorHostObject(CFAllocatorType);
unsafe impl SafeRead for CFAllocatorHostObject {}

impl CFAllocatorHostObject {
    pub fn is_system_default(&self) -> bool {
        self.0 == CFAllocatorType::SystemDefault
    }
    pub fn is_null(&self) -> bool {
        self.0 == CFAllocatorType::Null
    }
}

// TODO: support other types
#[repr(i32)]
#[derive(PartialEq)]
enum CFAllocatorType {
    /// For us, same as Default one, but not a NULL ptr.
    SystemDefault = 1,
    Null = 2,
}

pub type CFAllocatorRef = ConstPtr<CFAllocatorHostObject>;

pub const kCFAllocatorDefault: CFAllocatorRef = Ptr::null();

#[derive(Default)]
pub(super) struct State {
    system: Option<CFAllocatorRef>,
}

fn system_allocator(env: &mut Environment) -> CFAllocatorRef {
    if let Some(allocator) = env.framework_state.core_foundation.cf_allocator.system {
        return allocator;
    }
    let allocator = env
        .mem
        .alloc_and_write(CFAllocatorHostObject(CFAllocatorType::SystemDefault))
        .cast_const();
    env.framework_state.core_foundation.cf_allocator.system = Some(allocator);
    allocator
}

fn is_default(env: &Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()
}

fn CFAllocatorAllocate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    size: CFIndex,
    _hint: u32,
) -> MutVoidPtr {
    if size <= 0 || !is_default(env, allocator) {
        return Ptr::null();
    }
    env.mem.try_alloc(size as u32).unwrap_or_else(Ptr::null)
}

fn CFAllocatorDeallocate(env: &mut Environment, allocator: CFAllocatorRef, ptr: MutVoidPtr) {
    if !ptr.is_null() && is_default(env, allocator) {
        env.mem.free(ptr);
    }
}

fn CFAllocatorReallocate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    ptr: MutVoidPtr,
    size: CFIndex,
    hint: u32,
) -> MutVoidPtr {
    if size < 0 || !is_default(env, allocator) {
        return Ptr::null();
    }
    if size == 0 {
        CFAllocatorDeallocate(env, allocator, ptr);
        return Ptr::null();
    }
    if ptr.is_null() {
        return CFAllocatorAllocate(env, allocator, size, hint);
    }
    env.mem
        .try_realloc(ptr, size as u32)
        .unwrap_or_else(Ptr::null)
}

fn CFAllocatorGetDefault(env: &mut Environment) -> CFAllocatorRef {
    system_allocator(env)
}

fn CFGetAllocator(env: &mut Environment, _object: ConstVoidPtr) -> CFAllocatorRef {
    system_allocator(env)
}

fn CFAllocatorGetPreferredSizeForSize(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    size: CFIndex,
    _hint: u32,
) -> CFIndex {
    if size <= 0 || !is_default(env, allocator) {
        0
    } else {
        size
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFAllocatorAllocate(_, _, _)),
    export_c_func!(CFAllocatorDeallocate(_, _)),
    export_c_func!(CFAllocatorReallocate(_, _, _, _)),
    export_c_func!(CFAllocatorGetDefault()),
    export_c_func!(CFGetAllocator(_)),
    export_c_func!(CFAllocatorGetPreferredSizeForSize(_, _, _)),
];

pub const CONSTANTS: ConstantExports = &[
    ("_kCFAllocatorDefault", HostConstant::NullPtr),
    (
        "_kCFAllocatorSystemDefault",
        HostConstant::Custom(|env| {
            let allocator_ptr = system_allocator(env);
            env.mem.alloc_and_write(allocator_ptr).cast().cast_const()
        }),
    ),
    (
        "_kCFAllocatorNull",
        HostConstant::Custom(|env| {
            let allocator_ptr = env
                .mem
                .alloc_and_write(CFAllocatorHostObject(CFAllocatorType::Null));
            env.mem.alloc_and_write(allocator_ptr).cast().cast_const()
        }),
    ),
];
