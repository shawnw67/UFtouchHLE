/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIGeometry.h`
//!
//! See also [crate::frameworks::core_graphics::cg_geometry].

use crate::abi::impl_GuestRet_for_large_struct;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string;
use crate::mem::MutVoidPtr;
use crate::mem::SafeRead;
use crate::objc::{autorelease, id};
use crate::Environment;

/// Insets in points, returned as a 16-byte structure on 32-bit iOS.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
#[repr(C, packed)]
pub struct UIEdgeInsets {
    pub top: CGFloat,
    pub left: CGFloat,
    pub bottom: CGFloat,
    pub right: CGFloat,
}
unsafe impl SafeRead for UIEdgeInsets {}
impl_GuestRet_for_large_struct!(UIEdgeInsets);

#[derive(Default)]
pub struct State {
    pub default_malloc_zone: Option<MutVoidPtr>,
}

// Apple's documentation says all of these return zeroes if the input is not
// well-formed.
pub fn CGPointFromString(env: &mut Environment, string: id) -> CGPoint {
    // TODO: avoid copy
    ns_string::to_rust_string(env, string)
        .parse()
        .unwrap_or_default()
}
pub fn CGSizeFromString(env: &mut Environment, string: id) -> CGSize {
    // TODO: avoid copy
    ns_string::to_rust_string(env, string)
        .parse()
        .unwrap_or_default()
}
pub fn CGRectFromString(env: &mut Environment, string: id) -> CGRect {
    // TODO: avoid copy
    ns_string::to_rust_string(env, string)
        .parse()
        .unwrap_or_default()
}

pub fn NSStringFromCGPoint(env: &mut Environment, point: CGPoint) -> id {
    let s = ns_string::from_rust_string(env, point.to_string());
    autorelease(env, s)
}
pub fn NSStringFromCGSize(env: &mut Environment, size: CGSize) -> id {
    let s = ns_string::from_rust_string(env, size.to_string());
    autorelease(env, s)
}
pub fn NSStringFromCGRect(env: &mut Environment, rect: CGRect) -> id {
    let s = ns_string::from_rust_string(env, rect.to_string());
    autorelease(env, s)
}

fn NSDefaultMallocZone(env: &mut Environment) -> MutVoidPtr {
    if let Some(ptr) = env.framework_state.uikit.ui_geometry.default_malloc_zone {
        return ptr;
    }
    let ptr = env.mem.alloc(4);
    env.framework_state.uikit.ui_geometry.default_malloc_zone = Some(ptr);
    log_dbg!("NSDefaultMallocZone: allocated dummy zone at {:?}", ptr);
    ptr
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGPointFromString(_)),
    export_c_func!(CGSizeFromString(_)),
    export_c_func!(CGRectFromString(_)),
    export_c_func!(NSStringFromCGPoint(_)),
    export_c_func!(NSStringFromCGSize(_)),
    export_c_func!(NSStringFromCGRect(_)),
    export_c_func!(NSDefaultMallocZone()),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::GuestRet;
    use crate::mem::{Mem, PAGE_SIZE};

    #[test]
    fn edge_insets_return_in_guest_memory() {
        assert_eq!(UIEdgeInsets::SIZE_IN_MEM, Some(16));
        let mut mem = Mem::new();
        mem.set_null_segment_size(PAGE_SIZE);
        let ptr = mem.alloc(16);
        let insets = UIEdgeInsets {
            top: 1.0,
            left: 2.0,
            bottom: 3.0,
            right: 4.0,
        };
        insets.to_mem(ptr, &mut mem);
        assert_eq!(UIEdgeInsets::from_mem(ptr.cast_const(), &mem), insets);
    }
}
