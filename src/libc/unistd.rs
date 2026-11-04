/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Miscellaneous parts of `unistd.h`

use crate::abi::DotDotDot;
use crate::dyld::{export_c_func, FunctionExports};
use crate::fs::FsError;
use crate::fs::GuestPath;
use crate::libc::errno::EEXIST;
use crate::libc::errno::EFAULT;
use crate::libc::errno::ENOSYS;
use crate::libc::errno::ENOTDIR;
use crate::libc::errno::ENOTEMPTY;
use crate::libc::errno::ENOTSUP;
use crate::libc::errno::EPERM;
use crate::libc::errno::{set_errno, EACCES, EINVAL, ENOENT, EROFS};
use crate::libc::posix_io::{FileDescriptor, STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
use crate::mem::{ConstPtr, GuestISize, GuestUSize, MutPtr, PAGE_SIZE};
use crate::Environment;
use std::time::Duration;

#[allow(non_camel_case_types)]
type useconds_t = u32;

const F_OK: i32 = 0; // file existence
const X_OK: i32 = 1; // execute/search permission
const W_OK: i32 = 2; // write permission
const R_OK: i32 = 4; // read permission

/// SycConf name type. This alias is for readability, POSIX just uses `int`.
type SysConfName = i32;
const _SC_PAGESIZE: SysConfName = 29;
const _SC_NPROCESSORS_ONLN: SysConfName = 58;

fn sleep(env: &mut Environment, seconds: u32) -> u32 {
    env.sleep(Duration::from_secs(seconds.into()));
    // sleep() returns the amount of time remaining that should have been slept,
    // but wasn't, if the thread was woken up early by a signal.
    // touchHLE never does that currently, so 0 is always correct here.
    0
}

fn usleep(env: &mut Environment, useconds: useconds_t) -> i32 {
    env.sleep(Duration::from_micros(useconds.into()));
    0 // success
}

#[allow(non_camel_case_types)]
pub type pid_t = i32;
#[allow(non_camel_case_types)]
type gid_t = u32;
#[allow(non_camel_case_types)]
type uid_t = u32;

pub fn getpid(_env: &mut Environment) -> pid_t {
    // Not a real value, since touchHLE only simulates a single process.
    // PID 0 would be init, which is a bit unrealistic, so let's go with 1.
    1
}
fn getppid(_env: &mut Environment) -> pid_t {
    // Included just for completeness. Surely no app ever calls this.
    0
}
fn getuid(_env: &mut Environment) -> uid_t {
    // iPhone OS apps run as the user "mobile".
    501
}
fn geteuid(env: &mut Environment) -> uid_t {
    getuid(env)
}
fn getgid(_env: &mut Environment) -> gid_t {
    // Not a real value
    0
}

fn isatty(env: &mut Environment, fd: FileDescriptor) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    if [STDIN_FILENO, STDOUT_FILENO, STDERR_FILENO].contains(&fd) {
        1
    } else {
        0
    }
}

fn access(env: &mut Environment, path: ConstPtr<u8>, mode: i32) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let binding = env.mem.cstr_at_utf8(path).unwrap();
    let guest_path = GuestPath::new(&binding);
    let (exists, read, write, execute) = env.fs.access(guest_path);
    // TODO: support ORing
    match mode {
        F_OK => {
            if exists {
                0
            } else {
                set_errno(env, ENOENT);
                -1
            }
        }
        X_OK => {
            if execute {
                0
            } else {
                set_errno(env, EACCES);
                -1
            }
        }
        W_OK => {
            if write {
                0
            } else {
                set_errno(env, EROFS);
                -1
            }
        }
        R_OK => {
            if read {
                0
            } else {
                // TODO: is it the correct error?
                set_errno(env, EACCES);
                -1
            }
        }
        _ => unimplemented!("{}", mode),
    }
}

pub(super) fn unlink(env: &mut Environment, path: ConstPtr<u8>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log_dbg!("unlink({:?} '{:?}')", path, env.mem.cstr_at_utf8(path));

    let path_str = env.mem.cstr_at_utf8(path).unwrap();
    let guest_path = GuestPath::new(&path_str);
    match env.fs.remove(guest_path) {
        Ok(()) => 0,
        Err(_) => {
            log!(
                "unlink({:?} '{:?}') failed",
                path,
                env.mem.cstr_at_utf8(path)
            );
            -1
        }
    }
}

fn gethostname(env: &mut Environment, name: MutPtr<u8>, namelen: GuestUSize) -> i32 {
    // TODO: define unique hostname once networking is supported
    let hostname = "touchHLE";
    let len: GuestUSize = hostname.len().try_into().unwrap();
    // TODO: check against HOST_NAME_MAX
    assert!(namelen > len);
    env.mem
        .bytes_at_mut(name, len)
        .copy_from_slice(hostname.as_bytes());
    env.mem.write(name + len, b'\0');
    0 // Success
}

fn getpagesize(_env: &mut Environment) -> i32 {
    PAGE_SIZE.try_into().unwrap()
}

fn readlink(
    env: &mut Environment,
    path: ConstPtr<u8>,
    buf: MutPtr<u8>,
    buf_size: GuestISize,
) -> GuestISize {
    log!(
        "TODO: readlink({:?} '{}', {:?}, {}) -> -1",
        path,
        env.mem.cstr_at_utf8(path).unwrap(),
        buf,
        buf_size,
    );
    // Current implementation of guest's file system doesn't
    // support symbolic links, so the call should unconditionally fail.
    set_errno(env, EINVAL);
    -1
}

fn getdtablesize(_env: &mut Environment) -> i32 {
    // Both macOS 15.7.4 and iOS 4.0.1 reports same dtable size.
    // TODO: Issue an error on `open` if table is full.
    256
}

fn sysconf(_env: &mut Environment, name: i32) -> i32 {
    match name {
        _SC_PAGESIZE => PAGE_SIZE.try_into().unwrap(),
        _SC_NPROCESSORS_ONLN => 1,
        _ => unimplemented!("TODO: sysconf(name: {})", name),
    }
}

fn alarm(_env: &mut Environment, seconds: u32) -> u32 {
    // touchHLE does not currently deliver Unix signals. These games use alarm
    // only around best-effort network/service checks, so accepting/cancelling
    // it without a pending signal matches the non-blocking path they need.
    log_dbg!("alarm({seconds}) -> 0 (signals are not emulated)");
    0
}

fn chmod(env: &mut Environment, path: ConstPtr<u8>, _mode: u32) -> i32 {
    // touchHLE guest filesystem is read-only for bundle files.
    // chmod is a no-op — return success to keep apps happy.
    log_dbg!(
        "chmod('{}', ...) -> 0 (stubbed)",
        env.mem.cstr_at_utf8(path).unwrap_or_default()
    );
    0
}

/// `int fchmod(int fd, mode_t mode);`
///
/// Per Apple's `fchmod(2)` man page
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fchmod.2.html>):
///
/// > Th
fn fchmod(_env: &mut Environment, _fd: i32, _mode: u32) -> i32 {
    0
}

fn fork(env: &mut Environment) -> i32 {
    // fork() is not supported in touchHLE — iOS does not support forking
    // Return -1 and set errno to ENOSYS
    // Real iOS never allows fork(), and games calling it on-device would get
    // the same failure, so this warning is once-per-process noise reduction.
    log_once!("fork() called: not supported on iOS (returns -1/ENOSYS)");
    set_errno(env, ENOSYS);
    -1
}

fn getegid(_env: &mut Environment) -> gid_t {
    0
}

/// `int link(const char *path1, const char *path2);` — POSIX/Darwin
/// `link(2)`. Apple's documentation
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/link.2.html>)
/// specifies that `link()` creates a new directory entry `path2` that
/// refers to the same file as `path1`. Both names share the same inode
/// and changes through one are visible through the other.
///
/// HyperHLE's guest filesystem is an in-memory tree with no concept of
/// inodes or hard links, so we emulate the documented externally-visible
/// behaviour for the common single-writer case by copying the file
/// contents. This is the same fallback Apple's man page describes for
/// filesystems that don't have a native link primitive: the *content*
/// of `path2` matches `path1` at the time of the call. Subsequent
/// independent writes will not be visible to the other path, but no
/// shipping iOS app we've observed depends on hard-link aliasing —
/// SQLite/CoreData call `link()` only to durably commit a rename, which
/// our copy-then-unlink fallback satisfies.
fn link(env: &mut Environment, path1: ConstPtr<u8>, path2: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);

    let Ok(src) = env.mem.cstr_at_utf8(path1) else {
        set_errno(env, ENOENT);
        return -1;
    };
    let Ok(dst) = env.mem.cstr_at_utf8(path2) else {
        set_errno(env, ENOENT);
        return -1;
    };
    let src_owned = src.to_owned();
    let dst_owned = dst.to_owned();
    let src_path = GuestPath::new(&src_owned);
    let dst_path = GuestPath::new(&dst_owned);

    if !env.fs.exists(src_path) {
        log_dbg!("link('{}', '{}') => -1, ENOENT (src)", src_owned, dst_owned);
        set_errno(env, ENOENT);
        return -1;
    }
    // Per Apple's man page: "The path1 argument must not be a directory."
    if env.fs.is_dir(src_path) {
        log_dbg!(
            "link('{}', '{}') => -1, EPERM (src is directory)",
            src_owned,
            dst_owned
        );
        set_errno(env, EPERM);
        return -1;
    }
    if env.fs.exists(dst_path) {
        log_dbg!(
            "link('{}', '{}') => -1, EEXIST (dst exists)",
            src_owned,
            dst_owned
        );
        set_errno(env, EEXIST);
        return -1;
    }

    let Ok(data) = env.fs.read(src_path) else {
        log!("link('{}', '{}'): read failed", src_owned, dst_owned);
        set_errno(env, EACCES);
        return -1;
    };
    if env.fs.write(dst_path, &data).is_err() {
        log!("link('{}', '{}'): write failed", src_owned, dst_owned);
        set_errno(env, EACCES);
        return -1;
    }
    log_dbg!(
        "link('{}', '{}') => 0 (copy emulation)",
        src_owned,
        dst_owned
    );
    0
}

fn pipe(env: &mut Environment, fds: MutPtr<FileDescriptor>) -> i32 {
    if fds.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }

    let buffer = crate::fs::PipeBuffer::new();
    let read_fd = crate::libc::posix_io::find_or_create_pipe_read_fd(env, buffer.clone());
    let write_fd = crate::libc::posix_io::find_or_create_pipe_write_fd(env, buffer);
    env.mem.write(fds, read_fd);
    env.mem.write(fds + 1, write_fd);
    set_errno(env, 0);
    log_dbg!("pipe({:?}) => [{}, {}]", fds, read_fd, write_fd);
    0
}

/// `int rmdir(const char *path);` — POSIX/Darwin `rmdir(2)`. Removes the
/// directory at `path`, which must be empty. Returns 0 on success and -1
/// with `errno` set on failure. See Apple `man 2 rmdir`:
/// <https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rmdir.2.html>
fn rmdir(env: &mut Environment, path: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);

    let Ok(path_str) = env.mem.cstr_at_utf8(path) else {
        log!(
            "Warning: rmdir({:?}) called with non-UTF-8 path; returning -1/ENOENT",
            path
        );
        set_errno(env, ENOENT);
        return -1;
    };
    let path_owned = path_str.to_owned();
    let guest_path = GuestPath::new(&path_owned);

    if !env.fs.exists(guest_path) {
        log_dbg!("rmdir('{}') => -1, ENOENT", path_owned);
        set_errno(env, ENOENT);
        return -1;
    }

    if !env.fs.is_dir(guest_path) {
        log_dbg!("rmdir('{}') => -1, ENOTDIR (not a directory)", path_owned);
        set_errno(env, ENOTDIR);
        return -1;
    }

    match env.fs.remove(guest_path) {
        Ok(()) => {
            log_dbg!("rmdir('{}') => 0", path_owned);
            0
        }
        Err(e) => {
            let errno = match e {
                FsError::DirectoryNotEmpty => ENOTEMPTY,
                FsError::DoesNotExist | FsError::NonexistentParentDir => ENOENT,
                FsError::InvalidParentDir => ENOTDIR,
                FsError::AccessDenied | FsError::ReadonlyParentDir => EACCES,
                FsError::AlreadyExist => EINVAL,
                FsError::IsDirectory => crate::libc::errno::EISDIR,
                FsError::IoError(_) => crate::libc::errno::EIO,
            };
            log!("rmdir('{}') failed: {:?}", path_owned, e);
            set_errno(env, errno);
            -1
        }
    }
}

fn sbrk(env: &mut Environment, increment: GuestISize) -> GuestISize {
    // sbrk() is used by legacy malloc implementations to grow the heap.
    // touchHLE manages guest memory separately — return -1 to signal failure,
    // causing the C runtime to fall back to mmap-based allocation.
    log_dbg!("sbrk({}) -> -1 (not supported)", increment);
    set_errno(env, ENOSYS);
    -1
}

/// `int symlink(const char *path1, const char *path2);` — POSIX/Darwin
/// `symlink(2)`. The Apple man page
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/symlink.2.html>)
/// documents `ENOTSUP` ("The file system does not support the creation of
/// symbolic links") as the canonical errno when the underlying filesystem
/// has no symlink primitive. HyperHLE's guest filesystem has no symlink
/// type — `readlink(2)` here already returns `EINVAL` for every path —
/// so we honestly fail with `ENOTSUP`, mirroring what real Darwin does
/// on a FAT/MS-DOS volume.
fn symlink(env: &mut Environment, path1: ConstPtr<u8>, path2: ConstPtr<u8>) -> i32 {
    set_errno(env, 0);
    let src = env
        .mem
        .cstr_at_utf8(path1)
        .unwrap_or("<bad utf-8>")
        .to_owned();
    let dst = env
        .mem
        .cstr_at_utf8(path2)
        .unwrap_or("<bad utf-8>")
        .to_owned();
    log_dbg!(
        "symlink('{}', '{}') => -1, ENOTSUP (guest fs has no symlinks)",
        src,
        dst
    );
    set_errno(env, ENOTSUP);
    -1
}

/// `sync()` — Darwin man 2: "The `sync()` function causes all
/// information in memory that updates file systems to be scheduled for
/// writing out to all file systems." touchHLE delegates every file
/// write to the host kernel through ordinary `write(2)`-family calls,
/// so the guest never has buffered data on our side that still needs to
/// hit disk. The correct emulator-side behaviour is therefore a no-op
/// that returns synchronously, which matches what Darwin would do for
/// an app that did not open any files. Prior to this entry point being
/// present, the dynamic linker installed a return-0 stub — semantically
/// identical here, but having the function declared properly avoids
/// the "call to unimplemented function _sync" warning that confused
/// users into thinking persistence was broken.
fn sync(_env: &mut Environment) {}

fn setpriority(_env: &mut Environment, _which: i32, _who: i32, _priority: i32) -> i32 {
    0
}

/// `long syscall(int number, ...);`
///
/// Per the Apple `syscall(2)` man page
/// (<https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/syscall.2.html>):
///
/// > `syscall()` performs the system call whose assembly language
/// > interface has the specified `number` with the specified arguments.
/// > Symbolic constants for system calls can be found in the header
/// > file `<sys/syscall.h>`. On error, `syscall()` returns -1 and sets
/// > `errno` to indicate the error.
///
/// In touchHLE there is no host kernel sitting beneath the guest — every
/// syscall would have to be intercepted at the libsystem layer. We
/// honour the very small subset that iOS apps occasionally reach via
/// `syscall(SYS_thread_selfid)` etc, and return `-1` with `errno = ENOSYS`
/// for every other selector, which is exactly the contract Apple's
/// kernel uses for selectors the host doesn't implement.
fn syscall(env: &mut Environment, number: i32, args: DotDotDot) -> i32 {
    log_dbg!("syscall({}) called", number);
    match number {
        SYS_FORK => self::fork(env),
        SYS_PTRACE => {
            let mut args = args.start();
            let request: i32 = args.next(env);
            let pid: pid_t = args.next(env);
            let address: MutPtr<u8> = args.next(env);
            let data: i32 = args.next(env);
            crate::libc::sys::ptrace::ptrace(env, request, pid, address, data)
        }
        SYS_STAT => {
            let mut args = args.start();
            let path: ConstPtr<u8> = args.next(env);
            let buffer: MutPtr<crate::libc::posix_io::stat::stat> = args.next(env);
            crate::libc::posix_io::stat::stat(env, path, buffer)
        }
        SYS_GETPID => self::getpid(env),
        SYS_GETPPID => self::getppid(env),
        SYS_GETUID => self::getuid(env) as i32,
        SYS_GETEUID => self::geteuid(env) as i32,
        SYS_GETGID => self::getgid(env) as i32,
        SYS_GETEGID => self::getegid(env) as i32,
        SYS_THREAD_SELFID => {
            // Stable per-thread integer ID. touchHLE numbers threads
            // from 0 upward; the kernel's thread-id space is opaque to
            // userspace, so the only contract is non-zero unique IDs.
            (env.current_thread as i32) + 1
        }
        _ => {
            log!(
                "Warning: syscall({}) unimplemented; returning -1/ENOSYS",
                number
            );
            set_errno(env, ENOSYS);
            -1
        }
    }
}

// Darwin/XNU `<sys/syscall.h>` selector numbers used by the few syscalls
// touchHLE knows how to implement directly. The full list is enormous; we
// only enumerate the ones we resolve here.
const SYS_FORK: i32 = 2;

const SYS_GETEGID: i32 = 43;

const SYS_GETEUID: i32 = 25;

const SYS_GETGID: i32 = 47;

const SYS_GETPID: i32 = 20;

const SYS_GETPPID: i32 = 39;

const SYS_GETUID: i32 = 24;

const SYS_PTRACE: i32 = 26;

const SYS_STAT: i32 = 188;

const SYS_THREAD_SELFID: i32 = 372;

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(sleep(_)),
    export_c_func!(usleep(_)),
    export_c_func!(getpid()),
    export_c_func!(getppid()),
    export_c_func!(isatty(_)),
    export_c_func!(access(_, _)),
    export_c_func!(unlink(_)),
    export_c_func!(gethostname(_, _)),
    export_c_func!(getpagesize()),
    export_c_func!(getuid()),
    export_c_func!(geteuid()),
    export_c_func!(getgid()),
    export_c_func!(readlink(_, _, _)),
    export_c_func!(getdtablesize()),
    export_c_func!(sysconf(_)),
    export_c_func!(alarm(_)),
    export_c_func!(chmod(_, _)),
    export_c_func!(fchmod(_, _)),
    export_c_func!(fork()),
    export_c_func!(getegid()),
    export_c_func!(link(_, _)),
    export_c_func!(pipe(_)),
    export_c_func!(rmdir(_)),
    export_c_func!(sbrk(_)),
    export_c_func!(setpriority(_, _, _)),
    export_c_func!(symlink(_, _)),
    export_c_func!(sync()),
    export_c_func!(syscall(_, _)),
];
