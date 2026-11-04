//! Things from `NSObjCRuntime.h`.

use super::ns_string;
use crate::dyld::{export_c_func, FunctionExports};
use crate::objc::protocol_getName;
use crate::objc::{id, nil, Class, SEL};
use crate::Environment;

fn NSStringFromSelector(env: &mut Environment, selector: SEL) -> id {
    // TODO: caching?
    let string = selector.as_str(&env.mem).to_string();
    ns_string::from_rust_string(env, string)
}

fn NSSelectorFromString(env: &mut Environment, string: id) -> SEL {
    // TODO: avoid copy?
    let string = ns_string::to_rust_string(env, string);

    env.objc.register_host_selector(string.into(), &mut env.mem)
}

pub fn NSStringFromClass(env: &mut Environment, class: Class) -> id {
    if class.is_null() {
        return nil;
    }
    // TODO: caching?
    let string = env.objc.get_class_name(class).to_string();
    ns_string::from_rust_string(env, string)
}

fn NSClassFromString(env: &mut Environment, string: id) -> Class {
    if string == nil {
        return nil;
    }
    // TODO: avoid copy?
    let string = ns_string::to_rust_string(env, string);

    // AdSupport is not provided. P2G probes this optional framework on iOS 7.
    if string == "ASIdentifierManager" {
        return nil;
    }

    // While this method is supposed to return nil if the class is not found,
    // touchHLE is missing many classes that apps might expect to be present,
    // so this could be troublesome. So, let's use get_known_class, which panics
    // when it can't find the class. We could except certain classes or apps if
    // we need to.
    env.objc.get_known_class(&string, &mut env.mem)
}

/// `Protocol *NSProtocolFromString(NSString *name)`
///
/// Foundation wrapper around `objc_getProtocol` from the Objective-C
/// runtime. Looks up a protocol by name.
/// <https://developer.apple.com/documentation/foundation/1395208-nsprotocolfromstring>
pub fn NSProtocolFromString(env: &mut Environment, name: id) -> id {
    if name == nil {
        return nil;
    }
    let s = ns_string::to_rust_string(env, name).into_owned();
    let c_string = std::ffi::CString::new(s).unwrap_or_default();
    let bytes = c_string.as_bytes_with_nul();
    let buf = env.mem.alloc(bytes.len() as u32);
    env.mem
        .bytes_at_mut(buf.cast(), bytes.len() as u32)
        .copy_from_slice(bytes);
    let result = crate::objc::objc_getProtocol(env, buf.cast_const().cast());
    env.mem.free(buf);
    result
}

/// `NSString *NSStringFromProtocol(Protocol *proto)`
///
/// Apple's `NSObjCRuntime.h` documents this as a Foundation wrapper
/// around the Objective-C runtime call `protocol_getName(Protocol *)`
/// that returns the protocol's name as an `NSString`.
/// <https://developer.apple.com/documentation/foundation/1395294-nsstringfromprotocol>
///
/// touchHLE does not currently materialise real `Protocol *` instances
/// (see `objc_getProtocol` in `objc/classes.rs`), so the runtime side
/// returns NULL for the name lookup. We forward that, returning `nil`
/// to match the contract apps expect for an unknown protocol pointer.
pub fn NSStringFromProtocol(env: &mut Environment, proto: id) -> id {
    if proto.is_null() {
        return nil;
    }
    let name_ptr = protocol_getName(env, proto);
    if name_ptr.is_null() {
        return nil;
    }
    let Ok(name) = env.mem.cstr_at_utf8(name_ptr) else {
        return nil;
    };
    let owned = name.to_string();
    ns_string::from_rust_string(env, owned)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(NSStringFromSelector(_)),
    export_c_func!(NSSelectorFromString(_)),
    export_c_func!(NSClassFromString(_)),
    export_c_func!(NSStringFromClass(_)),
    export_c_func!(NSProtocolFromString(_)),
    export_c_func!(NSStringFromProtocol(_)),
];
