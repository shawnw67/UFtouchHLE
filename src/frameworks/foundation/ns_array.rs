/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The `NSArray` class cluster, including `NSMutableArray`.

use super::ns_enumerator::{fast_enumeration_helper, NSFastEnumerationState};
use super::ns_property_list_serialization::{
    deserialize_plist_from_file, NSPropertyListBinaryFormat_v1_0,
};
use super::{
    _nib_archive_decoder, ns_keyed_unarchiver, ns_string, ns_url, NSComparisonResult, NSNotFound,
    NSRange, NSUInteger,
};
use crate::abi::{CallFromHost, DotDotDot, GuestFunction};
use crate::frameworks::foundation::ns_keyed_archiver::{
    encode_object, get_value_to_encode_for_current_key,
};
use crate::fs::GuestPath;
use crate::libc::stdlib::qsort::qsort_generic;
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr, Ptr, SafeRead};
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, nil, objc_classes, release, retain, Class,
    ClassExports, HostObject, NSZonePtr, SEL,
};
use crate::{impl_HostObject_with_superclass, Environment};

struct ObjectEnumeratorHostObject {
    /// the enumerated collection, NSArray *
    array: id,
    /// an iterator
    iterator: std::vec::IntoIter<id>,
}
impl HostObject for ObjectEnumeratorHostObject {}

/// Belongs to _touchHLE_NSArray
#[derive(Debug, Default)]
pub(super) struct ArrayHostObject {
    pub(super) array: Vec<id>,
}
impl HostObject for ArrayHostObject {}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CFArrayCallBacks {
    pub version: i32,
    pub retain: GuestFunction,
    pub release: GuestFunction,
    pub copy_description: GuestFunction,
    pub equal: GuestFunction,
}
unsafe impl SafeRead for CFArrayCallBacks {}

struct CFArrayHostObject {
    superclass: ArrayHostObject,
    callbacks: CFArrayCallBacks,
}
impl_HostObject_with_superclass!(CFArrayHostObject);

fn cf_array_retain(env: &mut Environment, this: id, value: id) -> id {
    let callback = env.objc.borrow::<CFArrayHostObject>(this).callbacks.retain;
    if callback.to_ptr().is_null() {
        value
    } else {
        callback.call_from_host(env, (nil, value))
    }
}

fn cf_array_release(env: &mut Environment, this: id, value: id) {
    let callback = env.objc.borrow::<CFArrayHostObject>(this).callbacks.release;
    if !callback.to_ptr().is_null() {
        let _: () = callback.call_from_host(env, (nil, value));
    }
}

pub fn cf_array_equal(env: &mut Environment, array: id, left: id, right: id) -> Option<bool> {
    let callbacks = env
        .objc
        .get_host_object(array)
        .and_then(|object| object.as_any().downcast_ref::<CFArrayHostObject>())
        .map(|object| object.callbacks)?;
    Some(
        left == right
            || (!callbacks.equal.to_ptr().is_null()
                && callbacks.equal.call_from_host(env, (left, right))),
    )
}

/// Copy CF-backed arrays while preserving their element ownership callbacks.
pub fn cf_array_copy(env: &mut Environment, array: id, mutable: bool) -> Option<id> {
    let callbacks = env
        .objc
        .get_host_object(array)
        .and_then(|object| object.as_any().downcast_ref::<CFArrayHostObject>())
        .map(|object| object.callbacks);
    let plain_immutable: Class = msg_class![env; _touchHLE_NSArray_non_retaining class];
    let plain_mutable: Class = msg_class![env; _touchHLE_NSMutableArray_non_retaining class];
    let plain: bool = msg![env; array isKindOfClass:plain_immutable];
    let plain = plain || msg![env; array isKindOfClass:plain_mutable];
    if callbacks.is_none() && !plain {
        return None;
    }
    let values = env.objc.borrow::<ArrayHostObject>(array).array.clone();
    if !mutable {
        return Some(cf_array_new_immutable(env, callbacks, values));
    }
    let name = if callbacks.is_some() {
        "_touchHLE_NSMutableArray_cf_callbacks"
    } else {
        "_touchHLE_NSMutableArray_non_retaining"
    };
    let class = env.objc.get_known_class(name, &mut env.mem);
    let result = if let Some(callbacks) = callbacks {
        env.objc.alloc_object(
            class,
            Box::new(CFArrayHostObject {
                superclass: ArrayHostObject::default(),
                callbacks,
            }),
            &mut env.mem,
        )
    } else {
        env.objc
            .alloc_object(class, Box::<ArrayHostObject>::default(), &mut env.mem)
    };
    for value in values {
        let value = if callbacks.is_some() {
            cf_array_retain(env, result, value)
        } else {
            value
        };
        env.objc
            .borrow_mut::<ArrayHostObject>(result)
            .array
            .push(value);
    }
    Some(result)
}

pub fn cf_array_new_immutable(
    env: &mut Environment,
    callbacks: Option<CFArrayCallBacks>,
    values: Vec<id>,
) -> id {
    if let Some(callbacks) = callbacks {
        assert_eq!(callbacks.version, 0);
    }
    let name = if callbacks.is_some() {
        "_touchHLE_NSArray_cf_callbacks"
    } else {
        "_touchHLE_NSArray_non_retaining"
    };
    let class = env.objc.get_known_class(name, &mut env.mem);
    let array = if let Some(callbacks) = callbacks {
        env.objc.alloc_object(
            class,
            Box::new(CFArrayHostObject {
                superclass: ArrayHostObject::default(),
                callbacks,
            }),
            &mut env.mem,
        )
    } else {
        env.objc
            .alloc_object(class, Box::<ArrayHostObject>::default(), &mut env.mem)
    };
    for value in values {
        let value = if callbacks.is_some() {
            cf_array_retain(env, array, value)
        } else {
            value
        };
        env.objc
            .borrow_mut::<ArrayHostObject>(array)
            .array
            .push(value);
    }
    array
}

/// Replace a CF-backed mutable array range while respecting its callbacks.
pub fn cf_array_replace(
    env: &mut Environment,
    array: id,
    start: usize,
    len: usize,
    values: &[id],
) -> bool {
    let custom = env
        .objc
        .get_host_object(array)
        .is_some_and(|object| object.as_any().is::<CFArrayHostObject>());
    let plain_class: Class = msg_class![env; _touchHLE_NSMutableArray_non_retaining class];
    let plain: bool = msg![env; array isKindOfClass:plain_class];
    if !custom && !plain {
        return false;
    }
    let mut inserted = Vec::with_capacity(values.len());
    for &value in values {
        inserted.push(if custom {
            cf_array_retain(env, array, value)
        } else {
            value
        });
    }
    let removed: Vec<id> = env
        .objc
        .borrow_mut::<ArrayHostObject>(array)
        .array
        .splice(start..start + len, inserted)
        .collect();
    if custom {
        for value in removed {
            cf_array_release(env, array, value);
        }
    }
    true
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// NSArray is an abstract class. A subclass must provide:
// - (NSUInteger)count;
// - (id)objectAtIndex:(NSUInteger)index;
// We can pick whichever subclass we want for the various alloc methods.
// For the time being, that will always be _touchHLE_NSArray.
@implementation NSArray: NSObject


+ (id)allocWithZone:(NSZonePtr)zone {
    // NSArray might be subclassed by something which needs allocWithZone:
    // to have the normal behaviour. Unimplemented: call superclass alloc then.
    assert!(this == env.objc.get_known_class("NSArray", &mut env.mem));
    msg_class![env; _touchHLE_NSArray allocWithZone:zone]
}

+ (id)array {
    let array: id = msg![env; this new];
    autorelease(env, array)
}

+ (id)arrayWithArray:(id)other { // NSArray*
    let array: id = msg![env; this alloc];
    let array: id = msg![env; array initWithArray:other];
    autorelease(env, array)
}

// These probably comes from some category related to plists.
+ (id)arrayWithContentsOfFile:(id)path { // NSString*
    let array: id = msg![env; this alloc];
    let array: id = msg![env; array initWithContentsOfFile:path];
    autorelease(env, array)
}
+ (id)arrayWithContentsOfURL:(id)url { // NSURL*
    let array: id = msg![env; this alloc];
    let array: id = msg![env; array initWithContentsOfURL:url];
    autorelease(env, array)
}

+ (id)arrayWithObject:(id)object {
    retain(env, object);
    let objects = vec![object];
    let array = from_vec(env, objects);
    autorelease(env, array)
}
+ (id)arrayWithObjects:(id)first_obj, ...args {
    let objects = if first_obj == nil {
        vec![]
    } else {
        retain(env, first_obj);
        let mut objects = vec![first_obj];
        let mut varargs = args.start();
        loop {
            let next_arg: id = varargs.next(env);
            if next_arg.is_null() {
                break;
            }
            retain(env, next_arg);
            objects.push(next_arg);
        }
        objects
    };
    let array = from_vec(env, objects);
    autorelease(env, array)
}
+ (id)arrayWithObjects:(ConstPtr<id>)objects_ptr count:(NSUInteger)count {
    let array: id = msg![env; this alloc];
    let array: id = msg![env; array initWithObjects:objects_ptr count:count];
    autorelease(env, array)
}

// These probably comes from some category related to plists.
- (id)initWithContentsOfFile:(id)path { // NSString*
    release(env, this);
    let path = ns_string::to_rust_string(env, path);
    deserialize_plist_from_file(
        env,
        GuestPath::new(&path),
        /* array_expected: */ true,
    )
}
- (id)initWithContentsOfURL:(id)url { // NSURL*
    release(env, this);
    let path = ns_url::to_rust_path(env, url);
    deserialize_plist_from_file(env, &path, /* array_expected: */ true)
}

- (bool)writeToFile:(id)path // NSString*
         atomically:(bool)atomically {
    let error_desc: MutPtr<id> = Ptr::null();
    let data: id = msg_class![env; NSPropertyListSerialization
            dataFromPropertyList:this
                          format:NSPropertyListBinaryFormat_v1_0
                errorDescription:error_desc];
    let res = msg![env; data writeToFile:path atomically:atomically];
    log_dbg!(
        "[(NSArray *){:?} writeToFile:{:?} atomically:{}] -> {}",
        this,
        ns_string::to_rust_string(env, path),
        atomically,
        res
    );
    res
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

- (NSUInteger)indexOfObject:(id)object {
    let count: NSUInteger = msg![env; this count];
    for i in 0..count {
        let curr_object: id = msg![env; this objectAtIndex:i];
        let equal: bool = msg![env; object isEqual:curr_object];
        if equal {
            return i;
        }
    }
    NSNotFound as NSUInteger
}
- (bool)containsObject:(id)object {
    let idx: NSUInteger = msg![env; this indexOfObject:object];
    idx != NSNotFound as NSUInteger
}

- (id)firstObject {
    let size: NSUInteger = msg![env; this count];
    if size == 0 {
        return nil;
    }
    msg![env; this objectAtIndex:0u32]
}

- (id)lastObject {
    let size: NSUInteger = msg![env; this count];
    if size == 0 {
        return nil;
    }
    msg![env; this objectAtIndex:(size - 1)]
}

- (id)componentsJoinedByString:(id)str { // NSString *
    let res: id = msg_class![env; NSMutableString new];
    let count: NSUInteger = msg![env; this count];
    if count == 0 {
        autorelease(env, res);
        return res;
    }
    for i in 0..count {
        let curr_object: id = msg![env; this objectAtIndex:i];
        let curr_desc: id = msg![env; curr_object description];
        () = msg![env; res appendString:curr_desc];
        if i != count-1 {
            () = msg![env; res appendString:str];
        }
    }
    let res_imm = msg![env; res copy];
    release(env, res);
    autorelease(env, res_imm)
}

- (id)sortedArrayUsingFunction:(GuestFunction)comparator
                       context:(MutVoidPtr)context {
    let array = msg![env; this mutableCopy];
    () = msg![env; array sortUsingFunction:comparator context:context];
    let array_imm = msg![env; array copy];
    release(env, array);
    autorelease(env, array_imm)
}

- (NSUInteger)hash {
    // TODO: define better hash
    msg![env; this count]
}
- (bool)isEqual:(id)other {
    if this == other {
        return true;
    }
    let class: Class = msg_class![env; NSArray class];
    if !msg![env; other isKindOfClass:class] {
        return false;
    }
    msg![env; this isEqualToArray:other]
}
- (bool)isEqualToArray:(id)other { // NSArray *
    if other == nil {
        return false;
    }
    let count: NSUInteger = msg![env; this count];
    let other_count: NSUInteger = msg![env; other count];
    if count != other_count {
        return false;
    }
    for i in 0..count {
        let curr_object: id = msg![env; this objectAtIndex:i];
        let curr_other_object: id = msg![env; other objectAtIndex:i];
        let equal: bool = msg![env; curr_object isEqual:curr_other_object];
        if !equal {
            return false;
        }
    }
    true
}

// Modern Objective-C subscripting (array[index])
- (id)objectAtIndexedSubscript:(NSUInteger)index {
    msg![env; this objectAtIndex:index]
}

- (id)arrayByAddingObject:(id)object {
    let new_array: id = msg![env; this mutableCopy];
    () = msg![env; new_array addObject:object];
    let result: id = msg![env; new_array copy];
    release(env, new_array);
    autorelease(env, result)
}

- (())enumerateObjectsUsingBlock:(id)block {
    retain(env, this);
    let count: NSUInteger = msg![env; this count];
    let invoke = crate::libc::dispatch::block_invoke_function(env, block.cast_const().cast());
    let stop: MutPtr<bool> = env.mem.alloc_and_write(false);
    for i in 0..count {
        let object: id = msg![env; this objectAtIndex:i];
        let () = invoke.call_from_host(env, (block, object, i, stop));
        if env.mem.read(stop) {
            break;
        }
    }
    env.mem.free(stop.cast());
    release(env, this);
}

@end

// NSMutableArray is an abstract class. A subclass must provide everything
// NSArray provides, plus:
// - (void)insertObject:(id)object atIndex:(NSUInteger)index;
// - (void)removeObjectAtIndex:(NSUInteger)index;
// - (void)addObject:(id)object;
// - (void)removeLastObject
// - (void)replaceObjectAtIndex:(NSUInteger)index withObject:(id)object;
// Note that it inherits from NSArray, so we must ensure we override any default
// methods that would be inappropriate for mutability.
@implementation NSMutableArray: NSArray


+ (id)allocWithZone:(NSZonePtr)zone {
    // NSArray might be subclassed by something which needs allocWithZone:
    // to have the normal behaviour. Unimplemented: call superclass alloc then.
    assert!(this == env.objc.get_known_class("NSMutableArray", &mut env.mem));
    msg_class![env; _touchHLE_NSMutableArray allocWithZone:zone]
}

+ (id)arrayWithCapacity:(NSUInteger)capacity {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCapacity:capacity];
    autorelease(env, new)
}

+ (id)arrayWithArray:(id)array {
    let new: id = msg![env; this alloc];
    () = msg![env; new addObjectsFromArray:array];
    autorelease(env, new)
}

+ (id)arrayWithObjects:(id)firstObj, ...args {
    retain(env, firstObj);
    let mut objects = vec![firstObj];
    let mut varargs = args.start();
    loop {
        let next_arg: id = varargs.next(env);
        if next_arg.is_null() {
            break;
        }
        retain(env, next_arg);
        objects.push(next_arg);
    }
    let array = mutable_from_vec(env, objects);
    autorelease(env, array)
}

// These probably comes from some category related to plists.
- (id)initWithContentsOfFile:(id)path { // NSString*
    release(env, this);
    let path = ns_string::to_rust_string(env, path);
    let tmp = deserialize_plist_from_file(
        env,
        GuestPath::new(&path),
        /* array_expected: */ true,
    );
    if tmp == nil {
        return nil;
    }
    // We should respect mutability of the top most container!
    let res = msg_class![env; NSMutableArray alloc];
    let res = msg![env; res initWithArray:tmp];
    release(env, tmp);
    res
}
- (id)initWithContentsOfURL:(id)url { // NSURL*
    release(env, this);
    let path = ns_url::to_rust_path(env, url);
    let tmp = deserialize_plist_from_file(env, &path, /* array_expected: */ true);
    if tmp == nil {
        return nil;
    }
    // We should respect mutability of the top most container!
    let res = msg_class![env; NSMutableArray alloc];
    let res = msg![env; res initWithArray:tmp];
    release(env, tmp);
    res
}

- (())addObjectsFromArray:(id)other { // NSArray*
    let enumerator: id = msg![env; other objectEnumerator];
    loop {
        let next: id = msg![env; enumerator nextObject];
        if next == nil {
            break;
        }
        () = msg![env; this addObject:next];
    }
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    let other: id = msg_class![env; NSArray alloc];
    let other: id = msg![env; other initWithArray:this];
    other
}

// Modern Objective-C subscripting (array[index] = object)
- (())setObject:(id)object atIndexedSubscript:(NSUInteger)index {
    let count: NSUInteger = msg![env; this count];
    if index == count {
        () = msg![env; this addObject:object];
    } else {
        () = msg![env; this replaceObjectAtIndex:index withObject:object];
    }
}

@end

// Our private subclass that is the single implementation of NSArray for the
// time being.
@implementation _touchHLE_NSArray: NSArray

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(ArrayHostObject {
        array: Vec::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    init_with_coder_inner(env, this, coder)
}
- (())encodeWithCoder:(id)coder {
    encode_with_coder_inner(env, this, coder)
}

- (id)initWithArray:(id)array { // NSArray*
    let mut objects = Vec::new();
    let enumerator: id = msg![env; array objectEnumerator];
    loop {
        let next: id = msg![env; enumerator nextObject];
        if next == nil {
            break;
        }
        objects.push(next);
        retain(env, next);
    }
    env.objc.borrow_mut::<ArrayHostObject>(this).array = objects;
    this
}

- (id)initWithObjects:(id)first_obj, ...args {
    init_with_objects_inner(env, this, first_obj, args);
    this
}

- (id)initWithObjects:(ConstPtr<id>)objects_ptr count:(NSUInteger)count {
    let mut objects = Vec::new();
    for i in 0..count {
        let obj: id = env.mem.read(objects_ptr + i);
        retain(env, obj);
        objects.push(obj);
    }
    env.objc.borrow_mut::<ArrayHostObject>(this).array = objects;
    this
}

- (())dealloc {
    let host_object: &mut ArrayHostObject = env.objc.borrow_mut(this);
    let array = std::mem::take(&mut host_object.array);

    for object in array {
        release(env, object);
    }

    env.objc.dealloc_object(this, &mut env.mem)
}

// NSMutableCopying implementation
- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    mutable_copy_inner(env, this)
}

- (id)objectEnumerator { // NSEnumerator*
    object_enumerator_inner(env, this)
}
- (id)reverseObjectEnumerator { // NSEnumerator*
    reverse_object_enumerator_inner(env, this)
}

// NSFastEnumeration implementation
- (NSUInteger)countByEnumeratingWithState:(MutPtr<NSFastEnumerationState>)state
                                  objects:(MutPtr<id>)stackbuf
                                    count:(NSUInteger)len {
    let count: NSUInteger = msg![env; this count];
    fast_enumeration_helper(env, this, |env, idx| {
        if idx < count {
            msg![env; this objectAtIndex:idx]
        } else {
            nil
        }
    }, state, stackbuf, len)
}

// TODO: more init methods, etc

- (NSUInteger)count {
    env.objc.borrow::<ArrayHostObject>(this).array.len().try_into().unwrap()
}
- (id)objectAtIndex:(NSUInteger)index {
    // TODO: throw real exception rather than panic if out-of-bounds?
    env.objc.borrow::<ArrayHostObject>(this).array[index as usize]
}

- (id)description {
    build_description(env, this)
}

- (id)subarrayWithRange:(NSRange)range {
    let mut tmp = Vec::new();
    tmp.extend_from_slice(
        &env.objc.borrow::<ArrayHostObject>(this).array[range.location as usize..(range.location + range.length) as usize]
    );
    for &obj in &tmp {
        retain(env, obj);
    }
    let res = from_vec(env, tmp);
    autorelease(env, res)
}

- (id)sortedArrayUsingSelector:(SEL)comparator {
    let new = msg![env; this mutableCopy];
    () = msg![env; new sortUsingSelector:comparator];
    autorelease(env, new)
}

@end

// Special variant for use by CFArray with NULL callbacks: objects aren't
// necessarily Objective-C objects and won't be retained/released.
@implementation _touchHLE_NSArray_non_retaining: _touchHLE_NSArray

- (id)copyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, false).unwrap() }
- (id)mutableCopyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, true).unwrap() }

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation _touchHLE_NSArray_ObjectEnumerator: NSEnumerator

- (id)nextObject {
    let host_obj = env.objc.borrow_mut::<ObjectEnumeratorHostObject>(this);
    host_obj.iterator.next().unwrap_or(nil)
}

- (())dealloc {
    let host_obj = env.objc.borrow::<ObjectEnumeratorHostObject>(this);
    release(env, host_obj.array);
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

// Our private subclass that is the single implementation of NSMutableArray for
// the time being.
@implementation _touchHLE_NSMutableArray: NSMutableArray

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(ArrayHostObject {
        array: Vec::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithCapacity:(NSUInteger)capacity {
    env.objc.borrow_mut::<ArrayHostObject>(this).array.reserve(capacity as usize);
    this
}

- (id)initWithObjects:(id)first_obj, ...args {
    init_with_objects_inner(env, this, first_obj, args);
    this
}

- (id)initWithArray:(id)array { // NSArray*
    let mut objects = Vec::new();
    let enumerator: id = msg![env; array objectEnumerator];
    loop {
        let next: id = msg![env; enumerator nextObject];
        if next == nil {
            break;
        }
        objects.push(next);
        retain(env, next);
    }
    env.objc.borrow_mut::<ArrayHostObject>(this).array = objects;
    this
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    init_with_coder_inner(env, this, coder)
}
- (())encodeWithCoder:(id)coder {
    encode_with_coder_inner(env, this, coder)
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    let arr: id = msg_class![env; NSArray alloc];
    let array = env.objc.borrow::<ArrayHostObject>(this).array.clone();
    for &object in &array {
        retain(env, object);
    }
    env.objc.borrow_mut::<ArrayHostObject>(arr).array = array;
    arr
}

// NSMutableCopying implementation
- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    mutable_copy_inner(env, this)
}

- (())dealloc {
    let host_object: &mut ArrayHostObject = env.objc.borrow_mut(this);
    let array = std::mem::take(&mut host_object.array);

    for object in array {
        release(env, object);
    }

    env.objc.dealloc_object(this, &mut env.mem)
}

- (())makeObjectsPerformSelector:(SEL)sel {
    let count: NSUInteger = msg![env; this count];
    for idx in 0..count {
        let obj: id = msg![env; this objectAtIndex:idx];
        let _: id = msg![env; obj performSelector:sel];
    }
}

- (id)objectEnumerator { // NSEnumerator*
    object_enumerator_inner(env, this)
}
- (id)reverseObjectEnumerator { // NSEnumerator*
    reverse_object_enumerator_inner(env, this)
}

- (())sortUsingFunction:(GuestFunction)comparator
                context:(MutVoidPtr)context {
    let host_object: &mut ArrayHostObject = env.objc.borrow_mut(this);
    let mut array = std::mem::take(&mut host_object.array);
    let len = array.len().try_into().unwrap();
    let mut user_data = (env, &mut array);
    qsort_generic(
        &mut user_data,
        len,
        &mut |(env, array), l, r| {
            let (l, r): (usize, usize) = (l.try_into().unwrap(), r.try_into().unwrap());
            comparator.call_from_host(env, (array[l], array[r], context))
        },
        &mut |(_, array), l, r| {
            let (l, r): (usize, usize) = (l.try_into().unwrap(), r.try_into().unwrap());
            array.swap(l, r);
        },
    );
    let (env, _) = user_data;
    env.objc.borrow_mut::<ArrayHostObject>(this).array = array;
}

- (())sortUsingSelector:(SEL)comparator {
    let host_object: &mut ArrayHostObject = env.objc.borrow_mut(this);
    let mut array = std::mem::take(&mut host_object.array);
    let len = array.len().try_into().unwrap();
    let mut user_data = (env, &mut array);
    qsort_generic(
        &mut user_data,
        len,
        &mut |(env, array), l, r| {
            let (l, r): (usize, usize) = (l.try_into().unwrap(), r.try_into().unwrap());
            let res: NSComparisonResult = msg_send(env, (array[l], comparator, array[r]));
            res
        },
        &mut |(_, array), l, r| {
            let (l, r): (usize, usize) = (l.try_into().unwrap(), r.try_into().unwrap());
            array.swap(l, r);
        },
    );

    let (env, _) = user_data;
    env.objc.borrow_mut::<ArrayHostObject>(this).array = array;
}

// NSFastEnumeration implementation
- (NSUInteger)countByEnumeratingWithState:(MutPtr<NSFastEnumerationState>)state
                                  objects:(MutPtr<id>)stackbuf
                                    count:(NSUInteger)len {
    // TODO: check that array wasn't mutated!
    let count: NSUInteger = msg![env; this count];
    fast_enumeration_helper(env, this, |env, idx| {
        if idx < count {
            msg![env; this objectAtIndex:idx]
        } else {
            nil
        }
    }, state, stackbuf, len)
}

- (NSUInteger)count {
    env.objc.borrow::<ArrayHostObject>(this).array.len().try_into().unwrap()
}
- (id)objectAtIndex:(NSUInteger)index {
    // TODO: throw real exception rather than panic if out-of-bounds?
    env.objc.borrow::<ArrayHostObject>(this).array[index as usize]
}

- (id)description {
    build_description(env, this)
}

// TODO: more mutation methods

- (())insertObject:(id)object
           atIndex:(NSUInteger)index {
    retain(env, object);
    env.objc.borrow_mut::<ArrayHostObject>(this).array.insert(index as usize, object);
}

- (())exchangeObjectAtIndex:(NSUInteger)first withObjectAtIndex:(NSUInteger)second {
    env.objc.borrow_mut::<ArrayHostObject>(this).array.swap(first as usize, second as usize);
}

- (())addObject:(id)object {
    retain(env, object);
    env.objc.borrow_mut::<ArrayHostObject>(this).array.push(object);
}

- (())removeObject:(id)object {
    let mut to_remove = Vec::new();
    let count: NSUInteger = msg![env; this count];
    for i in 0..count {
        let curr_object: id = msg![env; this objectAtIndex:i];
        let equal: bool = msg![env; object isEqual:curr_object];
        if equal {
            to_remove.push(i);
        }
    }
    // TODO: runtime here is O(n^2), it could be O(n) instead
    // Remove from the end so earlier indices remain valid.
    for i in to_remove.into_iter().rev() {
        () = msg![env; this removeObjectAtIndex:i];
    }
}

- (())removeObjectAtIndex:(NSUInteger)index {
    let object = env.objc.borrow_mut::<ArrayHostObject>(this).array.remove(index as usize);
    release(env, object)
}

- (())replaceObjectAtIndex:(NSUInteger)index withObject:(id)obj {
    retain(env, obj);
    let object = std::mem::replace(&mut env.objc.borrow_mut::<ArrayHostObject>(this).array[index as usize], obj);
    release(env, object);
}

- (())removeLastObject {
    let object = env.objc.borrow_mut::<ArrayHostObject>(this).array.pop().unwrap();
    release(env, object)
}

- (())removeAllObjects {
    let host_object: &mut ArrayHostObject = env.objc.borrow_mut(this);
    let array = std::mem::take(&mut host_object.array);
    for object in array {
        release(env, object);
    }

    env.objc.borrow_mut::<ArrayHostObject>(this).array = Vec::new()
}

- (())removeObjectsInArray:(id)other { // NSArray*
    let other_count: NSUInteger = msg![env; other count];
    if other_count == 0 {
        return;
    }

    let host_object: &mut ArrayHostObject = env.objc.borrow_mut(this);
    let array = std::mem::take(&mut host_object.array);

    // Since removing objects individually has to iterate the whole array
    // for each object (with [array removeObject:obj]), so this just saves some
    // runtime costs.
    let other_array_set: id = msg_class![env; NSSet alloc];
    let other_array_set: id = msg![env; other_array_set initWithArray:other];
    let mut new_array = Vec::with_capacity(array.len());
    for object in array {
        let contains: bool = msg![env; other_array_set containsObject:object];
        if contains {
            release(env, object);
        } else {
            new_array.push(object);
        }
    }

    env.objc.borrow_mut::<ArrayHostObject>(this).array = new_array;
    release(env, other_array_set);
}

@end

// CFArray callbacks can manage arbitrary guest pointers, not just ObjC ids.
@implementation _touchHLE_NSArray_cf_callbacks: _touchHLE_NSArray

- (id)copyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, false).unwrap() }
- (id)mutableCopyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, true).unwrap() }

- (())dealloc {
    let old = std::mem::take(&mut env.objc.borrow_mut::<ArrayHostObject>(this).array);
    for object in old {
        cf_array_release(env, this, object);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation _touchHLE_NSMutableArray_cf_callbacks: _touchHLE_NSMutableArray

+ (id)allocWithZone:(NSZonePtr)_zone {
    let null = GuestFunction::null_ptr();
    let host_object = Box::new(CFArrayHostObject {
        superclass: ArrayHostObject::default(),
        callbacks: CFArrayCallBacks {
            version: 0, retain: null, release: null,
            copy_description: null, equal: null,
        },
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithCallbacks:(ConstPtr<CFArrayCallBacks>)callbacks {
    let callbacks = env.mem.read(callbacks);
    assert_eq!(callbacks.version, 0);
    env.objc.borrow_mut::<CFArrayHostObject>(this).callbacks = callbacks;
    this
}

- (id)copyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, false).unwrap() }
- (id)mutableCopyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, true).unwrap() }

- (())addObject:(id)object {
    let object = cf_array_retain(env, this, object);
    env.objc.borrow_mut::<ArrayHostObject>(this).array.push(object);
}

- (())insertObject:(id)object atIndex:(NSUInteger)index {
    let object = cf_array_retain(env, this, object);
    env.objc.borrow_mut::<ArrayHostObject>(this).array.insert(index as usize, object);
}

- (())replaceObjectAtIndex:(NSUInteger)index withObject:(id)object {
    let object = cf_array_retain(env, this, object);
    let old = std::mem::replace(&mut env.objc.borrow_mut::<ArrayHostObject>(this).array[index as usize], object);
    cf_array_release(env, this, old);
}

- (())removeObjectAtIndex:(NSUInteger)index {
    let old = env.objc.borrow_mut::<ArrayHostObject>(this).array.remove(index as usize);
    cf_array_release(env, this, old);
}

- (())removeLastObject {
    let old = env.objc.borrow_mut::<ArrayHostObject>(this).array.pop().unwrap();
    cf_array_release(env, this, old);
}

- (())removeAllObjects {
    let old = std::mem::take(&mut env.objc.borrow_mut::<ArrayHostObject>(this).array);
    for object in old {
        cf_array_release(env, this, object);
    }
}

- (())dealloc {
    let old = std::mem::take(&mut env.objc.borrow_mut::<ArrayHostObject>(this).array);
    for object in old {
        cf_array_release(env, this, object);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

// Special variant for use by CFArray with NULL callbacks: objects aren't
// necessarily Objective-C objects and won't be retained/released.
@implementation _touchHLE_NSMutableArray_non_retaining: _touchHLE_NSMutableArray

- (id)copyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, false).unwrap() }
- (id)mutableCopyWithZone:(NSZonePtr)_zone { cf_array_copy(env, this, true).unwrap() }

- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}

- (())addObject:(id)object {
    env.objc.borrow_mut::<ArrayHostObject>(this).array.push(object);
}

- (())insertObject:(id)object atIndex:(NSUInteger)index {
    env.objc.borrow_mut::<ArrayHostObject>(this).array.insert(index as usize, object);
}

- (())replaceObjectAtIndex:(NSUInteger)index withObject:(id)object {
    env.objc.borrow_mut::<ArrayHostObject>(this).array[index as usize] = object;
}

- (())removeAllObjects {
    env.objc.borrow_mut::<ArrayHostObject>(this).array.clear();
}

- (())removeObjectAtIndex:(NSUInteger)index {
    env.objc.borrow_mut::<ArrayHostObject>(this).array.remove(index as usize);
}

- (())removeLastObject {
    env.objc.borrow_mut::<ArrayHostObject>(this).array.pop().unwrap();
}

@end

};

/// Shortcut for host code, roughly equivalent to
/// `[[NSArray alloc] initWithObjects:count]` but without copying.
/// The elements should already be "retained by" the `Vec`.
pub fn from_vec(env: &mut Environment, objects: Vec<id>) -> id {
    let array: id = msg_class![env; NSArray alloc];
    env.objc.borrow_mut::<ArrayHostObject>(array).array = objects;
    array
}

/// Shortcut for host code, roughly equivalent to
/// `[[NSMutableArray alloc] initWithObjects:count]` but without copying.
/// The elements should already be "retained by" the `Vec`.
pub fn mutable_from_vec(env: &mut Environment, objects: Vec<id>) -> id {
    let array: id = msg_class![env; NSMutableArray alloc];
    env.objc.borrow_mut::<ArrayHostObject>(array).array = objects;
    array
}

/// A helper to build a description NSString
/// for a NSArray or a NSMutableArray.
fn build_description(env: &mut Environment, arr: id) -> id {
    // According to docs, this description should be formatted as property list.
    // But by the same docs, it's meant to be used for debugging purposes only.
    let desc: id = msg_class![env; NSMutableString new];
    let prefix: id = ns_string::from_rust_string(env, "(\n".to_string());
    () = msg![env; desc appendString:prefix];
    release(env, prefix);
    let values: Vec<id> = env.objc.borrow_mut::<ArrayHostObject>(arr).array.clone();
    for value in values {
        let value_desc: id = msg![env; value description];
        // TODO: respect nesting and padding
        let format = format!("\t{},\n", ns_string::to_rust_string(env, value_desc));
        let format = ns_string::from_rust_string(env, format);
        () = msg![env; desc appendString:format];
        release(env, format);
    }
    let suffix: id = ns_string::from_rust_string(env, ")".to_string());
    () = msg![env; desc appendString:suffix];
    release(env, suffix);
    let desc_imm = msg![env; desc copy];
    release(env, desc);
    autorelease(env, desc_imm)
}

/// A shared objectEnumerator helper method.
fn object_enumerator_inner(env: &mut Environment, arr: id) -> id {
    let array_host_object: &mut ArrayHostObject = env.objc.borrow_mut(arr);
    let vec = array_host_object.array.to_vec();
    object_enumerator_inner_helper(env, arr, vec)
}

/// A shared reverseObjectEnumerator helper method.
fn reverse_object_enumerator_inner(env: &mut Environment, arr: id) -> id {
    let array_host_object: &mut ArrayHostObject = env.objc.borrow_mut(arr);
    // TODO: avoid copying?
    let vec = array_host_object
        .array
        .iter()
        .rev()
        .cloned()
        .collect::<Vec<_>>();
    object_enumerator_inner_helper(env, arr, vec)
}

fn object_enumerator_inner_helper(env: &mut Environment, arr: id, vec: Vec<id>) -> id {
    let host_object = Box::new(ObjectEnumeratorHostObject {
        array: arr,
        iterator: vec.into_iter(),
    });
    retain(env, arr);
    let class = env
        .objc
        .get_known_class("_touchHLE_NSArray_ObjectEnumerator", &mut env.mem);
    let enumerator = env.objc.alloc_object(class, host_object, &mut env.mem);
    autorelease(env, enumerator)
}

fn mutable_copy_inner(env: &mut Environment, arr: id) -> id {
    let mut_arr: id = msg_class![env; NSMutableArray alloc];
    let array = env.objc.borrow::<ArrayHostObject>(arr).array.clone();
    for &object in &array {
        retain(env, object);
    }
    env.objc.borrow_mut::<ArrayHostObject>(mut_arr).array = array;
    mut_arr
}

fn init_with_coder_inner(env: &mut Environment, arr: id, coder: id) -> id {
    let class: Class = msg![env; coder class];
    let keyed_unarch_class: Class = msg_class![env; NSKeyedUnarchiver class];
    let nib_archive_class: Class = msg_class![env; _touchHLE_NIBArchiveDecoder class];
    // It seems that every NSArray item in an NSKeyedArchiver plist looks like:
    // {
    //   "$class" => (uid of NSArray class goes here),
    //   "NS.objects" => [
    //     // objects here
    //   ]
    // }
    // Presumably we need to call a `decodeFooBarForKey:` method on the NSCoder
    // here, passing in an NSString for "NS.objects". There is no method for
    // arrays though (maybe it's `decodeObjectForKey:`), and in any case
    // allocating an NSString here would be inconvenient, so let's just take a
    // shortcut.
    let objects = if env.objc.class_is_subclass_of(class, keyed_unarch_class) {
        ns_keyed_unarchiver::decode_current_array(env, coder)
    } else if env.objc.class_is_subclass_of(class, nib_archive_class) {
        _nib_archive_decoder::decode_current_array(env, coder)
    } else {
        unimplemented!()
    };

    let host_object: &mut ArrayHostObject = env.objc.borrow_mut(arr);
    assert!(host_object.array.is_empty());
    host_object.array = objects; // objects are already retained
    arr
}

fn encode_with_coder_inner(env: &mut Environment, arr: id, coder: id) {
    let host_obj: ArrayHostObject = std::mem::take(env.objc.borrow_mut(arr));
    let mut encoded_vals = vec![];
    for v in &host_obj.array {
        // TODO: support other type of coders, not only NSKeyedArchiver
        let vv = encode_object(env, coder, *v);
        encoded_vals.push(plist::Value::Uid(vv));
    }
    *env.objc.borrow_mut(arr) = host_obj;

    let scope = get_value_to_encode_for_current_key(env, coder);
    scope.insert("NS.objects".to_string(), plist::Value::Array(encoded_vals));
}

/// Helper for initWithObjects.
fn init_with_objects_inner(env: &mut Environment, arr: id, first_obj: id, args: DotDotDot) {
    retain(env, first_obj);
    let mut objects = vec![first_obj];
    let mut varargs = args.start();
    loop {
        let next_arg: id = varargs.next(env);
        if next_arg.is_null() {
            break;
        }
        retain(env, next_arg);
        objects.push(next_arg);
    }
    env.objc.borrow_mut::<ArrayHostObject>(arr).array = objects;
}
