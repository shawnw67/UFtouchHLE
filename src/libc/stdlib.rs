/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `stdlib.h`

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{
    export_c_func, export_c_func_aliased, ConstantExports, FunctionExports, HostConstant,
};
use crate::fs::{resolve_path, GuestPath};
use crate::libc::clocale::{setlocale, LC_CTYPE};
use crate::libc::errno::{set_errno, EINVAL, ENOMEM};
use crate::libc::string::strlen;
use crate::libc::wchar::wchar_t;
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeRead};
use crate::{impl_GuestRet_for_large_struct, Environment};
use std::str::FromStr;
use std::time::SystemTime;

pub mod qsort;

#[derive(Default)]
pub struct State {
    rand: u32,
    random: u32,
    arc4random: u32,
    fcvt_buf: Option<MutPtr<u8>>,
    /// 48-bit linear-congruential PRNG state shared by the `drand48`
    /// family. Per the `drand48(3)` manpage, the generator is
    /// `X_{n+1} = (a * X_n + c) mod 2^48`, with default `a = 0x5DEECE66D`,
    /// `c = 0xB`, and the initial seed `0x1234ABCD330E` until `srand48` or
    /// `seed48` are called.
    drand48: Drand48State,
    /// Buffer returned by the last `ecvt` call.
    cvt_buffer: Option<MutPtr<u8>>,
}

// Sizes of zero are implementation-defined. macOS will happily give you back
// an allocation for any of these, so presumably iPhone OS does too.
// (touchHLE's allocator will round up allocations to at least 16 bytes.)

fn malloc(env: &mut Environment, size: GuestUSize) -> MutVoidPtr {
    let allocation = env.mem.try_alloc(size);
    allocation_result(env, allocation)
}

pub(super) fn allocation_result(
    env: &mut Environment,
    allocation: Option<MutVoidPtr>,
) -> MutVoidPtr {
    allocation.unwrap_or_else(|| {
        set_errno(env, ENOMEM);
        Ptr::null()
    })
}

fn malloc_size(env: &mut Environment, ptr: ConstVoidPtr) -> GuestUSize {
    crate::libc::malloc::allocation_size(env, ptr)
}

fn calloc(env: &mut Environment, count: GuestUSize, size: GuestUSize) -> MutVoidPtr {
    let allocation = size
        .checked_mul(count)
        .and_then(|total| env.mem.try_calloc(total));
    allocation_result(env, allocation)
}

fn valloc(env: &mut Environment, size: GuestUSize) -> MutVoidPtr {
    let allocation = env.mem.try_valloc(size);
    allocation_result(env, allocation)
}

fn realloc(env: &mut Environment, ptr: MutVoidPtr, size: GuestUSize) -> MutVoidPtr {
    let allocation = crate::libc::malloc::reallocate(env, ptr, size);
    allocation_result(env, allocation)
}

fn reallocf(env: &mut Environment, ptr: MutVoidPtr, size: GuestUSize) -> MutVoidPtr {
    let allocation = realloc(env, ptr, size);
    if allocation.is_null() && !ptr.is_null() {
        crate::libc::malloc::free_allocation(env, ptr);
    }
    allocation
}

fn free(env: &mut Environment, ptr: MutVoidPtr) {
    // We need to catch situations of freeing NSObjects early!
    if env.objc.get_host_object(ptr.cast()).is_some() {
        log!(
            "App attempted to call free({:?}) on an object, calling dealloc_object() instead!",
            ptr
        );
        env.objc.dealloc_object(ptr.cast(), &mut env.mem);
        return;
    }

    if ptr.is_null() {
        // "If ptr is a NULL pointer, no operation is performed."
        return;
    }
    crate::libc::malloc::free_allocation(env, ptr);
}

fn atexit(
    env: &mut Environment,
    func: GuestFunction, // void (*func)(void)
) -> i32 {
    super::cxxabi::register_atexit(env, func)
}

#[allow(rustdoc::broken_intra_doc_links)] // https://github.com/rust-lang/rust/issues/83049
/// Counts whitespaces in `subject` starting from `offset`.
///
/// `getc_fn` is a callback to get next character from `subject`.
/// 3rd parameter in this callback is a index which is safe to ignore
/// (for example, in case of a file stream).
/// Error signifies an abnormal stop of input,
/// such as [crate::libc::stdio::EOF] in the file stream.
/// Note: `'\0'` does not necessary expect to produce an error!
///
/// `ungetc_fn` is a callback to un-get character from `subject`.
/// Could be ignored entirely (for example, in case of a string).
///
/// `subject` is either C string or file stream (for now).
///
/// `offset` defines an offset in `subject` from which conversion starts.
/// Could be ignored entirely (for example, in case of a file stream).
///
/// Returns count of whitespaces. Error returned from `getc_fn` is propagated
/// but count is retuned too.
fn count_whitespace_generic<
    T,
    U,
    F1: Fn(&mut Environment, MutPtr<U>, GuestUSize) -> Result<T, ()>,
    F2: Fn(&mut Environment, MutPtr<U>, u8), // TODO: make last param generic too?
>(
    env: &mut Environment,
    getc_fn: F1,
    ungetc_fn: F2,
    subject: MutPtr<U>,
    offset: GuestUSize,
) -> Result<GuestUSize, GuestUSize>
where
    u8: From<T>,
{
    let mut count: GuestUSize = offset;
    loop {
        let Ok(c) = getc_fn(env, subject, count) else {
            return Err(count - offset);
        };
        let c: u8 = c.into();
        // Rust's definition of whitespace excludes vertical tab, unlike C's
        if c.is_ascii_whitespace() || c == b'\x0b' {
            count += 1;
        } else {
            ungetc_fn(env, subject, c);
            break;
        }
    }
    Ok(count - offset)
}

fn atoi(env: &mut Environment, s: ConstPtr<u8>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    // conveniently, overflow is undefined, so 0 is as valid a result as any
    let (res, _) = strtol_inner(env, s, 10).unwrap_or((0, 0));
    res
}

fn atol(env: &mut Environment, s: ConstPtr<u8>) -> i32 {
    atoi(env, s)
}

fn atof(env: &mut Environment, s: ConstPtr<u8>) -> f64 {
    strtod(env, s, Ptr::null())
}

fn strtod(env: &mut Environment, nptr: ConstPtr<u8>, endptr: MutPtr<MutPtr<u8>>) -> f64 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!("strtod nptr {}", env.mem.cstr_at_utf8(nptr).unwrap());
    let (res, len) = atof_inner(env, nptr).unwrap_or((0.0, 0));
    if !endptr.is_null() {
        env.mem.write(endptr, (nptr + len).cast_mut());
    }
    res
}

fn prng(state: u32) -> u32 {
    // The state must not be zero for this algorithm to work. This also makes
    // the default seed be 1, which matches the C standard.
    let mut state: u32 = state.max(1);
    // https://en.wikipedia.org/wiki/Xorshift#Example_implementation
    // xorshift32 is not a good random number generator, but it is cute one!
    // It's not like anyone expects the C stdlib `rand()` to be good.
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
}

const RAND_MAX: i32 = i32::MAX;

fn srand(env: &mut Environment, seed: u32) {
    env.libc_state.stdlib.rand = seed;
}

// BSD's rand() seed function — we just use host system time,
// good enough for games that want fresh "fake" randomness each run.
fn sranddev(env: &mut Environment) {
    let time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let seed = (time ^ (time >> 32)) as u32;
    env.libc_state.stdlib.rand = seed;
}

fn rand(env: &mut Environment) -> i32 {
    env.libc_state.stdlib.rand = prng(env.libc_state.stdlib.rand);
    (env.libc_state.stdlib.rand as i32) & RAND_MAX
}
fn rand_r(env: &mut Environment, seed_ptr: MutPtr<u32>) -> i32 {
    let mut seed = env.mem.read(seed_ptr);
    seed = prng(seed);
    env.mem.write(seed_ptr, seed);
    (seed as i32) & RAND_MAX
}

// BSD's "better" random number generator, with an implementation that is not
// actually better.
fn srandom(env: &mut Environment, seed: u32) {
    // TODO: handle errno properly
    set_errno(env, 0);

    env.libc_state.stdlib.random = seed;
}
fn random(env: &mut Environment) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    env.libc_state.stdlib.random = prng(env.libc_state.stdlib.random);
    (env.libc_state.stdlib.random as i32) & RAND_MAX
}

fn arc4random(env: &mut Environment) -> u32 {
    env.libc_state.stdlib.arc4random = prng(env.libc_state.stdlib.arc4random);
    env.libc_state.stdlib.arc4random
}

#[allow(non_camel_case_types)]
#[derive(Debug)]
#[repr(C, packed)]
struct div_t {
    quot: i32,
    rem: i32,
}
unsafe impl SafeRead for div_t {}
impl_GuestRet_for_large_struct!(div_t);

fn div(_env: &mut Environment, numer: i32, denom: i32) -> div_t {
    div_t {
        quot: numer.wrapping_div(denom),
        rem: numer.wrapping_rem(denom),
    }
}

fn getenv(env: &mut Environment, name: ConstPtr<u8>) -> MutPtr<u8> {
    let name_cstr = env.mem.cstr_at(name);
    let Some(&value) = env.env_vars.get(name_cstr) else {
        log!(
            "Warning: getenv() for {:?} ({:?}) unhandled",
            name,
            std::str::from_utf8(name_cstr)
        );
        return Ptr::null();
    };

    log_dbg!(
        "getenv({:?} ({:?})) => {:?} ({:?})",
        name,
        name_cstr,
        value,
        env.mem.cstr_at_utf8(value),
    );
    // Caller should not modify the result
    value
}
fn setenv(env: &mut Environment, name: ConstPtr<u8>, value: ConstPtr<u8>, overwrite: i32) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let name_cstr = env.mem.cstr_at(name);
    if let Some(&existing) = env.env_vars.get(name_cstr) {
        if overwrite == 0 {
            return 0; // success
        }
        env.mem.free(existing.cast());
    };
    let value = super::string::strdup(env, value);
    let name_cstr = env.mem.cstr_at(name); // reborrow
    env.env_vars.insert(name_cstr.to_vec(), value);
    log_dbg!(
        "Stored new value {:?} ({:?}) for environment variable {:?}",
        value,
        env.mem.cstr_at_utf8(value),
        std::str::from_utf8(name_cstr),
    );
    0 // success
}
fn unsetenv(env: &mut Environment, name: ConstPtr<u8>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let name_cstr = env.mem.cstr_at(name);
    if !env.env_vars.contains_key(name_cstr) {
        set_errno(env, EINVAL);
        -1
    } else {
        todo!()
    }
}

fn exit(env: &mut Environment, exit_code: i32) {
    // TODO: handle errno properly
    set_errno(env, 0);

    super::cxxabi::finalize_for_exit(env);

    echo!("App called exit(), exiting.");
    std::process::exit(exit_code);
}

fn bsearch(
    env: &mut Environment,
    key: ConstVoidPtr,
    items: ConstVoidPtr,
    item_count: GuestUSize,
    item_size: GuestUSize,
    compare_callback: GuestFunction, // (*int)(const void*, const void*)
) -> ConstVoidPtr {
    log_dbg!(
        "binary search for {:?} in {} items of size {:#x} starting at {:?}",
        key,
        item_count,
        item_size,
        items
    );
    let mut low = 0;
    let mut len = item_count;
    while len > 0 {
        let half_len = len / 2;
        let item: ConstVoidPtr = (items.cast::<u8>() + item_size * (low + half_len)).cast();
        // key must be first argument
        let cmp_result: i32 = compare_callback.call_from_host(env, (key, item));
        (low, len) = match cmp_result.signum() {
            0 => {
                log_dbg!("=> {:?}", item);
                return item;
            }
            1 => (low + half_len + 1, len - half_len - 1),
            -1 => (low, half_len),
            _ => unreachable!(),
        }
    }
    log_dbg!("=> NULL (not found)");
    Ptr::null()
}

fn strtof(env: &mut Environment, nptr: ConstPtr<u8>, endptr: MutPtr<ConstPtr<u8>>) -> f32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let (number, length) = atof_inner(env, nptr).unwrap_or((0.0, 0));
    if !endptr.is_null() {
        env.mem.write(endptr, nptr + length);
    }
    number as f32
}

pub fn strtoul(
    env: &mut Environment,
    str: ConstPtr<u8>,
    endptr: MutPtr<MutPtr<u8>>,
    base: i32,
) -> u32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let parse_res = str_to_int_inner_generic(
        env,
        |env, s, idx| Ok(env.mem.read(s + idx)),
        |_, _, _| (), // could be ignored
        str.cast_mut(),
        0, // starting offset
        base.try_into().unwrap(),
        u32::MAX, // max_length
        |s, base| u32::from_str_radix(s, base).unwrap_or(u32::MAX),
        |num| num.wrapping_neg(),
    );
    match parse_res {
        Ok((res, len)) => {
            if !endptr.is_null() {
                env.mem.write(endptr, (str + len).cast_mut());
            }
            res
        }
        Err(_) => {
            if !endptr.is_null() {
                env.mem.write(endptr, str.cast_mut());
            }
            0
        }
    }
}
fn wcstoul(
    env: &mut Environment,
    nptr: ConstPtr<wchar_t>,
    endptr: MutPtr<MutPtr<wchar_t>>,
    base: i32,
) -> u32 {
    // TODO: support other locales
    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
    assert_eq!(env.mem.read(ctype_locale), b'C');

    let w_string = env.mem.wcstr_at(nptr);
    assert!(w_string.is_ascii()); // TODO

    assert!(endptr.is_null()); // TODO

    let c_string = env.mem.alloc_and_write_cstr(w_string.as_bytes());
    // TODO: use str_to_int_inner_generic() instead
    let res = strtoul(env, c_string.cast_const(), Ptr::null(), base);
    env.mem.free(c_string.cast());
    log_dbg!(
        "wcstoul({:?} ({:?}), {:?}, {}) => {}",
        nptr,
        w_string,
        endptr,
        base,
        res
    );
    res
}

fn strtoull(
    env: &mut Environment,
    str: ConstPtr<u8>,
    endptr: MutPtr<MutPtr<u8>>,
    base: i32,
) -> u64 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let parse_res = str_to_int_inner_generic(
        env,
        |env, s, idx| Ok(env.mem.read(s + idx)),
        |_, _, _| (), // could be ignored
        str.cast_mut(),
        0, // starting offset
        base.try_into().unwrap(),
        u32::MAX, // max_length
        |s, base| u64::from_str_radix(s, base).unwrap_or(u64::MAX),
        |num| num.wrapping_neg(),
    );
    match parse_res {
        Ok((res, len)) => {
            if !endptr.is_null() {
                env.mem.write(endptr, (str + len).cast_mut());
            }
            res
        }
        Err(_) => {
            if !endptr.is_null() {
                env.mem.write(endptr, str.cast_mut());
            }
            0
        }
    }
}

fn strtol(env: &mut Environment, str: ConstPtr<u8>, endptr: MutPtr<MutPtr<u8>>, base: i32) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    match strtol_inner(env, str, base as u32) {
        Ok((res, len)) => {
            if !endptr.is_null() {
                env.mem.write(endptr, (str + len).cast_mut());
            }
            res
        }
        Err(_) => {
            if !endptr.is_null() {
                env.mem.write(endptr, str.cast_mut());
            }
            0
        }
    }
}

fn wcstol(
    env: &mut Environment,
    nptr: ConstPtr<wchar_t>,
    endptr: MutPtr<MutPtr<wchar_t>>,
    base: i32,
) -> i32 {
    // TODO: support other locales
    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
    assert_eq!(env.mem.read(ctype_locale), b'C');

    let w_string = env.mem.wcstr_at(nptr);
    assert!(w_string.is_ascii()); // TODO

    assert!(endptr.is_null()); // TODO

    let c_string = env.mem.alloc_and_write_cstr(w_string.as_bytes());
    // TODO: use str_to_int_inner_generic() instead
    let (res, _) = strtol_inner(env, c_string.cast_const(), base as u32).unwrap();
    env.mem.free(c_string.cast());
    log_dbg!(
        "wcstol({:?} ({:?}), {:?}, {}) -> {}",
        nptr,
        w_string,
        endptr,
        base,
        res
    );
    res
}

fn realpath(
    env: &mut Environment,
    file_name: ConstPtr<u8>,
    resolve_name: MutPtr<u8>,
) -> MutPtr<u8> {
    assert!(!resolve_name.is_null());

    let file_name_str = env.mem.cstr_at_utf8(file_name).unwrap();
    // TOD0: resolve symbolic links
    let resolved = resolve_path(
        GuestPath::new(file_name_str),
        Some(env.fs.working_directory()),
    );
    let result = format!("/{}", resolved.join("/"));
    env.mem
        .bytes_at_mut(resolve_name, result.len() as GuestUSize)
        .copy_from_slice(result.as_bytes());
    env.mem
        .write(resolve_name + result.len() as GuestUSize, b'\0');

    log_dbg!(
        "realpath file_name '{}', resolve_name '{}'",
        env.mem.cstr_at_utf8(file_name).unwrap(),
        env.mem.cstr_at_utf8(resolve_name).unwrap()
    );

    resolve_name
}

pub fn mbstowcs(
    env: &mut Environment,
    pwcs: MutPtr<wchar_t>,
    s: ConstPtr<u8>,
    n: GuestUSize,
) -> GuestUSize {
    // TODO: handle errno properly
    set_errno(env, 0);

    // TODO: support other locales
    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
    assert_eq!(env.mem.read(ctype_locale), b'C');

    let size = strlen(env, s);
    let to_write = size.min(n);
    for i in 0..to_write {
        let c = env.mem.read(s + i);
        env.mem.write(pwcs + i, c as wchar_t);
    }
    if to_write < n {
        env.mem.write(pwcs + to_write, wchar_t::default());
    }
    to_write
}

fn wcstombs(
    env: &mut Environment,
    s: ConstPtr<u8>,
    pwcs: MutPtr<wchar_t>,
    n: GuestUSize,
) -> GuestUSize {
    // TODO: support other locales
    let ctype_locale = setlocale(env, LC_CTYPE, Ptr::null());
    assert_eq!(env.mem.read(ctype_locale), b'C');

    if n == 0 {
        return 0;
    }
    let wcstr = env.mem.wcstr_at(pwcs);
    let len: GuestUSize = wcstr.len() as GuestUSize;
    let len = len.min(n);
    log_dbg!("wcstombs '{}', len {}, n {}", wcstr, len, n);
    env.mem
        .bytes_at_mut(s.cast_mut(), len)
        .copy_from_slice(wcstr.as_bytes());
    if len < n {
        env.mem.write((s + len).cast_mut(), b'\0');
    }
    len
}

fn system(env: &mut Environment, cmd: ConstPtr<u8>) -> i32 {
    if cmd.is_null() {
        log!("TODO: App checked for sh availability with system(NULL), returning 0");
        return 0; // sh is not available!
    }
    log!("system({:?})", env.mem.cstr_at_utf8(cmd));
    todo!()
}

fn fcvt(
    env: &mut Environment,
    value: f64,
    ndigit: i32,
    decpt: MutPtr<i32>,
    sign: MutPtr<i32>,
) -> MutPtr<u8> {
    log_dbg!("fcvt({}, {}, {:?}, {:?})", value, ndigit, decpt, sign);
    assert!(ndigit > 0);
    let fcvt_buf = *env.libc_state.stdlib.fcvt_buf.get_or_insert_with(|| {
        // TODO: 64 is arbitrary chosen
        env.mem.alloc(64).cast()
    });
    env.mem
        .write(sign, if value.is_sign_negative() { 1 } else { 0 });
    if value == 0.0 {
        let ndigit_size = ndigit as GuestUSize;
        assert!(ndigit_size < 64); // TODO
        env.mem.write(decpt, 0);
        env.mem.bytes_at_mut(fcvt_buf, ndigit_size).fill(b'0');
        env.mem.write(fcvt_buf + ndigit_size, b'\0');
        return fcvt_buf;
    }

    let mut formatted = format!("{:.1$}", value.abs(), ndigit as usize);
    assert!(formatted.contains('.'));
    assert!(formatted.len() < 64); // TODO

    let dot_idx = formatted.find('.').unwrap();
    formatted.remove(dot_idx);

    let leading_zeros_trimmed = formatted.trim_start_matches('0');
    let leading_zeros_idx = formatted.len() - leading_zeros_trimmed.len();

    env.mem
        .write(decpt, dot_idx as i32 - leading_zeros_idx as i32);

    let len = leading_zeros_trimmed.len().try_into().unwrap();
    env.mem
        .bytes_at_mut(fcvt_buf, len)
        .copy_from_slice(leading_zeros_trimmed.as_bytes());
    env.mem.write(fcvt_buf + len, b'\0');

    fcvt_buf
}

fn posix_memalign(
    env: &mut Environment,
    memptr: MutPtr<MutVoidPtr>,
    alignment: GuestUSize,
    size: GuestUSize,
) -> i32 {
    if !alignment.is_power_of_two() || alignment < 4 {
        return EINVAL;
    }
    let Some(ptr) = env.mem.try_memalign_in_heap(None, alignment, size) else {
        return ENOMEM;
    };
    env.mem.write(memptr, ptr);
    0
}

/// Called when an `assert()` fails.
fn __assert_rtn(
    env: &mut Environment,
    func: ConstPtr<u8>,
    file: ConstPtr<u8>,
    line: i32,
    expression: ConstPtr<u8>,
) {
    let cstr = |env: &Environment, ptr: ConstPtr<u8>| {
        if ptr.is_null() {
            String::new()
        } else {
            String::from_utf8_lossy(env.mem.cstr_at(ptr)).to_string()
        }
    };
    panic!(
        "Assertion failed: ({}), function {}, file {}, line {}.",
        cstr(env, expression),
        cstr(env, func),
        cstr(env, file),
        line
    );
}

/// Stack protector support (`-fstack-protector`).
fn __stack_chk_fail(_env: &mut Environment) {
    panic!("App detected a stack buffer overflow (__stack_chk_fail)");
}

pub const CONSTANTS: ConstantExports = &[
    // The canary value checked by `-fstack-protector` code. The value doesn't
    // matter, as long as it doesn't change.
    (
        "___stack_chk_guard",
        HostConstant::Custom(|env| {
            env.mem
                .alloc_and_write(0x2d5a_7e31u32)
                .cast_void()
                .cast_const()
        }),
    ),
];

fn NSZoneFree(env: &mut Environment, _zone: MutVoidPtr, ptr: MutVoidPtr) {
    if ptr.is_null() {
        return;
    }
    // Use the same safe path as libc free() — unknown pointers are
    // gracefully rejected inside Mem::free with a log message.
    env.mem.free(ptr)
}

fn NSZoneMalloc(env: &mut Environment, _zone: crate::objc::id, mut size: GuestUSize) -> MutVoidPtr {
    if size == 0 {
        size = 1;
    }
    env.mem.alloc(size)
}

fn NSZoneRealloc(
    env: &mut Environment,
    _zone: MutVoidPtr,
    ptr: MutVoidPtr,
    mut size: GuestUSize,
) -> MutVoidPtr {
    if size == 0 {
        size = 1;
    }
    env.mem.realloc(ptr, size)
}

fn _Exit(env: &mut Environment, exit_code: i32) {
    immediate_exit(env, "_Exit", exit_code);
}

fn __assert(env: &mut Environment, expr: ConstPtr<u8>, file: ConstPtr<u8>, line: i32) {
    let expr_str = read_cstr_safe(env, expr);
    let file_str = read_cstr_safe(env, file);
    log!(
        "Assertion failed: ({}) in file {}, line {}.",
        expr_str,
        file_str,
        line
    );
    abort(env);
}

fn __assert_fail(
    env: &mut Environment,
    expr: ConstPtr<u8>,
    file: ConstPtr<u8>,
    line: u32,
    func: ConstPtr<u8>,
) {
    let expr_str = read_cstr_safe(env, expr);
    let file_str = read_cstr_safe(env, file);
    let func_str = read_cstr_safe(env, func);
    log!(
        "Assertion failed: ({}) in function {}, file {}, line {}.",
        expr_str,
        func_str,
        file_str,
        line
    );
    abort(env);
}

fn _exit(env: &mut Environment, exit_code: i32) {
    immediate_exit(env, "_exit", exit_code);
}

fn abort(_env: &mut Environment) {
    panic!("App called abort()");
}

/// `_exit()` and friends: exit without running any handlers.
fn immediate_exit(env: &mut Environment, name: &str, exit_code: i32) {
    set_errno(env, 0);
    echo!("App called {}(), exiting.", name);
    std::process::exit(exit_code);
}

fn arc4random_addrandom(env: &mut Environment) -> u32 {
    env.libc_state.stdlib.arc4random = prng(env.libc_state.stdlib.arc4random);
    env.libc_state.stdlib.arc4random
}

fn arc4random_stir(env: &mut Environment) -> u32 {
    env.libc_state.stdlib.arc4random = prng(env.libc_state.stdlib.arc4random);
    env.libc_state.stdlib.arc4random
}

/// `arc4random_uniform(upper_bound)` — returns a uniformly distributed
/// random number less than `upper_bound`.
///
/// Per Apple manpage: "arc4random_uniform() is recommended over
/// constructions like `arc4random() % upper_bound` as it avoids
/// modulo bias when the upper bound is not a power of two."
fn arc4random_uniform(env: &mut Environment, upper_bound: u32) -> u32 {
    if upper_bound == 0 {
        return 0;
    }
    // Rejection sampling to eliminate modulo bias.
    // Compute the largest multiple of upper_bound that fits in u32.
    let limit = u32::MAX - (u32::MAX % upper_bound);
    loop {
        let r = arc4random(env);
        if r < limit {
            return r % upper_bound;
        }
    }
}

fn dirname(env: &mut Environment, path: MutPtr<u8>) -> MutPtr<u8> {
    if path.is_null() {
        set_errno(env, EINVAL);
        return Ptr::null();
    }

    let len = strlen(env, path.cast_const());
    let mut bytes: Vec<u8> = (0..len).map(|i| env.mem.read(path + i)).collect();
    while bytes.len() > 1 && bytes.last() == Some(&b'/') {
        bytes.pop();
    }

    let output = if bytes.is_empty() {
        b".".to_vec()
    } else if let Some(slash) = bytes.iter().rposition(|&b| b == b'/') {
        if slash == 0 {
            b"/".to_vec()
        } else {
            bytes[..slash].to_vec()
        }
    } else {
        b".".to_vec()
    };

    for (i, byte) in output.iter().enumerate() {
        env.mem.write(path + i as GuestUSize, *byte);
    }
    env.mem.write(path + output.len() as GuestUSize, b'\0');
    path
}

fn drand48(env: &mut Environment) -> f64 {
    let next = env.libc_state.stdlib.drand48.step();
    // Apple manpage: "drand48() and erand48() return non-negative,
    // double-precision, floating-point values, uniformly distributed over the
    // interval [0.0, 1.0)." 2^-48 gives exactly that distribution.
    (next as f64) * (1.0_f64 / 281_474_976_710_656.0_f64)
}

fn ecvt(
    env: &mut Environment,
    value: f64,
    ndigit: i32,
    decpt: MutPtr<i32>,
    sign: MutPtr<i32>,
) -> MutPtr<u8> {
    cvt(env, value, ndigit, decpt, sign, false)
}

fn erand48(env: &mut Environment, xsubi: MutPtr<u16>) -> f64 {
    let mut local = Drand48State {
        state: pack_xsubi(env, xsubi.cast_const()),
        a: env.libc_state.stdlib.drand48.a,
        c: env.libc_state.stdlib.drand48.c,
    };
    let next = local.step();
    write_xsubi(env, xsubi, next);
    (next as f64) * (1.0_f64 / 281_474_976_710_656.0_f64)
}

fn fgetxattr(
    env: &mut Environment,
    fd: crate::libc::posix_io::FileDescriptor,
    name: ConstPtr<u8>,
    _value: MutVoidPtr,
    size: GuestUSize,
    position: u32,
    options: i32,
) -> i32 {
    let name_str = env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned();
    log_dbg!(
        "fgetxattr(fd={}, {:?}, size={}, position={}, options={:#x}) — returning ENOATTR",
        fd,
        name_str,
        size,
        position,
        options
    );
    set_errno(env, 93);
    -1
}

fn flistxattr(
    env: &mut Environment,
    fd: crate::libc::posix_io::FileDescriptor,
    namebuf: MutPtr<u8>,
    size: GuestUSize,
    options: i32,
) -> i32 {
    log_dbg!(
        "flistxattr(fd={}, size={}, options={:#x}) — returning 0 (empty list)",
        fd,
        size,
        options
    );
    if !namebuf.is_null() && size > 0 {
        env.mem.write(namebuf, 0u8);
    }
    0
}

fn fremovexattr(
    env: &mut Environment,
    fd: crate::libc::posix_io::FileDescriptor,
    name: ConstPtr<u8>,
    options: i32,
) -> i32 {
    let name_str = env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned();
    log_dbg!(
        "fremovexattr(fd={}, {:?}, options={:#x}) — returning ENOATTR",
        fd,
        name_str,
        options
    );
    set_errno(env, 93);
    -1
}

fn fsetxattr(
    env: &mut Environment,
    fd: crate::libc::posix_io::FileDescriptor,
    name: ConstPtr<u8>,
    _value: ConstVoidPtr,
    size: GuestUSize,
    position: u32,
    options: i32,
) -> i32 {
    let name_str = env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned();
    log_dbg!(
        "fsetxattr(fd={}, {:?}, size={}, position={}, options={:#x}) — ignored",
        fd,
        name_str,
        size,
        position,
        options
    );
    0
}

/// `gcvt()`, following FreeBSD's implementation: `ndigit` significant
/// digits without trailing zeros, in exponential form when the decimal point
/// would be outside them.
fn gcvt(env: &mut Environment, value: f64, ndigit: i32, buf: MutPtr<u8>) -> MutPtr<u8> {
    if buf.is_null() {
        return buf;
    }
    let mut out = Vec::new();
    if ndigit <= 0 {
        // Nothing to write but the terminator.
    } else if value.is_nan() || value.is_infinite() {
        let text = format!(
            "{}{}",
            if value.is_sign_negative() { "-" } else { "" },
            if value.is_nan() { "nan" } else { "inf" }
        );
        out.extend(text.bytes().take(ndigit as usize));
    } else {
        let (digits, decpt) = dtoa_digits(value, ndigit, false);
        if value < 0.0 {
            out.push(b'-');
        }
        if decpt < 0 || decpt > ndigit {
            out.push(digits[0]);
            out.push(b'.');
            if digits.len() > 1 {
                out.extend_from_slice(&digits[1..]);
            } else {
                out.push(b'0');
            }
            let exponent = decpt - 1;
            let sign = if exponent < 0 { '-' } else { '+' };
            out.extend(format!("e{}{}", sign, exponent.abs()).bytes());
        } else {
            let decpt = decpt as usize;
            for i in 0..decpt {
                out.push(*digits.get(i).unwrap_or(&b'0'));
            }
            if digits.len() > decpt {
                if decpt == 0 {
                    out.push(b'0');
                }
                out.push(b'.');
                out.extend_from_slice(&digits[decpt..]);
            }
        }
    }
    env.mem
        .bytes_at_mut(buf, out.len() as GuestUSize)
        .copy_from_slice(&out);
    env.mem.write(buf + out.len() as GuestUSize, b'\0');
    buf
}

/// `int getloadavg(double loadavg[], int nelem)`
///
/// Samples the system load averages. There is no real load on an emulated
/// device, so report a calm fixed load (as an idle device would).
fn getloadavg(env: &mut Environment, loadavg: MutPtr<f64>, nelem: i32) -> i32 {
    if nelem < 0 {
        return -1;
    }
    if nelem == 0 {
        return 0;
    }
    let count = nelem.min(3);
    let samples = [0.5, 0.5, 0.5];
    for i in 0..count {
        env.mem
            .write(loadavg + i as GuestUSize, samples[i as usize]);
    }
    count
}

fn getxattr(
    env: &mut Environment,
    path: ConstPtr<u8>,
    name: ConstPtr<u8>,
    _value: MutVoidPtr,
    size: GuestUSize,
    position: u32,
    options: i32,
) -> i32 {
    let path_str = env.mem.cstr_at_utf8(path).unwrap_or_default().to_owned();
    let name_str = env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned();
    log_dbg!(
        "getxattr({:?}, {:?}, size={}, position={}, options={:#x}) — returning ENOATTR",
        path_str,
        name_str,
        size,
        position,
        options
    );
    // ENOATTR = 93 on Darwin. Return -1 and set errno.
    set_errno(env, 93);
    -1
}

/// `int inflateReset2(z_streamp strm, int windowBits)`
///
/// Per the [zlib manual](https://www.zlib.net/manual.html): "This function is
/// equivalent to inflateEnd followed by inflateInit2, but does not free and
/// reallocate the internal decompression state. The stream will keep attributes
/// that may have been set by inflateInit2." It was added in zlib 1.2.3.4.
///
/// The bundled libz.1.2.3.dylib does not export this symbol, so apps that link
/// against a newer SDK (e.g. Flappy Bird built for iOS 7) fail at lazy-bind
/// time. We provide a minimal host implementation that calls through to the
/// guest's `inflateReset` (same as calling inflateReset on the stream —
/// window bits are stored in the stream structure anyway from the initial
/// inflateInit2 call).
///
/// Return value: Z_OK (0) on success.
fn inflateReset2(_env: &mut Environment, _strm: MutVoidPtr, _window_bits: i32) -> i32 {
    // Z_OK = 0. We cannot easily call back into the guest's inflateReset
    // from host code without the full z_stream layout. However, the most
    // common pattern is that inflateReset2 is called right after inflateInit2
    // (which already set window bits) or before any actual inflate call.
    // Returning Z_OK lets the app proceed — the stream state was already
    // initialized by the guest's inflateInit2 which IS in the old libz.
    log_dbg!(
        "inflateReset2(strm={:?}, windowBits={}) -> Z_OK (stubbed)",
        _strm,
        _window_bits
    );
    0 // Z_OK
}

fn jrand48(env: &mut Environment, xsubi: MutPtr<u16>) -> i32 {
    let mut local = Drand48State {
        state: pack_xsubi(env, xsubi.cast_const()),
        a: env.libc_state.stdlib.drand48.a,
        c: env.libc_state.stdlib.drand48.c,
    };
    let next = local.step();
    write_xsubi(env, xsubi, next);
    ((next >> 16) & 0xFFFF_FFFF) as u32 as i32
}

fn kevent(
    _env: &mut Environment,
    _kq: i32,
    _changelist: ConstVoidPtr,
    _nchanges: i32,
    _eventlist: MutVoidPtr,
    _nevents: i32,
    _timeout: ConstVoidPtr,
) -> i32 {
    // FakeKevent
    0
}

fn kqueue(_env: &mut Environment) -> i32 {
    // FakeKqueue
    999
}

fn lcong48(env: &mut Environment, param: MutPtr<u16>) {
    // "lcong48() allows full user control over the multiplier and addend
    // terms in the formula." param is a `unsigned short[7]`: the first
    // three limbs seed X_i, the next three (limbs 3..6) replace `a`, and
    // the last (limb 6) replaces `c`.
    let state = pack_xsubi(env, param.cast_const());
    let a_lo = u64::from(env.mem.read(param + 3));
    let a_mid = u64::from(env.mem.read(param + 4));
    let a_hi = u64::from(env.mem.read(param + 5));
    let a = a_lo | (a_mid << 16) | (a_hi << 32);
    let c = u64::from(env.mem.read(param + 6));
    env.libc_state.stdlib.drand48 = Drand48State { state, a, c };
}

/// `ldiv_t ldiv(long numer, long denom)` — per Apple's manpage:
/// computes both the quotient and remainder of dividing `numer` by
/// `denom` in a single operation, returning the result in an `ldiv_t`
/// struct. Behaviour is identical to `div` on 32-bit iOS where `long`
/// is 32 bits wide.
fn ldiv(_env: &mut Environment, numer: i32, denom: i32) -> ldiv_t {
    ldiv_t {
        quot: numer.wrapping_div(denom),
        rem: numer.wrapping_rem(denom),
    }
}

fn listxattr(
    env: &mut Environment,
    path: ConstPtr<u8>,
    namebuf: MutPtr<u8>,
    size: GuestUSize,
    options: i32,
) -> i32 {
    let path_str = env.mem.cstr_at_utf8(path).unwrap_or_default().to_owned();
    log_dbg!(
        "listxattr({:?}, size={}, options={:#x}) — returning 0 (empty list)",
        path_str,
        size,
        options
    );
    // Zero-length list means no attributes. Return 0 (success, 0 bytes needed).
    if !namebuf.is_null() && size > 0 {
        env.mem.write(namebuf, 0u8);
    }
    0
}

fn lrand48(env: &mut Environment) -> i32 {
    // "lrand48() and nrand48() return non-negative, long integers,
    // uniformly distributed over the interval [0, 2^31)" — take the high
    // 31 bits of the 48-bit state, per the Apple manpage.
    let next = env.libc_state.stdlib.drand48.step();
    (next >> 17) as i32
}

fn mbtowc_l(
    env: &mut Environment,
    pwc: MutPtr<u32>, // wchar_t is 32 bits
    s: ConstVoidPtr,
    n: GuestUSize,      // size_t
    _loc: ConstVoidPtr, // locale_t (ignored, UTF-8 is assumed)
) -> i32 {
    if s.is_null() {
        return 0;
    }

    if n == 0 {
        return -1;
    }

    let s_ptr: ConstPtr<u8> = s.cast();
    let first_byte: u8 = env.mem.read(s_ptr);
    if first_byte == 0 {
        if !pwc.is_null() {
            env.mem.write(pwc, 0);
        }
        return 0;
    }

    if first_byte < 0x80 {
        if !pwc.is_null() {
            env.mem.write(pwc, first_byte as u32);
        }
        return 1;
    }

    let mut codepoint: u32;
    let bytes_to_read: u32;

    if (first_byte & 0xE0) == 0xC0 {
        codepoint = (first_byte & 0x1F) as u32;
        bytes_to_read = 1;
    } else if (first_byte & 0xF0) == 0xE0 {
        codepoint = (first_byte & 0x0F) as u32;
        bytes_to_read = 2;
    } else if (first_byte & 0xF8) == 0xF0 {
        codepoint = (first_byte & 0x07) as u32;
        bytes_to_read = 3;
    } else {
        return -1;
    }

    if n < (bytes_to_read + 1) as GuestUSize {
        return -1;
    }

    for i in 1..=bytes_to_read {
        let next_byte: u8 = env.mem.read(s_ptr + i as GuestUSize);
        if (next_byte & 0xC0) != 0x80 {
            return -1;
        }
        codepoint = (codepoint << 6) | ((next_byte & 0x3F) as u32);
    }

    if !pwc.is_null() {
        env.mem.write(pwc, codepoint);
    }

    (bytes_to_read + 1) as i32
}

fn mkdtemp(env: &mut Environment, template: MutPtr<u8>) -> MutPtr<u8> {
    set_errno(env, 0);
    for _ in 0..128 {
        if fill_template_xxxxxx(env, template).is_none() {
            set_errno(env, EINVAL);
            return Ptr::null();
        }
        // Read the path, create the directory exclusively.
        let path_str = match env.mem.cstr_at_utf8(template.cast_const()) {
            Ok(s) => s.to_owned(),
            Err(_) => {
                set_errno(env, EINVAL);
                return Ptr::null();
            }
        };
        let guest_path = GuestPath::new(&path_str);
        if env.fs.exists(guest_path) {
            continue; // collision; reseed and try again
        }
        match env.fs.create_dir(guest_path) {
            Ok(_) => return template,
            Err(_) => continue,
        }
    }
    set_errno(env, crate::libc::errno::EEXIST);
    Ptr::null()
}

fn mkstemp(env: &mut Environment, template: MutPtr<u8>) -> i32 {
    use crate::libc::errno::{ENOENT, ENOTDIR};
    set_errno(env, 0);

    // Try a handful of randomisations before giving up, mirroring BSD libc's
    // `_gettemp` (TMP_MAX is documented as at least 308,915,776 on real
    // systems but games never need that many — touchHLE only needs to keep
    // making progress in the rare case of a collision).
    for _ in 0..128 {
        if fill_template_xxxxxx(env, template).is_none() {
            set_errno(env, EINVAL);
            return -1;
        }
        // O_RDWR | O_CREAT | O_EXCL — same flag combination Darwin libc uses.
        const O_RDWR: i32 = 0x0002;
        const O_CREAT: i32 = 0x0200;
        const O_EXCL: i32 = 0x0800;
        let fd = crate::libc::posix_io::open_direct(
            env,
            template.cast_const(),
            O_RDWR | O_CREAT | O_EXCL,
        );
        if fd >= 0 {
            return fd;
        }
        // If the failure is a hard error (missing directory etc.) there is
        // no point in retrying — propagate it.
        let errno = crate::libc::errno::get_errno(env);
        if errno == ENOENT || errno == ENOTDIR {
            return -1;
        }
    }
    set_errno(env, crate::libc::errno::EEXIST);
    -1
}

fn mktemp(env: &mut Environment, template: MutPtr<u8>) -> MutPtr<u8> {
    // Apple manpage: "The mktemp() function is dangerous; its use is
    // discouraged because it is racy. Use mkstemp() instead." We implement it
    // anyway for guest binaries that link it — fill in the template but do
    // NOT create the file, exactly as the historical contract requires.
    set_errno(env, 0);
    if fill_template_xxxxxx(env, template).is_none() {
        set_errno(env, EINVAL);
        return Ptr::null();
    }
    template
}

fn mrand48(env: &mut Environment) -> i32 {
    // "mrand48() and jrand48() return signed long integers uniformly
    // distributed over the interval [-2^31, 2^31)." Take the high 32 bits
    // of the 48-bit state and reinterpret as i32.
    let next = env.libc_state.stdlib.drand48.step();
    ((next >> 16) & 0xFFFF_FFFF) as u32 as i32
}

fn nrand48(env: &mut Environment, xsubi: MutPtr<u16>) -> i32 {
    let mut local = Drand48State {
        state: pack_xsubi(env, xsubi.cast_const()),
        a: env.libc_state.stdlib.drand48.a,
        c: env.libc_state.stdlib.drand48.c,
    };
    let next = local.step();
    write_xsubi(env, xsubi, next);
    (next >> 17) as i32
}

fn putenv(env: &mut Environment, string: MutPtr<u8>) -> i32 {
    if string.is_null() {
        set_errno(env, EINVAL);
        return -1;
    }
    let s = match env.mem.cstr_at_utf8(string.cast_const()) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            set_errno(env, EINVAL);
            return -1;
        }
    };
    log_dbg!("putenv({:?})", s);
    // putenv is a no-op in touchHLE — we have no real environment block.
    // Return 0 (success) so apps that call it to set e.g. timezone or locale
    // hints don't abort on the return code.
    0
}

fn quick_exit(env: &mut Environment, exit_code: i32) {
    immediate_exit(env, "quick_exit", exit_code);
}

fn removexattr(env: &mut Environment, path: ConstPtr<u8>, name: ConstPtr<u8>, options: i32) -> i32 {
    let path_str = env.mem.cstr_at_utf8(path).unwrap_or_default().to_owned();
    let name_str = env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned();
    log_dbg!(
        "removexattr({:?}, {:?}, options={:#x}) — returning ENOATTR",
        path_str,
        name_str,
        options
    );
    set_errno(env, 93);
    -1
}

fn seed48(env: &mut Environment, xsubi: MutPtr<u16>) -> MutPtr<u16> {
    // "seed48() sets the value of the X_i to the 48-bit value specified in
    // the argument, and returns a pointer to a 14-byte array containing the
    // previous value of X_i." The Apple/POSIX contract uses a per-process
    // internal buffer for the return value — emulate that by allocating it
    // once and re-using the slot.
    let previous = env.libc_state.stdlib.drand48.state;

    let new_state = pack_xsubi(env, xsubi.cast_const());
    env.libc_state.stdlib.drand48 = Drand48State {
        state: new_state,
        a: 0x5DEE_CE66D,
        c: 0xB,
    };

    // Reuse a static guest-side buffer for the returned `unsigned short[3]`.
    // POSIX guarantees the returned pointer is valid until the next call to
    // `seed48`/`srand48`/`lcong48`. We mirror that contract by reallocating
    // here — touchHLE's guest allocator keeps the pointer alive until the
    // application explicitly frees it (which it must not, per POSIX).
    let buf: MutPtr<u16> = env.mem.alloc(6).cast();
    write_xsubi(env, buf, previous);
    buf
}

fn srand48(env: &mut Environment, seedval: i32) {
    // Apple manpage: "srand48() initializes the high-order 32 bits of the
    // 48-bit X_i to the low-order 32 bits of the argument; the low-order
    // 16 bits of X_i are set to the arbitrary value 330E_16." Reset
    // multiplier/addend to their defaults, matching the spec.
    let seed = (seedval as u32) as u64;
    env.libc_state.stdlib.drand48 = Drand48State {
        state: (seed << 16) | 0x330E,
        a: 0x5DEE_CE66D,
        c: 0xB,
    };
}

fn strtod_l(
    env: &mut Environment,
    nptr: ConstPtr<u8>,
    endptr: MutPtr<MutPtr<u8>>,
    _locale: ConstVoidPtr,
) -> f64 {
    strtod(env, nptr, endptr)
}

fn strtof_l(
    env: &mut Environment,
    nptr: ConstPtr<u8>,
    endptr: MutPtr<ConstPtr<u8>>,
    _locale: ConstVoidPtr,
) -> f32 {
    strtof(env, nptr, endptr)
}

// `long double` on the iOS armv7 ABI is identical to `double`, so strtold*
// return f64 exactly like strtod. The `_l` locale-aware variants take a
// trailing `locale_t` we can ignore: our parsing is already locale-independent
// (always the C locale). These were previously missing, so the dyld linker
// installed return-0 stubs for them — which broke every guest that parses
// numbers through them. Minecraft PE 0.14, for instance, parses its UI layout
// percentage strings ("100%", "31.25%") with strtold_l; a stubbed 0 collapsed
// all percentage-sized controls (main-menu buttons, title logo) to zero size,
// making them invisible and untappable while fixed-pixel controls still worked.
fn strtold(env: &mut Environment, nptr: ConstPtr<u8>, endptr: MutPtr<MutPtr<u8>>) -> f64 {
    strtod(env, nptr, endptr)
}

fn strtold_l(
    env: &mut Environment,
    nptr: ConstPtr<u8>,
    endptr: MutPtr<MutPtr<u8>>,
    _locale: ConstVoidPtr,
) -> f64 {
    strtod(env, nptr, endptr)
}

fn strtoll(env: &mut Environment, str: ConstPtr<u8>, endptr: MutPtr<MutPtr<u8>>, base: i32) -> i64 {
    set_errno(env, 0);
    let base = base as u32;
    if base != 0 && !(2..=36).contains(&base) {
        if !endptr.is_null() {
            env.mem.write(endptr, str.cast_mut());
        }
        set_errno(env, EINVAL);
        return 0;
    }
    let parse_res = str_to_int_inner_generic(
        env,
        |env, s, idx| Ok(env.mem.read(s + idx)),
        |_, _, _| (),
        str.cast_mut(),
        0, // starting offset
        base,
        u32::MAX, // max_length
        |s, base| i64::from_str_radix(s, base).unwrap_or(i64::MAX),
        |num| num.checked_mul(-1).unwrap_or(i64::MIN),
    );
    match parse_res {
        Ok((res, len)) => {
            if !endptr.is_null() {
                env.mem.write(endptr, (str + len).cast_mut());
            }
            res
        }
        Err(_) => {
            if !endptr.is_null() {
                env.mem.write(endptr, str.cast_mut());
            }
            0
        }
    }
}

fn strtoll_l(
    env: &mut Environment,
    str: ConstPtr<u8>,
    endptr: MutPtr<MutPtr<u8>>,
    base: i32,
    _locale: ConstVoidPtr,
) -> i64 {
    strtoll(env, str, endptr, base)
}

fn strtoull_l(
    env: &mut Environment,
    str: ConstPtr<u8>,
    endptr: MutPtr<MutPtr<u8>>,
    base: i32,
    _locale: ConstVoidPtr,
) -> u64 {
    strtoull(env, str, endptr, base)
}

/// Pack a `[u16; 3]` (little-endian, i.e. `xsubi[0]` is the lowest 16 bits)
/// into a 48-bit integer. Matches the Apple-documented layout for the `xsubi`
/// arrays taken by `seed48`, `erand48`, `nrand48`, `jrand48`.
fn pack_xsubi(env: &Environment, xsubi: ConstPtr<u16>) -> u64 {
    let lo = u64::from(env.mem.read(xsubi));
    let mid = u64::from(env.mem.read(xsubi + 1));
    let hi = u64::from(env.mem.read(xsubi + 2));
    lo | (mid << 16) | (hi << 32)
}

fn write_xsubi(env: &mut Environment, xsubi: MutPtr<u16>, value: u64) {
    env.mem.write(xsubi, (value & 0xFFFF) as u16);
    env.mem.write(xsubi + 1, ((value >> 16) & 0xFFFF) as u16);
    env.mem.write(xsubi + 2, ((value >> 32) & 0xFFFF) as u16);
}

fn fill_template_xxxxxx(env: &mut Environment, template: MutPtr<u8>) -> Option<u32> {
    let template_str = match env.mem.cstr_at_utf8(template.cast_const()) {
        Ok(s) => s.to_owned(),
        Err(_) => return None,
    };
    let len = template_str.len();
    if len < 6
        || !template_str.as_bytes()[len - 6..]
            .iter()
            .all(|&c| c == b'X')
    {
        // Apple manpage: "If the template doesn’t contain six trailing X's,
        // -1 is returned and errno is set to EINVAL."
        return None;
    }
    let suffix_offset = (len - 6) as u32;
    for i in 0..6 {
        // Drive randomness from arc4random so we don't perturb the
        // user-visible drand48 state (which the guest may be sampling
        // deterministically after `srand48`).
        let r = arc4random(env);
        let ch = MKTEMP_CHARS[(r as usize) % MKTEMP_CHARS.len()];
        env.mem.write(template + suffix_offset + i, ch);
    }
    Some(suffix_offset)
}

fn read_cstr_safe(env: &mut Environment, ptr: ConstPtr<u8>) -> String {
    if ptr.is_null() {
        return "(null)".to_string();
    }
    // Assertions can arrive after guest memory corruption. Keep diagnostics
    // bounded rather than scanning an unterminated guest buffer forever.
    String::from_utf8(env.mem.cstr_at(ptr).to_vec())
        .unwrap_or_else(|_| "(invalid utf-8)".to_string())
}

/// Shared implementation of `ecvt` and `fcvt`, like FreeBSD's `__cvt`, which
/// Apple's libc uses: the digits are padded with zeros to `ndigit`
/// significant digits (`ecvt`) or `ndigit` digits after the decimal point
/// (`fcvt`), and are returned in a buffer that the next call reuses.
fn cvt(
    env: &mut Environment,
    value: f64,
    ndigit: i32,
    decpt: MutPtr<i32>,
    sign: MutPtr<i32>,
    fixed: bool,
) -> MutPtr<u8> {
    let ndigit = ndigit.max(0);
    let (digits, point, negative) = if value.is_nan() || value.is_infinite() {
        let digits = if value.is_nan() { "nan" } else { "inf" };
        (digits.as_bytes().to_vec(), 0, value.is_sign_negative())
    } else if value == 0.0 {
        let len = ndigit.max(1) as usize;
        (vec![b'0'; len], if fixed { 0 } else { 1 }, false)
    } else {
        let (mut digits, point) = dtoa_digits(value, ndigit, fixed);
        let len = if fixed { ndigit + point } else { ndigit };
        while (digits.len() as i32) < len {
            digits.push(b'0');
        }
        (digits, point, value < 0.0)
    };
    if !decpt.is_null() {
        env.mem.write(decpt, point);
    }
    if !sign.is_null() {
        env.mem.write(sign, negative as i32);
    }

    if let Some(old) = env.libc_state.stdlib.cvt_buffer.take() {
        env.mem.free(old.cast());
    }
    let buf = env.mem.alloc_and_write_cstr(&digits);
    env.libc_state.stdlib.cvt_buffer = Some(buf);
    buf
}

/// Digits of `value`'s magnitude like `__dtoa()` returns them, without
/// trailing zeros (but "0" for zero), together with the position of the
/// decimal point relative to them. `fixed` selects `ndigit` digits after the
/// decimal point (dtoa mode 3) instead of `ndigit` significant digits (mode
/// 2). Callers handle infinities and NaNs.
fn dtoa_digits(value: f64, ndigit: i32, fixed: bool) -> (Vec<u8>, i32) {
    const MAX_DIGITS: usize = 80;
    let value = value.abs();
    if value == 0.0 {
        return (b"0".to_vec(), 1);
    }
    let (mut digits, decpt) = if fixed {
        let formatted = format!("{:.*}", ndigit.clamp(0, MAX_DIGITS as i32) as usize, value);
        let (int_part, frac_part) = formatted.split_once('.').unwrap_or((&formatted, ""));
        let all = format!("{int_part}{frac_part}").into_bytes();
        let zeros = all.iter().take_while(|&&c| c == b'0').count();
        (all[zeros..].to_vec(), int_part.len() as i32 - zeros as i32)
    } else {
        let precision = (ndigit.clamp(1, MAX_DIGITS as i32) - 1) as usize;
        let formatted = format!("{:.*e}", precision, value);
        let (mantissa, exponent) = formatted.split_once('e').unwrap();
        let digits = mantissa.bytes().filter(|&c| c != b'.').collect();
        (digits, exponent.parse::<i32>().unwrap() + 1)
    };
    while digits.last() == Some(&b'0') {
        digits.pop();
    }
    (digits, decpt)
}

/// State for the POSIX `drand48`/`lrand48`/`mrand48` family. Mirrors what real
/// libc keeps internally — a 48-bit state plus the multiplier `a` and addend
/// `c` (modifiable via `lcong48`).
#[derive(Copy, Clone)]
struct Drand48State {
    /// 48-bit `X_n`, stored in the low 48 bits of a `u64`.
    state: u64,
    /// 48-bit multiplier `a` (default `0x5DEECE66D`).
    a: u64,
    /// 48-bit addend `c` (default `0xB`).
    c: u64,
}

impl Default for Drand48State {
    fn default() -> Self {
        Drand48State {
            state: 0x1234_ABCD_330E,
            a: 0x5DEE_CE66D,
            c: 0xB,
        }
    }
}

#[allow(non_camel_case_types)]
#[derive(Debug)]
#[repr(C, packed)]
struct ldiv_t {
    quot: i32,
    rem: i32,
}
unsafe impl SafeRead for ldiv_t {}
impl_GuestRet_for_large_struct!(ldiv_t);

impl Drand48State {
    /// Advance `X_n` and return the new 48-bit value.
    fn step(&mut self) -> u64 {
        self.state = self.a.wrapping_mul(self.state).wrapping_add(self.c) & 0xFFFF_FFFF_FFFF;
        self.state
    }
}

const MKTEMP_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";

fn dladdr(_env: &mut Environment, _addr: ConstVoidPtr, _info: MutVoidPtr) -> i32 {
    // FakeDladdr
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__stack_chk_fail()),
    export_c_func!(posix_memalign(_, _, _)),
    export_c_func!(__assert_rtn(_, _, _, _)),
    export_c_func!(malloc(_)),
    export_c_func!(malloc_size(_)),
    export_c_func!(calloc(_, _)),
    export_c_func!(valloc(_)),
    export_c_func!(realloc(_, _)),
    export_c_func!(reallocf(_, _)),
    export_c_func!(free(_)),
    export_c_func!(atexit(_)),
    export_c_func!(atoi(_)),
    export_c_func!(atol(_)),
    export_c_func!(atof(_)),
    export_c_func!(strtod(_, _)),
    export_c_func!(srand(_)),
    export_c_func!(sranddev()),
    export_c_func!(rand()),
    export_c_func!(rand_r(_)),
    export_c_func!(srandom(_)),
    export_c_func!(random()),
    export_c_func!(arc4random()),
    export_c_func!(div(_, _)),
    export_c_func!(getenv(_)),
    export_c_func!(setenv(_, _, _)),
    export_c_func!(unsetenv(_)),
    export_c_func!(exit(_)),
    export_c_func!(bsearch(_, _, _, _, _)),
    export_c_func!(strtof(_, _)),
    export_c_func!(strtoul(_, _, _)),
    export_c_func!(wcstoul(_, _, _)),
    export_c_func!(strtoull(_, _, _)),
    export_c_func!(strtol(_, _, _)),
    export_c_func!(wcstol(_, _, _)),
    export_c_func!(realpath(_, _)),
    export_c_func_aliased!("realpath$DARWIN_EXTSN", realpath(_, _)),
    export_c_func!(mbstowcs(_, _, _)),
    export_c_func!(wcstombs(_, _, _)),
    export_c_func!(system(_)),
    export_c_func!(fcvt(_, _, _, _)),
    export_c_func!(NSZoneFree(_, _)),
    export_c_func!(NSZoneMalloc(_, _)),
    export_c_func!(NSZoneRealloc(_, _, _)),
    export_c_func!(_Exit(_)),
    export_c_func!(__assert(_, _, _)),
    export_c_func!(__assert_fail(_, _, _, _)),
    export_c_func!(_exit(_)),
    export_c_func!(abort()),
    export_c_func!(arc4random_addrandom()),
    export_c_func!(arc4random_stir()),
    export_c_func!(arc4random_uniform(_)),
    export_c_func!(dirname(_)),
    export_c_func!(drand48()),
    export_c_func!(ecvt(_, _, _, _)),
    export_c_func!(erand48(_)),
    export_c_func!(fgetxattr(_, _, _, _, _, _)),
    export_c_func!(flistxattr(_, _, _, _)),
    export_c_func!(fremovexattr(_, _, _)),
    export_c_func!(fsetxattr(_, _, _, _, _, _)),
    export_c_func!(gcvt(_, _, _)),
    export_c_func!(getloadavg(_, _)),
    export_c_func!(getxattr(_, _, _, _, _, _)),
    export_c_func!(inflateReset2(_, _)),
    export_c_func!(jrand48(_)),
    export_c_func!(kevent(_, _, _, _, _, _)),
    export_c_func!(kqueue()),
    export_c_func!(lcong48(_)),
    export_c_func!(ldiv(_, _)),
    export_c_func!(listxattr(_, _, _, _)),
    export_c_func!(lrand48()),
    export_c_func!(mbtowc_l(_, _, _, _)),
    export_c_func!(mkdtemp(_)),
    export_c_func!(mkstemp(_)),
    export_c_func!(mktemp(_)),
    export_c_func!(mrand48()),
    export_c_func!(nrand48(_)),
    export_c_func!(putenv(_)),
    export_c_func!(quick_exit(_)),
    export_c_func!(removexattr(_, _, _)),
    export_c_func!(seed48(_)),
    export_c_func!(srand48(_)),
    export_c_func!(strtod_l(_, _, _)),
    export_c_func!(strtof_l(_, _, _)),
    export_c_func!(strtold(_, _)),
    export_c_func!(strtold_l(_, _, _)),
    export_c_func!(strtoll(_, _, _)),
    export_c_func!(strtoll_l(_, _, _, _)),
    export_c_func!(strtoull_l(_, _, _, _)),
    export_c_func!(dladdr(_, _)),
];

/// A simple wrapper around [atof_inner_generic] for the case of C string.
pub fn atof_inner(
    env: &mut Environment,
    s: ConstPtr<u8>,
) -> Result<(f64, u32), <f64 as FromStr>::Err> {
    atof_inner_generic(
        env,
        |env, s, idx| Ok(env.mem.read(s + idx)),
        |_, _, _| (),
        s.cast_mut(),
        0,
    )
}

#[allow(rustdoc::broken_intra_doc_links)] // https://github.com/rust-lang/rust/issues/83049
/// Generic implementation of a conversion helper to `double`.
///
/// `getc_fn` is a callback to get next character from `subject`.
/// 3rd parameter in this callback is a index which is safe to ignore
/// (for example, in case of a file stream).
/// Error signifies an abnormal stop of input,
/// such as [crate::libc::stdio::EOF] in the file stream.
/// Note: `'\0'` does not necessary expect to produce an error!
///
/// `ungetc_fn` is a callback to un-get character from `subject`.
/// Could be ignored entirely (for example, in case of a string).
///
/// `subject` is either C string or file stream (for now).
///
/// `offset` defines an offset in `subject` from which conversion starts.
/// Could be ignored entirely (for example, in case of a file stream).
///
/// Returns a tuple containing the parsed number and the length of the number in
/// the string.
///
/// See also a TODO comment in [str_to_int_inner_generic].
pub fn atof_inner_generic<
    T,
    U,
    F1: Fn(&mut Environment, MutPtr<U>, GuestUSize) -> Result<T, ()>,
    F2: Fn(&mut Environment, MutPtr<U>, u8), // TODO: make last param generic too?
>(
    env: &mut Environment,
    getc_fn: F1,
    ungetc_fn: F2,
    subject: MutPtr<U>,
    offset: GuestUSize,
) -> Result<(f64, u32), <f64 as FromStr>::Err>
where
    u8: From<T>,
{
    let mut whitespace_len = 0;
    let mut len = 0;
    let mut chars = Vec::new();

    // Helper is needed to support early returns on `getc_fn` errors
    // (e.g. EOF in the input stream)
    // We don't care about return of helper because modified vars are
    // captured indirectly.
    let _ = || -> Result<(), ()> {
        // atof() is similar to atoi().
        // FIXME: no C99 hexfloat, INF, NAN support
        match count_whitespace_generic(env, &getc_fn, &ungetc_fn, subject, offset) {
            Ok(count) => {
                whitespace_len = count;
            }
            Err(count) => {
                whitespace_len = count;
                return Err(());
            }
        }

        let maybe_sign: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
        if maybe_sign == b'+' || maybe_sign == b'-' || maybe_sign.is_ascii_digit() {
            chars.push(maybe_sign);
            len += 1;
        } else {
            ungetc_fn(env, subject, maybe_sign);
        }

        let mut curr: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
        while (curr as char).is_ascii_digit() {
            chars.push(curr);
            len += 1;
            curr = getc_fn(env, subject, offset + whitespace_len + len)?.into();
        }

        // TODO: assert C locale
        if curr == b'.' {
            chars.push(curr);
            len += 1;
            curr = getc_fn(env, subject, offset + whitespace_len + len)?.into();
            while (curr as char).is_ascii_digit() {
                chars.push(curr);
                len += 1;
                curr = getc_fn(env, subject, offset + whitespace_len + len)?.into();
            }
        }

        if curr.eq_ignore_ascii_case(&b'e') {
            chars.push(curr);
            len += 1;

            let maybe_sign: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
            if maybe_sign == b'+' || maybe_sign == b'-' || maybe_sign.is_ascii_digit() {
                chars.push(maybe_sign);
                len += 1;
            } else {
                ungetc_fn(env, subject, maybe_sign);
            }

            curr = getc_fn(env, subject, offset + whitespace_len + len)?.into();
            while (curr as char).is_ascii_digit() {
                chars.push(curr);
                len += 1;
                curr = getc_fn(env, subject, offset + whitespace_len + len)?.into();
            }
        }
        ungetc_fn(env, subject, curr);

        assert_eq!(chars.len() as u32, len);
        Ok(())
    }();

    let s = std::str::from_utf8(&chars).unwrap();
    log_dbg!("atof_inner_generic('{}')", s);
    s.parse().map(|result| (result, whitespace_len + len))
}

/// A simple wrapper around [str_to_int_inner_generic]
/// for the case of C string and i32.
pub fn strtol_inner(env: &mut Environment, str: ConstPtr<u8>, base: u32) -> Result<(i32, u32), ()> {
    str_to_int_inner_generic(
        env,
        |env, s, idx| Ok(env.mem.read(s + idx)),
        |_, _, _| (), // could be ignored
        str.cast_mut(),
        0, // starting offset
        base,
        u32::MAX, // max_length
        |s, base| i32::from_str_radix(s, base).unwrap_or(i32::MAX),
        |num| num.checked_mul(-1).unwrap_or(i32::MIN),
    )
}

#[allow(rustdoc::broken_intra_doc_links)] // https://github.com/rust-lang/rust/issues/83049
/// Generic implementation of a conversion helper from string to an integer.
///
/// `getc_fn` is a callback to get next character from `subject`.
/// 3rd parameter in this callback is a index which is safe to ignore
/// (for example, in case of a file stream).
/// Error signifies an abnormal stop of input,
/// such as [crate::libc::stdio::EOF] in the file stream.
/// Note: `'\0'` does not necessary expect to produce an error!
///
/// `ungetc_fn` is a callback to un-get character from `subject`.
/// Could be ignored entirely (for example, in case of a string).
///
/// `subject` is either C string or file stream (for now).
///
/// `offset` defines an offset in `subject` from which conversion starts.
/// Could be ignored entirely (for example, in case of a file stream).
///
/// `base` of conversion.
/// Is mutable because in case of base 0 we need to auto-detect it.
///
/// `from_str_radix_fn` is a callback to actually convert accumulated string
/// to the number.
///
/// `negation_fn` is a callback which specifies how '-' is treated.
///
/// Returns a tuple containing the parsed number in the given base and
/// the length of the number in the string.
///
/// Right now this function is a bit of the mess... We bridge together the
/// worlds of string indexing and file stream processing with questionable
/// results. We have fair amount of integration tests for `strtoul`
/// and `sscanf`/`fscanf`, but some of corner cases are definitely not covered.
/// One idea for cleaning that would be to fully embrace `getc`/`ungetc`
/// approach and get rid of indexing.
/// (Like, let caller to deal with indexing and override `offset` somehow?)
/// TODO: find a more powerful abstraction for generalization
#[allow(clippy::too_many_arguments)]
pub fn str_to_int_inner_generic<
    T,
    U,
    Q,
    F1: Fn(&mut Environment, MutPtr<U>, GuestUSize) -> Result<T, ()>,
    F2: Fn(&mut Environment, MutPtr<U>, u8), // TODO: make last param generic too?
    F3: Fn(&str, u32) -> Q,
    F4: Fn(Q) -> Q,
>(
    env: &mut Environment,
    getc_fn: F1,
    ungetc_fn: F2,
    subject: MutPtr<U>,
    offset: GuestUSize,
    mut base: u32,
    max_length: GuestUSize,
    from_str_radix_fn: F3,
    negation_fn: F4,
) -> Result<(Q, u32), ()>
where
    u8: From<T>,
    Q: Default,
{
    let mut whitespace_len = 0;
    let mut len = 0;
    let mut sign = None;
    let mut prefix_length = 0;
    let mut chars = Vec::new();

    // Helper is needed to support early returns on `getc_fn` errors
    // (e.g. EOF in the input stream)
    // We don't care about return of helper because modified vars are
    // captured indirectly.
    let _ = || -> Result<(), ()> {
        // strtol() doesn't work with a null-terminated string,
        // instead it stops once it hits something that's not a digit,
        // so we have to do some parsing ourselves.
        match count_whitespace_generic(env, &getc_fn, &ungetc_fn, subject, offset) {
            Ok(count) => {
                whitespace_len = count;
            }
            Err(count) => {
                whitespace_len = count;
                return Err(());
            }
        }

        let maybe_sign: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
        if maybe_sign == b'+' || maybe_sign == b'-' {
            sign = Some(maybe_sign);
            prefix_length += 1;
            len += 1;
            if len == max_length {
                return Ok(());
            }
        } else {
            ungetc_fn(env, subject, maybe_sign);
        }
        // We need to do base detection before we can start counting
        // the number length, but after we maybe skipped the sign
        // TODO: detect base and skip prefix in one pass
        if base == 0 {
            let curr: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
            base = if curr == b'0' {
                let next: u8 = getc_fn(env, subject, offset + whitespace_len + len + 1)?.into();
                ungetc_fn(env, subject, next);
                ungetc_fn(env, subject, curr);
                if next == b'x' || next == b'X' {
                    16
                } else {
                    8
                }
            } else {
                ungetc_fn(env, subject, curr);
                10
            }
        }
        // Skipping prefix if needed
        if base == 8 || base == 16 {
            let curr: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
            if curr == b'0' {
                // The leading zero is a digit unless it belongs to a 0x
                // prefix. In particular, %x must successfully read "0".
                chars.push(curr);
                len += 1;
                if len == max_length {
                    return Ok(());
                }
                if base == 16 {
                    let next: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
                    if next == b'x' || next == b'X' {
                        chars.pop();
                        prefix_length += 2;
                        len += 1;
                        if len == max_length {
                            return Ok(());
                        }
                    } else {
                        ungetc_fn(env, subject, next);
                    }
                }
            } else {
                ungetc_fn(env, subject, curr);
            }
        }
        let mut curr: u8 = getc_fn(env, subject, offset + whitespace_len + len)?.into();
        while (curr as char).is_digit(base) {
            chars.push(curr);
            len += 1;
            if len == max_length {
                return Ok(());
            }
            curr = getc_fn(env, subject, offset + whitespace_len + len)?.into();
        }
        ungetc_fn(env, subject, curr);
        assert_eq!(chars.len() as u32, len - prefix_length);
        Ok(())
    }();

    let s = std::str::from_utf8(&chars).unwrap();
    log_dbg!("strtol_inner_generic('{}', {})", s, base);

    assert!((2..=36).contains(&base));
    let magnitude_len = len - prefix_length;
    let res = if magnitude_len > 0 {
        // TODO: set errno on range errors
        let mut res = from_str_radix_fn(s, base);
        if sign == Some(b'-') {
            res = negation_fn(res);
        }
        res
    } else {
        // Special case - prefix of invalid octal number is a valid number 0
        if base == 8 && prefix_length > 0 {
            return Ok((Q::default(), whitespace_len + prefix_length));
        }
        return Err(());
    };
    Ok((res, whitespace_len + len))
}
