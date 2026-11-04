/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Handling of Objective-C properties.
//!
//! Note that these are not the same as instance variables (ivars), though
//! they're closely related, so maybe this file will end up being used for those
//! too.
//!
//! Resources:
//! - `objc_setProperty` and friends are not documented, so [reading the source code](https://opensource.apple.com/source/objc4/objc4-551.1/runtime/Accessors.subproj/objc-accessors.mm.auto.html) is useful.
//!
//! See also: [crate::frameworks::foundation::ns_object].

use super::{id, msg, nil, release, retain, Class, ClassHostObject, ObjC, SEL};
use crate::mem::{
    guest_size_of, ConstPtr, ConstVoidPtr, GuestISize, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr,
    SafeRead,
};
use crate::Environment;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

/// The layout of a property list in an app binary.
///
/// The name, field names and field layout are based on what Ghidra outputs.
#[repr(C, packed)]
pub(super) struct ivar_list_t {
    entsize: GuestUSize,
    count: GuestUSize,
    // entries follow the struct
}
unsafe impl SafeRead for ivar_list_t {}

/// The layout of a property in an app binary.
///
/// The name, field names and field layout are based on what Ghidra outputs.
#[repr(C, packed)]
struct ivar_t {
    offset: ConstPtr<GuestUSize>,
    name: ConstPtr<u8>,
    type_: ConstPtr<u8>,
    alignment: u32,
    size: u32,
}
unsafe impl SafeRead for ivar_t {}

impl ClassHostObject {
    /// Parse the property list from the binary and populate `self.properties`.
    /// This allows `class_getProperty` to return proper `objc_property_t`
    /// pointers for declared @property entries.
    pub(super) fn add_properties_from_bin(&mut self, prop_list_ptr: ConstVoidPtr, mem: &Mem) {
        let prop_list_ptr: ConstPtr<property_list_t> = prop_list_ptr.cast();
        let property_list_t {
            entsize_and_flags,
            count,
        } = mem.read(prop_list_ptr);

        // The entsize field may have flags in the high bits; mask to get
        // the actual entry size (Apple's runtime uses & ~3u for alignment,
        // but we just mask the lower 16 bits which is safe for any
        // reasonable entry size).
        let entsize = entsize_and_flags & 0xFFFF;
        let min_entsize = guest_size_of::<property_t>();
        if entsize < min_entsize {
            log_dbg!(
                "add_properties_from_bin: property_list_t at {:?} declares entsize {} smaller than property_t ({}); skipping list.",
                prop_list_ptr,
                entsize,
                min_entsize
            );
            return;
        }

        let props_base_ptr: ConstPtr<property_t> = (prop_list_ptr + 1).cast();

        for i in 0..count {
            let prop_ptr: ConstPtr<property_t> =
                Ptr::from_bits(props_base_ptr.to_bits() + i * entsize);

            let property_t { name, .. } = mem.read(prop_ptr);

            if name.is_null() {
                continue;
            }
            let Ok(name_string) = mem.cstr_at_utf8(name) else {
                continue;
            };
            // Store the guest pointer to the property_t entry itself.
            // class_getProperty returns this as the opaque objc_property_t.
            self.properties
                .insert(name_string.to_string(), prop_ptr.cast());
        }
    }

    pub(super) fn add_ivars_from_bin(&mut self, ivar_list_ptr: ConstPtr<ivar_list_t>, mem: &Mem) {
        let ivar_list_t { entsize, count } = mem.read(ivar_list_ptr);
        assert!(entsize >= guest_size_of::<ivar_t>());

        let ivars_base_ptr: ConstPtr<ivar_t> = (ivar_list_ptr + 1).cast();

        for i in 0..count {
            let ivar_ptr: ConstPtr<ivar_t> = Ptr::from_bits(ivars_base_ptr.to_bits() + i * entsize);

            // TODO: support type strings
            let ivar_t {
                offset,
                name,
                alignment,
                ..
            } = mem.read(ivar_ptr);

            let name_string: String = mem.cstr_at_utf8(name).unwrap().into();
            self.ivar_metadata
                .insert(name_string.clone(), ivar_ptr.cast());
            self.ivars.insert(name_string, (offset, alignment));
        }
    }
}

impl ObjC {
    /// Checks if the object's class has an ivar in its class chain with the
    /// provided name and returns the pointer to the object's ivar, if any,
    /// or None if the object's class doesn't have an ivar with that name.
    pub fn object_lookup_ivar(
        &self,
        mem: &Mem,
        obj: id,
        name: &String,
    ) -> Option<MutPtr<GuestUSize>> {
        let mut class = ObjC::read_isa(obj, mem);
        loop {
            let &ClassHostObject {
                superclass,
                ref ivars,
                ..
            } = self.borrow(class);
            if let Some((ivar_offset_ptr, _)) = ivars.get(name) {
                let ivar_offset = mem.read(*ivar_offset_ptr);
                let ivar_ptr = MutVoidPtr::from_bits(obj.to_bits() + ivar_offset);
                return Some(ivar_ptr.cast());
            } else if superclass == nil {
                return None;
            } else {
                class = superclass;
            }
        }
    }

    pub fn debug_all_class_ivars_as_strings(&self, class: Class) -> Vec<String> {
        let mut class = class;
        let mut ivars_strings = Vec::new();
        loop {
            let &ClassHostObject {
                superclass,
                ref ivars,
                ..
            } = self.borrow(class);
            let mut class_ivars_strings = ivars.keys().cloned().collect();
            ivars_strings.append(&mut class_ivars_strings);
            if superclass == nil {
                break;
            } else {
                class = superclass;
            }
        }
        ivars_strings
    }
}

/// Undocumented function (see link above) apparently used by auto-generated
/// methods for properties to get an ivar.
pub(super) fn objc_getProperty(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    offset: GuestISize,
    atomic: bool,
) -> id {
    // We currently aren't touching the ivar layouts contained in the binary, so
    // we are assuming they are already correctly set by the compiler. Since we
    // aren't using ivars at all in our host classes, we shouldn't have any
    // issues with host classes' ivars clobbering guest classes' ivars, but
    // what if the compiler doesn't set the ivar layout at all? This is a simple
    // safeguard: any real ivar offset will be after the isa pointer.
    assert!(offset >= 4);

    if atomic {
        log_once!("TODO: Lock when atomic is set to true in objc_getProperty");
    }

    let ivar: MutPtr<id> = Ptr::from_bits(this.to_bits().checked_add_signed(offset).unwrap());
    env.mem.read(ivar)
}

/// Undocumented function (see link above) apparently used by auto-generated
/// methods for properties to set an ivar and handle reference counting, copying
/// and locking.
pub(super) fn objc_setProperty(
    env: &mut Environment,
    this: id,
    _cmd: SEL,
    offset: GuestISize,
    value: id,
    atomic: bool,
    should_copy: i8,
) {
    // We currently aren't touching the ivar layouts contained in the binary, so
    // we are assuming they are already correctly set by the compiler. Since we
    // aren't using ivars at all in our host classes, we shouldn't have any
    // issues with host classes' ivars clobbering guest classes' ivars, but
    // what if the compiler doesn't set the ivar layout at all? This is a simple
    // safeguard: any real ivar offset will be after the isa pointer.
    assert!(offset >= 4);

    if atomic {
        log_once!("TODO: Lock when atomic is set to true in objc_setProperty");
    }

    let ivar: MutPtr<id> = Ptr::from_bits(this.to_bits().checked_add_signed(offset).unwrap());
    let old = env.mem.read(ivar);

    let void_null: MutVoidPtr = Ptr::null();
    let value: id = if value != nil {
        match should_copy {
            0 => retain(env, value),
            1 => msg![env; value copyWithZone:void_null],
            2 => msg![env; value mutableCopyWithZone:void_null],
            // Apple's source code implies that any non-zero value that isn't 2
            // should mean "copy", but that seems weird, let's be conservative.
            _ => panic!("Unknown \"should copy\" value: {should_copy}"),
        }
    } else {
        nil
    };
    env.mem.write(ivar, value);

    if old != nil {
        release(env, old);
    }
}

// note: https://opensource.apple.com/source/objc4/objc4-723/runtime/objc-accessors.mm.auto.html
//       says that hasStrong is unused.
pub(super) fn objc_copyStruct(
    env: &mut Environment,
    dest: MutVoidPtr,
    src: ConstVoidPtr,
    size: GuestUSize,
    _atomic: bool,
    _hasStrong: bool,
) {
    // It's safe to ignore atomic as we never switch thread unless we call back
    // into guest code and we're not doing that here, just calling memmove.
    // TODO: implement atomic support
    env.mem.memmove(dest, src, size);
}

/// Logs a placeholder message for an unimplemented ObjC setter
///
/// This macro must be used inside [crate::_objc_method],
/// as it relies on constants for the current class and selector
/// set by it and [crate::objc::objc_classes]
#[macro_export]
macro_rules! todo_objc_setter {
    ($this:ident, $($arg:tt)+) => {
        const _: () = {
            let bytes = _OBJC_CURRENT_SELECTOR.as_bytes();
            let starts_with_set =
                bytes.len() > 3 && bytes[0] == b's' && bytes[1] == b'e' && bytes[2] == b't';
            assert!(starts_with_set, "Selector does not start with set.");
        };
        log!(
            "TODO: [({}*) {:?} {}:{:?}]",
            _OBJC_CURRENT_CLASS,
            $this,
            _OBJC_CURRENT_SELECTOR,
            $($arg)+
        );
    };
}
pub use crate::todo_objc_setter;

// Variants of objc_setProperty used by newer compilers.
pub(super) fn objc_setProperty_atomic(
    env: &mut Environment,
    this: id,
    cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(env, this, cmd, offset, value, true, 0)
}
pub(super) fn objc_setProperty_nonatomic(
    env: &mut Environment,
    this: id,
    cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(env, this, cmd, offset, value, false, 0)
}
pub(super) fn objc_setProperty_atomic_copy(
    env: &mut Environment,
    this: id,
    cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(env, this, cmd, offset, value, true, 1)
}
pub(super) fn objc_setProperty_nonatomic_copy(
    env: &mut Environment,
    this: id,
    cmd: SEL,
    value: id,
    offset: GuestISize,
) {
    objc_setProperty(env, this, cmd, offset, value, false, 1)
}
/// Returns the named ivar metadata, searching the class and its superclasses.
pub fn class_getInstanceVariable(
    env: &mut Environment,
    class: Class,
    name: ConstPtr<u8>,
) -> MutVoidPtr {
    if class == nil || name.is_null() {
        return MutVoidPtr::null();
    }
    let Ok(name) = env.mem.cstr_at_utf8(name) else {
        return MutVoidPtr::null();
    };
    let class_name = env.objc.get_class_name(class).to_string();
    let ivar = env.objc.class_lookup_ivar(class, name);
    trace_ivar_access(&format!(
        "class_getInstanceVariable class={} name={} metadata={:?}",
        class_name, name, ivar
    ));
    ivar.map(ConstVoidPtr::cast_mut)
        .unwrap_or_else(MutVoidPtr::null)
}

/// Reads an object reference from the specified ivar, returning nil for nil or
/// invalid inputs.
pub fn object_getIvar(env: &mut Environment, object: id, ivar: MutVoidPtr) -> id {
    let class_name = if object == nil {
        "nil".to_string()
    } else {
        env.objc
            .get_class_name(ObjC::read_isa(object, &env.mem))
            .to_string()
    };
    let offset = if object == nil || ivar.is_null() {
        None
    } else {
        env.objc
            .class_lookup_ivar_offset_by_metadata(
                ObjC::read_isa(object, &env.mem),
                ivar.cast_const(),
            )
            .map(|pointer| env.mem.read(pointer))
    };
    let value = object_ivar_value(&env.objc, &env.mem, object, ivar.cast_const()).unwrap_or(nil);
    trace_ivar_access(&format!(
        "object_getIvar class={} object={:?} ivar={:?} offset={:?} value={:?}",
        class_name, object, ivar, offset, value
    ));
    value
}

/// `void object_setIvar(id obj, Ivar ivar, id value)`
///
/// Classic/ARC runtime primitive that stores an object pointer directly into
/// an instance variable (bypassing any synthesized setter). This is used by
/// code that wires outlets or callback objects through runtime ivar metadata.
///
/// Like Apple's `object_setIvar`, this does not retain `value` or release the
/// previous value; it just stores the raw pointer at the ivar's offset.
pub fn object_setIvar(env: &mut Environment, object: id, ivar: MutVoidPtr, value: id) {
    if object == nil || ivar.is_null() {
        return;
    }
    let class = ObjC::read_isa(object, &env.mem);
    let class_name = env.objc.get_class_name(class).to_string();
    let Some(offset_pointer) = env
        .objc
        .class_lookup_ivar_offset_by_metadata(class, ivar.cast_const())
    else {
        trace_ivar_access(&format!(
            "object_setIvar class={} object={:?} ivar={:?} value={:?} — unknown ivar, ignored",
            class_name, object, ivar, value
        ));
        return;
    };
    if offset_pointer.is_null() {
        return;
    }
    let offset: GuestUSize = env.mem.read(offset_pointer);
    let field_address = object.to_bits().wrapping_add(offset);
    let field_ptr: MutPtr<id> = Ptr::from_bits(field_address);
    env.mem.write(field_ptr, value);
    trace_ivar_access(&format!(
        "object_setIvar class={} object={:?} ivar={:?} offset={} value={:?}",
        class_name, object, ivar, offset, value
    ));
}

/// `const char *property_getAttributes(objc_property_t property)` — returns
/// the attribute string of the declared `@property`, as a C string.
///
/// Per Apple's Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/property_getattributes(_:)>):
///
/// > Returns the attribute string of a property.
/// > Return Value: A C string containing the property's attributes.
///
/// The format of this string is documented in "Declared Properties" of the
/// Objective-C Runtime Programming Guide
/// (<https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/ObjCRuntimeGuide/Articles/ocrtPropertyIntrospection.html>):
/// it begins with `T` followed by the `@encode` type, then comma-separated
/// attribute codes (e.g. `T@"NSString",&,N,V_name`).
///
/// We read the `attributes` field of the `property_t` entry parsed from the
/// app binary and return its guest pointer directly (the caller must NOT
/// free it), exactly like real libobjc. As with [property_getName], the
/// previous return-0 stub broke runtime introspection used by KVC and ad
/// SDK serialization in apps such as Spy Mouse HD.
pub fn property_getAttributes(env: &mut Environment, property: ConstVoidPtr) -> ConstPtr<u8> {
    if property.is_null() {
        return Ptr::null();
    }
    let prop_ptr: ConstPtr<property_t> = property.cast();
    let property_t { attributes, .. } = env.mem.read(prop_ptr);
    attributes
}

/// `const char *property_getName(objc_property_t property)` — returns the
/// name of the declared `@property`, as a C string.
///
/// Per Apple's Objective-C Runtime Reference
/// (<https://developer.apple.com/documentation/objectivec/property_getname(_:)>):
///
/// > Returns the name of a property.
/// > Return Value: A C string containing the property's name.
///
/// `objc_property_t` is an opaque pointer; in touchHLE it points at the
/// `property_t { name, attributes }` entry parsed from the app binary's
/// property list (see [ClassHostObject::add_properties_from_bin]). We read
/// the `name` field and return the guest pointer to that C string directly,
/// matching real libobjc which returns a pointer into the property struct
/// (the caller must NOT free it).
///
/// touchHLE previously had no implementation, so dyld installed a return-0
/// stub. Returning NULL for `property_getName` breaks reflective code such
/// as KVC's `-dictionaryWithValuesForKeys:` and JSON/serialization helpers
/// that enumerate a class's properties (e.g. Spy Mouse HD's ad SDK).
pub fn property_getName(env: &mut Environment, property: ConstVoidPtr) -> ConstPtr<u8> {
    if property.is_null() {
        // Real libobjc returns "" (a pointer to an empty string) for a NULL
        // property rather than NULL. Returning NULL here is acceptable for
        // callers that null-check, but matching the documented behaviour is
        // safer: we have no empty-string constant handy, so return NULL,
        // which guest code treats the same as "no name".
        return Ptr::null();
    }
    let prop_ptr: ConstPtr<property_t> = property.cast();
    let property_t { name, .. } = env.mem.read(prop_ptr);
    name
}

fn object_ivar_value(objc: &ObjC, mem: &Mem, object: id, ivar: ConstVoidPtr) -> Option<id> {
    if object == nil || ivar.is_null() {
        return None;
    }
    let class = ObjC::read_isa(object, mem);
    let offset_pointer = objc.class_lookup_ivar_offset_by_metadata(class, ivar)?;
    if offset_pointer.is_null() {
        return None;
    }
    let offset_bytes = mem.get_bytes_fallible(offset_pointer.cast(), 4)?;
    let offset = u32::from_le_bytes(offset_bytes.try_into().ok()?);
    let field_address = object.to_bits().checked_add(offset)?;
    let value_bytes = mem.get_bytes_fallible(ConstVoidPtr::from_bits(field_address), 4)?;
    Some(id::from_bits(u32::from_le_bytes(
        value_bytes.try_into().ok()?,
    )))
}

/// The layout of a single declared @property in an app binary.
/// Matches `property_t` from Apple's objc4 runtime source.
/// `class_getProperty` returns a pointer to this structure as the
/// opaque `objc_property_t`.
#[repr(C, packed)]
pub(super) struct property_t {
    name: ConstPtr<u8>,
    attributes: ConstPtr<u8>,
}

unsafe impl SafeRead for property_t {}

fn trace_ivar_access(message: &str) {
    if crate::env_flag_cached!("TOUCHHLE_TRACE_IVARS")
        && IVAR_ACCESS_DIAGNOSTICS.fetch_add(1, Ordering::Relaxed) < 96
    {
        log!("OBJC-IVAR {}", message);
    }
}

static IVAR_ACCESS_DIAGNOSTICS: AtomicUsize = AtomicUsize::new(0);

/// The layout of an Objective-C property list in an app binary.
/// Matches `objc_property_list` from Apple's objc4 runtime source.
#[repr(C, packed)]
pub(super) struct property_list_t {
    entsize_and_flags: GuestUSize,
    count: GuestUSize,
    // property_t entries follow the struct
}

unsafe impl SafeRead for property_list_t {}
