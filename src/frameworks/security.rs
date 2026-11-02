/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Security framework.
//!
//! The keychain is currently empty: adding items succeeds but nothing is
//! stored. Security random bytes are supplied by the host operating system.

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant, HostDylib};
use crate::frameworks::core_foundation::CFTypeRef;
use crate::mem::{GuestUSize, MutPtr};
use crate::objc::nil;
use crate::Environment;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/Security.framework/Security",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};

type OSStatus = i32;
const errSecSuccess: OSStatus = 0;
const errSecItemNotFound: OSStatus = -25300;
const errSecParam: OSStatus = -50;

/// `CFDictionaryRef`
type CFDictionaryRef = CFTypeRef;

fn fill_random_with<E>(bytes: &mut [u8], fill: impl FnOnce(&mut [u8]) -> Result<(), E>) -> i32 {
    match fill(bytes) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

fn SecRandomCopyBytes(
    env: &mut Environment,
    _random: CFTypeRef,
    count: GuestUSize,
    bytes: MutPtr<u8>,
) -> i32 {
    if count == 0 {
        return 0;
    }
    if bytes.is_null() {
        return errSecParam;
    }
    fill_random_with(env.mem.bytes_at_mut(bytes, count), getrandom::getrandom)
}

fn SecItemAdd(
    _env: &mut Environment,
    attributes: CFDictionaryRef,
    result: MutPtr<CFTypeRef>,
) -> OSStatus {
    log!(
        "TODO: SecItemAdd({:?}, {:?}): not storing anything",
        attributes,
        result
    );
    errSecSuccess
}

fn SecItemCopyMatching(
    env: &mut Environment,
    query: CFDictionaryRef,
    result: MutPtr<CFTypeRef>,
) -> OSStatus {
    log!(
        "SecItemCopyMatching({:?}, {:?}): the keychain is empty",
        query,
        result
    );
    if !result.is_null() {
        env.mem.write(result, nil);
    }
    errSecItemNotFound
}

fn SecItemUpdate(
    _env: &mut Environment,
    query: CFDictionaryRef,
    attributes: CFDictionaryRef,
) -> OSStatus {
    log!(
        "SecItemUpdate({:?}, {:?}): the keychain is empty",
        query,
        attributes
    );
    errSecItemNotFound
}

fn SecItemDelete(_env: &mut Environment, query: CFDictionaryRef) -> OSStatus {
    log!("SecItemDelete({:?}): the keychain is empty", query);
    errSecItemNotFound
}

const CONSTANTS: ConstantExports = &[
    ("_kSecRandomDefault", HostConstant::NullPtr),
    ("_kSecClass", HostConstant::NSString("class")),
    ("_kSecClassGenericPassword", HostConstant::NSString("genp")),
    ("_kSecAttrService", HostConstant::NSString("svce")),
    ("_kSecAttrAccount", HostConstant::NSString("acct")),
    ("_kSecAttrGeneric", HostConstant::NSString("gena")),
    ("_kSecReturnData", HostConstant::NSString("r_Data")),
    ("_kSecValueData", HostConstant::NSString("v_Data")),
    ("_kSecMatchLimit", HostConstant::NSString("m_Limit")),
    ("_kSecMatchLimitOne", HostConstant::NSString("m_LimitOne")),
];

const FUNCTIONS: FunctionExports = &[
    export_c_func!(SecRandomCopyBytes(_, _, _)),
    export_c_func!(SecItemAdd(_, _)),
    export_c_func!(SecItemCopyMatching(_, _)),
    export_c_func!(SecItemUpdate(_, _)),
    export_c_func!(SecItemDelete(_)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_fill_reports_failure_and_preserves_generated_bytes() {
        let mut bytes = [0; 4];
        assert_eq!(
            fill_random_with(&mut bytes, |out| {
                out.copy_from_slice(&[1, 2, 3, 4]);
                Ok::<(), ()>(())
            }),
            0
        );
        assert_eq!(bytes, [1, 2, 3, 4]);
        assert_eq!(fill_random_with(&mut bytes, |_| Err(())), -1);
    }
}
