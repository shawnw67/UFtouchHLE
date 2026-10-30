/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CFUUID values, their ARM ABI and per-Environment uniquing.
//! HyperHLE-Reborn ff8eb827 supplies additional API candidates; their byte
//! signatures, return storage and ownership are reimplemented for UF.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_string::CFStringRef;
use super::CFTypeRef;
use crate::abi::{impl_GuestRet_for_large_struct, GuestArg};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::{ns_string, NSUInteger};
use crate::mem::SafeRead;
use crate::objc::{autorelease, id, msg, nil, objc_classes, retain, ClassExports, HostObject};
use crate::Environment;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

pub type CFUUIDRef = CFTypeRef;

/// The C aggregate is sixteen consecutive UInt8 fields, alignment one.
/// An array has the same layout, and preserves all bit patterns unchanged.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CFUUIDBytes {
    bytes: [u8; 16],
}
unsafe impl SafeRead for CFUUIDBytes {}
impl_GuestRet_for_large_struct!(CFUUIDBytes);

impl GuestArg for CFUUIDBytes {
    const REG_COUNT: usize = 4;

    fn from_regs(regs: &[u32]) -> Self {
        let mut bytes = [0; 16];
        for (i, chunk) in bytes.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            chunk.copy_from_slice(&regs[i].to_le_bytes());
        }
        Self { bytes }
    }

    fn to_regs(self, regs: &mut [u32]) {
        for (reg, chunk) in regs.iter_mut().zip(self.bytes.as_chunks::<4>().0.iter()) {
            *reg = u32::from_le_bytes(*chunk);
        }
    }
}

#[derive(Default)]
pub struct State {
    // Ordinary entries do not retain objects and are removed in dealloc.
    values: HashMap<Uuid, CFUUIDRef>,
    // These entries own one permanent reference for the public Get contract.
    constants: HashSet<Uuid>,
}

struct CFUUIDHostObject {
    uuid: Uuid,
}
impl HostObject for CFUUIDHostObject {}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation _touchHLE_CFUUID: NSObject
- (bool)isEqual:(id)other {
    equal(env, this, other).unwrap_or(false)
}
- (u32)hash {
    env.objc.borrow::<CFUUIDHostObject>(this).uuid.as_bytes().iter()
        .fold(2166136261u32, |hash, byte| (hash ^ u32::from(*byte)).wrapping_mul(16777619))
}
- (id)description {
    let string = CFUUIDCreateString(env, kCFAllocatorDefault, this);
    autorelease(env, string)
}
- (())dealloc {
    let uuid = env.objc.borrow::<CFUUIDHostObject>(this).uuid;
    let state = &mut env.framework_state.core_foundation.cf_uuid;
    assert!(!state.constants.contains(&uuid), "Deallocating a borrowed CFUUID constant");
    assert_eq!(state.values.remove(&uuid), Some(this));
    env.objc.dealloc_object(this, &mut env.mem)
}
@end
};

/// Only claims CFUUID comparisons; other CFType behavior stays unchanged.
pub(super) fn equal(env: &Environment, a: CFTypeRef, b: CFTypeRef) -> Option<bool> {
    let get = |object| {
        env.objc
            .get_host_object(object)
            .and_then(|value| value.as_any().downcast_ref::<CFUUIDHostObject>())
    };
    match (get(a), get(b)) {
        (Some(a), Some(b)) => Some(a.uuid == b.uuid),
        (None, None) => None,
        _ => Some(false),
    }
}

fn validate_allocator(env: &Environment, allocator: CFAllocatorRef) {
    assert!(
        allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default(),
        "CFUUID custom allocators are not implemented"
    );
}

fn intern(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    uuid: Uuid,
    constant: bool,
) -> CFUUIDRef {
    validate_allocator(env, allocator);
    let state = &mut env.framework_state.core_foundation.cf_uuid;
    if let Some(&object) = state.values.get(&uuid) {
        // Create returns ownership; a first Get also roots an ordinary value
        // so releasing its existing owners cannot invalidate the constant.
        if !constant || state.constants.insert(uuid) {
            retain(env, object);
        }
        return object;
    }
    let class = env.objc.get_known_class("_touchHLE_CFUUID", &mut env.mem);
    let object = env
        .objc
        .alloc_object(class, Box::new(CFUUIDHostObject { uuid }), &mut env.mem);
    let state = &mut env.framework_state.core_foundation.cf_uuid;
    state.values.insert(uuid, object);
    if constant {
        state.constants.insert(uuid);
    }
    object
}

fn CFUUIDCreate(env: &mut Environment, allocator: CFAllocatorRef) -> CFUUIDRef {
    intern(env, allocator, Uuid::new_v4(), false)
}

fn CFUUIDCreateWithBytes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    b0: u8,
    b1: u8,
    b2: u8,
    b3: u8,
    b4: u8,
    b5: u8,
    b6: u8,
    b7: u8,
    b8: u8,
    b9: u8,
    b10: u8,
    b11: u8,
    b12: u8,
    b13: u8,
    b14: u8,
    b15: u8,
) -> CFUUIDRef {
    intern(
        env,
        allocator,
        Uuid::from_bytes([
            b0, b1, b2, b3, b4, b5, b6, b7, b8, b9, b10, b11, b12, b13, b14, b15,
        ]),
        false,
    )
}

fn CFUUIDGetConstantUUIDWithBytes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    b0: u8,
    b1: u8,
    b2: u8,
    b3: u8,
    b4: u8,
    b5: u8,
    b6: u8,
    b7: u8,
    b8: u8,
    b9: u8,
    b10: u8,
    b11: u8,
    b12: u8,
    b13: u8,
    b14: u8,
    b15: u8,
) -> CFUUIDRef {
    intern(
        env,
        allocator,
        Uuid::from_bytes([
            b0, b1, b2, b3, b4, b5, b6, b7, b8, b9, b10, b11, b12, b13, b14, b15,
        ]),
        true,
    )
}

fn CFUUIDCreateFromUUIDBytes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: CFUUIDBytes,
) -> CFUUIDRef {
    intern(env, allocator, Uuid::from_bytes(bytes.bytes), false)
}

fn CFUUIDGetUUIDBytes(env: &mut Environment, uuid: CFUUIDRef) -> CFUUIDBytes {
    CFUUIDBytes {
        bytes: *env.objc.borrow::<CFUUIDHostObject>(uuid).uuid.as_bytes(),
    }
}

fn hex_digit(c: u16) -> Option<u8> {
    match c {
        0x30..=0x39 => Some((c - 0x30) as u8),
        0x41..=0x46 => Some((c - 0x41 + 10) as u8),
        0x61..=0x66 => Some((c - 0x61 + 10) as u8),
        _ => None,
    }
}

/// Historical CF reads at most 100 UTF-16 units, skips an initial non-hex
/// prefix, then consumes fixed byte pairs and four separator positions.
fn parse_units(input: &[u16]) -> Option<Uuid> {
    let input = &input[..input.len().min(100)];
    if input.is_empty() {
        return None;
    }
    let mut offset = input
        .iter()
        .position(|&c| hex_digit(c).is_some())
        .unwrap_or(input.len());
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        if offset + 1 < input.len() {
            *byte = hex_digit(input[offset])
                .zip(hex_digit(input[offset + 1]))
                .map_or(0, |(hi, lo)| (hi << 4) | lo);
            offset += 2;
        }
        if matches!(index, 3 | 5 | 7 | 9) {
            offset += 1;
        }
    }
    Some(Uuid::from_bytes(bytes))
}

fn CFUUIDCreateFromString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFStringRef,
) -> CFUUIDRef {
    if string == nil {
        return nil;
    }
    let length: NSUInteger = msg![env; string length];
    let length = length.min(100) as usize;
    let mut units = [0u16; 100];
    for (i, unit) in units[..length].iter_mut().enumerate() {
        *unit = msg![env; string characterAtIndex:(i as NSUInteger)];
    }
    parse_units(&units[..length]).map_or(nil, |uuid| intern(env, allocator, uuid, false))
}

fn CFUUIDCreateString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    uuid: CFUUIDRef,
) -> CFStringRef {
    validate_allocator(env, allocator);
    let uuid = env.objc.borrow::<CFUUIDHostObject>(uuid).uuid;
    ns_string::from_rust_string(env, uuid.hyphenated().to_string().to_uppercase())
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFUUIDCreate(_)),
    export_c_func!(CFUUIDCreateString(_, _)),
    export_c_func!(CFUUIDCreateWithBytes(
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _
    )),
    export_c_func!(CFUUIDGetConstantUUIDWithBytes(
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _
    )),
    export_c_func!(CFUUIDCreateFromUUIDBytes(_, _)),
    export_c_func!(CFUUIDGetUUIDBytes(_)),
    export_c_func!(CFUUIDCreateFromString(_, _)),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::GuestRet;
    use crate::mem::{Mem, MutPtr};

    #[test]
    fn bytes_have_the_real_layout_and_preserve_all_register_bits() {
        assert_eq!(std::mem::size_of::<CFUUIDBytes>(), 16);
        assert_eq!(std::mem::align_of::<CFUUIDBytes>(), 1);
        let words = [0x7f800001, 0xffc01234, 0x80000000, 0xffffffff];
        let value = <CFUUIDBytes as GuestArg>::from_regs(&words);
        assert_eq!(
            value.bytes,
            [1, 0, 0x80, 0x7f, 0x34, 0x12, 0xc0, 0xff, 0, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff]
        );
        let mut output = [0x12345678; 8];
        <CFUUIDBytes as GuestArg>::to_regs(value, &mut output[..4]);
        assert_eq!(&output[..4], &words);
        assert_eq!(&output[4..], &[0x12345678; 4]);
    }

    #[test]
    fn aggregate_returns_write_sixteen_bytes_and_preserve_canaries() {
        assert_eq!(CFUUIDBytes::SIZE_IN_MEM, Some(16));
        let mut mem = Mem::new();
        mem.set_null_segment_size(4096);
        let guarded: MutPtr<u8> = mem.alloc(24).cast();
        mem.bytes_at_mut(guarded, 24).fill(0xa5);
        let value = CFUUIDBytes {
            bytes: std::array::from_fn(|i| (i as u8).wrapping_mul(17)),
        };
        value.to_mem((guarded + 4).cast(), &mut mem);
        let copy = CFUUIDBytes::from_mem((guarded + 4).cast_const().cast(), &mem);
        assert_eq!(value, copy);
        assert_eq!(mem.bytes_at(guarded.cast_const(), 4), &[0xa5; 4]);
        assert_eq!(mem.bytes_at((guarded + 20).cast_const(), 4), &[0xa5; 4]);
    }

    #[test]
    fn parser_handles_limits_and_partial_input_without_out_of_bounds_reads() {
        assert_eq!(parse_units(&[]), None);
        assert_eq!(parse_units(&[b'5' as u16]), Some(Uuid::nil()));
        let partial = parse_units(&[b'5' as u16, b'5' as u16]).unwrap();
        assert_eq!(partial.as_bytes()[0], 0x55);
        assert_eq!(&partial.as_bytes()[1..], &[0; 15]);
        assert_eq!(parse_units(&[0xd800, 0xdc00]), Some(Uuid::nil()));
        let mut beyond = vec![b'z' as u16; 100];
        beyond.extend("550E8400-E29B-41D4-A716-446655440000".encode_utf16());
        assert_eq!(parse_units(&beyond), Some(Uuid::nil()));
    }

    #[test]
    fn parser_matches_recorded_native_cf_cases() {
        for line in include_str!("../../../tests/fixtures/cfuuid-native.tsv")
            .lines()
            .filter(|line| !line.starts_with('#'))
        {
            let (input, expected) = line.split_once('\t').unwrap();
            let input = if input == "-" {
                Vec::new()
            } else {
                input
                    .as_bytes()
                    .chunks_exact(4)
                    .map(|chunk| {
                        u16::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap()
                    })
                    .collect()
            };
            let actual = parse_units(&input).map(|uuid| uuid.simple().to_string());
            assert_eq!(
                actual.as_deref(),
                (expected != "-").then_some(expected),
                "{input:?}"
            );
        }
    }
}
