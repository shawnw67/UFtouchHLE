/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFDictionary` and `CFMutableDictionary`.
//!
//! These are toll-free bridged to `NSDictionary` and `NSMutableDictionary` in
//! Apple's implementation. Here they are the same types.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFHashCode, CFIndex, CFRelease, CFRetain};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{
    export_c_func, ConstantExports, Dyld, FunctionExports, HostConstant, HostFunction,
};
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::cf_type::{CFEqual, CFHash};
use crate::frameworks::foundation::ns_dictionary::{
    cf_dictionary_create, cf_dictionary_entries, CFDictionaryKeyCallBacks,
    CFDictionaryValueCallBacks,
};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::{ConstPtr, ConstVoidPtr, Mem, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

pub type CFDictionaryRef = super::CFTypeRef;
pub type CFMutableDictionaryRef = super::CFTypeRef;

fn CFDictionaryCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    keys: ConstPtr<ConstVoidPtr>,
    values: ConstPtr<ConstVoidPtr>,
    count: CFIndex,
    key_callbacks: ConstPtr<CFDictionaryKeyCallBacks>,
    value_callbacks: ConstPtr<CFDictionaryValueCallBacks>,
) -> CFDictionaryRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let count: u32 = count.try_into().unwrap();
    let entries: Vec<(id, id)> = (0..count)
        .map(|i| {
            (
                env.mem.read(keys + i).cast_mut().cast(),
                env.mem.read(values + i).cast_mut().cast(),
            )
        })
        .collect();
    cf_dictionary_create(env, key_callbacks, value_callbacks, &entries)
}

fn CFDictionaryCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    dict: CFDictionaryRef,
) -> CFDictionaryRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    msg![env; dict copy]
}

fn CFDictionaryCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
    dict: CFDictionaryRef,
) -> CFMutableDictionaryRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    assert_eq!(capacity, 0); // Nonzero capacity semantics are not implemented.
    msg![env; dict mutableCopy]
}

fn CFDictionaryCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
    key_callbacks: ConstPtr<CFDictionaryKeyCallBacks>,
    value_callbacks: ConstPtr<CFDictionaryValueCallBacks>,
) -> CFMutableDictionaryRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert_eq!(capacity, 0); // TODO: fixed capacity support

    let new = msg_class![env; _touchHLE_NSMutableDictionary_non_retaining alloc];
    msg![env; new initWithKeyCallbacks:key_callbacks andValueCallbacks:value_callbacks]
}

fn CFDictionaryAddValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key: ConstVoidPtr,
    value: ConstVoidPtr,
) {
    let key: id = key.cast().cast_mut();
    let res = super::super::foundation::ns_dictionary::cf_lookup(env, dict, key);
    log_dbg!(
        "CFDictionaryAddValue dict {:?} k {:?} v {:?}; res {:?}",
        dict,
        key,
        value,
        res
    );
    if res.is_none() {
        let value: id = value.cast().cast_mut();
        msg![env; dict setObject:value forKey:key]
    }
}

fn CFDictionarySetValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key: ConstVoidPtr,
    value: ConstVoidPtr,
) {
    log_dbg!("CFDictionarySetValue k {:?} v {:?}", key, value);
    let key: id = key.cast().cast_mut();
    let value: id = value.cast().cast_mut();
    msg![env; dict setObject:value forKey:key]
}

fn CFDictionaryReplaceValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key: ConstVoidPtr,
    value: ConstVoidPtr,
) {
    if super::super::foundation::ns_dictionary::cf_lookup(env, dict, key.cast_mut().cast())
        .is_some()
    {
        CFDictionarySetValue(env, dict, key, value);
    }
}

fn CFDictionaryRemoveValue(env: &mut Environment, dict: CFMutableDictionaryRef, key: ConstVoidPtr) {
    let key: id = key.cast().cast_mut();
    log_dbg!("CFDictionaryRemoveValue dict {:?} key {:?}", dict, key);
    () = msg![env; dict removeObjectForKey:key];
}

fn CFDictionaryRemoveAllValues(env: &mut Environment, dict: CFMutableDictionaryRef) {
    msg![env; dict removeAllObjects]
}

fn CFDictionaryGetValue(
    env: &mut Environment,
    dict: CFMutableDictionaryRef,
    key: ConstVoidPtr,
) -> ConstVoidPtr {
    let key: id = key.cast().cast_mut();
    let res: id = msg![env; dict objectForKey:key];
    res.cast().cast_const()
}

fn CFDictionaryGetValueIfPresent(
    env: &mut Environment,
    dict: CFDictionaryRef,
    key: ConstVoidPtr,
    output: MutPtr<ConstVoidPtr>,
) -> bool {
    let key: id = key.cast().cast_mut();
    let Some(value) = super::super::foundation::ns_dictionary::cf_lookup(env, dict, key) else {
        return false;
    };
    if !output.is_null() {
        env.mem.write(output, value.cast().cast_const());
    }
    true
}

fn CFDictionaryContainsKey(
    env: &mut Environment,
    dict: CFDictionaryRef,
    key: ConstVoidPtr,
) -> bool {
    CFDictionaryGetValueIfPresent(env, dict, key, MutPtr::null())
}

fn CFDictionaryContainsValue(
    env: &mut Environment,
    dict: CFDictionaryRef,
    value: ConstVoidPtr,
) -> bool {
    super::super::foundation::ns_dictionary::cf_count_values(
        env,
        dict,
        value.cast_mut().cast(),
        true,
    ) != 0
}

fn CFDictionaryGetCountOfKey(
    env: &mut Environment,
    dict: CFDictionaryRef,
    key: ConstVoidPtr,
) -> CFIndex {
    CFDictionaryContainsKey(env, dict, key) as CFIndex
}

fn CFDictionaryGetCountOfValue(
    env: &mut Environment,
    dict: CFDictionaryRef,
    value: ConstVoidPtr,
) -> CFIndex {
    super::super::foundation::ns_dictionary::cf_count_values(
        env,
        dict,
        value.cast_mut().cast(),
        false,
    )
}

fn CFDictionaryApplyFunction(
    env: &mut Environment,
    dict: CFDictionaryRef,
    function: GuestFunction,
    context: MutVoidPtr,
) {
    assert!(!function.to_ptr().is_null());
    let entries = if let Some(entries) = cf_dictionary_entries(env, dict) {
        entries
    } else {
        let keys: id = msg![env; dict allKeys];
        let count: NSUInteger = msg![env; keys count];
        (0..count)
            .map(|index| {
                let key: id = msg![env; keys objectAtIndex:index];
                let value: id = msg![env; dict objectForKey:key];
                (key, value)
            })
            .collect()
    };
    for (key, value) in entries {
        let _: () = function.call_from_host(env, (key, value, context));
    }
}

fn CFDictionaryGetCount(env: &mut Environment, dict: CFDictionaryRef) -> CFIndex {
    let count: NSUInteger = msg![env; dict count];
    log_dbg!("CFDictionaryGetCount dict {:?} {}", dict, count);
    count.try_into().unwrap()
}

fn CFDictionaryGetKeysAndValues(
    env: &mut Environment,
    dict: CFDictionaryRef,
    keys: ConstPtr<MutVoidPtr>,
    values: ConstPtr<MutVoidPtr>,
) {
    let mut key_ptr = keys.cast_mut();
    let mut val_ptr = values.cast_mut();
    // TODO: use keyEnumerator
    let keys_arr: id = msg![env; dict allKeys];
    let enumerator: id = msg![env; keys_arr objectEnumerator];
    let mut key: id;
    let mut val: id;
    loop {
        key = msg![env; enumerator nextObject];
        if key == nil {
            break;
        }
        if !key_ptr.is_null() {
            env.mem.write(key_ptr, key.cast());
            key_ptr += 1;
        }
        if !val_ptr.is_null() {
            val = msg![env; dict objectForKey:key];
            log_dbg!(
                "CFDictionaryGetKeysAndValues dict {:?} key {:?} val {:?}",
                dict,
                key,
                val
            );
            env.mem.write(val_ptr, val.cast());
            val_ptr += 1;
        }
    }
}

// Default CFDictionary callbacks
fn _touchHLE_CFDictionary_retain(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    value: ConstVoidPtr,
) -> ConstVoidPtr {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    CFRetain(env, value.cast_mut().cast()).cast_const().cast()
}
fn _touchHLE_CFDictionary_release(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    value: ConstVoidPtr,
) {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    CFRelease(env, value.cast_mut().cast());
}
fn _touchHLE_CFDictionary_copyDescription(
    env: &mut Environment,
    value: ConstVoidPtr,
) -> CFStringRef {
    super::cf_type::CFCopyDescription(env, value.cast_mut().cast())
}
fn _touchHLE_CFDictionary_equal(
    env: &mut Environment,
    value1: ConstVoidPtr,
    value2: ConstVoidPtr,
) -> bool {
    CFEqual(env, value1.cast_mut().cast(), value2.cast_mut().cast())
}
fn _touchHLE_CFDictionary_hash(env: &mut Environment, value: ConstVoidPtr) -> CFHashCode {
    CFHash(env, value.cast_mut().cast())
}

pub(super) struct DefaultCallbackFunctions {
    pub(super) retain: GuestFunction,
    pub(super) release: GuestFunction,
    pub(super) copy_desc: GuestFunction,
    pub(super) equal: GuestFunction,
    hash: GuestFunction,
}
pub(super) fn create_default_callback_functions(
    mem: &mut Mem,
    dyld: &mut Dyld,
) -> DefaultCallbackFunctions {
    let retain_sym = "__touchHLE_CFDictionary_retain";
    let retain_hf: HostFunction =
        &(_touchHLE_CFDictionary_retain as fn(&mut Environment, _, _) -> _);
    let retain_gf = dyld.create_guest_function(mem, retain_sym, retain_hf);

    let release_sym = "__touchHLE_CFDictionary_release";
    let release_hf: HostFunction = &(_touchHLE_CFDictionary_release as fn(&mut Environment, _, _));
    let release_gf = dyld.create_guest_function(mem, release_sym, release_hf);

    let copy_desc_sym = "__touchHLE_CFDictionary_copyDescription";
    let copy_desc_hf: HostFunction =
        &(_touchHLE_CFDictionary_copyDescription as fn(&mut Environment, _) -> _);
    let copy_desc_gf = dyld.create_guest_function(mem, copy_desc_sym, copy_desc_hf);

    let equal_sym = "__touchHLE_CFDictionary_equal";
    let equal_hf: HostFunction = &(_touchHLE_CFDictionary_equal as fn(&mut Environment, _, _) -> _);
    let equal_gf = dyld.create_guest_function(mem, equal_sym, equal_hf);

    let hash_sym = "__touchHLE_CFDictionary_hash";
    let hash_hf: HostFunction = &(_touchHLE_CFDictionary_hash as fn(&mut Environment, _) -> _);
    let hash_gf = dyld.create_guest_function(mem, hash_sym, hash_hf);

    DefaultCallbackFunctions {
        retain: retain_gf,
        release: release_gf,
        copy_desc: copy_desc_gf,
        equal: equal_gf,
        hash: hash_gf,
    }
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFTypeDictionaryKeyCallBacks",
        HostConstant::Custom(|env| {
            let common = create_default_callback_functions(&mut env.mem, &mut env.dyld);
            let callbacks = CFDictionaryKeyCallBacks {
                version: 0, // always 0
                retain: common.retain,
                release: common.release,
                copy_desc: common.copy_desc,
                equal: common.equal,
                hash: common.hash,
            };
            env.mem.alloc_and_write(callbacks).cast_void().cast_const()
        }),
    ),
    (
        "_kCFTypeDictionaryValueCallBacks",
        HostConstant::Custom(|env| {
            // All the functions here (except `hash` one)
            // are the same as for `kCFTypeDictionaryKeyCallBacks`,
            // but we still re-create guest functions for the sake
            // of the (current) code simplicity
            // TODO: create related guest functions only once, not twice
            let common = create_default_callback_functions(&mut env.mem, &mut env.dyld);
            let callbacks = CFDictionaryValueCallBacks {
                version: 0, // always 0
                retain: common.retain,
                release: common.release,
                copy_desc: common.copy_desc,
                equal: common.equal,
            };
            env.mem.alloc_and_write(callbacks).cast_void().cast_const()
        }),
    ),
];

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFDictionaryCreate(_, _, _, _, _, _)),
    export_c_func!(CFDictionaryCreateCopy(_, _)),
    export_c_func!(CFDictionaryCreateMutableCopy(_, _, _)),
    export_c_func!(CFDictionaryCreateMutable(_, _, _, _)),
    export_c_func!(CFDictionaryAddValue(_, _, _)),
    export_c_func!(CFDictionarySetValue(_, _, _)),
    export_c_func!(CFDictionaryReplaceValue(_, _, _)),
    export_c_func!(CFDictionaryRemoveValue(_, _)),
    export_c_func!(CFDictionaryRemoveAllValues(_)),
    export_c_func!(CFDictionaryGetValue(_, _)),
    export_c_func!(CFDictionaryGetValueIfPresent(_, _, _)),
    export_c_func!(CFDictionaryContainsKey(_, _)),
    export_c_func!(CFDictionaryContainsValue(_, _)),
    export_c_func!(CFDictionaryGetCountOfKey(_, _)),
    export_c_func!(CFDictionaryGetCountOfValue(_, _)),
    export_c_func!(CFDictionaryApplyFunction(_, _, _)),
    export_c_func!(CFDictionaryGetCount(_)),
    export_c_func!(CFDictionaryGetKeysAndValues(_, _, _)),
];
