/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSIndexPath`, including UIKit's row/section and item/section additions.

use crate::frameworks::foundation::{NSInteger, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, objc_classes, Class, ClassExports, HostObject, NSZonePtr,
};

struct NSIndexPathHostObject {
    section: NSInteger,
    row: NSInteger,
}
impl HostObject for NSIndexPathHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSIndexPath: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSIndexPathHostObject { section: 0, row: 0 });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)indexPathForRow:(NSInteger)row inSection:(NSInteger)section {
    let new: id = msg![env; this alloc];
    *env.objc.borrow_mut::<NSIndexPathHostObject>(new) = NSIndexPathHostObject { section, row };
    autorelease(env, new)
}
+ (id)indexPathForItem:(NSInteger)item inSection:(NSInteger)section {
    msg![env; this indexPathForRow:item inSection:section]
}

- (NSInteger)section {
    env.objc.borrow::<NSIndexPathHostObject>(this).section
}
- (NSInteger)row {
    env.objc.borrow::<NSIndexPathHostObject>(this).row
}
- (NSInteger)item {
    env.objc.borrow::<NSIndexPathHostObject>(this).row
}
- (NSUInteger)length {
    2
}

- (bool)isEqual:(id)other {
    let class: Class = msg_class![env; NSIndexPath class];
    let is_index_path: bool = msg![env; other isKindOfClass:class];
    if !is_index_path {
        return false;
    }
    let &NSIndexPathHostObject { section, row } = env.objc.borrow(this);
    let other = env.objc.borrow::<NSIndexPathHostObject>(other);
    section == other.section && row == other.row
}
- (NSUInteger)hash {
    let &NSIndexPathHostObject { section, row } = env.objc.borrow(this);
    ((section as NSUInteger) << 16) ^ (row as NSUInteger)
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    crate::objc::retain(env, this)
}

@end

};
