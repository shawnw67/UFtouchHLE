/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFArray` and `CFMutableArray`.
//!
//! These are toll-free bridged to `NSArray` and `NSMutableArray` in Apple's
//! implementation. Here they are the same types.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{kCFNotFound, CFIndex, CFRange};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_array::{
    cf_array_equal, cf_array_new_immutable, cf_array_replace, CFArrayCallBacks,
};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class, release, retain, Class};
use crate::Environment;

#[allow(dead_code)]
pub type CFArrayRef = super::CFTypeRef;
pub type CFMutableArrayRef = super::CFTypeRef;

pub const CONSTANTS: ConstantExports = &[(
    "_kCFTypeArrayCallBacks",
    HostConstant::Custom(|env| {
        let common =
            super::cf_dictionary::create_default_callback_functions(&mut env.mem, &mut env.dyld);
        env.mem
            .alloc_and_write(CFArrayCallBacks {
                version: 0,
                retain: common.retain,
                release: common.release,
                copy_description: common.copy_desc,
                equal: common.equal,
            })
            .cast_void()
            .cast_const()
    }),
)];

fn CFArrayCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    values: ConstPtr<ConstVoidPtr>,
    count: CFIndex,
    callbacks: ConstPtr<CFArrayCallBacks>,
) -> CFArrayRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let count: u32 = count.try_into().unwrap();
    let callbacks = if callbacks.is_null() {
        None
    } else {
        Some(env.mem.read(callbacks))
    };
    let values = (0..count)
        .map(|i| env.mem.read(values + i).cast_mut().cast())
        .collect();
    cf_array_new_immutable(env, callbacks, values)
}

fn CFArrayCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
    callbacks: ConstPtr<CFArrayCallBacks>,
) -> CFMutableArrayRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert!(capacity == 0); // TODO: fixed capacity support
    if callbacks.is_null() {
        msg_class![env; _touchHLE_NSMutableArray_non_retaining new]
    } else {
        let array: id = msg_class![env; _touchHLE_NSMutableArray_cf_callbacks alloc];
        msg![env; array initWithCallbacks:callbacks]
    }
}

fn CFArrayGetCount(env: &mut Environment, array: CFArrayRef) -> CFIndex {
    let count: NSUInteger = msg![env; array count];
    count.try_into().unwrap()
}

fn CFArrayCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    array: CFArrayRef,
) -> CFArrayRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    msg![env; array copy]
}

fn CFArrayCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
    array: CFArrayRef,
) -> CFMutableArrayRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    assert_eq!(capacity, 0); // Nonzero capacity semantics are not implemented.
    msg![env; array mutableCopy]
}

fn CFArrayGetValueAtIndex(env: &mut Environment, array: CFArrayRef, idx: CFIndex) -> ConstVoidPtr {
    let idx: NSUInteger = idx.try_into().unwrap();
    let value: id = msg![env; array objectAtIndex:idx];
    value.cast().cast_const()
}

fn CFArrayGetValues(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    values: MutPtr<ConstVoidPtr>,
) {
    let start: NSUInteger = range.location.try_into().unwrap();
    let len: NSUInteger = range.length.try_into().unwrap();
    let count: NSUInteger = msg![env; array count];
    assert!(start <= count && len <= count - start);
    for offset in 0..len {
        let value: id = msg![env; array objectAtIndex:(start + offset)];
        env.mem.write(values + offset, value.cast().cast_const());
    }
}

fn CFArrayApplyFunction(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    function: GuestFunction,
    context: MutVoidPtr,
) {
    let start: NSUInteger = range.location.try_into().unwrap();
    let len: NSUInteger = range.length.try_into().unwrap();
    let count: NSUInteger = msg![env; array count];
    assert!(start <= count && len <= count - start);
    assert!(!function.to_ptr().is_null());
    for index in start..start + len {
        let value: id = msg![env; array objectAtIndex:index];
        let _: () = function.call_from_host(env, (value.cast_const().cast_void(), context));
    }
}

fn CFArrayBSearchValues(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    value: ConstVoidPtr,
    comparator: GuestFunction,
    context: MutVoidPtr,
) -> CFIndex {
    let start: NSUInteger = range.location.try_into().unwrap();
    let len: NSUInteger = range.length.try_into().unwrap();
    let count: NSUInteger = msg![env; array count];
    assert!(start <= count && len <= count - start);
    assert!(!comparator.to_ptr().is_null());
    let mut low = start;
    let mut high = start + len;
    while low < high {
        let mid = low + (high - low) / 2;
        let candidate: id = msg![env; array objectAtIndex:mid];
        let order: CFIndex =
            comparator.call_from_host(env, (value, candidate.cast_const().cast_void(), context));
        if order < 0 {
            high = mid;
        } else if order > 0 {
            low = mid + 1;
        } else {
            return mid.try_into().unwrap();
        }
    }
    low.try_into().unwrap()
}

fn find_values(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    value: ConstVoidPtr,
) -> (CFIndex, CFIndex, CFIndex) {
    let start: NSUInteger = range.location.try_into().unwrap();
    let len: NSUInteger = range.length.try_into().unwrap();
    let count: NSUInteger = msg![env; array count];
    assert!(start <= count && len <= count - start);
    let immutable: Class = msg_class![env; _touchHLE_NSArray_non_retaining class];
    let mutable: Class = msg_class![env; _touchHLE_NSMutableArray_non_retaining class];
    let non_retaining: bool = msg![env; array isKindOfClass:immutable];
    let non_retaining = non_retaining || msg![env; array isKindOfClass:mutable];
    let needle: id = value.cast().cast_mut();
    let (mut first, mut last, mut matches) = (kCFNotFound, kCFNotFound, 0);
    for index in start..start + len {
        let item: id = msg![env; array objectAtIndex:index];
        let equal = match cf_array_equal(env, array, item, needle) {
            Some(equal) => equal,
            None => item == needle || (!non_retaining && msg![env; item isEqual:needle]),
        };
        if equal {
            let index = index.try_into().unwrap();
            if first == kCFNotFound {
                first = index;
            }
            last = index;
            matches += 1;
        }
    }
    (first, last, matches)
}

fn CFArrayContainsValue(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    value: ConstVoidPtr,
) -> bool {
    find_values(env, array, range, value).0 != kCFNotFound
}

fn CFArrayGetFirstIndexOfValue(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    value: ConstVoidPtr,
) -> CFIndex {
    find_values(env, array, range, value).0
}

fn CFArrayGetLastIndexOfValue(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    value: ConstVoidPtr,
) -> CFIndex {
    find_values(env, array, range, value).1
}

fn CFArrayGetCountOfValue(
    env: &mut Environment,
    array: CFArrayRef,
    range: CFRange,
    value: ConstVoidPtr,
) -> CFIndex {
    find_values(env, array, range, value).2
}

fn CFArrayAppendValue(env: &mut Environment, array: CFMutableArrayRef, value: ConstVoidPtr) {
    let value: id = value.cast().cast_mut();
    msg![env; array addObject:value]
}

fn CFArrayAppendArray(
    env: &mut Environment,
    array: CFMutableArrayRef,
    other: CFArrayRef,
    range: CFRange,
) {
    let start: NSUInteger = range.location.try_into().unwrap();
    let len: NSUInteger = range.length.try_into().unwrap();
    let count: NSUInteger = msg![env; other count];
    assert!(start <= count && len <= count - start);
    // Snapshot the source range before appending, even for self-append.
    let values: Vec<id> = (start..start + len)
        .map(|index| msg![env; other objectAtIndex:index])
        .collect();
    for value in values {
        let _: () = msg![env; array addObject:value];
    }
}

fn CFArrayReplaceValues(
    env: &mut Environment,
    array: CFMutableArrayRef,
    range: CFRange,
    new_values: ConstPtr<ConstVoidPtr>,
    new_count: CFIndex,
) {
    let start: NSUInteger = range.location.try_into().unwrap();
    let len: NSUInteger = range.length.try_into().unwrap();
    let count: NSUInteger = msg![env; array count];
    assert!(start <= count && len <= count - start);
    let new_count: u32 = new_count.try_into().unwrap();
    let values: Vec<id> = (0..new_count)
        .map(|i| env.mem.read(new_values + i).cast_mut().cast())
        .collect();
    if cf_array_replace(env, array, start as usize, len as usize, &values) {
        return;
    }
    // Keep bridged Objective-C objects alive while removing the old range.
    for &value in &values {
        retain(env, value);
    }
    for index in (start..start + len).rev() {
        let _: () = msg![env; array removeObjectAtIndex:index];
    }
    for (offset, &value) in values.iter().enumerate() {
        let index = start + u32::try_from(offset).unwrap();
        let _: () = msg![env; array insertObject:value atIndex:index];
        release(env, value);
    }
}

fn CFArrayInsertValueAtIndex(
    env: &mut Environment,
    array: CFMutableArrayRef,
    idx: CFIndex,
    value: ConstVoidPtr,
) {
    let idx: NSUInteger = idx.try_into().unwrap();
    let value: id = value.cast().cast_mut();
    msg![env; array insertObject:value atIndex:idx]
}

fn CFArraySetValueAtIndex(
    env: &mut Environment,
    array: CFMutableArrayRef,
    idx: CFIndex,
    value: ConstVoidPtr,
) {
    let idx: NSUInteger = idx.try_into().unwrap();
    let count: NSUInteger = msg![env; array count];
    let value: id = value.cast().cast_mut();
    if idx == count {
        msg![env; array addObject:value]
    } else {
        msg![env; array replaceObjectAtIndex:idx withObject:value]
    }
}

fn CFArrayRemoveAllValues(env: &mut Environment, array: CFMutableArrayRef) {
    msg![env; array removeAllObjects]
}

fn CFArrayExchangeValuesAtIndices(
    env: &mut Environment,
    array: CFMutableArrayRef,
    first: CFIndex,
    second: CFIndex,
) {
    let first: NSUInteger = first.try_into().unwrap();
    let second: NSUInteger = second.try_into().unwrap();
    msg![env; array exchangeObjectAtIndex:first withObjectAtIndex:second]
}

fn CFArraySortValues(
    env: &mut Environment,
    array: CFMutableArrayRef,
    range: CFRange,
    comparator: GuestFunction,
    context: MutVoidPtr,
) {
    let start: NSUInteger = range.location.try_into().unwrap();
    let len: NSUInteger = range.length.try_into().unwrap();
    let count: NSUInteger = msg![env; array count];
    assert!(start <= count && len <= count - start);
    assert!(!comparator.to_ptr().is_null());
    let mut sorted: Vec<(usize, id)> = (0..len)
        .map(|offset| {
            (
                offset as usize,
                msg![env; array objectAtIndex:(start + offset)],
            )
        })
        .collect();
    sorted.sort_by(|(_, left), (_, right)| {
        let order: CFIndex = comparator.call_from_host(
            env,
            (
                left.cast_const().cast_void(),
                right.cast_const().cast_void(),
                context,
            ),
        );
        order.cmp(&0)
    });
    // Use swaps to move elements without changing ownership.
    let mut positions: Vec<usize> = (0..sorted.len()).collect();
    let mut originals = positions.clone();
    for (target, (original, _)) in sorted.iter().enumerate() {
        let source = positions[*original];
        if source != target {
            let first = start + target as NSUInteger;
            let second = start + source as NSUInteger;
            let _: () = msg![env; array exchangeObjectAtIndex:first withObjectAtIndex:second];
            originals.swap(target, source);
            positions[originals[target]] = target;
            positions[originals[source]] = source;
        }
    }
}

fn CFArrayRemoveValueAtIndex(env: &mut Environment, array: CFMutableArrayRef, idx: CFIndex) {
    let idx: NSUInteger = idx.try_into().unwrap();
    msg![env; array removeObjectAtIndex:idx]
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFArrayCreate(_, _, _, _)),
    export_c_func!(CFArrayCreateMutable(_, _, _)),
    export_c_func!(CFArrayCreateCopy(_, _)),
    export_c_func!(CFArrayCreateMutableCopy(_, _, _)),
    export_c_func!(CFArrayGetCount(_)),
    export_c_func!(CFArrayGetValueAtIndex(_, _)),
    export_c_func!(CFArrayGetValues(_, _, _)),
    export_c_func!(CFArrayApplyFunction(_, _, _, _)),
    export_c_func!(CFArrayBSearchValues(_, _, _, _, _)),
    export_c_func!(CFArrayContainsValue(_, _, _)),
    export_c_func!(CFArrayGetFirstIndexOfValue(_, _, _)),
    export_c_func!(CFArrayGetLastIndexOfValue(_, _, _)),
    export_c_func!(CFArrayGetCountOfValue(_, _, _)),
    export_c_func!(CFArrayAppendValue(_, _)),
    export_c_func!(CFArrayAppendArray(_, _, _)),
    export_c_func!(CFArrayReplaceValues(_, _, _, _)),
    export_c_func!(CFArrayInsertValueAtIndex(_, _, _)),
    export_c_func!(CFArraySetValueAtIndex(_, _, _)),
    export_c_func!(CFArrayRemoveAllValues(_)),
    export_c_func!(CFArrayExchangeValuesAtIndices(_, _, _)),
    export_c_func!(CFArraySortValues(_, _, _, _)),
    export_c_func!(CFArrayRemoveValueAtIndex(_, _)),
];
