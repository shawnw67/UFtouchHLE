/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSAttributedString` and `NSMutableAttributedString`.
//!
//! Attributes are currently stored for the whole string rather than in runs.

use super::{ns_dictionary, ns_string, NSRange, NSUInteger};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};

struct NSAttributedStringHostObject {
    /// `NSString*`, an immutable copy.
    string: id,
    /// `UIFont*` of the whole string, if one was set.
    font: id,
    /// Immutable dictionary for the whole string, or nil when empty.
    attributes: id,
}

fn set_attributes(env: &mut crate::Environment, this: id, attributes: id) {
    let copy: id = msg![env; attributes copy];
    let old = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<NSAttributedStringHostObject>(this)
            .attributes,
        copy,
    );
    release(env, old);
}

fn mutable_attributes(env: &mut crate::Environment, this: id) -> id {
    let attributes = env
        .objc
        .borrow::<NSAttributedStringHostObject>(this)
        .attributes;
    if attributes == nil {
        msg_class![env; NSMutableDictionary new]
    } else {
        msg![env; attributes mutableCopy]
    }
}

fn set_font(env: &mut crate::Environment, this: id, font: id) {
    retain(env, font);
    let old = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<NSAttributedStringHostObject>(this)
            .font,
        font,
    );
    release(env, old);
}

fn set_attribute(env: &mut crate::Environment, this: id, name: id, value: id) {
    if name != nil && ns_string::to_rust_string(env, name) == "NSFont" {
        set_font(env, this, value);
    }
}
impl HostObject for NSAttributedStringHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSAttributedString: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSAttributedStringHostObject { string: nil, font: nil, attributes: nil });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    let empty: id = msg_class![env; NSString new];
    env.objc.borrow_mut::<NSAttributedStringHostObject>(this).string = empty;
    this
}
- (id)initWithString:(id)string { // NSString*
    let string: id = msg![env; string copy];
    env.objc.borrow_mut::<NSAttributedStringHostObject>(this).string = string;
    this
}
- (id)initWithString:(id)string // NSString*
          attributes:(id)attributes { // NSDictionary*
    let this: id = msg![env; this initWithString:string];
    set_attributes(env, this, attributes);
    if attributes != nil {
        let key = ns_string::get_static_str(env, "NSFont");
        let font: id = msg![env; attributes objectForKey:key];
        set_font(env, this, font);
    }
    this
}
- (id)initWithAttributedString:(id)other {
    let string: id = msg![env; other string];
    let this: id = msg![env; this initWithString:string];
    let font = env.objc.borrow::<NSAttributedStringHostObject>(other).font;
    let attributes = env.objc.borrow::<NSAttributedStringHostObject>(other).attributes;
    set_attributes(env, this, attributes);
    set_font(env, this, font);
    this
}

- (())dealloc {
    let &NSAttributedStringHostObject { string, font, attributes } = env.objc.borrow(this);
    release(env, string);
    release(env, font);
    release(env, attributes);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)copyWithZone:(NSZonePtr)_zone {
    let copy: id = msg_class![env; NSAttributedString alloc];
    msg![env; copy initWithAttributedString:this]
}
- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    let copy: id = msg_class![env; NSMutableAttributedString alloc];
    msg![env; copy initWithAttributedString:this]
}

- (id)string {
    env.objc.borrow::<NSAttributedStringHostObject>(this).string
}
- (NSUInteger)length {
    let string = env.objc.borrow::<NSAttributedStringHostObject>(this).string;
    msg![env; string length]
}
- (id)description {
    env.objc.borrow::<NSAttributedStringHostObject>(this).string
}

// From UIKit's NSStringDrawing additions (iOS 7).
- (CGRect)boundingRectWithSize:(CGSize)size
                       options:(NSUInteger)_options
                       context:(id)_context { // NSStringDrawingContext*
    let mut font = env.objc.borrow::<NSAttributedStringHostObject>(this).font;
    if font == nil {
        // The default font for attributed strings.
        font = msg_class![env; UIFont systemFontOfSize:(12.0 as CGFloat)];
    }
    let string = env.objc.borrow::<NSAttributedStringHostObject>(this).string;
    let size: CGSize = msg![env; string sizeWithFont:font constrainedToSize:size];
    CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size,
    }
}

- (id)attributesAtIndex:(NSUInteger)_index
         effectiveRange:(MutPtr<NSRange>)range {
    if !range.is_null() {
        let length: NSUInteger = msg![env; this length];
        env.mem.write(range, NSRange { location: 0, length });
    }
    let attributes = env.objc.borrow::<NSAttributedStringHostObject>(this).attributes;
    if attributes != nil { attributes } else {
        let dict = ns_dictionary::dict_from_keys_and_objects(env, &[]);
        autorelease(env, dict)
    }
}

@end

@implementation NSMutableAttributedString: NSAttributedString

- (id)copyWithZone:(NSZonePtr)_zone {
    let copy: id = msg_class![env; NSAttributedString alloc];
    msg![env; copy initWithAttributedString:this]
}

- (id)mutableString {
    // TODO: changes to the returned string should change this string
    let string = env.objc.borrow::<NSAttributedStringHostObject>(this).string;
    let mutable: id = msg![env; string mutableCopy];
    autorelease(env, mutable)
}

- (())replaceCharactersInRange:(NSRange)range
                    withString:(id)replacement { // NSString*
    let string = env.objc.borrow::<NSAttributedStringHostObject>(this).string;
    let NSRange { location, length } = range;
    let before: id = msg![env; string substringToIndex:location];
    let after: id = msg![env; string substringFromIndex:(location + length)];
    let new_string: id = msg![env; before stringByAppendingString:replacement];
    let new_string: id = msg![env; new_string stringByAppendingString:after];
    let new_string: id = msg![env; new_string copy];
    env.objc.borrow_mut::<NSAttributedStringHostObject>(this).string = new_string;
    release(env, string);
}
- (())appendAttributedString:(id)other {
    let other_string: id = msg![env; other string];
    let string = env.objc.borrow::<NSAttributedStringHostObject>(this).string;
    let new_string: id = msg![env; string stringByAppendingString:other_string];
    let new_string: id = msg![env; new_string copy];
    env.objc.borrow_mut::<NSAttributedStringHostObject>(this).string = new_string;
    release(env, string);
}
- (())setAttributedString:(id)other {
    let other_string: id = msg![env; other string];
    let new_string: id = msg![env; other_string copy];
    let string = env.objc.borrow::<NSAttributedStringHostObject>(this).string;
    env.objc.borrow_mut::<NSAttributedStringHostObject>(this).string = new_string;
    release(env, string);
    let attributes = env.objc.borrow::<NSAttributedStringHostObject>(other).attributes;
    set_attributes(env, this, attributes);
    let font = env.objc.borrow::<NSAttributedStringHostObject>(other).font;
    set_font(env, this, font);
}

// These operations currently apply to the whole string.
- (())addAttribute:(id)name // NSString*
             value:(id)value
             range:(NSRange)_range {
    set_attribute(env, this, name, value);
    let attributes = mutable_attributes(env, this);
    () = msg![env; attributes setObject:value forKey:name];
    set_attributes(env, this, attributes);
    release(env, attributes);
}
- (())addAttributes:(id)attributes // NSDictionary*
              range:(NSRange)_range {
    let key = ns_string::get_static_str(env, "NSFont");
    let font: id = msg![env; attributes objectForKey:key];
    if font != nil {
        set_font(env, this, font);
    }
    if attributes != nil {
        let merged = mutable_attributes(env, this);
        () = msg![env; merged addEntriesFromDictionary:attributes];
        set_attributes(env, this, merged);
        release(env, merged);
    }
}
- (())setAttributes:(id)attributes // NSDictionary*
              range:(NSRange)_range {
    let font: id = if attributes == nil {
        nil
    } else {
        let key = ns_string::get_static_str(env, "NSFont");
        msg![env; attributes objectForKey:key]
    };
    set_font(env, this, font);
    set_attributes(env, this, attributes);
}
- (())removeAttribute:(id)name // NSString*
                range:(NSRange)_range {
    let attributes = mutable_attributes(env, this);
    () = msg![env; attributes removeObjectForKey:name];
    set_attributes(env, this, attributes);
    release(env, attributes);
    if name != nil && ns_string::to_rust_string(env, name) == "NSFont" { set_font(env, this, nil); }
}
- (())beginEditing {
}
- (())endEditing {
}

@end

};
