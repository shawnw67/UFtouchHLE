/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `errno.h`

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::mem::{ConstPtr, GuestUSize, Mem, MutPtr};
use crate::Environment;
use std::io::Write;

pub const EPERM: i32 = 1;
pub const ENOENT: i32 = 2;
pub const ESRCH: i32 = 3;
pub const EINTR: i32 = 4;
pub const EIO: i32 = 5;
pub const EBADF: i32 = 9;
pub const ECHILD: i32 = 10;
pub const EDEADLK: i32 = 11;
pub const ENOMEM: i32 = 12;
pub const EACCES: i32 = 13;
pub const EFAULT: i32 = 14;
pub const EBUSY: i32 = 16;
pub const EEXIST: i32 = 17;
pub const ENOTDIR: i32 = 20;
pub const EISDIR: i32 = 21;
pub const EINVAL: i32 = 22;
pub const ESPIPE: i32 = 29;
pub const EROFS: i32 = 30;
pub const ERANGE: i32 = 34;
pub const EAGAIN: i32 = 35;
pub const EPROTONOSUPPORT: i32 = 43;
pub const ENOTSUP: i32 = 45;
pub const ECONNRESET: i32 = 54;
pub const ETIMEDOUT: i32 = 60;
pub const ENAMETOOLONG: i32 = 63;
pub const EOVERFLOW: i32 = 84;

#[derive(Default)]
pub struct State {
    errnos: std::collections::HashMap<crate::ThreadId, MutPtr<i32>>,
    strings_cache: std::collections::HashMap<i32, ConstPtr<u8>>,
}
impl State {
    fn errno_ptr_for_thread(
        &mut self,
        mem: &mut crate::mem::Mem,
        thread: crate::ThreadId,
    ) -> MutPtr<i32> {
        *self
            .errnos
            .entry(thread)
            .or_insert_with(|| mem.alloc_and_write(0i32))
    }

    pub fn set_errno_for_thread(
        &mut self,
        mem: &mut crate::mem::Mem,
        thread: crate::ThreadId,
        val: i32,
    ) {
        let ptr = self.errno_ptr_for_thread(mem, thread);
        mem.write(ptr, val);
    }

    fn remove_thread(&mut self, mem: &mut crate::mem::Mem, thread: crate::ThreadId) {
        if let Some(ptr) = self.errnos.remove(&thread) {
            mem.free(ptr.cast());
        }
    }
}

/// Helper functions, not a part of libc errno
pub(crate) fn initialize_thread(env: &mut Environment, thread: crate::ThreadId) {
    env.libc_state
        .errno
        .set_errno_for_thread(&mut env.mem, thread, 0);
}

pub(crate) fn destroy_thread(env: &mut Environment, thread: crate::ThreadId) {
    env.libc_state.errno.remove_thread(&mut env.mem, thread);
}

#[cfg(test)]
mod thread_storage_tests {
    use super::*;
    use crate::mem::Mem;

    #[test]
    fn exiting_threads_release_errno_storage_without_changing_other_threads() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(4096);
        let mut state = State::default();
        state.set_errno_for_thread(&mut mem, 0, EINVAL);
        state.set_errno_for_thread(&mut mem, 1, ENOMEM);
        let main = state.errnos[&0];
        let worker = state.errnos[&1];
        assert_ne!(main, worker);
        state.remove_thread(&mut mem, 1);
        assert!(!state.errnos.contains_key(&1));
        assert_eq!(mem.read(main), EINVAL);
        // Deleting an absent thread is harmless; a fresh one starts at zero.
        state.remove_thread(&mut mem, 1);
        let fresh = state.errno_ptr_for_thread(&mut mem, 2);
        assert_eq!(mem.read(fresh), 0);
        assert_eq!(mem.read(main), EINVAL);
    }
}

pub fn set_errno(env: &mut Environment, val: i32) {
    env.libc_state
        .errno
        .set_errno_for_thread(&mut env.mem, env.current_thread, val);
}
pub fn get_errno(env: &mut Environment) -> i32 {
    let ptr = env
        .libc_state
        .errno
        .errno_ptr_for_thread(&mut env.mem, env.current_thread);
    env.mem.read(ptr)
}

/// Diagnostic read only: unlike get_errno, never allocates guest memory.
pub(crate) fn peek_errno(env: &Environment) -> Option<i32> {
    env.libc_state
        .errno
        .errnos
        .get(&env.current_thread)
        .map(|p| env.mem.read(*p))
}

fn __error(env: &mut Environment) -> MutPtr<i32> {
    env.libc_state
        .errno
        .errno_ptr_for_thread(&mut env.mem, env.current_thread)
}

fn perror(env: &mut Environment, s: ConstPtr<u8>) {
    let errno_ptr = __error(env);
    let str_error = strerror(env, env.mem.read(errno_ptr));
    let errno_msg = format!("{}\n", env.mem.cstr_at_utf8(str_error).unwrap());
    let msg = if !s.is_null() {
        if let Ok(str) = env.mem.cstr_at_utf8(s) {
            format!("{str}: {errno_msg}")
        } else {
            errno_msg.to_string()
        }
    } else {
        errno_msg.to_string()
    };
    let _ = std::io::stderr().write_all(msg.as_bytes());
}

/// Indexed Darwin error texts. Historical strerror treats index 0 as unknown.
const ERROR_STRINGS: &[&str] = &[
    "Undefined error: 0",
    "Operation not permitted",
    "No such file or directory",
    "No such process",
    "Interrupted system call",
    "Input/output error",
    "Device not configured",
    "Argument list too long",
    "Exec format error",
    "Bad file descriptor",
    "No child processes",
    "Resource deadlock avoided",
    "Cannot allocate memory",
    "Permission denied",
    "Bad address",
    "Block device required",
    "Resource busy",
    "File exists",
    "Cross-device link",
    "Operation not supported by device",
    "Not a directory",
    "Is a directory",
    "Invalid argument",
    "Too many open files in system",
    "Too many open files",
    "Inappropriate ioctl for device",
    "Text file busy",
    "File too large",
    "No space left on device",
    "Illegal seek",
    "Read-only file system",
    "Too many links",
    "Broken pipe",
    "Numerical argument out of domain",
    "Result too large",
    "Resource temporarily unavailable",
    "Operation now in progress",
    "Operation already in progress",
    "Socket operation on non-socket",
    "Destination address required",
    "Message too long",
    "Protocol wrong type for socket",
    "Protocol not available",
    "Protocol not supported",
    "Socket type not supported",
    "Operation not supported",
    "Protocol family not supported",
    "Address family not supported by protocol family",
    "Address already in use",
    "Can't assign requested address",
    "Network is down",
    "Network is unreachable",
    "Network dropped connection on reset",
    "Software caused connection abort",
    "Connection reset by peer",
    "No buffer space available",
    "Socket is already connected",
    "Socket is not connected",
    "Can't send after socket shutdown",
    "Too many references: can't splice",
    "Operation timed out",
    "Connection refused",
    "Too many levels of symbolic links",
    "File name too long",
    "Host is down",
    "No route to host",
    "Directory not empty",
    "Too many processes",
    "Too many users",
    "Disc quota exceeded",
    "Stale NFS file handle",
    "Too many levels of remote in path",
    "RPC struct is bad",
    "RPC version wrong",
    "RPC prog. not avail",
    "Program version wrong",
    "Bad procedure for program",
    "No locks available",
    "Function not implemented",
    "Inappropriate file type or format",
    "Authentication error",
    "Need authenticator",
    "Device power is off",
    "Device error",
    "Value too large to be stored in data type",
    "Bad executable (or shared library)",
    "Bad CPU type in executable",
    "Shared library version mismatch",
    "Malformed Mach-o file",
    "Operation canceled",
    "Identifier removed",
    "No message of desired type",
    "Illegal byte sequence",
    "Attribute not found",
    "Bad message",
    "EMULTIHOP (Reserved)",
    "No message available on STREAM",
    "ENOLINK (Reserved)",
    "No STREAM resources",
    "Not a STREAM",
    "Protocol error",
    "STREAM ioctl timeout",
    "Operation not supported on socket",
    "Policy not found",
    "State not recoverable",
    "Previous owner died",
];

fn error_message(err_num: i32) -> (std::borrow::Cow<'static, str>, i32) {
    match usize::try_from(err_num)
        .ok()
        .filter(|&i| i != 0)
        .and_then(|i| ERROR_STRINGS.get(i))
    {
        Some(&message) => (std::borrow::Cow::Borrowed(message), 0),
        None => (
            std::borrow::Cow::Owned(format!("Unknown error: {err_num}")),
            EINVAL,
        ),
    }
}

fn render_error(mem: &mut Mem, err_num: i32, buf: MutPtr<u8>, n: GuestUSize) -> i32 {
    let (message, status) = error_message(err_num);
    if n != 0 {
        let count = message.len().min((n - 1) as usize) as GuestUSize;
        mem.bytes_at_mut(buf, count)
            .copy_from_slice(&message.as_bytes()[..count as usize]);
        mem.write(buf + count, 0u8);
    }
    // Darwin reports EINVAL for unknown errors even if the buffer is short.
    if status != 0 {
        status
    } else if message.len() >= n as usize {
        ERANGE
    } else {
        0
    }
}

fn strerror_r(env: &mut Environment, err_num: i32, buf: MutPtr<u8>, n: GuestUSize) -> i32 {
    render_error(&mut env.mem, err_num, buf, n)
}

#[cfg(test)]
mod error_buffer_tests {
    use super::*;
    use crate::mem::Ptr;

    #[test]
    fn zero_and_short_buffers_preserve_boundaries_and_darwin_error_priority() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(4096);
        assert_eq!(render_error(&mut mem, EINVAL, Ptr::null(), 0), ERANGE);
        assert_eq!(render_error(&mut mem, -1, Ptr::null(), 0), EINVAL);
        let buf: MutPtr<u8> = mem.alloc(8).cast();
        mem.bytes_at_mut(buf, 8).fill(b'Z');
        assert_eq!(render_error(&mut mem, EINVAL, buf, 4), ERANGE);
        assert_eq!(mem.bytes_at(buf.cast_const(), 8), b"Inv\0ZZZZ");
        assert_eq!(render_error(&mut mem, -1, buf, 4), EINVAL);
        assert_eq!(mem.bytes_at(buf.cast_const(), 8), b"Unk\0ZZZZ");
    }

    #[test]
    fn known_and_unknown_messages_share_the_existing_darwin_text_table() {
        let mut mem = Mem::new();
        mem.set_null_segment_size(4096);
        let buf: MutPtr<u8> = mem.alloc(128).cast();
        for code in 1..ERROR_STRINGS.len() as i32 {
            assert_eq!(render_error(&mut mem, code, buf, 128), 0);
            assert_eq!(mem.cstr_at(buf), ERROR_STRINGS[code as usize].as_bytes());
        }
        assert_eq!(render_error(&mut mem, i32::MIN, buf, 128), EINVAL);
        assert_eq!(mem.cstr_at(buf), b"Unknown error: -2147483648");
        assert_eq!(render_error(&mut mem, 0, buf, 128), EINVAL);
        assert_eq!(mem.cstr_at(buf), b"Unknown error: 0");
    }
}

fn strerror(env: &mut Environment, err_num: i32) -> ConstPtr<u8> {
    let (str, status) = error_message(err_num);
    if status != 0 {
        set_errno(env, status);
    }
    if let Some(&c_str) = env.libc_state.errno.strings_cache.get(&err_num) {
        c_str
    } else {
        let new_c_str = env.mem.alloc_and_write_cstr(str.as_bytes()).cast_const();
        env.libc_state
            .errno
            .strings_cache
            .insert(err_num, new_c_str);
        new_c_str
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__error()),
    export_c_func!(perror(_)),
    export_c_func!(strerror(_)),
    export_c_func!(strerror_r(_, _, _)),
];
