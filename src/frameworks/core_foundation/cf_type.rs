/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFType` (type-generic functions etc).

use super::{CFHashCode, CFIndex};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::NSUInteger;
use crate::objc::Class;
use crate::{msg, objc};
use crate::{msg_class, Environment};

pub type CFTypeRef = objc::id;

pub fn CFRetain(env: &mut Environment, object: CFTypeRef) -> CFTypeRef {
    assert!(!object.is_null()); // not allowed, unlike for normal objc objects
    objc::retain(env, object)
}
pub fn CFRelease(env: &mut Environment, object: CFTypeRef) {
    objc::release(env, object);
}

pub fn CFGetRetainCount(env: &mut Environment, object: CFTypeRef) -> CFIndex {
    let count: NSUInteger = msg![env; object retainCount];
    count as CFIndex
}

pub fn CFEqual(env: &mut Environment, object1: CFTypeRef, object2: CFTypeRef) -> bool {
    if object1 == object2 {
        return true;
    }
    if let Some(equal) = super::cf_uuid::equal(env, object1, object2) {
        return equal;
    }
    let number_class: Class = msg_class![env; NSNumber class];
    let number1: bool = msg![env; object1 isKindOfClass:number_class];
    let number2: bool = msg![env; object2 isKindOfClass:number_class];
    if number1 || number2 {
        return number1 && number2 && msg![env; object1 isEqual:object2];
    }
    let data_class: Class = msg_class![env; NSData class];
    let data1: bool = msg![env; object1 isKindOfClass:data_class];
    let data2: bool = msg![env; object2 isKindOfClass:data_class];
    if data1 || data2 {
        return data1 && data2 && msg![env; object1 isEqual:object2];
    }
    // TODO: other classes
    let str_class: Class = msg_class![env; NSString class];
    let object1_class: Class = msg![env; object1 class];
    assert!(msg![env; object1_class isKindOfClass:str_class]);
    let object2_class: Class = msg![env; object2 class];
    assert!(msg![env; object2_class isKindOfClass:str_class]);
    // TODO: use isEqual: once it is fixed
    msg![env; object1 isEqualToString:object2]
}

pub fn CFHash(env: &mut Environment, object: CFTypeRef) -> CFHashCode {
    msg![env; object hash]
}

pub(super) fn CFCopyDescription(env: &mut Environment, object: CFTypeRef) -> CFTypeRef {
    let description: CFTypeRef = msg![env; object description];
    if description.is_null() {
        return description;
    }
    objc::retain(env, description)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFRetain(_)),
    export_c_func!(CFRelease(_)),
    export_c_func!(CFGetRetainCount(_)),
    export_c_func!(CFEqual(_, _)),
    export_c_func!(CFHash(_)),
    export_c_func!(CFCopyDescription(_)),
];
