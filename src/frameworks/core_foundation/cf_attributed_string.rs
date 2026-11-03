/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Core Foundation attributed strings, backed by NSAttributedString.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_dictionary::CFDictionaryRef;
use super::cf_string::CFStringRef;
use super::{CFIndex, CFRange, CFTypeRef};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::MutPtr;
use crate::objc::{id, msg, msg_class, nil};
use crate::Environment;

type CFAttributedStringRef = CFTypeRef;

fn CFAttributedStringCreate(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFStringRef,
    attributes: CFDictionaryRef,
) -> CFAttributedStringRef {
    if string.is_null()
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return nil;
    }
    let result: id = msg_class![env; NSAttributedString alloc];
    msg![env; result initWithString:string attributes:attributes]
}

fn CFAttributedStringCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFAttributedStringRef,
) -> CFAttributedStringRef {
    if string.is_null()
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return nil;
    }
    msg![env; string copy]
}

fn CFAttributedStringGetLength(env: &mut Environment, string: CFAttributedStringRef) -> CFIndex {
    if string.is_null() {
        return 0;
    }
    let length: NSUInteger = msg![env; string length];
    length.try_into().unwrap_or(CFIndex::MAX)
}

fn CFAttributedStringGetString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CFStringRef {
    if string.is_null() {
        return nil;
    }
    msg![env; string string]
}

fn CFAttributedStringGetAttributes(
    env: &mut Environment,
    string: CFAttributedStringRef,
    location: CFIndex,
    effective_range: MutPtr<CFRange>,
) -> CFDictionaryRef {
    let length = CFAttributedStringGetLength(env, string);
    if location < 0 || location >= length {
        return nil;
    }
    if !effective_range.is_null() {
        env.mem.write(
            effective_range,
            CFRange {
                location: 0,
                length,
            },
        );
    }
    let index = location as NSUInteger;
    let null_range: MutPtr<crate::frameworks::foundation::NSRange> = MutPtr::null();
    msg![env; string attributesAtIndex:index effectiveRange:null_range]
}

fn CFAttributedStringGetAttribute(
    env: &mut Environment,
    string: CFAttributedStringRef,
    location: CFIndex,
    name: CFStringRef,
    effective_range: MutPtr<CFRange>,
) -> CFTypeRef {
    let attributes = CFAttributedStringGetAttributes(env, string, location, effective_range);
    if attributes.is_null() || name.is_null() {
        return nil;
    }
    msg![env; attributes objectForKey:name]
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFAttributedStringCreate(_, _, _)),
    export_c_func!(CFAttributedStringCreateCopy(_, _)),
    export_c_func!(CFAttributedStringGetLength(_)),
    export_c_func!(CFAttributedStringGetString(_)),
    export_c_func!(CFAttributedStringGetAttributes(_, _, _)),
    export_c_func!(CFAttributedStringGetAttribute(_, _, _, _)),
];
