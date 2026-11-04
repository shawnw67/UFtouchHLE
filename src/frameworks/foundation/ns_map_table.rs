/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSMapTable`, Foundation's mutable object-to-object map.
//!
//! `NSMapTable` is commonly used by analytics and game middleware because it
//! can be configured with weak or pointer-personality keys. The guest runtime
//! does not currently expose zeroing weak references or custom pointer
//! functions, so options deliberately degrade to the regular strong object
//! semantics used by `NSMutableDictionary`. This still preserves the API's
//! important observable behavior: a non-nil mutable map with Objective-C
//! `hash`/`isEqual:` key lookup and balanced ownership of stored objects.

use super::ns_dictionary::DictionaryHostObject;
use super::ns_enumerator::{fast_enumeration_helper, NSFastEnumerationState};
use super::NSUInteger;
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, retain, ClassExports, HostObject, NSZonePtr,
};

/// `NSPointerFunctionsOptions`. The options are recorded for introspection but
/// use the strong object-personality representation described above.
pub type NSMapTableOptions = NSUInteger;

#[derive(Debug, Default)]
struct MapTableHostObject {
    dict: DictionaryHostObject,
    key_options: NSMapTableOptions,
    value_options: NSMapTableOptions,
}
impl HostObject for MapTableHostObject {}

fn all_keys(env: &mut crate::Environment, this: id) -> id {
    let host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    let keys: Vec<id> = host.dict.iter_keys().collect();
    *env.objc.borrow_mut(this) = host;
    for &key in &keys {
        retain(env, key);
    }
    let array = crate::frameworks::foundation::ns_array::from_vec(env, keys);
    autorelease(env, array)
}

fn all_values(env: &mut crate::Environment, this: id) -> id {
    let host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    let values: Vec<id> = host
        .dict
        .map
        .values()
        .flatten()
        .map(|&(_key, value)| value)
        .collect();
    *env.objc.borrow_mut(this) = host;
    for &value in &values {
        retain(env, value);
    }
    let array = crate::frameworks::foundation::ns_array::from_vec(env, values);
    autorelease(env, array)
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSMapTable: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<MapTableHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)mapTableWithKeyOptions:(NSMapTableOptions)key_options
                valueOptions:(NSMapTableOptions)value_options {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithKeyOptions:key_options
                                       valueOptions:value_options
                                           capacity:0u32];
    autorelease(env, new)
}

+ (id)strongToStrongObjectsMapTable {
    msg![env; this mapTableWithKeyOptions:0u32 valueOptions:0u32]
}

+ (id)weakToStrongObjectsMapTable {
    // NSPointerFunctionsWeakMemory is 5. Weak storage degrades to strong
    // storage until the guest Objective-C runtime has zeroing weak references.
    msg![env; this mapTableWithKeyOptions:5u32 valueOptions:0u32]
}

+ (id)strongToWeakObjectsMapTable {
    msg![env; this mapTableWithKeyOptions:0u32 valueOptions:5u32]
}

+ (id)weakToWeakObjectsMapTable {
    msg![env; this mapTableWithKeyOptions:5u32 valueOptions:5u32]
}

- (id)init {
    msg![env; this initWithKeyOptions:0u32 valueOptions:0u32 capacity:0u32]
}

- (id)initWithKeyOptions:(NSMapTableOptions)key_options
            valueOptions:(NSMapTableOptions)value_options
                capacity:(NSUInteger)_capacity {
    let host = env.objc.borrow_mut::<MapTableHostObject>(this);
    host.key_options = key_options;
    host.value_options = value_options;
    this
}

- (())dealloc {
    let mut host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    host.dict.release(env);
    env.objc.dealloc_object(this, &mut env.mem)
}

// MARK: - Querying

- (NSUInteger)count {
    env.objc.borrow::<MapTableHostObject>(this).dict.count
}

- (id)objectForKey:(id)key {
    if key == nil {
        return nil;
    }
    let host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    let result = host.dict.lookup(env, key);
    *env.objc.borrow_mut(this) = host;
    result
}

- (id)keyEnumerator {
    let keys = all_keys(env, this);
    msg![env; keys objectEnumerator]
}

- (id)objectEnumerator {
    let values = all_values(env, this);
    msg![env; values objectEnumerator]
}

- (id)dictionaryRepresentation {
    let host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    let pairs: Vec<(id, id)> = host
        .dict
        .map
        .values()
        .flatten()
        .copied()
        .collect();
    *env.objc.borrow_mut(this) = host;

    let dictionary: id = msg_class![env; NSMutableDictionary dictionary];
    for (key, value) in pairs {
        () = msg![env; dictionary setObject:value forKey:key];
    }
    let result: id = msg![env; dictionary copy];
    autorelease(env, result)
}

- (NSUInteger)countByEnumeratingWithState:(MutPtr<NSFastEnumerationState>)state
                                  objects:(MutPtr<id>)stackbuf
                                    count:(NSUInteger)len {
    let keys = all_keys(env, this);
    let count: NSUInteger = msg![env; keys count];
    fast_enumeration_helper(env, this, |env, index| {
        if index < count {
            msg![env; keys objectAtIndex:index]
        } else {
            nil
        }
    }, state, stackbuf, len)
}

// MARK: - Mutation

- (())setObject:(id)object forKey:(id)key {
    if object == nil || key == nil {
        return;
    }
    let mut host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    host.dict.insert(env, key, object, /* copy_key: */ false);
    *env.objc.borrow_mut(this) = host;
}

- (())removeObjectForKey:(id)key {
    if key == nil {
        return;
    }
    let mut host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    host.dict.remove(env, key);
    *env.objc.borrow_mut(this) = host;
}

- (())removeAllObjects {
    let mut host: MapTableHostObject = std::mem::take(env.objc.borrow_mut(this));
    host.dict.release(env);
    *env.objc.borrow_mut(this) = MapTableHostObject {
        dict: DictionaryHostObject::default(),
        key_options: host.key_options,
        value_options: host.value_options,
    };
}

@end

};
