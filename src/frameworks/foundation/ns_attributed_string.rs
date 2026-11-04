/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSAttributedString` and `NSMutableAttributedString`.
//!
//! Attributes are stored in ranges of UTF-16 code units.

use super::{ns_dictionary, ns_string, NSRange, NSUInteger};
use crate::abi::CallFromHost;
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
    /// Non-overlapping runs, in string order. Each run owns its dictionary.
    runs: Vec<(NSRange, id)>,
}

fn replace_runs(env: &mut crate::Environment, this: id, runs: Vec<(NSRange, id)>) {
    let old = std::mem::replace(
        &mut env
            .objc
            .borrow_mut::<NSAttributedStringHostObject>(this)
            .runs,
        runs,
    );
    for (_, attributes) in old {
        release(env, attributes);
    }
}

fn copy_runs(env: &mut crate::Environment, this: id) -> Vec<(NSRange, id)> {
    let runs = env
        .objc
        .borrow::<NSAttributedStringHostObject>(this)
        .runs
        .clone();
    for (_, attributes) in &runs {
        retain(env, *attributes);
    }
    runs
}

fn splice_runs(
    env: &mut crate::Environment,
    this: id,
    range: NSRange,
    replacement_len: NSUInteger,
    replacement: Vec<(NSRange, id)>,
) {
    let old = env
        .objc
        .borrow::<NSAttributedStringHostObject>(this)
        .runs
        .clone();
    let end = range.location + range.length;
    let mut runs = Vec::new();
    for (run, attrs) in &old {
        let stop = (run.location + run.length).min(range.location);
        if run.location < stop {
            runs.push((
                NSRange {
                    location: run.location,
                    length: stop - run.location,
                },
                retain(env, *attrs),
            ));
        }
    }
    for (mut run, attrs) in replacement {
        run.location += range.location;
        runs.push((run, attrs));
    }
    for (run, attrs) in &old {
        let start = run.location.max(end);
        let stop = run.location + run.length;
        if start < stop {
            runs.push((
                NSRange {
                    location: start - range.length + replacement_len,
                    length: stop - start,
                },
                retain(env, *attrs),
            ));
        }
    }
    replace_runs(env, this, runs);
}

fn edit_attributes(
    env: &mut crate::Environment,
    this: id,
    range: NSRange,
    edit: impl Fn(&mut crate::Environment, id),
) {
    let length: NSUInteger = msg![env; this length];
    let Some(end) = range.location.checked_add(range.length) else {
        return;
    };
    if end > length || range.length == 0 {
        return;
    }
    let old = env
        .objc
        .borrow::<NSAttributedStringHostObject>(this)
        .runs
        .clone();
    let mut runs = Vec::new();
    for (run, attributes) in old {
        let run_end = run.location + run.length;
        let start = run.location.max(range.location);
        let stop = run_end.min(end);
        if start >= stop {
            runs.push((run, retain(env, attributes)));
            continue;
        }
        if run.location < start {
            runs.push((
                NSRange {
                    location: run.location,
                    length: start - run.location,
                },
                retain(env, attributes),
            ));
        }
        let mutable: id = if attributes == nil {
            msg_class![env; NSMutableDictionary new]
        } else {
            msg![env; attributes mutableCopy]
        };
        edit(env, mutable);
        let count: NSUInteger = msg![env; mutable count];
        let updated: id = if count == 0 {
            nil
        } else {
            msg![env; mutable copy]
        };
        release(env, mutable);
        runs.push((
            NSRange {
                location: start,
                length: stop - start,
            },
            updated,
        ));
        if stop < run_end {
            runs.push((
                NSRange {
                    location: stop,
                    length: run_end - stop,
                },
                retain(env, attributes),
            ));
        }
    }
    replace_runs(env, this, runs);
}

/// Return the value and its longest matching run, bounded by `limit`.
pub(crate) fn attribute_range(
    env: &mut crate::Environment,
    this: id,
    index: NSUInteger,
    limit: NSRange,
    key: Option<id>,
) -> (id, NSRange) {
    let runs = env
        .objc
        .borrow::<NSAttributedStringHostObject>(this)
        .runs
        .clone();
    let Some(at) = runs
        .iter()
        .position(|(r, _)| index >= r.location && index < r.location + r.length)
    else {
        return (
            nil,
            NSRange {
                location: index,
                length: 0,
            },
        );
    };
    let value = |env: &mut crate::Environment, attributes: id| -> id {
        match key {
            None => attributes,
            Some(_) if attributes == nil => nil,
            Some(key) => msg![env; attributes objectForKey:key],
        }
    };
    let target = value(env, runs[at].1);
    let equals = |env: &mut crate::Environment, other: id| -> bool {
        let other = value(env, other);
        if target == other {
            true
        } else if target == nil || other == nil {
            false
        } else {
            msg![env; target isEqual:other]
        }
    };
    let mut first = at;
    let mut last = at;
    while first > 0 && equals(env, runs[first - 1].1) {
        first -= 1;
    }
    while last + 1 < runs.len() && equals(env, runs[last + 1].1) {
        last += 1;
    }
    let start = runs[first].0.location.max(limit.location);
    let end = (runs[last].0.location + runs[last].0.length)
        .min(limit.location.saturating_add(limit.length));
    (
        target,
        NSRange {
            location: start,
            length: end.saturating_sub(start),
        },
    )
}

fn enumerate_attributes(
    env: &mut crate::Environment,
    this: id,
    range: NSRange,
    options: NSUInteger,
    block: id,
    key: Option<id>,
) {
    if block == nil {
        return;
    }
    let length: NSUInteger = msg![env; this length];
    let Some(end) = range.location.checked_add(range.length) else {
        return;
    };
    if end > length {
        return;
    }
    let mut position = range.location;
    let mut segments = Vec::new();
    while position < end {
        let (mut value, effective) = attribute_range(env, this, position, range, key);
        if effective.length == 0 {
            break;
        }
        if key.is_none() && value == nil {
            value = ns_dictionary::dict_from_keys_and_objects(env, &[]);
        } else {
            retain(env, value);
        }
        position = effective.location + effective.length;
        segments.push((effective, value));
    }
    if options & 2 != 0 {
        segments.reverse();
    }
    let invoke = crate::libc::dispatch::block_invoke_function(env, block.cast_const().cast());
    let stop: MutPtr<bool> = env.mem.alloc_and_write(false);
    for (effective, value) in &segments {
        let _: () = invoke.call_from_host(env, (block, *value, *effective, stop));
        if env.mem.read(stop) {
            break;
        }
    }
    env.mem.free(stop.cast());
    for (_, value) in segments {
        release(env, value);
    }
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
    let host_object = Box::new(NSAttributedStringHostObject { string: nil, font: nil, attributes: nil, runs: Vec::new() });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)init {
    let empty: id = msg_class![env; NSString new];
    env.objc.borrow_mut::<NSAttributedStringHostObject>(this).string = empty;
    this
}
- (id)initWithString:(id)string { // NSString*
    let string: id = msg![env; string copy];
    let length: NSUInteger = msg![env; string length];
    env.objc.borrow_mut::<NSAttributedStringHostObject>(this).string = string;
    if length != 0 { replace_runs(env, this, vec![(NSRange { location: 0, length }, nil)]); }
    this
}
- (id)initWithString:(id)string // NSString*
          attributes:(id)attributes { // NSDictionary*
    let this: id = msg![env; this initWithString:string];
    set_attributes(env, this, attributes);
    let length: NSUInteger = msg![env; this length];
    if length != 0 { let count: NSUInteger = msg![env; attributes count]; let copy: id = if count == 0 { nil } else { msg![env; attributes copy] }; replace_runs(env, this, vec![(NSRange { location: 0, length }, copy)]); }
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
    let runs = copy_runs(env, other);
    replace_runs(env, this, runs);
    this
}

- (())dealloc {
    let object = env.objc.borrow::<NSAttributedStringHostObject>(this);
    let (string, font, attributes) = (object.string, object.font, object.attributes);
    replace_runs(env, this, Vec::new());
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

- (id)attributedSubstringFromRange:(NSRange)range {
    let length: NSUInteger = msg![env; this length];
    let Some(end) = range.location.checked_add(range.length) else { return nil; };
    if end > length { return nil; }
    let text: id = msg![env; this string];
    let text: id = msg![env; text substringWithRange:range];
    let result: id = msg_class![env; NSAttributedString alloc];
    let result: id = msg![env; result initWithString:text];
    let old = env.objc.borrow::<NSAttributedStringHostObject>(this).runs.clone();
    let mut runs = Vec::new();
    for (run, attrs) in old {
        let start = run.location.max(range.location);
        let stop = (run.location + run.length).min(end);
        if start < stop { runs.push((NSRange { location: start - range.location, length: stop - start }, retain(env, attrs))); }
    }
    replace_runs(env, result, runs);
    autorelease(env, result)
}

- (id)attribute:(id)name atIndex:(NSUInteger)index effectiveRange:(MutPtr<NSRange>)range {
    let length: NSUInteger = msg![env; this length];
    if index >= length || name == nil { return nil; }
    let (value, effective) = attribute_range(env, this, index, NSRange { location: 0, length }, Some(name));
    if !range.is_null() { env.mem.write(range, effective); }
    value
}

- (())enumerateAttributesInRange:(NSRange)range options:(NSUInteger)options usingBlock:(id)block {
    enumerate_attributes(env, this, range, options, block, None);
}
- (())enumerateAttribute:(id)name inRange:(NSRange)range options:(NSUInteger)options usingBlock:(id)block {
    if name != nil { enumerate_attributes(env, this, range, options, block, Some(name)); }
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

- (id)attributesAtIndex:(NSUInteger)index
         effectiveRange:(MutPtr<NSRange>)range {
    let length: NSUInteger = msg![env; this length];
    if index >= length { return nil; }
    let (attributes, effective) = attribute_range(env, this, index, NSRange { location: 0, length }, None);
    if !range.is_null() {
        env.mem.write(range, effective);
    }
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
    let old_length: NSUInteger = msg![env; string length];
    if location.checked_add(length).is_none_or(|end| end > old_length) { return; }
    let replacement_len: NSUInteger = msg![env; replacement length];
    let attrs = if old_length == 0 { nil } else {
        let inherited_index = if length == 0 && location > 0 { location - 1 } else { location.min(old_length - 1) };
        attribute_range(env, this, inherited_index, NSRange { location: 0, length: old_length }, None).0
    };
    let replacement_runs = if replacement_len == 0 { Vec::new() } else { vec![(NSRange { location: 0, length: replacement_len }, retain(env, attrs))] };
    splice_runs(env, this, range, replacement_len, replacement_runs);
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
    let length: NSUInteger = msg![env; string length];
    let other_length: NSUInteger = msg![env; other_string length];
    let runs = copy_runs(env, other);
    splice_runs(env, this, NSRange { location: length, length: 0 }, other_length, runs);
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
    let runs = copy_runs(env, other);
    replace_runs(env, this, runs);
}

- (())addAttribute:(id)name // NSString*
             value:(id)value
             range:(NSRange)range {
    if name == nil || value == nil { return; }
    edit_attributes(env, this, range, |env, attrs| { let _: () = msg![env; attrs setObject:value forKey:name]; });
    set_attribute(env, this, name, value);
    let attributes = mutable_attributes(env, this);
    () = msg![env; attributes setObject:value forKey:name];
    set_attributes(env, this, attributes);
    release(env, attributes);
}
- (())addAttributes:(id)attributes // NSDictionary*
              range:(NSRange)range {
    if attributes != nil { edit_attributes(env, this, range, |env, attrs| { let _: () = msg![env; attrs addEntriesFromDictionary:attributes]; }); }
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
              range:(NSRange)range {
    edit_attributes(env, this, range, |env, attrs| {
        let _: () = msg![env; attrs removeAllObjects];
        if attributes != nil { let _: () = msg![env; attrs addEntriesFromDictionary:attributes]; }
    });
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
                range:(NSRange)range {
    if name == nil { return; }
    edit_attributes(env, this, range, |env, attrs| { let _: () = msg![env; attrs removeObjectForKey:name]; });
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
