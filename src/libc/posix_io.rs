/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! POSIX I/O functions (`fcntl.h`, parts of `unistd.h`, etc)

pub mod stat;
pub mod statvfs;

use crate::abi::DotDotDot;
use crate::dyld::{export_c_func, FunctionExports};
use crate::fs::{FsError, GuestFile, GuestOpenOptions, GuestPath};
use crate::libc::errno::{
    set_errno, EACCES, EBADF, EEXIST, EFAULT, EINTR, EINVAL, EIO, EISDIR, ENOENT, ENOTDIR,
    EOVERFLOW, ESPIPE,
};
use crate::libc::sys::socket::{close_socket, duplicate_socket};
use crate::libc::unistd::pid_t;
use crate::mem::{
    ConstPtr, ConstVoidPtr, GuestISize, GuestUSize, MutPtr, MutVoidPtr, Ptr, SafeRead,
};
use crate::Environment;
use std::io::{Read, Seek, SeekFrom, Write};

#[derive(Default)]
pub struct State {
    /// File descriptors _other than stdin, stdout, and stderr_
    files: Vec<Option<PosixFileHostObject>>,
}
impl State {
    fn file_for_fd(&mut self, fd: FileDescriptor) -> Option<&mut PosixFileHostObject> {
        self.files
            .get_mut(fd_to_file_idx(fd))
            .and_then(|file_or_none| file_or_none.as_mut())
    }

    pub fn guest_file(&self, fd: FileDescriptor) -> Option<&GuestFile> {
        if fd < NORMAL_FILENO_BASE {
            return None;
        }
        self.files
            .get(fd_to_file_idx(fd))
            .and_then(|file_or_none| file_or_none.as_ref())
            .map(|file_obj| &file_obj.file)
    }

    pub(crate) fn is_fd_open(&mut self, fd: FileDescriptor) -> bool {
        self.files.get(fd_to_file_idx(fd)).is_some()
    }
}

struct PosixFileHostObject {
    file: GuestFile,
    needs_flush: bool,
    reached_eof: bool,
    flags: i32,
    save_dump: bool,
}

// TODO: stdin/stdout/stderr handling somehow
fn file_idx_to_fd(idx: usize) -> FileDescriptor {
    FileDescriptor::try_from(idx)
        .unwrap()
        .checked_add(NORMAL_FILENO_BASE)
        .unwrap()
}
fn fd_to_file_idx(fd: FileDescriptor) -> usize {
    fd.checked_sub(NORMAL_FILENO_BASE).unwrap() as usize
}

/// File descriptor type. This alias is for readability, POSIX just uses `int`.
pub type FileDescriptor = i32;
pub const STDIN_FILENO: FileDescriptor = 0;
pub const STDOUT_FILENO: FileDescriptor = 1;
pub const STDERR_FILENO: FileDescriptor = 2;
const NORMAL_FILENO_BASE: FileDescriptor = STDERR_FILENO + 1;

/// Flags bitfield for `open`. This alias is for readability, POSIX just uses
/// `int`.
pub type OpenFlag = i32;
pub const O_RDONLY: OpenFlag = 0x0;
pub const O_WRONLY: OpenFlag = 0x1;
pub const O_RDWR: OpenFlag = 0x2;
pub const O_ACCMODE: OpenFlag = O_RDWR | O_WRONLY | O_RDONLY;

pub const O_NONBLOCK: OpenFlag = 0x4;
pub const O_APPEND: OpenFlag = 0x8;
pub const O_SHLOCK: OpenFlag = 0x10;
pub const O_NOFOLLOW: OpenFlag = 0x100;
pub const O_CREAT: OpenFlag = 0x200;
pub const O_TRUNC: OpenFlag = 0x400;
pub const O_EXCL: OpenFlag = 0x800;
/// There are no terminals, so this has no effect.
pub const O_NOCTTY: OpenFlag = 0x20000;

/// File control command flags.
/// This alias is for readability, POSIX just uses `int`.
pub type FileControlCommand = i32;
const F_GETFD: FileControlCommand = 1;
const F_DUPFD: FileControlCommand = 0;
const F_SETFD: FileControlCommand = 2;
const F_GETLK: FileControlCommand = 7;
const F_SETLK: FileControlCommand = 8;
const F_RDADVISE: FileControlCommand = 44;
const F_NOCACHE: FileControlCommand = 48;

/// File Descriptor flags.
/// This alias is for readability, POSIX just uses `int`.
pub type FDFlag = i32;
pub const FD_CLOEXEC: FDFlag = 1;

/// Record Locking flags.
/// This alias is for readability, POSIX just uses `short`
pub type RecordLockingFlag = i16;
pub const F_RDLCK: RecordLockingFlag = 1;
pub const F_UNLCK: RecordLockingFlag = 2;
pub const F_WRLCK: RecordLockingFlag = 3;

#[repr(C, packed)]
#[derive(Debug)]
#[allow(non_camel_case_types)]
struct flock {
    start: off_t,
    len: off_t,
    pid: pid_t,
    lock_type: i16,
    whence: i16,
}
unsafe impl SafeRead for flock {}

// HyperHLE's Subway Surfers compatibility work identified dup() as necessary
// when the game opens its data file. Descriptors share the underlying file.
fn duplicate_file(env: &mut Environment, fd: FileDescriptor) -> Option<PosixFileHostObject> {
    if fd < NORMAL_FILENO_BASE {
        set_errno(env, EBADF);
        return None;
    }
    let Some(source) = env.libc_state.posix_io.file_for_fd(fd) else {
        set_errno(env, EBADF);
        return None;
    };
    let file = if matches!(source.file, GuestFile::Socket) {
        GuestFile::Socket
    } else {
        source.file.share().unwrap()
    };
    Some(PosixFileHostObject {
        file,
        needs_flush: source.needs_flush,
        reached_eof: false,
        flags: 0,
        save_dump: source.save_dump,
    })
}

fn duplicate_min(env: &mut Environment, fd: FileDescriptor, min: FileDescriptor) -> FileDescriptor {
    if !(0..10240).contains(&min) {
        set_errno(env, EINVAL);
        return -1;
    }
    let Some(file) = duplicate_file(env, fd) else {
        return -1;
    };
    let first = fd_to_file_idx(min.max(NORMAL_FILENO_BASE));
    let files = &mut env.libc_state.posix_io.files;
    let idx = (first..files.len())
        .find(|&i| files[i].is_none())
        .unwrap_or(files.len().max(first));
    if idx >= fd_to_file_idx(10240) {
        set_errno(env, EINVAL);
        return -1;
    }
    if files.len() <= idx {
        files.resize_with(idx + 1, || None);
    }
    let socket = matches!(file.file, GuestFile::Socket);
    files[idx] = Some(file);
    let new_fd = file_idx_to_fd(idx);
    if socket {
        assert!(duplicate_socket(env, fd, new_fd));
    }
    new_fd
}

fn dup(env: &mut Environment, fd: FileDescriptor) -> FileDescriptor {
    duplicate_min(env, fd, 0)
}

fn dup2(env: &mut Environment, fd: FileDescriptor, target: FileDescriptor) -> FileDescriptor {
    if !(NORMAL_FILENO_BASE..10240).contains(&target) {
        set_errno(env, EBADF);
        return -1;
    }
    if fd == target {
        if env.libc_state.posix_io.file_for_fd(fd).is_some() {
            return target;
        }
        set_errno(env, EBADF);
        return -1;
    }
    let Some(file) = duplicate_file(env, fd) else {
        return -1;
    };
    let idx = fd_to_file_idx(target);
    if env
        .libc_state
        .posix_io
        .files
        .get(idx)
        .is_some_and(Option::is_some)
    {
        close(env, target);
    }
    let files = &mut env.libc_state.posix_io.files;
    if files.len() <= idx {
        files.resize_with(idx + 1, || None);
    }
    let socket = matches!(file.file, GuestFile::Socket);
    files[idx] = Some(file);
    if socket {
        assert!(duplicate_socket(env, fd, target));
    }
    target
}

pub type FLockFlag = i32;
pub const LOCK_SH: FLockFlag = 1;
#[allow(dead_code)]
pub const LOCK_EX: FLockFlag = 2;
#[allow(dead_code)]
pub const LOCK_NB: FLockFlag = 4;
#[allow(dead_code)]
pub const LOCK_UN: FLockFlag = 8;

fn open(env: &mut Environment, path: ConstPtr<u8>, flags: i32, _args: DotDotDot) -> FileDescriptor {
    // TODO: handle errno properly
    set_errno(env, 0);

    // TODO: parse variadic arguments and pass them on (file creation mode)
    self::open_direct(env, path, flags)
}

/// Special extension for host code: [open] without the [DotDotDot].
pub fn open_direct(env: &mut Environment, path: ConstPtr<u8>, flags: i32) -> FileDescriptor {
    // TODO: support more flags, this list is not complete
    assert!(
        flags
            & !(O_ACCMODE
                | O_NONBLOCK
                | O_APPEND
                | O_SHLOCK
                | O_NOFOLLOW
                | O_CREAT
                | O_TRUNC
                | O_EXCL
                | O_NOCTTY)
            == 0
    );

    if path.is_null() {
        log_dbg!("open({:?}, {:#x}) => -1", path, flags);
        set_errno(env, EFAULT);
        return -1;
    }

    // TODO: respect the mode (in the variadic arguments) when creating a file
    // Note: NONBLOCK flag is ignored, assumption is all file I/O is fast
    let mut needs_flush = false;
    let mut options = GuestOpenOptions::new();
    match flags & O_ACCMODE {
        O_RDONLY => {
            options.read();
        }
        O_WRONLY => {
            options.write();
            needs_flush = true;
        }
        O_RDWR => {
            options.read().write();
            needs_flush = true;
        }
        _ => panic!(),
    };
    if (flags & O_APPEND) != 0 {
        options.append();
    }
    if (flags & O_CREAT) != 0 {
        options.create();
    }
    if (flags & O_TRUNC) != 0 {
        options.truncate();
    }
    if (flags & O_EXCL) != 0 {
        options.exclusive();
    }

    let path_string = match env.mem.cstr_at_utf8(path) {
        Ok(path_str) => path_str.to_owned(),
        Err(err) => {
            log!(
                "open() error, unable to treat {:?} as utf8 str: {:?}",
                path,
                err
            );
            // TODO: set errno
            return -1;
        }
    };
    // TODO: symlinks don't exist in the FS yet, so we can't "not follow" them.
    if flags & O_NOFOLLOW != 0 {
        log!("Ignoring O_NOFOLLOW when opening {:?}", path_string);
    }
    let res = match env
        .fs
        .open_with_options(GuestPath::new(&path_string), options)
    {
        Ok(file) => {
            let host_object = PosixFileHostObject {
                file,
                needs_flush,
                reached_eof: false,
                flags: 0,
                save_dump: path_string.contains("savedata") && crate::save_research::enabled(),
            };

            find_or_create_fd(env, host_object)
        }
        Err(error) => {
            log!("Warning: open({path_string:?}, {flags:#x}) failed with: {error:?}, returning -1");
            let errno = match error {
                FsError::AccessDenied => EACCES,
                FsError::AlreadyExist => EEXIST,
                FsError::DoesNotExist => ENOENT,
                FsError::InvalidParentDir => ENOTDIR,
                FsError::IsDirectory => EISDIR,
                FsError::NonexistentParentDir => ENOENT,
                FsError::ReadonlyParentDir => EACCES,
                FsError::IoError(_) => EIO,
                _ => unimplemented!(),
            };
            set_errno(env, errno);
            -1
        }
    };
    if res != -1 && (flags & O_SHLOCK) != 0 {
        // TODO: Handle possible errors
        flock(env, res, LOCK_SH);
    }
    log_dbg!(
        "open({:?} {:?}, {:#x}) => {:?}",
        path,
        path_string,
        flags,
        res
    );
    res
}

pub fn read(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: MutVoidPtr,
    size: GuestUSize,
) -> GuestISize {
    // TODO: handle errno properly
    set_errno(env, 0);

    if buffer.is_null() {
        // TODO: set errno to EFAULT
        return -1;
    }

    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        log!(
            "Warning: read({:?}, {:?}, {:#x}) called with unknown fd, returning -1",
            fd,
            buffer,
            size,
        );
        // TODO: set errno
        return -1;
    };

    let buffer_slice = env.mem.bytes_at_mut(buffer.cast(), size);
    match file.file.read(buffer_slice) {
        Ok(bytes_read) => {
            if file.save_dump && bytes_read > 0 {
                crate::save_research::append(
                    "savedata_read_stream.bin",
                    &buffer_slice[..bytes_read],
                );
                log!(
                    "[POSIX_READ] savedata read {} bytes (fd={:?})",
                    bytes_read,
                    fd
                );
            }
            if bytes_read == 0 && size != 0 {
                // need to set EOF
                file.reached_eof = true;
            }
            let short_read = bytes_read < buffer_slice.len();
            let diagnostic = env.options.performance_test;
            if short_read && diagnostic.is_some() {
                env.framework_state.performance.short_reads += 1;
            }
            if short_read && !diagnostic.is_some_and(|mode| mode.quiet()) {
                log!(
                    "Warning: read({:?}, {:?}, {:#x}) read only {:#x} bytes",
                    fd,
                    buffer,
                    size,
                    bytes_read,
                );
            } else {
                log_dbg!(
                    "read({:?}, {:?}, {:#x}) => {:#x}",
                    fd,
                    buffer,
                    size,
                    bytes_read,
                );
            }
            bytes_read.try_into().unwrap()
        }
        Err(e) => {
            let res = match e.kind() {
                std::io::ErrorKind::IsADirectory => {
                    set_errno(env, EISDIR);
                    // the returned value was validated on iOS
                    0
                }
                _ => {
                    // TODO: set errno
                    -1
                }
            };
            log!(
                "Warning: read({:?}, {:?}, {:#x}) encountered error {:?}, returning {}",
                fd,
                buffer,
                size,
                e,
                res,
            );
            res
        }
    }
}

pub fn pread(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: MutVoidPtr,
    size: GuestUSize,
    offset: off_t,
) -> GuestISize {
    // Rust doesn't provide a way of reading at a specific offset on windows
    // without affecting the underlying files cursor. Rather than bringing in a
    // library that does this or dealing with the unsafe windows API directly
    // (ReadFile + Overlapped), we can emulate the behavior with a set of seek
    // and read calls.

    // Errno is set by downstream lseek and read calls
    let original_position = lseek(env, fd, 0, SEEK_CUR);
    if original_position == -1 {
        return -1;
    }

    if lseek(env, fd, offset, SEEK_SET) == -1 {
        return -1;
    }

    let bytes_read = read(env, fd, buffer, size);

    assert!(lseek(env, fd, original_position, SEEK_SET) != -1);

    bytes_read
}

/// Helper for C `feof()`.
pub(super) fn eof(env: &mut Environment, fd: FileDescriptor) -> i32 {
    let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();
    if file.reached_eof {
        1
    } else {
        0
    }
}

/// Helper for C `clearerr()`.
pub(super) fn clearerr(env: &mut Environment, fd: FileDescriptor) {
    // TODO: handle errno properly
    set_errno(env, 0);

    let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();
    file.reached_eof = false;
}

/// Helper for C `fflush()`.
pub(super) fn fflush(env: &mut Environment, fd: FileDescriptor) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        // TODO: set errno to EBADF
        return -1;
    };
    match file.file.flush() {
        Ok(_) => 0,
        Err(_) => -1,
    }
}

pub fn write(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: ConstVoidPtr,
    size: GuestUSize,
) -> GuestISize {
    // TODO: handle errno properly
    set_errno(env, 0);

    // TODO: error handling for unknown fd?
    let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();

    if buffer.is_null() {
        assert_eq!(size, 0);
        return 0;
    }

    let buffer_slice = env.mem.bytes_at(buffer.cast(), size);
    if file.save_dump {
        crate::save_research::append("savedata_write_stream.bin", buffer_slice);
        // Dump guest memory around the write buffer to look for the plaintext.
        let buf_addr: u32 = buffer.to_bits();
        let start = buf_addr.saturating_sub(0x800000) & !0xfff;
        let size_dump = 0x1800000usize;
        let Some(pre) = env.mem.get_bytes_fallible(
            crate::mem::Ptr::from_bits(start),
            size_dump.try_into().unwrap(),
        ) else {
            log!(
                "[POSIX_WRITE] savedata write {} bytes buffer_guest={:#x} (mem dump failed)",
                size,
                buf_addr
            );
            return match file.file.write(buffer_slice) {
                Ok(bytes_written) => bytes_written.try_into().unwrap(),
                Err(_e) => -1,
            };
        };
        crate::save_research::write("savedata_pre_write_mem.bin", pre);
        log!(
            "[POSIX_WRITE] savedata write {} bytes buffer_guest={:#x} pre_mem_dump={:#x}-{:#x}",
            size,
            buf_addr,
            start,
            start + size_dump as u32
        );
        // Export inner-cipher write trace captured by the CPU write hook.
        crate::cpu::dump_mhp2g_write_log();
        let regs = env.cpu.regs();
        let mut stack_trace = String::new();
        // Unwind guest stack: LR of the frame that called the libc write.
        // AAPCS: callee entry pushes {.., lr}; saved LR sits at [sp+4*n].
        // Start by reading the saved LR at the top of the current frame.
        let sp = regs[13];
        let mut lr = regs[14];
        for _ in 0..10 {
            if lr & 1 != 0 {
                lr &= !1; // Thumb bit
            }
            if !(0x8000..0x70000000).contains(&lr) {
                break;
            }
            stack_trace.push_str(&format!("{:#x} <- ", lr));
            // Read a candidate saved LR from the guest stack, trying several
            // offsets (frame layouts vary).
            let mut found = false;
            for off in [
                0u32, 4, 8, 12, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60,
            ] {
                let addr = sp.wrapping_add(off);
                let Some(bytes) = env
                    .mem
                    .get_bytes_fallible(crate::mem::Ptr::from_bits(addr), 4)
                else {
                    continue;
                };
                let cand = u32::from_le_bytes(bytes.try_into().unwrap()) & !1;
                if (0x8000..0x70000000).contains(&cand) {
                    lr = cand;
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }
        log!(
            "[POSIX_WRITE] savedata write {} bytes pc={:#x} lr={:#x} sp={:#x} trace: {}",
            size,
            regs[15],
            regs[14],
            sp,
            stack_trace
        );
    }
    match file.file.write(buffer_slice) {
        Ok(bytes_written) => {
            if bytes_written < buffer_slice.len() {
                log!(
                    "Warning: write({:?}, {:?}, {:#x}) wrote only {:#x} bytes",
                    fd,
                    buffer,
                    size,
                    bytes_written,
                );
            } else {
                log_dbg!(
                    "write({:?}, {:?}, {:#x}) => {:#x}",
                    fd,
                    buffer,
                    size,
                    bytes_written,
                );
            }
            bytes_written.try_into().unwrap()
        }
        Err(e) => {
            // TODO: set errno
            log!(
                "Warning: write({:?}, {:?}, {:#x}) encountered error {:?}, returning -1",
                fd,
                buffer,
                size,
                e,
            );
            -1
        }
    }
}

pub fn pwrite(
    env: &mut Environment,
    fd: FileDescriptor,
    buffer: ConstVoidPtr,
    size: GuestUSize,
    offset: off_t,
) -> GuestISize {
    // Rust doesn't provide a way of writing at a specific offset on windows
    // without affecting the underlying files cursor. Rather than bringing in a
    // library that does this or dealing with the unsafe windows API directly
    // (WriteFile + Overlapped), we can emulate the behavior with a set of seek
    // and write calls.

    // Errno is set by downstream lseek and write calls
    let original_position = lseek(env, fd, 0, SEEK_CUR);
    if original_position == -1 {
        return -1;
    }

    if lseek(env, fd, offset, SEEK_SET) == -1 {
        return -1;
    }

    let bytes_written = write(env, fd, buffer, size);

    assert!(lseek(env, fd, original_position, SEEK_SET) != -1);

    bytes_written
}

#[allow(non_camel_case_types)]
pub type off_t = i64;
pub const SEEK_SET: i32 = 0;
pub const SEEK_CUR: i32 = 1;
pub const SEEK_END: i32 = 2;
pub fn lseek(env: &mut Environment, fd: FileDescriptor, offset: off_t, whence: i32) -> off_t {
    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        log!("lseek({:?}, {:#x}, {}) => {}", fd, offset, whence, -1);
        set_errno(env, EBADF);
        return -1;
    };

    if !file.file.is_seekable() {
        log!(
            "Warning: lseek({:?}, {:#x}, {}) => -1. Called with unseekable fd.",
            fd,
            offset,
            whence
        );
        set_errno(env, ESPIPE);
        return -1;
    }

    let start_position = match whence {
        SEEK_SET => 0,
        SEEK_CUR => match file.file.stream_position() {
            Ok(pos) => pos,
            Err(seek_error) => {
                match seek_error.kind() {
                    std::io::ErrorKind::IsADirectory => set_errno(env, EISDIR),
                    _ => unimplemented!("Unexpected seek error {:?}", seek_error),
                }
                return -1;
            }
        },
        SEEK_END => match file.file.stream_len() {
            Ok(len) => len,
            Err(seek_error) => {
                match seek_error.kind() {
                    std::io::ErrorKind::IsADirectory => set_errno(env, EISDIR),
                    _ => unimplemented!("Unexpected seek error {:?}", seek_error),
                }
                return -1;
            }
        },
        _ => {
            log!(
                "Warning: lseek({:?}, {:#x}, {}) => -1. Called with invalid \"whence\".",
                fd,
                offset,
                whence
            );
            set_errno(env, EINVAL);
            return -1;
        }
    };

    let seek_position = match start_position.checked_add_signed(offset) {
        Some(position) => position,
        None => {
            let (error_msg, errno) = if offset >= 0 {
                ("Seek position does not fit in off_t.", EOVERFLOW)
            } else {
                ("Negative seek position.", EINVAL)
            };
            log!(
                "Warning: lseek({:?}, {:#x}, {}) => -1. {}",
                fd,
                offset,
                whence,
                error_msg
            );
            set_errno(env, errno);
            return -1;
        }
    };

    if seek_position > off_t::MAX as u64 {
        log!(
            "Warning: lseek({:?}, {:#x}, {}) => -1. Seek position does not fit in off_t.",
            fd,
            offset,
            whence
        );
        set_errno(env, EOVERFLOW);
        return -1;
    }

    let res = match file.file.seek(SeekFrom::Start(seek_position)) {
        Ok(new_offset) => {
            // TODO: this side-effect should be tightened to `fseek`
            // "A successful call to the fseek() function clears
            // the end-of-file indicator for the stream..."
            file.reached_eof = false;

            new_offset.try_into().unwrap()
        }
        Err(seek_error) => {
            match seek_error.kind() {
                std::io::ErrorKind::InvalidInput => set_errno(env, EINVAL),
                std::io::ErrorKind::IsADirectory => set_errno(env, EISDIR),
                _ => unimplemented!("Unexpected seek error {:?}", seek_error),
            }
            log!(
                "Warning: lseek({:?}, {:#x}, {}) failed with error: {:?}, returning -1",
                fd,
                offset,
                whence,
                seek_error
            );
            return -1;
        }
    };
    log_dbg!("lseek({:?}, {:#x}, {}) => {}", fd, offset, whence, res);
    res
}

pub fn close(env: &mut Environment, fd: FileDescriptor) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    if matches!(fd, STDIN_FILENO | STDOUT_FILENO | STDERR_FILENO) {
        log_dbg!("close({:?}) => 0", fd);
        return 0;
    }

    if fd < 0
        || env
            .libc_state
            .posix_io
            .files
            .get(fd_to_file_idx(fd))
            .is_none()
    {
        set_errno(env, EBADF);
        log!("Warning: close({:?}) failed, returning -1", fd);
        return -1;
    }

    let result = match env.libc_state.posix_io.files[fd_to_file_idx(fd)].take() {
        Some(file) => {
            // The actual closing of the file happens implicitly when `file`
            // falls out of scope. The return value is about whether actions
            // performed before closing succeed or not.
            match file.file {
                // Closing directories requires no other actions
                GuestFile::Directory => 0,
                // Socket is a special case
                GuestFile::Socket => {
                    close_socket(env, fd);
                    0
                }
                // Files must be synced if they require flushing
                _ => {
                    if !file.needs_flush {
                        0
                    } else {
                        match file.file.sync_all() {
                            Ok(()) => 0,
                            Err(_) => {
                                // TODO: set errno
                                -1
                            }
                        }
                    }
                }
            }
        }
        None => {
            set_errno(env, EBADF);
            -1
        }
    };

    if result == 0 {
        log_dbg!("close({:?}) => 0", fd);
    } else {
        log!("Warning: close({:?}) failed, returning -1", fd);
    }
    result
}

fn rename(env: &mut Environment, old: ConstPtr<u8>, new: ConstPtr<u8>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let old = env.mem.cstr_at_utf8(old).unwrap();
    let new = env.mem.cstr_at_utf8(new).unwrap();
    let res = match env.fs.rename(GuestPath::new(&old), GuestPath::new(&new)) {
        Ok(_) => 0,
        Err(_) => -1,
    };
    log_dbg!("rename('{}', '{}') => {}", old, new, res);
    res
}

pub fn getcwd(env: &mut Environment, buf_ptr: MutPtr<u8>, buf_size: GuestUSize) -> MutPtr<u8> {
    let working_directory = env.fs.working_directory();
    if !env.fs.is_dir(working_directory) {
        // TODO: set errno to ENOENT
        log!(
            "Warning: getcwd({:?}, {:#x}) failed, returning NULL",
            buf_ptr,
            buf_size
        );
        return Ptr::null();
    }

    let working_directory = env.fs.working_directory().as_str().as_bytes();

    if buf_ptr.is_null() {
        // The buffer size argument is presumably ignored in this mode.
        // This mode is an extension, which might explain the strange API.
        let res = env.mem.alloc_and_write_cstr(working_directory);
        log_dbg!("getcwd(NULL, _) => {:?} ({:?})", res, working_directory);
        return res;
    }

    // Includes space for null terminator
    let res_size: GuestUSize = u32::try_from(working_directory.len()).unwrap() + 1;

    if buf_size < res_size {
        // TODO: set errno to EINVAL or ERANGE as appropriate
        log!(
            "Warning: getcwd({:?}, {:#x}) failed, returning NULL",
            buf_ptr,
            buf_size
        );
        return Ptr::null();
    }

    let buf = env.mem.bytes_at_mut(buf_ptr, res_size);
    buf[..(res_size - 1) as usize].copy_from_slice(working_directory);
    buf[(res_size - 1) as usize] = b'\0';

    log_dbg!(
        "getcwd({:?}, {:#x}) => {:?}, wrote {:?} ({:#x} bytes)",
        buf_ptr,
        buf_size,
        buf_ptr,
        working_directory,
        res_size
    );
    buf_ptr
}

fn chdir(env: &mut Environment, path_ptr: ConstPtr<u8>) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let path = GuestPath::new(env.mem.cstr_at_utf8(path_ptr).unwrap());
    match env.fs.change_working_directory(path) {
        Ok(new) => {
            log_dbg!(
                "chdir({:?}) => 0, new working directory: {:?}",
                path_ptr,
                new,
            );
            0
        }
        Err(()) => {
            log!("Warning: chdir({:?}) failed, could not change working directory to {:?}, returning -1", path_ptr, path);
            // TODO: set errno
            -1
        }
    }
}
// TODO: fchdir(), once open() on a directory is supported.

fn fcntl(
    env: &mut Environment,
    fd: FileDescriptor,
    cmd: FileControlCommand,
    args: DotDotDot,
) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    if fd >= NORMAL_FILENO_BASE
        && env
            .libc_state
            .posix_io
            .files
            .get(fd_to_file_idx(fd))
            .is_none()
    {
        set_errno(env, EBADF);
        return -1;
    }

    match cmd {
        F_DUPFD => {
            let min: i32 = args.start().next(env);
            return duplicate_min(env, fd, min);
        }
        F_GETFD => {
            let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();
            return file.flags;
        }
        F_SETFD => {
            // SET/GETFD are responsible for managing the CLOEXEC flag on an FD.
            // When set, exec* calls automatically close open FDs to prevent
            // them from leaking. Since touchHLE only runs a single process and
            // non-jailbroken apps can not call exec* functions, it's safe to
            // ignore the flag being set.

            // TODO: When exec* is added implement CLOEXEC functionality
            let flags: i32 = args.start().next(env);
            assert!(matches!(flags, FD_CLOEXEC | 0));
            if flags & FD_CLOEXEC == FD_CLOEXEC {
                log!(
                    "TODO: fcntl({}, F_SETFD, {}) called. CLOEXEC currently not supported.",
                    fd,
                    flags
                );
            }
            let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();
            file.flags = flags;
        }
        F_GETLK => {
            let lock_ptr: MutPtr<flock> = args.start().next(env);
            let mut lock = env.mem.read(lock_ptr);

            if let Err(error_code) = validate_lock(env, fd, &lock) {
                set_errno(env, error_code);
                return -1;
            }

            // Since locks are never set, claim no conflict by setting lock_type
            // to F_UNLCK. For more info check F_SETLK match arm.
            // TODO: actually check locks set by other processes and return
            // a conflict if it exists
            log!(
                "TODO: fcntl({}, F_GETLK, {:?}) called. Locking unimplemented, any conflicts will be unreported.",
                fd,
                lock
            );
            lock.lock_type = F_UNLCK;
            env.mem.write(lock_ptr, lock);
        }
        F_SETLK => {
            let lock_ptr: MutPtr<flock> = args.start().next(env);
            let lock = env.mem.read(lock_ptr);

            if let Err(error_code) = validate_lock(env, fd, &lock) {
                set_errno(env, error_code);
                return -1;
            }

            // POSIX locks are process based which means that any threads within
            // a process don't conflict with its own process's locks.
            // For example, setting a lock that conflicts with another lock set
            // by a thread in the same process results in the lock being either:
            // upgraded, extended, split, etc., but it will not conflict.
            // Practically, since touchHLE supports only one process, POSIX
            // locks don't do anything, so they can temporarily be ignored.
            // TODO: Actually set locks when multiproccess support is added and
            // the file system supports it.
            log!(
                "TODO: fcntl({}, F_SETLK, {:?}) called. Locking unimplemented, ignoring lock.",
                fd,
                lock
            );
        }
        F_NOCACHE => {
            let mut args = args.start();
            let arg: i32 = args.next(env);
            assert_eq!(arg, 1);
            log!(
                "TODO: Ignoring enabling F_NOCACHE for file descriptor {}",
                fd
            );
        }
        F_RDADVISE => {
            log_dbg!("TODO: Ignoring F_RDADVISE for file descriptor {}", fd);
        }
        _ => unimplemented!(),
    }
    0 // success
}

fn flock(env: &mut Environment, fd: FileDescriptor, operation: FLockFlag) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    log!("TODO: flock({:?}, {:?})", fd, operation);
    0
}

pub fn fsync(env: &mut Environment, fd: FileDescriptor) -> i32 {
    let Some(file) = env.libc_state.posix_io.file_for_fd(fd) else {
        log!(
            "Warning: fsync({:?}) called with unknown fd, returning -1",
            fd,
        );
        set_errno(env, EBADF);
        return -1;
    };

    match file.file.sync_all() {
        Ok(()) => 0,
        Err(error) => {
            match error.kind() {
                std::io::ErrorKind::PermissionDenied => {
                    log!(
                        "Warning: fsync({:?}) sync failed with error: {:?}, returning 0 to match expected behavior",
                        fd,
                        error
                    );
                    return 0;
                }
                std::io::ErrorKind::Unsupported => set_errno(env, EINVAL),
                std::io::ErrorKind::Interrupted => set_errno(env, EINTR),
                _ => set_errno(env, EIO),
            }

            log!(
                "Warning: fsync({:?}) sync failed with error: {:?}, returning -1",
                fd,
                error
            );
            -1
        }
    }
}

pub fn ftruncate(env: &mut Environment, fd: FileDescriptor, len: off_t) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();
    match file.file.set_len(len as u64) {
        Ok(()) => 0,
        Err(_) => -1, // TODO: set errno
    }
}

fn truncate(env: &mut Environment, path_ptr: ConstPtr<u8>, len: off_t) -> i32 {
    // TODO: handle errno properly
    set_errno(env, 0);

    let path_string = match env.mem.cstr_at_utf8(path_ptr) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            return -1; // TODO: set errno
        }
    };

    let fd = open_direct(env, path_ptr, O_WRONLY);
    if fd < 0 {
        log_dbg!("truncate('{}', {}) => -1", path_string, len);
        return -1;
    }

    let res = ftruncate(env, fd, len);

    close(env, fd);

    log_dbg!("truncate('{}', {}) => {}", path_string, len, res);
    res
}

/// Extended attributes aren't supported, but pretend they were set (apps use
/// this to exclude files from iCloud backups).
fn setxattr(
    env: &mut Environment,
    path: ConstPtr<u8>,
    name: ConstPtr<u8>,
    _value: ConstVoidPtr,
    _size: GuestUSize,
    _position: u32,
    _options: i32,
) -> i32 {
    log!(
        "TODO: setxattr({:?}, {:?}, ...) (ignored)",
        env.mem.cstr_at_utf8(path),
        env.mem.cstr_at_utf8(name)
    );
    0
}

fn creat(env: &mut Environment, path: ConstPtr<u8>, _mode: u32) -> i32 {
    // creat(path, mode) == open(path, O_WRONLY|O_CREAT|O_TRUNC)
    // O_WRONLY=0x0001, O_CREAT=0x0200, O_TRUNC=0x0400
    let flags = 0x0001 | 0x0200 | 0x0400;
    open_direct(env, path, flags)
}

fn open_dprotected_np(
    env: &mut Environment,
    path: ConstPtr<u8>,
    flags: i32,
    _class: i32,
    _dpflags: i32,
    args: DotDotDot,
) -> FileDescriptor {
    open(env, path, flags, args)
}

/// `int poll(struct pollfd *fds, nfds_t nfds, int timeout)` — полная
/// реализация: готовность сокетов (через
/// socket_poll_events), обычных файлов и pipe-концов;
/// блокировка с yield'ом гостевого ранлайна
/// (env.sleep, как в select); записи `revents` всегда по
/// возврату.
fn poll(env: &mut Environment, fds: MutPtr<PollFd>, nfds: u32, timeout: i32) -> i32 {
    set_errno(env, 0);
    if nfds > 0 && fds.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }
    // POSIX: EINVAL, если nfds больше {OPEN_MAX} (на iOS = 10240).
    if nfds > 10240 {
        set_errno(env, EINVAL);
        return -1;
    }

    // timeout < 0 — ждать бессрочно; 0 — ровно один
    // опрос.
    let deadline = if timeout < 0 {
        None
    } else {
        Some(std::time::Instant::now() + std::time::Duration::from_millis(timeout as u64))
    };
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

    let mut revents = vec![0i16; nfds as usize];
    let mut count;
    loop {
        let mut ready = 0i32;
        for i in 0..nfds {
            let pfd: PollFd = env.mem.read(fds + i);
            let re = evaluate_poll_fd(env, pfd.fd, pfd.events);
            revents[i as usize] = re;
            if re != 0 {
                ready += 1;
            }
        }
        count = ready;
        if ready > 0 || timeout == 0 {
            break;
        }
        // env.sleep() уступает гостевому ранлайну —
        // остальные потоки идут.
        match deadline {
            Some(deadline) => {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                env.sleep(POLL_INTERVAL.min(deadline - now));
            }
            None => env.sleep(POLL_INTERVAL),
        }
    }

    // revents пишутся всегда (в том числе по
    // таймауту — там нули).
    for i in 0..nfds {
        let p = fds + i;
        let pfd: PollFd = env.mem.read(p);
        // Собираем структуру заново: запись в
        // поля упакованного struct через
        // place-выражение здесь не используется
        // нарочно.
        env.mem.write(
            p,
            PollFd {
                fd: pfd.fd,
                events: pfd.events,
                revents: revents[i as usize],
            },
        );
    }
    count
}

/// `struct pollfd { int fd; short events; short revents; }` (32-битный
/// гость).
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct PollFd {
    fd: FileDescriptor,
    events: i16,
    revents: i16,
}

unsafe impl SafeRead for PollFd {}

/// Оценить readiness одного fd для poll(2):
/// возвращает `revents`. Нормальные биты
/// (POLLIN/POLLOUT/…) отдаются только если
/// запросили их в `events`; POLLERR/POLLHUP/POLLNVAL —
/// всегда (POSIX).
fn evaluate_poll_fd(env: &mut Environment, fd: FileDescriptor, events: i16) -> i16 {
    if fd < 0 {
        // Отрицательные fd игнорируются: revents=0, в
        // счёт не идут.
        return 0;
    }
    if fd < NORMAL_FILENO_BASE {
        // stdin/stdout/stderr: запись всегда возможна;
        // данных для чтения в HLE нет, поэтому POLLIN
        // не сообщается.
        let mut revents = 0;
        if fd == STDOUT_FILENO || fd == STDERR_FILENO {
            revents |= POLLOUT | POLLWRNORM;
        }
        return revents & events;
    }
    if is_socket(env, fd) {
        return crate::libc::sys::socket::socket_poll_events(env, fd, events);
    }
    let Some(file_obj) = env.libc_state.posix_io.guest_file(fd) else {
        // Файл не открыт → POLLNVAL (не ошибка, а
        // готовый fd, как в POSIX).
        return POLLNVAL;
    };
    // file_obj: &GuestFile (из guest_file).
    match file_obj {
        GuestFile::PipeRead(pipe) => {
            let pipe = pipe.borrow();
            let mut revents = 0;
            if pipe.poll_has_data() {
                revents |= POLLIN | POLLRDNORM;
            }
            if !pipe.poll_has_writers() {
                // Писателей нет → EOF на читающем
                // конце (HUP не зависит от events, но POLLIN
                // сообщаем только если запросили).
                return (revents & events) | POLLHUP;
            }
            revents & events
        }
        GuestFile::PipeWrite(pipe) => {
            let pipe = pipe.borrow();
            if !pipe.poll_has_readers() {
                // Читателей нет → запись
                // невозможна → POLLERR.
                return POLLERR;
            }
            (POLLOUT | POLLWRNORM) & events
        }
        // Обычные файлы/каталоги/случайные
        // числа: POSIX гарантирует — никогда не
        // блокируются, всегда готовы и на
        // чтение, и на запись.
        _ => (POLLIN | POLLOUT | POLLRDNORM | POLLWRNORM) & events,
    }
}

pub(crate) const POLLERR: i16 = 0x0008;

pub(crate) const POLLHUP: i16 = 0x0010;

/// Значения `events`/`revents` для poll(2) (sys/poll.h, Darwin).
pub(crate) const POLLIN: i16 = 0x0001;
pub(crate) const POLLPRI: i16 = 0x0002;

pub(crate) const POLLNVAL: i16 = 0x0020;

pub(crate) const POLLOUT: i16 = 0x0004;

pub(crate) const POLLRDNORM: i16 = 0x0040;

pub(crate) const POLLWRNORM: i16 = 0x0100;

pub fn find_or_create_pipe_read_fd(
    env: &mut Environment,
    buffer: std::rc::Rc<std::cell::RefCell<crate::fs::PipeBuffer>>,
) -> FileDescriptor {
    find_or_create_fd(
        env,
        PosixFileHostObject {
            file: GuestFile::PipeRead(crate::fs::PipeEnd::new(buffer, true)),
            needs_flush: false,
            reached_eof: false,
            flags: 0,
            save_dump: false,
        },
    )
}

pub fn find_or_create_pipe_write_fd(
    env: &mut Environment,
    buffer: std::rc::Rc<std::cell::RefCell<crate::fs::PipeBuffer>>,
) -> FileDescriptor {
    find_or_create_fd(
        env,
        PosixFileHostObject {
            file: GuestFile::PipeWrite(crate::fs::PipeEnd::new(buffer, false)),
            needs_flush: false,
            reached_eof: false,
            flags: 0,
            save_dump: false,
        },
    )
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(dup(_)),
    export_c_func!(dup2(_, _)),
    export_c_func!(setxattr(_, _, _, _, _, _)),
    export_c_func!(open(_, _, _)),
    export_c_func!(read(_, _, _)),
    export_c_func!(pread(_, _, _, _)),
    export_c_func!(write(_, _, _)),
    export_c_func!(pwrite(_, _, _, _)),
    export_c_func!(lseek(_, _, _)),
    export_c_func!(close(_)),
    export_c_func!(rename(_, _)),
    export_c_func!(getcwd(_, _)),
    export_c_func!(chdir(_)),
    export_c_func!(fcntl(_, _, _)),
    export_c_func!(flock(_, _)),
    export_c_func!(fsync(_)),
    export_c_func!(ftruncate(_, _)),
    export_c_func!(truncate(_, _)),
    export_c_func!(creat(_, _)),
    export_c_func!(open_dprotected_np(_, _, _, _, _)),
    export_c_func!(poll(_, _, _)),
];

/// Helper function, not part of API
fn find_or_create_fd(env: &mut Environment, host_object: PosixFileHostObject) -> FileDescriptor {
    let idx = if let Some(free_idx) = env
        .libc_state
        .posix_io
        .files
        .iter()
        .position(|f| f.is_none())
    {
        env.libc_state.posix_io.files[free_idx] = Some(host_object);
        free_idx
    } else {
        let idx = env.libc_state.posix_io.files.len();
        env.libc_state.posix_io.files.push(Some(host_object));
        idx
    };
    file_idx_to_fd(idx)
}

/// Helper function for socket creation, not part of API
pub fn find_or_create_socket(env: &mut Environment) -> FileDescriptor {
    let host_object = PosixFileHostObject {
        file: GuestFile::Socket,
        needs_flush: false,
        reached_eof: false,
        flags: 0,
        save_dump: false,
    };
    find_or_create_fd(env, host_object)
}

/// Helper function for socket check, not part of API
pub fn is_socket(env: &mut Environment, fd: FileDescriptor) -> bool {
    let guest_file = &env
        .libc_state
        .posix_io
        .files
        .get(fd_to_file_idx(fd))
        .unwrap()
        .as_ref()
        .unwrap()
        .file;
    matches!(guest_file, GuestFile::Socket)
}

/// Helper function to validate lock, not part of API. Assumes fd is a valid
/// file descriptor
fn validate_lock(env: &mut Environment, fd: FileDescriptor, lock: &flock) -> Result<(), i32> {
    let lock_type = lock.lock_type;
    if !matches!(lock_type, F_RDLCK | F_UNLCK | F_WRLCK) {
        return Err(EINVAL);
    }

    let whence = lock.whence as i32;
    let lock_start = match whence {
        SEEK_SET => lock.start,
        SEEK_CUR => {
            let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();
            let file_position = file.file.stream_position().unwrap();
            file_position as i64 + lock.start
        }
        SEEK_END => {
            let file = env.libc_state.posix_io.file_for_fd(fd).unwrap();
            let size: i64 = file.file.stream_len().unwrap().try_into().unwrap();
            size + lock.start
        }
        _ => {
            return Err(EINVAL);
        }
    };

    if lock_start < 0 {
        return Err(EINVAL);
    }

    Ok(())
}
