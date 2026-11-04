/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Objective-C runtime.
//!
//! Apple's [Programming with Objective-C](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/ProgrammingWithObjectiveC/Introduction/Introduction.html)
//! is a useful introduction to the language from a user's perspective.
//! There are further resources in the child modules of this module, but they
//! are more implementation-specific.
//!
//! The strategy for this emulator will be to provide our own implementations of
//! an Objective-C runtime and libraries for it (Foundation etc). These
//! implementations will be "host code": Rust code forming part of the emulator,
//! not emulated code. The runtime will need to be able to handle classes that
//! originate from the guest app, classes defined by the host, and sometimes
//! classes that are both (considering Objective-C's support for inheritance,
//! categories and dynamic class editing).

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant, HostDylib};
use crate::objc::classes::___objc_personality_v0;
use crate::objc::classes::imp_implementationWithBlock;
use crate::objc::messages::ThreadInitializer;
use crate::{Environment, MutexId};
use classes::class_addMethod;
use classes::class_conformsToProtocol;
use classes::class_copyIvarList;
use classes::class_copyMethodList;
use classes::class_copyPropertyList;
use classes::class_copyProtocolList;
use classes::class_getClassMethod;
use classes::class_getInstanceMethod;
use classes::class_getMethodImplementation_stret;
use classes::class_getName;
use classes::class_isMetaClass;
use classes::class_respondsToSelector;
use classes::class_setSuperclass;
use classes::method_copyArgumentType;
use classes::method_copyReturnType;
use classes::method_exchangeImplementations;
use classes::method_getArgumentType;
use classes::method_getImplementation;
use classes::method_getName;
use classes::method_getNumberOfArguments;
use classes::method_getReturnType;
use classes::method_getTypeEncoding;
use classes::method_setImplementation;
use classes::objc_alloc;
use classes::objc_allocWithZone;
use classes::objc_allocateClassPair;
use classes::objc_autorelease;
use classes::objc_autoreleaseReturnValue;
use classes::objc_begin_catch;
use classes::objc_copyClassList;
use classes::objc_copyClassNamesForImage;
use classes::objc_copyImageNames;
use classes::objc_disposeClassPair;
use classes::objc_end_catch;
use classes::objc_getClassList;
use classes::objc_getMetaClass;
pub use classes::objc_getProtocol;
use classes::objc_getRequiredClass;
use classes::objc_lookUpClass;
use classes::objc_readClassPair;
use classes::objc_registerClassPair;
use classes::objc_release;
use classes::objc_retain;
use classes::objc_retainAutorelease;
use classes::objc_retainAutoreleaseReturnValue;
use classes::objc_retainAutoreleasedReturnValue;
pub use classes::objc_retainBlock;
use classes::objc_storeStrong;
use classes::objc_unsafeClaimAutoreleasedReturnValue;
use classes::object_getClassName;
use classes::object_getIndexedIvars;
use classes::protocol_conformsToProtocol;
use classes::protocol_getMethodDescription;
pub use classes::protocol_getName;
use classes::swift_getInitializedObjCClass;
use messages::objc_msgSendSuper2_stret;
use properties::class_getInstanceVariable;
use properties::object_getIvar;
use properties::object_setIvar;
use properties::property_getAttributes;
use properties::property_getName;
use selectors::sel_getName;
use selectors::sel_getUid;
use selectors::sel_isEqual;

mod classes;
mod messages;
mod methods;
mod objects;
mod properties;
mod selectors;
mod synchronization;
mod type_encoding;
mod weak;

pub use classes::{objc_classes, Class, ClassExports, ClassTemplate};
pub use messages::{
    autorelease, msg, msg_class, msg_send, msg_send_no_initialize, msg_send_no_type_checking,
    msg_send_super2, msg_super, objc_super, release, retain, TERMINATING,
};
pub use methods::{HostIMP, IMP};
pub use objects::{
    id, impl_HostObject_with_superclass, nil, AnyHostObject, HostObject, TrivialHostObject,
};
pub use properties::todo_objc_setter;
pub use selectors::{selector, SEL};

use classes::{
    class_getInstanceSize, class_getMethodImplementation, class_getProperty, class_getSuperclass,
    class_replaceMethod, objc_getClass, ClassHostObject, FakeClass, UnimplementedClass,
};
pub(crate) use messages::objc_msgSend;
use messages::{objc_msgSendSuper2, objc_msgSend_stret, MsgSendSignature, MsgSendSuperSignature};
use methods::method_list_t;
use objects::{objc_object, object_getClass, HostObjectEntry};
use properties::{
    ivar_list_t, objc_copyStruct, objc_getProperty, objc_setProperty, objc_setProperty_atomic,
    objc_setProperty_atomic_copy, objc_setProperty_nonatomic, objc_setProperty_nonatomic_copy,
};
use selectors::sel_registerName;
use synchronization::{objc_sync_enter, objc_sync_exit};

/// Typedef for `NSZone *`. This is a [fossil type] found in the signature of
/// `allocWithZone:` and similar methods. Its value is always ignored.
///
/// [fossil type]: https://en.wiktionary.org/wiki/fossil_word
pub type NSZonePtr = crate::mem::MutVoidPtr;

/// Main type holding Objective-C runtime state.
pub struct ObjC {
    /// Known selectors (interned method name strings).
    selectors: crate::fast_hash::FastHashMap<String, SEL>,

    /// Mapping of known (guest) object pointers to their host objects.
    ///
    /// If an object isn't in this map, we will consider it not to exist.
    objects: crate::fast_hash::FastHashMap<id, HostObjectEntry>,

    /// Known classes.
    ///
    /// Look at the `isa` to get the metaclass for a class.
    classes: crate::fast_hash::FastHashMap<String, Class>,

    /// Mutexes used in @synchronized blocks (objc_sync_enter/exit).
    sync_mutexes: crate::fast_hash::FastHashMap<id, MutexId>,

    /// Mutexes for running the +initialize function.
    initializer_threads: crate::fast_hash::FastHashMap<id, ThreadInitializer>,

    /// Temporary storage for optional type information when sending a message.
    /// Type information isn't part of the `objc_msgSend` ABI, so an alternative
    /// channel is needed.
    message_type_info: Option<(std::any::TypeId, &'static str)>,

    /// Weak references to each object (see the `weak` module).
    pub(crate) weak_refs: crate::fast_hash::FastHashMap<id, Vec<crate::mem::MutPtr<id>>>,

    /// Associated objects, keyed by object and key. The bool says whether the
    /// value is retained.
    pub(crate) associated_objects:
        crate::fast_hash::FastHashMap<(id, crate::mem::GuestUSize), (id, bool)>,

    /// Stable guest handles for `Method`s made by the runtime API.
    pub(super) method_handles: crate::fast_hash::FastHashMap<(Class, SEL), crate::mem::MutVoidPtr>,

    /// Guest tokens for [methods::IMP]s of host methods.
    pub(super) imp_tokens: crate::fast_hash::FastHashMap<crate::mem::GuestUSize, methods::IMP>,

    /// Reverse of [Self::imp_tokens], keyed by host function address.
    pub(super) host_imp_tokens: crate::fast_hash::FastHashMap<usize, crate::mem::MutVoidPtr>,
}

impl ObjC {
    /// Returns the name of a selector, panicking if it is unknown.
    pub fn get_selector_name(&self, sel: SEL) -> &str {
        self.selectors
            .iter()
            .find(|(_k, v)| **v == sel)
            .map(|(k, _v)| k.as_str())
            .expect("get_selector_name: unknown selector")
    }

    pub fn new() -> ObjC {
        ObjC {
            selectors: Default::default(),
            objects: Default::default(),
            classes: Default::default(),
            sync_mutexes: Default::default(),
            initializer_threads: Default::default(),
            message_type_info: None,
            weak_refs: Default::default(),
            associated_objects: Default::default(),
            method_handles: Default::default(),
            imp_tokens: Default::default(),
            host_imp_tokens: Default::default(),
        }
    }
}

pub const DYLIB: HostDylib = HostDylib {
    path: "/usr/lib/libobjc.A.dylib",
    aliases: &["/usr/lib/libobjc.dylib"],
    class_exports: &[],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS, weak::FUNCTIONS],
};

const CONSTANTS: ConstantExports = &[
    // We don't use these in our Objective-C runtime, but exporting useless
    // symbols for these silences the warning about the unhandled relocation,
    // and avoids a linker error for the integration tests.
    ("__objc_empty_vtable", HostConstant::NullPtr),
    ("__objc_empty_cache", HostConstant::NullPtr),
    // Used by the unwinding tables of code that catches Objective-C
    // exceptions. touchHLE doesn't support unwinding through guest code, so
    // these are only needed for linking.
    ("___objc_personality_v0", HostConstant::NullPtr),
    ("_OBJC_EHTYPE_$_NSException", HostConstant::NullPtr),
];

/// Called by `for (x in y)` loops if the collection changes while looping.
fn objc_enumerationMutation(env: &mut Environment, object: id) {
    let class: Class = msg![env; object class];
    panic!(
        "Collection {:?} (class {:?}) was mutated while being enumerated",
        object,
        env.objc.get_class_name(class)
    );
}

/// `@throw`. touchHLE can't unwind guest code to a `@catch` block, so throwing
/// an exception ends the app, with the exception's details.
fn objc_exception_throw(env: &mut Environment, exception: id) {
    let class: Class = msg![env; exception class];
    let class_name = env.objc.get_class_name(class).to_string();
    let description: id = msg![env; exception description];
    let description = if description == nil {
        "(nil)".to_string()
    } else {
        crate::frameworks::foundation::ns_string::to_rust_string(env, description).to_string()
    };
    panic!("Objective-C exception thrown: {class_name} {exception:?}: {description}");
}
fn objc_exception_rethrow(_env: &mut Environment) {
    panic!("Objective-C exception rethrown");
}

const FUNCTIONS: FunctionExports = &[
    export_c_func!(class_getInstanceSize(_)),
    export_c_func!(class_getSuperclass(_)),
    export_c_func!(class_getProperty(_, _)),
    export_c_func!(class_getMethodImplementation(_, _)),
    export_c_func!(class_replaceMethod(_, _, _, _)),
    export_c_func!(objc_msgSend(_, _)),
    export_c_func!(objc_msgSend_stret(_, _, _)),
    export_c_func!(objc_msgSendSuper2(_, _)),
    export_c_func!(objc_getClass(_)),
    export_c_func!(objc_getProperty(_, _, _, _)),
    export_c_func!(objc_setProperty(_, _, _, _, _, _)),
    export_c_func!(objc_setProperty_atomic(_, _, _, _)),
    export_c_func!(objc_setProperty_nonatomic(_, _, _, _)),
    export_c_func!(objc_setProperty_atomic_copy(_, _, _, _)),
    export_c_func!(objc_setProperty_nonatomic_copy(_, _, _, _)),
    export_c_func!(objc_copyStruct(_, _, _, _, _)),
    export_c_func!(objc_enumerationMutation(_)),
    export_c_func!(objc_exception_throw(_)),
    export_c_func!(objc_exception_rethrow()),
    export_c_func!(objc_sync_enter(_)),
    export_c_func!(objc_sync_exit(_)),
    export_c_func!(object_getClass(_)),
    export_c_func!(sel_registerName(_)),
    export_c_func!(___objc_personality_v0(_, _, _, _, _)),
    export_c_func!(class_addMethod(_, _, _, _)),
    export_c_func!(class_conformsToProtocol(_, _)),
    export_c_func!(class_copyIvarList(_, _)),
    export_c_func!(class_copyMethodList(_, _)),
    export_c_func!(class_copyPropertyList(_, _)),
    export_c_func!(class_copyProtocolList(_, _)),
    export_c_func!(class_getClassMethod(_, _)),
    export_c_func!(class_getInstanceMethod(_, _)),
    export_c_func!(class_getInstanceVariable(_, _)),
    export_c_func!(class_getMethodImplementation_stret(_, _)),
    export_c_func!(class_getName(_)),
    export_c_func!(class_isMetaClass(_)),
    export_c_func!(class_respondsToSelector(_, _)),
    export_c_func!(class_setSuperclass(_, _)),
    export_c_func!(imp_implementationWithBlock(_)),
    export_c_func!(method_copyArgumentType(_, _)),
    export_c_func!(method_copyReturnType(_)),
    export_c_func!(method_exchangeImplementations(_, _)),
    export_c_func!(method_getArgumentType(_, _, _, _)),
    export_c_func!(method_getImplementation(_)),
    export_c_func!(method_getName(_)),
    export_c_func!(method_getNumberOfArguments(_)),
    export_c_func!(method_getReturnType(_, _, _)),
    export_c_func!(method_getTypeEncoding(_)),
    export_c_func!(method_setImplementation(_, _)),
    export_c_func!(objc_alloc(_)),
    export_c_func!(objc_allocWithZone(_, _)),
    export_c_func!(objc_allocateClassPair(_, _, _)),
    export_c_func!(objc_autorelease(_)),
    export_c_func!(objc_autoreleaseReturnValue(_)),
    export_c_func!(objc_begin_catch(_)),
    export_c_func!(objc_copyClassList(_)),
    export_c_func!(objc_copyClassNamesForImage(_, _)),
    export_c_func!(objc_copyImageNames(_)),
    export_c_func!(objc_disposeClassPair(_)),
    export_c_func!(objc_end_catch(_)),
    export_c_func!(objc_getAssociatedObject(_, _)),
    export_c_func!(objc_getClassList(_, _)),
    export_c_func!(objc_getMetaClass(_, _)),
    export_c_func!(objc_getProtocol(_)),
    export_c_func!(objc_getRequiredClass(_)),
    export_c_func!(objc_lookUpClass(_)),
    export_c_func!(objc_msgSendSuper2_stret(_, _)),
    export_c_func!(objc_readClassPair(_, _)),
    export_c_func!(objc_registerClassPair(_)),
    export_c_func!(objc_release(_)),
    export_c_func!(objc_removeAssociatedObjects(_)),
    export_c_func!(objc_retain(_)),
    export_c_func!(objc_retainAutorelease(_)),
    export_c_func!(objc_retainAutoreleaseReturnValue(_)),
    export_c_func!(objc_retainAutoreleasedReturnValue(_)),
    export_c_func!(objc_retainBlock(_)),
    export_c_func!(objc_setAssociatedObject(_, _, _, _)),
    export_c_func!(objc_storeStrong(_, _)),
    export_c_func!(objc_unsafeClaimAutoreleasedReturnValue(_)),
    export_c_func!(object_getClassName(_)),
    export_c_func!(object_getIndexedIvars(_)),
    export_c_func!(object_getIvar(_, _)),
    export_c_func!(object_setIvar(_, _, _)),
    export_c_func!(property_getAttributes(_)),
    export_c_func!(property_getName(_)),
    export_c_func!(protocol_conformsToProtocol(_, _)),
    export_c_func!(protocol_getMethodDescription(_, _, _, _)),
    export_c_func!(protocol_getName(_)),
    export_c_func!(sel_getName(_)),
    export_c_func!(sel_getUid(_)),
    export_c_func!(sel_isEqual(_, _)),
    export_c_func!(swift_getInitializedObjCClass(_)),
];
/// `id objc_getAssociatedObject(id object, const void *key)`
///
/// Returns the value associated with a given object for a given key.
/// <https://developer.apple.com/documentation/objectivec/1418625-objc_getassociatedobject>
fn objc_getAssociatedObject(
    env: &mut Environment,
    object: id,
    key: crate::mem::ConstVoidPtr,
) -> id {
    if object.is_null() {
        return nil;
    }
    env.objc
        .associated_objects
        .get(&(object, key.to_bits()))
        .map(|(val, _)| *val)
        .unwrap_or(nil)
}

/// `void objc_removeAssociatedObjects(id object)`
///
/// Removes all associations for a given object.
/// <https://developer.apple.com/documentation/objectivec/1418718-objc_removeassociatedobjects>
fn objc_removeAssociatedObjects(env: &mut Environment, object: id) {
    if object.is_null() {
        return;
    }
    // Collect keys and values to avoid borrowing issues.
    let to_remove: Vec<((id, crate::mem::GuestUSize), (id, bool))> = env
        .objc
        .associated_objects
        .iter()
        .filter(|((obj, _), _)| *obj == object)
        .map(|(k, v)| (*k, *v))
        .collect();

    for (k, (val, was_retained)) in to_remove {
        env.objc.associated_objects.remove(&k);
        if was_retained && !val.is_null() {
            release(env, val);
        }
    }
}

/// `void objc_setAssociatedObject(id object, const void *key, id value,
///                                 objc_AssociationPolicy policy)`
///
/// Sets an associated value for a key on an object.
/// <https://developer.apple.com/documentation/objectivec/1418509-objc_setassociatedobject>
fn objc_setAssociatedObject(
    env: &mut Environment,
    object: id,
    key: crate::mem::ConstVoidPtr,
    value: id,
    policy: ObjcAssociationPolicy,
) {
    if object.is_null() {
        log_dbg!("objc_setAssociatedObject: object is nil, ignoring");
        return;
    }

    let map_key = (object, key.to_bits());

    // Release any old retained value for this slot.
    if let Some((old_val, was_retained)) = env.objc.associated_objects.remove(&map_key) {
        if was_retained && !old_val.is_null() {
            release(env, old_val);
        }
    }

    // A nil value means "remove the association" — we already removed it
    // above.
    if value.is_null() {
        return;
    }

    let is_retained = match policy {
        OBJC_ASSOCIATION_RETAIN_NONATOMIC | OBJC_ASSOCIATION_RETAIN => {
            retain(env, value);
            true
        }
        OBJC_ASSOCIATION_COPY_NONATOMIC | OBJC_ASSOCIATION_COPY => {
            // Send `-copy` to get an independent copy, then retain is implicit
            // because the copy call already returns a +1 object.
            let copied: id = msg![env; value copy];
            env.objc.associated_objects.insert(map_key, (copied, true));
            return;
        }
        // OBJC_ASSOCIATION_ASSIGN (0) and any unknown policy: weak storage.
        _ => false,
    };

    env.objc
        .associated_objects
        .insert(map_key, (value, is_retained));
}

/// 0o1403 (octal) = 771 decimal
const OBJC_ASSOCIATION_COPY: ObjcAssociationPolicy = 0o1403;

const OBJC_ASSOCIATION_COPY_NONATOMIC: ObjcAssociationPolicy = 3;

/// 0o1401 (octal) = 769 decimal
const OBJC_ASSOCIATION_RETAIN: ObjcAssociationPolicy = 0o1401;

const OBJC_ASSOCIATION_RETAIN_NONATOMIC: ObjcAssociationPolicy = 1;

/// Policy constants for `objc_setAssociatedObject`.
/// Source: Apple `<objc/runtime.h>`, `objc_AssociationPolicy` enum.
type ObjcAssociationPolicy = crate::mem::GuestUSize;
