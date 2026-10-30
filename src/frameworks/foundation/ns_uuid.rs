/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! NSUUID's immutable value operations. Adapted from HyperHLE-Reborn
//! ff8eb827, using UF's existing uuid crate instead of a time-seeded PRNG.
//! NSCoding needs archive fixtures before its layout can be implemented.

use super::ns_string;
use crate::mem::{ConstPtr, MutPtr};
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};
use uuid::Uuid;

#[derive(Default)]
struct NSUUIDHostObject {
    uuid: Uuid,
}
impl HostObject for NSUUIDHostObject {}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation NSUUID: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<NSUUIDHostObject>::default(), &mut env.mem)
}
+ (id)UUID {
    let value: id = msg![env; this alloc];
    let value: id = msg![env; value init];
    autorelease(env, value)
}
- (id)init {
    env.objc.borrow_mut::<NSUUIDHostObject>(this).uuid = Uuid::new_v4();
    this
}
- (id)initWithUUIDString:(id)string {
    let parsed = if string == nil {
        None
    } else {
        Uuid::parse_str(&ns_string::to_rust_string(env, string)).ok()
    };
    if let Some(uuid) = parsed {
        env.objc.borrow_mut::<NSUUIDHostObject>(this).uuid = uuid;
        this
    } else {
        release(env, this);
        nil
    }
}
- (id)initWithUUIDBytes:(ConstPtr<u8>)bytes {
    let bytes = env.mem.bytes_at(bytes, 16).try_into().unwrap();
    env.objc.borrow_mut::<NSUUIDHostObject>(this).uuid = Uuid::from_bytes(bytes);
    this
}
- (())getUUIDBytes:(MutPtr<u8>)bytes {
    let uuid = env.objc.borrow::<NSUUIDHostObject>(this).uuid;
    env.mem.bytes_at_mut(bytes, 16).copy_from_slice(uuid.as_bytes());
}
- (id)UUIDString {
    let uuid = env.objc.borrow::<NSUUIDHostObject>(this).uuid;
    let string = ns_string::from_rust_string(env, uuid.hyphenated().to_string().to_uppercase());
    autorelease(env, string)
}
- (bool)isEqual:(id)other {
    if other == nil { return false; }
    let class = env.objc.get_known_class("NSUUID", &mut env.mem);
    let compatible: bool = msg![env; other isKindOfClass:class];
    compatible && env.objc.borrow::<NSUUIDHostObject>(this).uuid
        == env.objc.borrow::<NSUUIDHostObject>(other).uuid
}
- (u32)hash {
    let uuid = env.objc.borrow::<NSUUIDHostObject>(this).uuid;
    uuid.as_bytes().iter().fold(2166136261u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(16777619)
    })
}
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this);
    this
}
- (id)description {
    msg![env; this UUIDString]
}
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end
};
