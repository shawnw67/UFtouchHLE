/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFData` and `CFMutableData`.
//!
//! These are toll-free bridged to `NSData` and `NSMutableData` in Apple's
//! implementation. Here they are the same types.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFIndex, CFRange};
use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::frameworks::foundation::{NSRange, NSUInteger};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{id, msg, msg_class};
use crate::Environment;

pub type CFDataRef = super::CFTypeRef;
pub type CFMutableDataRef = super::CFTypeRef;

pub fn CFDataCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
) -> CFDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let bytes: ConstVoidPtr = bytes.cast();
    let length: NSUInteger = length.try_into().unwrap();
    let new: id = msg_class![env; NSData alloc];
    msg![env; new initWithBytes:bytes length:length]
}

fn CFDataCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    data: CFDataRef,
) -> CFDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    msg![env; data copy]
}

fn CFDataCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
) -> CFMutableDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    assert_eq!(capacity, 0); // Nonzero capacity semantics are not implemented.
    let data: id = msg_class![env; NSMutableData alloc];
    msg![env; data initWithCapacity:0u32]
}

fn CFDataCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    capacity: CFIndex,
    data: CFDataRef,
) -> CFMutableDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    assert_eq!(capacity, 0); // Nonzero capacity semantics are not implemented.
    msg![env; data mutableCopy]
}

fn CFDataGetMutableBytePtr(env: &mut Environment, data: CFMutableDataRef) -> MutPtr<u8> {
    let ptr: MutVoidPtr = msg![env; data mutableBytes];
    ptr.cast()
}

fn CFDataSetLength(env: &mut Environment, data: CFMutableDataRef, length: CFIndex) {
    let length: NSUInteger = length.try_into().unwrap();
    msg![env; data setLength:length]
}

fn CFDataIncreaseLength(env: &mut Environment, data: CFMutableDataRef, extra_length: CFIndex) {
    let extra_length: NSUInteger = extra_length.try_into().unwrap();
    msg![env; data increaseLengthBy:extra_length]
}

fn CFDataAppendBytes(
    env: &mut Environment,
    data: CFMutableDataRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
) {
    let length: NSUInteger = length.try_into().unwrap();
    msg![env; data appendBytes:bytes length:length]
}

fn CFDataReplaceBytes(
    env: &mut Environment,
    data: CFMutableDataRef,
    range: CFRange,
    replacement: ConstPtr<u8>,
    replacement_length: CFIndex,
) {
    let start: NSUInteger = range.location.try_into().unwrap();
    let length: NSUInteger = range.length.try_into().unwrap();
    let replacement_length: NSUInteger = replacement_length.try_into().unwrap();
    let old_length: NSUInteger = msg![env; data length];
    assert!(start <= old_length && length <= old_length - start);
    let new_length = (old_length - length)
        .checked_add(replacement_length)
        .expect("CFData length overflow");
    let new_length_cf: CFIndex = new_length
        .try_into()
        .expect("CFData length exceeds CFIndex");
    let old_bytes: ConstVoidPtr = msg![env; data bytes];
    let source_start = u64::from(replacement.to_bits());
    let source_end = source_start + u64::from(replacement_length);
    let old_start = u64::from(old_bytes.to_bits());
    let old_end = old_start + u64::from(old_length);
    let overlap = replacement_length != 0 && source_start < old_end && old_start < source_end;
    let saved = overlap.then(|| env.mem.bytes_at(replacement, replacement_length).to_vec());
    if new_length > old_length {
        CFDataSetLength(env, data, new_length_cf);
    }
    let mut bytes = CFDataGetMutableBytePtr(env, data);
    let tail = old_length - start - length;
    if tail != 0 {
        env.mem.memmove(
            (bytes + start + replacement_length).cast_void(),
            (bytes + start + length).cast_const().cast_void(),
            tail,
        );
    }
    if new_length < old_length {
        CFDataSetLength(env, data, new_length_cf);
        bytes = CFDataGetMutableBytePtr(env, data);
    }
    if replacement_length != 0 {
        if let Some(saved) = saved {
            env.mem
                .bytes_at_mut(bytes + start, replacement_length)
                .copy_from_slice(&saved);
        } else {
            env.mem.memmove(
                (bytes + start).cast_void(),
                replacement.cast_void(),
                replacement_length,
            );
        }
    }
}

fn CFDataDeleteBytes(env: &mut Environment, data: CFMutableDataRef, range: CFRange) {
    CFDataReplaceBytes(env, data, range, ConstPtr::null(), 0);
}

fn CFDataCreateWithBytesNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
    deallocator: CFAllocatorRef,
) -> CFDataRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let free_when_done = if deallocator == kCFAllocatorDefault {
        true
    } else {
        let deallocator = env.mem.read(deallocator);
        assert!(deallocator.is_null() || deallocator.is_system_default());
        !deallocator.is_null()
    };
    let bytes: MutVoidPtr = bytes.cast().cast_mut();
    let length: NSUInteger = length.try_into().unwrap();
    let new: id = msg_class![env; NSData alloc];
    msg![env; new initWithBytesNoCopy:bytes length:length freeWhenDone:free_when_done]
}

pub fn CFDataGetLength(env: &mut Environment, data: CFDataRef) -> CFIndex {
    let len: NSUInteger = msg![env; data length];
    len.try_into().unwrap()
}

pub fn CFDataGetBytePtr(env: &mut Environment, data: CFDataRef) -> ConstPtr<u8> {
    let ptr: ConstVoidPtr = msg![env; data bytes];
    ptr.cast()
}

fn CFDataGetBytes(env: &mut Environment, data: CFDataRef, range: CFRange, buffer: MutPtr<u8>) {
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length: range.length.try_into().unwrap(),
    };
    msg![env; data getBytes:buffer range:range]
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFDataCreate(_, _, _)),
    export_c_func!(CFDataCreateCopy(_, _)),
    export_c_func!(CFDataCreateMutable(_, _)),
    export_c_func!(CFDataCreateMutableCopy(_, _, _)),
    export_c_func!(CFDataAppendBytes(_, _, _)),
    export_c_func!(CFDataReplaceBytes(_, _, _, _)),
    export_c_func!(CFDataDeleteBytes(_, _)),
    export_c_func!(CFDataCreateWithBytesNoCopy(_, _, _, _)),
    export_c_func!(CFDataGetLength(_)),
    export_c_func!(CFDataGetBytePtr(_)),
    export_c_func!(CFDataGetMutableBytePtr(_)),
    export_c_func!(CFDataGetBytes(_, _, _)),
    export_c_func!(CFDataSetLength(_, _)),
    export_c_func!(CFDataIncreaseLength(_, _)),
];
