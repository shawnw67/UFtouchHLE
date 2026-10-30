/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `string.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr};
use crate::Environment;
use std::cmp::Ordering;

use super::generic_char::GenericChar;

mod shuffle;
mod version;

#[derive(Default)]
pub struct State {
    strtok: Option<MutPtr<u8>>,
}

fn strtok(env: &mut Environment, s: MutPtr<u8>, sep: ConstPtr<u8>) -> MutPtr<u8> {
    let s = if s.is_null() {
        env.libc_state.string.strtok.unwrap_or_default()
    } else {
        s
    };

    let (token, next) = tokenize(&mut env.mem, s, sep);
    env.libc_state.string.strtok = if next.is_null() { None } else { Some(next) };
    token
}

fn strtok_r(
    env: &mut Environment,
    s: MutPtr<u8>,
    sep: ConstPtr<u8>,
    save: MutPtr<MutPtr<u8>>,
) -> MutPtr<u8> {
    let start = if s.is_null() { env.mem.read(save) } else { s };
    let (token, next) = tokenize(&mut env.mem, start, sep);
    env.mem.write(save, next);
    token
}

fn tokenize(mem: &mut Mem, s: MutPtr<u8>, sep: ConstPtr<u8>) -> (MutPtr<u8>, MutPtr<u8>) {
    if s.is_null() {
        return (Ptr::null(), Ptr::null());
    }
    let mut delimiters = [false; 256];
    for &byte in mem.cstr_at(sep) {
        delimiters[usize::from(byte)] = true;
    }
    let mut token_start = s;
    loop {
        let c = mem.read(token_start);
        if c == b'\0' {
            return (Ptr::null(), Ptr::null());
        } else if delimiters[usize::from(c)] {
            token_start += 1;
        } else {
            break;
        }
    }

    let mut token_end = token_start;
    let next_token = loop {
        let c = mem.read(token_end);
        if delimiters[usize::from(c)] {
            mem.write(token_end, b'\0');
            break token_end + 1;
        } else if c == b'\0' {
            break Ptr::null();
        } else {
            token_end += 1;
        }
    };

    (token_start, next_token)
}

// HyperHLE-Reborn provides these APIs, but its strlen(...).min(n) approach
// reads beyond n. All bounded byte-string operations here share this scan.
fn bounded_len(mem: &Mem, string: ConstPtr<u8>, limit: GuestUSize) -> GuestUSize {
    for i in 0..limit {
        if mem.read(string + i) == 0 {
            return i;
        }
    }
    limit
}

fn strnlen(env: &mut Environment, string: ConstPtr<u8>, limit: GuestUSize) -> GuestUSize {
    bounded_len(&env.mem, string, limit)
}

fn duplicate_bounded(mem: &mut Mem, src: ConstPtr<u8>, n: GuestUSize) -> Option<MutPtr<u8>> {
    let len = bounded_len(mem, src, n);
    let output: MutPtr<u8> = mem.try_alloc(len.checked_add(1)?)?.cast();
    if len != 0 {
        mem.memmove(output.cast(), src.cast(), len);
    }
    mem.write(output + len, 0u8);
    Some(output)
}

fn strndup(env: &mut Environment, src: ConstPtr<u8>, n: GuestUSize) -> MutPtr<u8> {
    duplicate_bounded(&mut env.mem, src, n).unwrap_or_else(|| {
        crate::libc::errno::set_errno(env, crate::libc::errno::ENOMEM);
        Ptr::null()
    })
}

fn concatenate_bounded(
    mem: &mut Mem,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    size: GuestUSize,
) -> GuestUSize {
    let dest_len = bounded_len(mem, dest.cast_const(), size);
    let src_len = bounded_len(mem, src, GuestUSize::MAX);
    if dest_len < size {
        let count = src_len.min(size - dest_len - 1);
        if count != 0 {
            mem.memmove((dest + dest_len).cast(), src.cast(), count);
        }
        mem.write(dest + dest_len + count, 0u8);
    }
    dest_len.wrapping_add(src_len)
}

fn strlcat(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    size: GuestUSize,
) -> GuestUSize {
    concatenate_bounded(&mut env.mem, dest, src, size)
}

fn concatenate_checked(
    mem: &mut Mem,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    n: GuestUSize,
    capacity: GuestUSize,
) -> MutPtr<u8> {
    let dest_len = bounded_len(mem, dest.cast_const(), capacity);
    assert!(dest_len < capacity, "__strncat_chk: buffer overflow");
    let remaining = capacity - dest_len;
    let count = bounded_len(mem, src, n.min(remaining));
    assert!(count < remaining, "__strncat_chk: buffer overflow");
    if count != 0 {
        mem.memmove((dest + dest_len).cast(), src.cast(), count);
    }
    mem.write(dest + dest_len + count, 0u8);
    dest
}

fn __strncat_chk(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    n: GuestUSize,
    capacity: GuestUSize,
) -> MutPtr<u8> {
    concatenate_checked(&mut env.mem, dest, src, n, capacity)
}

fn memrchr(env: &mut Environment, string: ConstVoidPtr, c: i32, n: GuestUSize) -> ConstVoidPtr {
    for i in (0..n).rev() {
        if env.mem.read(string.cast::<u8>() + i) == c as u8 {
            return (string.cast::<u8>() + i).cast();
        }
    }
    Ptr::null()
}

fn index(env: &mut Environment, string: ConstPtr<u8>, c: i32) -> ConstPtr<u8> {
    strchr(env, string, c as u8)
}

fn find_binary(
    mem: &Mem,
    haystack: ConstVoidPtr,
    haystack_len: GuestUSize,
    needle: ConstVoidPtr,
    needle_len: GuestUSize,
) -> ConstVoidPtr {
    if needle_len == 0 {
        return haystack;
    }
    if needle_len > haystack_len {
        return Ptr::null();
    }
    let hay = mem.bytes_at(haystack.cast(), haystack_len);
    let needle = mem.bytes_at(needle.cast(), needle_len);
    memchr::memmem::find(hay, needle).map_or_else(Ptr::null, |offset| {
        (haystack.cast::<u8>() + GuestUSize::try_from(offset).unwrap()).cast()
    })
}

fn memmem(
    env: &mut Environment,
    haystack: ConstVoidPtr,
    haystack_len: GuestUSize,
    needle: ConstVoidPtr,
    needle_len: GuestUSize,
) -> ConstVoidPtr {
    find_binary(&env.mem, haystack, haystack_len, needle, needle_len)
}

fn copy_to_end(
    mem: &mut Mem,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    limit: Option<GuestUSize>,
) -> MutPtr<u8> {
    let len = bounded_len(mem, src, limit.unwrap_or(GuestUSize::MAX));
    let count = limit.unwrap_or_else(|| len.checked_add(1).expect("stpcpy length overflow"));
    if len != 0 {
        mem.memmove(dest.cast(), src.cast(), len);
    }
    if count > len {
        mem.bytes_at_mut(dest + len, count - len).fill(0);
    }
    dest + len
}

fn stpcpy(env: &mut Environment, dest: MutPtr<u8>, src: ConstPtr<u8>) -> MutPtr<u8> {
    copy_to_end(&mut env.mem, dest, src, None)
}

fn stpncpy(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    n: GuestUSize,
) -> MutPtr<u8> {
    copy_to_end(&mut env.mem, dest, src, Some(n))
}

fn strchrnul(env: &mut Environment, string: ConstPtr<u8>, c: i32) -> ConstPtr<u8> {
    let mut offset = 0;
    loop {
        let byte = env.mem.read(string + offset);
        if byte == c as u8 || byte == 0 {
            return string + offset;
        }
        offset += 1;
    }
}

fn strcasestr(env: &mut Environment, haystack: ConstPtr<u8>, needle: ConstPtr<u8>) -> ConstPtr<u8> {
    // Match UF's existing strcasecmp ASCII folding; locale tables are not
    // implemented. Byte slices also preserve non-UTF-8 input and offsets.
    let hay = env.mem.cstr_at(haystack);
    let needle = env.mem.cstr_at(needle);
    if needle.is_empty() {
        return haystack;
    }
    hay.windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
        .map_or_else(Ptr::null, |offset| {
            haystack + GuestUSize::try_from(offset).unwrap()
        })
}

fn bcopy(env: &mut Environment, src: ConstVoidPtr, dest: MutVoidPtr, n: GuestUSize) {
    if n != 0 {
        env.mem.memmove(dest, src, n);
    }
}

fn explicit_bzero(env: &mut Environment, dest: MutVoidPtr, n: GuestUSize) {
    if n == 0 {
        return;
    }
    for byte in env.mem.bytes_at_mut(dest.cast(), n) {
        // SAFETY: bytes_at_mut validated the range; each byte is writable.
        // Volatile stores keep this explicit wipe observable to the compiler.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
}

fn strverscmp(env: &mut Environment, a: ConstPtr<u8>, b: ConstPtr<u8>) -> i32 {
    version::compare(env.mem.cstr_at(a), env.mem.cstr_at(b))
}

fn strfry(env: &mut Environment, string: MutPtr<u8>) -> MutPtr<u8> {
    let len = bounded_len(&env.mem, string.cast_const(), GuestUSize::MAX);
    if len <= 1 {
        return string;
    }
    if let Err(error) = shuffle::shuffle(env.mem.bytes_at_mut(string, len)) {
        log!("strfry: host entropy unavailable: {error}");
        crate::libc::errno::set_errno(env, crate::libc::errno::EIO);
        return Ptr::null();
    }
    string
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use std::panic::{catch_unwind, AssertUnwindSafe};

    fn memory() -> Mem {
        let mut mem = Mem::new();
        mem.set_null_segment_size(4096);
        mem
    }

    #[test]
    fn bounded_reads_stop_at_the_end_of_guest_address_space() {
        let mut mem = memory();
        let tail = MutPtr::<u8>::from_bits(u32::MAX - 3);
        mem.bytes_at_mut(tail, 4).copy_from_slice(b"abcd");
        assert_eq!(bounded_len(&mem, tail.cast_const(), 4), 4);
        let output = duplicate_bounded(&mut mem, tail.cast_const(), 4).unwrap();
        assert_eq!(mem.cstr_at(output), b"abcd");
        let source = mem.alloc_and_write_cstr(b"xy");
        assert_eq!(
            concatenate_bounded(&mut mem, tail, source.cast_const(), 4),
            6
        );
        assert_eq!(mem.bytes_at(tail.cast_const(), 4), b"abcd");
    }

    #[test]
    fn zero_limits_do_not_access_string_or_destination() {
        let mut mem = memory();
        assert_eq!(bounded_len(&mem, Ptr::null(), 0), 0);
        let copy = duplicate_bounded(&mut mem, Ptr::null(), 0).unwrap();
        assert_eq!(mem.cstr_at(copy), b"");
        let src = mem.alloc_and_write_cstr(b"abc");
        assert_eq!(
            concatenate_bounded(&mut mem, Ptr::null(), src.cast_const(), 0),
            3
        );
    }

    #[test]
    fn fortified_failures_do_not_modify_destination_or_adjacent_canaries() {
        let mut mem = memory();
        let dest: MutPtr<u8> = mem.alloc(8).cast();
        let source = mem.alloc_and_write_cstr(b"xyz");
        for (initial, cap) in [(b"ab\0ZZZZZ", 4), (b"abcZZZZZ", 3), (b"ab\0ZZZZZ", 0)] {
            mem.bytes_at_mut(dest, 8).copy_from_slice(initial);
            let rejected = catch_unwind(AssertUnwindSafe(|| {
                concatenate_checked(&mut mem, dest, source.cast_const(), 3, cap);
            }));
            assert!(rejected.is_err());
            assert_eq!(mem.bytes_at(dest.cast_const(), 8), initial);
        }
        mem.bytes_at_mut(dest, 8).copy_from_slice(b"ab\0ZZZZZ");
        assert_eq!(
            concatenate_checked(&mut mem, dest, source.cast_const(), 3, 6),
            dest
        );
        assert_eq!(mem.bytes_at(dest.cast_const(), 8), b"abxyz\0ZZ");
    }

    #[test]
    fn fallible_allocation_rejects_rounding_overflow_without_poisoning_heap() {
        let mut mem = memory();
        assert!(mem.try_alloc(u32::MAX).is_none());
        let ptr: MutPtr<u8> = mem.try_alloc(4).unwrap().cast();
        mem.bytes_at_mut(ptr, 4).copy_from_slice(b"test");
        assert_eq!(mem.bytes_at(ptr.cast_const(), 4), b"test");
        mem.free(ptr.cast());
    }

    #[test]
    fn bounded_binary_search_preserves_end_of_address_space_and_zero_limits() {
        let mut mem = memory();
        let tail = MutPtr::<u8>::from_bits(u32::MAX - 3);
        mem.bytes_at_mut(tail, 4).copy_from_slice(b"a\0bc");
        let needle = mem.alloc_and_write_cstr(b"bc");
        assert_eq!(
            find_binary(
                &mem,
                tail.cast_const().cast(),
                4,
                needle.cast_const().cast(),
                2
            ),
            (tail + 2).cast_const().cast()
        );
        assert_eq!(
            find_binary(&mem, tail.cast_const().cast(), 4, Ptr::null(), 0),
            tail.cast_const().cast()
        );
        assert_eq!(
            find_binary(&mem, Ptr::null(), 0, Ptr::null(), 1),
            Ptr::null()
        );
    }

    #[test]
    fn bounded_end_copy_stops_at_the_limit_and_preserves_destination_canaries() {
        let mut mem = memory();
        let tail = MutPtr::<u8>::from_bits(u32::MAX - 3);
        mem.bytes_at_mut(tail, 4).copy_from_slice(b"abcd");
        let dest: MutPtr<u8> = mem.alloc(8).cast();
        mem.bytes_at_mut(dest, 8).fill(b'Z');
        assert_eq!(
            copy_to_end(&mut mem, dest, tail.cast_const(), Some(4)),
            dest + 4
        );
        assert_eq!(mem.bytes_at(dest.cast_const(), 8), b"abcdZZZZ");
        assert_eq!(
            copy_to_end(&mut mem, Ptr::null(), Ptr::null(), Some(0)),
            Ptr::null()
        );
        mem.bytes_at_mut(tail, 4).copy_from_slice(b"a\0ZZ");
        assert_eq!(
            copy_to_end(&mut mem, dest, tail.cast_const(), Some(6)),
            dest + 1
        );
        assert_eq!(mem.bytes_at(dest.cast_const(), 8), b"a\0\0\0\0\0ZZ");
    }
}

// Functions shared with wchar.rs

fn bzero(env: &mut Environment, dest: MutVoidPtr, count: GuestUSize) {
    memset(env, dest, 0, count);
}
fn memset(env: &mut Environment, dest: MutVoidPtr, ch: i32, count: GuestUSize) -> MutVoidPtr {
    GenericChar::<u8>::memset(env, dest.cast(), ch as u8, count, GuestUSize::MAX).cast()
}
fn __memset_chk(
    env: &mut Environment,
    dest: MutVoidPtr,
    ch: i32,
    count: GuestUSize,
    dest_count: GuestUSize,
) -> MutVoidPtr {
    GenericChar::<u8>::memset(env, dest.cast(), ch as u8, count, dest_count).cast()
}
fn memset_pattern4(env: &mut Environment, b: MutVoidPtr, pattern4: ConstVoidPtr, len: GuestUSize) {
    memset_pattern_inner(env, b, pattern4, len, 4)
}
fn memset_pattern8(env: &mut Environment, b: MutVoidPtr, pattern8: ConstVoidPtr, len: GuestUSize) {
    memset_pattern_inner(env, b, pattern8, len, 8)
}
fn memset_pattern16(
    env: &mut Environment,
    b: MutVoidPtr,
    pattern16: ConstVoidPtr,
    len: GuestUSize,
) {
    memset_pattern_inner(env, b, pattern16, len, 16)
}
fn memset_pattern_inner(
    env: &mut Environment,
    b: MutVoidPtr,
    pattern: ConstVoidPtr,
    len: GuestUSize,
    pattern_len: GuestUSize,
) {
    assert!(matches!(pattern_len, 4 | 8 | 16));
    let mut tmp = [0; 16];
    tmp[..pattern_len as usize].copy_from_slice(env.mem.bytes_at(pattern.cast(), pattern_len));
    let mut target: MutPtr<u8> = b.cast();
    for _ in 0..(len / pattern_len) {
        env.mem
            .bytes_at_mut(target, pattern_len)
            .copy_from_slice(&tmp[..pattern_len as usize]);
        target += pattern_len;
    }
    for i in 0..(len % pattern_len) {
        let value = env.mem.read(pattern.cast() + i);
        env.mem.write(target, value);
        target += 1;
    }
}
fn memcpy(
    env: &mut Environment,
    dest: MutVoidPtr,
    src: ConstVoidPtr,
    size: GuestUSize,
) -> MutVoidPtr {
    GenericChar::<u8>::memcpy(env, dest.cast(), src.cast(), size, GuestUSize::MAX).cast()
}
fn __memcpy_chk(
    env: &mut Environment,
    dest: MutVoidPtr,
    src: ConstVoidPtr,
    size: GuestUSize,
    dest_size: GuestUSize,
) -> MutVoidPtr {
    GenericChar::<u8>::memcpy(env, dest.cast(), src.cast(), size, dest_size).cast()
}
fn memmove(
    env: &mut Environment,
    dest: MutVoidPtr,
    src: ConstVoidPtr,
    size: GuestUSize,
) -> MutVoidPtr {
    GenericChar::<u8>::memmove(env, dest.cast(), src.cast(), size, GuestUSize::MAX).cast()
}
fn __memmove_chk(
    env: &mut Environment,
    dest: MutVoidPtr,
    src: ConstVoidPtr,
    size: GuestUSize,
    dest_size: GuestUSize,
) -> MutVoidPtr {
    GenericChar::<u8>::memmove(env, dest.cast(), src.cast(), size, dest_size).cast()
}
fn memchr(env: &mut Environment, string: ConstVoidPtr, c: i32, size: GuestUSize) -> ConstVoidPtr {
    GenericChar::<u8>::memchr(env, string.cast(), c as u8, size).cast()
}
fn memcmp(env: &mut Environment, a: ConstVoidPtr, b: ConstVoidPtr, size: GuestUSize) -> i32 {
    GenericChar::<u8>::memcmp(env, a.cast(), b.cast(), size)
}
pub(crate) fn strlen(env: &mut Environment, s: ConstPtr<u8>) -> GuestUSize {
    GenericChar::<u8>::strlen(env, s)
}
pub(super) fn strcpy(env: &mut Environment, dest: MutPtr<u8>, src: ConstPtr<u8>) -> MutPtr<u8> {
    GenericChar::<u8>::strcpy(env, dest, src, GuestUSize::MAX)
}
fn __strcpy_chk(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    size: GuestUSize,
) -> MutPtr<u8> {
    GenericChar::<u8>::strcpy(env, dest, src, size)
}
fn strcat(env: &mut Environment, dest: MutPtr<u8>, src: ConstPtr<u8>) -> MutPtr<u8> {
    GenericChar::<u8>::strcat(env, dest, src, GuestUSize::MAX)
}
fn __strcat_chk(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    size: GuestUSize,
) -> MutPtr<u8> {
    GenericChar::<u8>::strcat(env, dest, src, size)
}
fn strspn(env: &mut Environment, s: ConstPtr<u8>, charset: ConstPtr<u8>) -> GuestUSize {
    GenericChar::<u8>::strspn(env, s, charset)
}
fn strcspn(env: &mut Environment, s: ConstPtr<u8>, charset: ConstPtr<u8>) -> GuestUSize {
    GenericChar::<u8>::strcspn(env, s, charset)
}
pub(crate) fn strncpy(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    size: GuestUSize,
) -> MutPtr<u8> {
    GenericChar::<u8>::strncpy(env, dest, src, size, GuestUSize::MAX)
}
fn __strncpy_chk(
    env: &mut Environment,
    dest: MutPtr<u8>,
    src: ConstPtr<u8>,
    size: GuestUSize,
    dest_size: GuestUSize,
) -> MutPtr<u8> {
    GenericChar::<u8>::strncpy(env, dest, src, size, dest_size)
}
fn strsep(env: &mut Environment, stringp: MutPtr<MutPtr<u8>>, delim: ConstPtr<u8>) -> MutPtr<u8> {
    let orig = env.mem.read(stringp);
    if orig.is_null() {
        return Ptr::null();
    }
    let tmp = orig;
    let mut i = 0;
    loop {
        let c = env.mem.read(tmp + i);
        if c == b'\0' {
            env.mem.write(stringp, Ptr::null());
            break;
        }
        let mut j = 0;
        loop {
            let cc = env.mem.read(delim + j);
            if c == cc {
                env.mem.write(tmp + i, b'\0');
                env.mem.write(stringp, tmp + i + 1);
                return orig;
            }
            if cc == b'\0' {
                break;
            }
            j += 1;
        }
        i += 1;
    }
    orig
}
pub(crate) fn strdup(env: &mut Environment, src: ConstPtr<u8>) -> MutPtr<u8> {
    GenericChar::<u8>::strdup(env, src)
}
pub fn strcmp(env: &mut Environment, a: ConstPtr<u8>, b: ConstPtr<u8>) -> i32 {
    GenericChar::<u8>::strcmp(env, a, b)
}
fn strncmp(env: &mut Environment, a: ConstPtr<u8>, b: ConstPtr<u8>, n: GuestUSize) -> i32 {
    GenericChar::<u8>::strncmp(env, a, b, n)
}
fn strcasecmp(env: &mut Environment, a: ConstPtr<u8>, b: ConstPtr<u8>) -> i32 {
    // TODO: generalize to wide chars
    let mut offset = 0;
    loop {
        let char_a = env.mem.read(a + offset).to_ascii_lowercase();
        let char_b = env.mem.read(b + offset).to_ascii_lowercase();
        offset += 1;

        match char_a.cmp(&char_b) {
            Ordering::Less => return -1,
            Ordering::Greater => return 1,
            Ordering::Equal => {
                if char_a == u8::default() {
                    return 0;
                } else {
                    continue;
                }
            }
        }
    }
}
fn strncasecmp(env: &mut Environment, a: ConstPtr<u8>, b: ConstPtr<u8>, n: GuestUSize) -> i32 {
    // TODO: generalize to wide chars
    if n == 0 {
        return 0;
    }

    let mut offset = 0;
    loop {
        let char_a = env.mem.read(a + offset).to_ascii_lowercase();
        let char_b = env.mem.read(b + offset).to_ascii_lowercase();
        offset += 1;

        match char_a.cmp(&char_b) {
            Ordering::Less => return -1,
            Ordering::Greater => return 1,
            Ordering::Equal => {
                if offset == n || char_a == u8::default() {
                    return 0;
                } else {
                    continue;
                }
            }
        }
    }
}
fn strncat(env: &mut Environment, s1: MutPtr<u8>, s2: ConstPtr<u8>, n: GuestUSize) -> MutPtr<u8> {
    GenericChar::<u8>::strncat(env, s1, s2, n)
}
fn strstr(env: &mut Environment, string: ConstPtr<u8>, substring: ConstPtr<u8>) -> ConstPtr<u8> {
    GenericChar::<u8>::strstr(env, string, substring)
}
fn strchr(env: &mut Environment, path: ConstPtr<u8>, c: u8) -> ConstPtr<u8> {
    GenericChar::<u8>::strchr(env, path, c)
}
fn strrchr(env: &mut Environment, path: ConstPtr<u8>, c: u8) -> ConstPtr<u8> {
    GenericChar::<u8>::strrchr(env, path, c)
}
fn strpbrk(env: &mut Environment, s: ConstPtr<u8>, charset: ConstPtr<u8>) -> ConstPtr<u8> {
    GenericChar::<u8>::strpbrk(env, s, charset)
}
fn strlcpy(
    env: &mut Environment,
    dst: MutPtr<u8>,
    src: ConstPtr<u8>,
    size: GuestUSize,
) -> GuestUSize {
    GenericChar::<u8>::strlcpy(env, dst, src, size)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(strtok(_, _)),
    export_c_func!(strtok_r(_, _, _)),
    export_c_func!(strnlen(_, _)),
    export_c_func!(strndup(_, _)),
    export_c_func!(strlcat(_, _, _)),
    export_c_func!(__strncat_chk(_, _, _, _)),
    export_c_func!(memrchr(_, _, _)),
    export_c_func!(index(_, _)),
    export_c_func!(memmem(_, _, _, _)),
    export_c_func!(stpcpy(_, _)),
    export_c_func!(stpncpy(_, _, _)),
    export_c_func!(strchrnul(_, _)),
    export_c_func!(strcasestr(_, _)),
    export_c_func!(bcopy(_, _, _)),
    export_c_func!(explicit_bzero(_, _)),
    export_c_func!(strverscmp(_, _)),
    export_c_func!(strfry(_)),
    export_c_func!(bzero(_, _)),
    // Functions shared with wchar.rs
    export_c_func!(memset(_, _, _)),
    export_c_func!(__memset_chk(_, _, _, _)),
    export_c_func!(memset_pattern4(_, _, _)),
    export_c_func!(memset_pattern8(_, _, _)),
    export_c_func!(memset_pattern16(_, _, _)),
    export_c_func!(memcpy(_, _, _)),
    export_c_func!(__memcpy_chk(_, _, _, _)),
    export_c_func!(memmove(_, _, _)),
    export_c_func!(__memmove_chk(_, _, _, _)),
    export_c_func!(memchr(_, _, _)),
    export_c_func!(memcmp(_, _, _)),
    export_c_func!(strlen(_)),
    export_c_func!(strcpy(_, _)),
    export_c_func!(__strcpy_chk(_, _, _)),
    export_c_func!(strcat(_, _)),
    export_c_func!(strspn(_, _)),
    export_c_func!(strcspn(_, _)),
    export_c_func!(__strcat_chk(_, _, _)),
    export_c_func!(strncpy(_, _, _)),
    export_c_func!(__strncpy_chk(_, _, _, _)),
    export_c_func!(strsep(_, _)),
    export_c_func!(strdup(_)),
    export_c_func!(strcmp(_, _)),
    export_c_func!(strncmp(_, _, _)),
    export_c_func!(strcasecmp(_, _)),
    export_c_func!(strncasecmp(_, _, _)),
    export_c_func!(strncat(_, _, _)),
    export_c_func!(strstr(_, _)),
    export_c_func!(strchr(_, _)),
    export_c_func!(strrchr(_, _)),
    export_c_func!(strpbrk(_, _)),
    export_c_func!(strlcpy(_, _, _)),
];
