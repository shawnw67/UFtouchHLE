/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ctype.h`

use super::wchar::wchar_t;
use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::mem::{ConstVoidPtr, MutVoidPtr, Ptr, SafeRead};
use crate::Environment;

/// Called by inlined `tolower()` on Darwin
fn __tolower(_env: &mut Environment, c: i32) -> i32 {
    if (c as u8) as i32 == c {
        (c as u8).to_ascii_lowercase().into()
    } else {
        c
    }
}
/// Called by inlined `toupper()` on Darwin
fn __toupper(_env: &mut Environment, c: i32) -> i32 {
    if (c as u8) as i32 == c {
        (c as u8).to_ascii_uppercase().into()
    } else {
        c
    }
}

fn __maskrune(env: &mut Environment, rune: i32, mask: u32) -> i32 {
    // TODO: do not re-create rune table on each call
    let default_rune_locale_ptr = get_default_rune_locale(env);
    let rune_locale: RuneLocale = env.mem.read(default_rune_locale_ptr.cast());
    env.mem.free(default_rune_locale_ptr.cast_mut());
    (rune_locale.runetype[(rune & 0xFF) as usize] & mask) as i32
}

#[allow(non_camel_case_types)]
type darwin_rune_t = wchar_t;

const LOOKUP_TABLE_SIZE: usize = 1 << 8;

/// Darwin inlines its implementation of the ctype functions and so this struct
/// is part of its ABI. The names have had their leading underscores removed.
#[repr(C, packed)]
struct RuneLocale {
    magic: [u8; 8],
    /// Fixed-width string naming the encoding
    encoding: [u8; 32],

    getrune: GuestFunction, // TODO
    putrune: GuestFunction, // TODO
    invalid_rune: darwin_rune_t,

    /// Bits represent type of character
    runetype: [u32; LOOKUP_TABLE_SIZE],
    map_lower: [darwin_rune_t; LOOKUP_TABLE_SIZE],
    map_upper: [darwin_rune_t; LOOKUP_TABLE_SIZE],

    variable: MutVoidPtr, // extra data, not used
    variable_len: i32,

    ncharclasses: i32,     // extra data, not used
    charclass: MutVoidPtr, // type should be pointer to RuneCharClass
}
unsafe impl SafeRead for RuneLocale {}

fn default_rune_locale() -> RuneLocale {
    let mut runetype = [0u32; LOOKUP_TABLE_SIZE];
    let mut map_lower = [0 as darwin_rune_t; LOOKUP_TABLE_SIZE];
    let mut map_upper = [0 as darwin_rune_t; LOOKUP_TABLE_SIZE];

    for idx in 0..LOOKUP_TABLE_SIZE {
        let c: u8 = idx.try_into().unwrap();

        let as_lower = c.to_ascii_lowercase();
        let as_upper = c.to_ascii_uppercase();

        let mut as_runetype = 0u32;
        if c.is_ascii_alphabetic() {
            as_runetype |= 0x100;
        }
        if c.is_ascii_control() {
            as_runetype |= 0x200;
        }
        if c.is_ascii_digit() {
            as_runetype |= 0x400;
        }
        if c.is_ascii_graphic() {
            as_runetype |= 0x800;
        }
        if c.is_ascii_lowercase() {
            as_runetype |= 0x1000;
        }
        if c.is_ascii_punctuation() {
            as_runetype |= 0x2000;
        }
        // Rust's definition excludes vertical tab
        if c.is_ascii_whitespace() || c == b'\x0b' {
            as_runetype |= 0x4000;
        }
        if c.is_ascii_uppercase() {
            as_runetype |= 0x8000;
        }
        if c.is_ascii_hexdigit() {
            as_runetype |= 0x10000;
        }
        // isblank()
        if c == b' ' || c == b'\t' {
            as_runetype |= 0x20000;
        }
        // isprint()
        if c.is_ascii_graphic() || c == b' ' {
            as_runetype |= 0x40000;
        }
        // TODO: There are some other flags: "ideogram", "special", "phonogram",
        // and a character "width" between 0 and 4. These aren't standard C and
        // aren't implemented here.

        runetype[idx] = as_runetype;
        map_lower[idx] = as_lower.into();
        map_upper[idx] = as_upper.into();
    }

    let mut encoding = [0u8; 32];
    encoding[0..4].copy_from_slice(b"NONE"); // this is the real value!

    RuneLocale {
        magic: *b"RuneMagA",
        encoding,
        getrune: GuestFunction::null_ptr(), // TODO
        putrune: GuestFunction::null_ptr(), // TODO
        invalid_rune: -1,                   // probably not correct
        runetype,
        map_lower,
        map_upper,
        variable: Ptr::null(),
        variable_len: 0,
        ncharclasses: 0,
        charclass: Ptr::null(),
    }
}

fn get_default_rune_locale(env: &mut Environment) -> ConstVoidPtr {
    env.mem
        .alloc_and_write(default_rune_locale())
        .cast()
        .cast_const()
}

#[cfg(test)]
mod tests {
    use super::default_rune_locale;

    #[test]
    fn inlined_case_conversion_matches_function_calls() {
        let locale = default_rune_locale();
        for c in 0u8..=255 {
            assert_eq!(locale.map_lower[c as usize] as u8, c.to_ascii_lowercase());
            assert_eq!(locale.map_upper[c as usize] as u8, c.to_ascii_uppercase());
            assert_eq!(
                locale.runetype[c as usize] & 0x1000 != 0,
                c.is_ascii_lowercase()
            );
            assert_eq!(
                locale.runetype[c as usize] & 0x8000 != 0,
                c.is_ascii_uppercase()
            );
        }
    }
}

// The exported ctype functions receive either EOF or an unsigned char value.
// Use the same classification bits as Darwin's inlined ctype macros.
fn ctype_mask(c: i32, mask: u32) -> i32 {
    u8::try_from(c)
        .map(|c| (rune_type(c) & mask) as i32)
        .unwrap_or(0)
}

fn rune_type(c: u8) -> u32 {
    let mut bits = 0;
    if c.is_ascii_alphabetic() {
        bits |= 0x100;
    }
    if c.is_ascii_control() {
        bits |= 0x200;
    }
    if c.is_ascii_digit() {
        bits |= 0x400;
    }
    if c.is_ascii_graphic() {
        bits |= 0x800;
    }
    if c.is_ascii_lowercase() {
        bits |= 0x1000;
    }
    if c.is_ascii_punctuation() {
        bits |= 0x2000;
    }
    // Rust's ASCII whitespace predicate excludes vertical tab.
    if c.is_ascii_whitespace() || c == b'\x0b' {
        bits |= 0x4000;
    }
    if c.is_ascii_uppercase() {
        bits |= 0x8000;
    }
    if c.is_ascii_hexdigit() {
        bits |= 0x10000;
    }
    if c == b' ' || c == b'\t' {
        bits |= 0x20000; // isblank
    }
    if c.is_ascii_graphic() || c == b' ' {
        bits |= 0x40000; // isprint
    }
    // Other Darwin bits (ideogram, special, phonogram, width) remain unset.
    bits
}

pub const CONSTANTS: ConstantExports = &[
    (
        "__DefaultRuneLocale",
        HostConstant::Custom(get_default_rune_locale),
    ),
    // Maximum bytes per multibyte character in the current locale ("C").
    (
        "___mb_cur_max",
        HostConstant::Custom(|env| env.mem.alloc_and_write(1i32).cast_void().cast_const()),
    ),
];

fn isalnum(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x100 | 0x400)
}

fn isalpha(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x100)
}

fn isblank(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x20000)
}

fn iscntrl(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x200)
}

fn isdigit(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x400)
}

fn isgraph(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x800)
}

fn islower(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x1000)
}

fn isprint(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x40000)
}

fn ispunct(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x2000)
}

fn isspace(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x4000)
}

fn isupper(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x8000)
}

fn isxdigit(_env: &mut Environment, c: i32) -> i32 {
    ctype_mask(c, 0x10000)
}

fn tolower(env: &mut Environment, c: i32) -> i32 {
    __tolower(env, c)
}

fn toupper(env: &mut Environment, c: i32) -> i32 {
    __toupper(env, c)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__tolower(_)),
    export_c_func!(__toupper(_)),
    export_c_func!(__maskrune(_, _)),
    export_c_func!(isalnum(_)),
    export_c_func!(isalpha(_)),
    export_c_func!(isblank(_)),
    export_c_func!(iscntrl(_)),
    export_c_func!(isdigit(_)),
    export_c_func!(isgraph(_)),
    export_c_func!(islower(_)),
    export_c_func!(isprint(_)),
    export_c_func!(ispunct(_)),
    export_c_func!(isspace(_)),
    export_c_func!(isupper(_)),
    export_c_func!(isxdigit(_)),
    export_c_func!(tolower(_)),
    export_c_func!(toupper(_)),
];
