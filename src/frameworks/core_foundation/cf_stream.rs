/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Core Foundation memory streams.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFIndex, CFTypeRef};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_string;
use crate::fs::{GuestFile, GuestOpenOptions, GuestPath};
use crate::mem::{ConstPtr, MutPtr};
use crate::objc::{id, msg, nil, objc_classes, ClassExports, HostObject};
use crate::Environment;
use std::io::Write;

type CFReadStreamRef = CFTypeRef;
type CFWriteStreamRef = CFTypeRef;
const NOT_OPEN: i32 = 0;
const OPEN: i32 = 2;
const AT_END: i32 = 5;
const CLOSED: i32 = 6;
const ERROR: i32 = 7;

struct MemoryReadStream {
    bytes: ConstPtr<u8>,
    length: u32,
    offset: u32,
    owns_bytes: bool,
    status: i32,
    file_path: Option<String>,
}
impl HostObject for MemoryReadStream {}

enum WriteBuffer {
    Fixed {
        bytes: MutPtr<u8>,
        capacity: u32,
        offset: u32,
    },
    Allocated(Vec<u8>),
    File {
        path: String,
        file: Option<GuestFile>,
        append: bool,
    },
}
struct MemoryWriteStream {
    buffer: WriteBuffer,
    status: i32,
}
impl HostObject for MemoryWriteStream {}

const DATA_WRITTEN: &str = "kCFStreamPropertyDataWritten";
const APPEND_TO_FILE: &str = "kCFStreamPropertyAppendToFile";
pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFStreamPropertyDataWritten",
        HostConstant::NSString(DATA_WRITTEN),
    ),
    (
        "_kCFStreamPropertyAppendToFile",
        HostConstant::NSString(APPEND_TO_FILE),
    ),
];

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation _Podroid_CFMemoryReadStream: NSObject
- (())dealloc {
    let host = env.objc.borrow::<MemoryReadStream>(this);
    let (bytes, owns) = (host.bytes, host.owns_bytes);
    if owns && !bytes.is_null() { env.mem.free(bytes.cast_mut().cast()); }
    env.objc.dealloc_object(this, &mut env.mem)
}
@end
@implementation _Podroid_CFMemoryWriteStream: NSObject
@end
};

fn allocate_write_stream(env: &mut Environment, buffer: WriteBuffer) -> CFWriteStreamRef {
    let class = env
        .objc
        .get_known_class("_Podroid_CFMemoryWriteStream", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(MemoryWriteStream {
            buffer,
            status: NOT_OPEN,
        }),
        &mut env.mem,
    )
}

fn CFWriteStreamCreateWithBuffer(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: MutPtr<u8>,
    capacity: CFIndex,
) -> CFWriteStreamRef {
    if capacity < 0
        || (capacity > 0 && bytes.is_null())
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return nil;
    }
    allocate_write_stream(
        env,
        WriteBuffer::Fixed {
            bytes,
            capacity: capacity as u32,
            offset: 0,
        },
    )
}

fn CFWriteStreamCreateWithAllocatedBuffers(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    buffer_allocator: CFAllocatorRef,
) -> CFWriteStreamRef {
    if !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
        || !(buffer_allocator == kCFAllocatorDefault
            || env.mem.read(buffer_allocator).is_system_default())
    {
        return nil;
    }
    allocate_write_stream(env, WriteBuffer::Allocated(Vec::new()))
}

fn CFWriteStreamCreateWithFile(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url: id,
) -> CFWriteStreamRef {
    if url.is_null()
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return nil;
    }
    let is_file: bool = msg![env; url isFileURL];
    if !is_file {
        return nil;
    }
    let path: id = msg![env; url path];
    if path.is_null() {
        return nil;
    }
    let path = ns_string::to_rust_string(env, path).into_owned();
    allocate_write_stream(
        env,
        WriteBuffer::File {
            path,
            file: None,
            append: false,
        },
    )
}

fn CFWriteStreamSetProperty(
    env: &mut Environment,
    stream: CFWriteStreamRef,
    name: id,
    value: id,
) -> bool {
    if name.is_null() || value.is_null() || ns_string::to_rust_string(env, name) != APPEND_TO_FILE {
        return false;
    }
    let append: bool = msg![env; value boolValue];
    let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
    if host.status != NOT_OPEN {
        return false;
    }
    if let WriteBuffer::File {
        append: setting, ..
    } = &mut host.buffer
    {
        *setting = append;
        true
    } else {
        false
    }
}

fn CFWriteStreamOpen(env: &mut Environment, stream: CFWriteStreamRef) -> bool {
    let host = env.objc.borrow::<MemoryWriteStream>(stream);
    if host.status != NOT_OPEN {
        return false;
    }
    let file_request = if let WriteBuffer::File { path, append, .. } = &host.buffer {
        Some((path.clone(), *append))
    } else {
        None
    };
    if let Some((path, append)) = file_request {
        let mut options = GuestOpenOptions::new();
        options.write().create();
        if append {
            options.append();
        } else {
            options.truncate();
        }
        let Ok(opened) = env.fs.open_with_options(GuestPath::new(&path), options) else {
            env.objc.borrow_mut::<MemoryWriteStream>(stream).status = ERROR;
            return false;
        };
        let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
        if let WriteBuffer::File { file, .. } = &mut host.buffer {
            *file = Some(opened);
        }
    }
    let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
    host.status = OPEN;
    true
}

fn CFWriteStreamClose(env: &mut Environment, stream: CFWriteStreamRef) {
    let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
    if let WriteBuffer::File { file, .. } = &mut host.buffer {
        *file = None;
    }
    host.status = CLOSED;
}

fn CFWriteStreamGetStatus(env: &mut Environment, stream: CFWriteStreamRef) -> i32 {
    env.objc.borrow::<MemoryWriteStream>(stream).status
}

fn CFWriteStreamCanAcceptBytes(env: &mut Environment, stream: CFWriteStreamRef) -> bool {
    let host = env.objc.borrow::<MemoryWriteStream>(stream);
    host.status == OPEN
        && match &host.buffer {
            WriteBuffer::Fixed {
                capacity, offset, ..
            } => offset < capacity,
            WriteBuffer::Allocated(_) => true,
            WriteBuffer::File { file, .. } => file.is_some(),
        }
}

fn CFWriteStreamWrite(
    env: &mut Environment,
    stream: CFWriteStreamRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
) -> CFIndex {
    if length < 0 || (length > 0 && bytes.is_null()) {
        return -1;
    }
    let host = env.objc.borrow::<MemoryWriteStream>(stream);
    if host.status == AT_END {
        return 0;
    }
    if host.status != OPEN {
        return -1;
    }
    if length == 0 {
        return 0;
    }
    let data = env.mem.bytes_at(bytes, length as u32).to_vec();
    let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
    match &mut host.buffer {
        WriteBuffer::Fixed {
            bytes,
            capacity,
            offset,
        } => {
            let count = (length as u32).min(*capacity - *offset);
            if count > 0 {
                env.mem
                    .bytes_at_mut(*bytes + *offset, count)
                    .copy_from_slice(&data[..count as usize]);
            }
            *offset += count;
            if *offset == *capacity {
                host.status = AT_END;
            }
            count as CFIndex
        }
        WriteBuffer::Allocated(buffer) => {
            if buffer.try_reserve(data.len()).is_err() {
                host.status = ERROR;
                return -1;
            }
            buffer.extend_from_slice(&data);
            length
        }
        WriteBuffer::File { file, .. } => match file.as_mut().unwrap().write(&data) {
            Ok(count) => count as CFIndex,
            Err(_) => {
                host.status = ERROR;
                -1
            }
        },
    }
}

fn CFWriteStreamCopyProperty(
    env: &mut Environment,
    stream: CFWriteStreamRef,
    name: id,
) -> CFTypeRef {
    if name.is_null() || ns_string::to_rust_string(env, name) != DATA_WRITTEN {
        return nil;
    }
    let host = env.objc.borrow::<MemoryWriteStream>(stream);
    let WriteBuffer::Allocated(data) = &host.buffer else {
        return nil;
    };
    let data = data.clone();
    let Ok(length) = CFIndex::try_from(data.len()) else {
        return nil;
    };
    let bytes: MutPtr<u8> = env.mem.alloc((length as u32).max(1)).cast();
    env.mem
        .bytes_at_mut(bytes, length as u32)
        .copy_from_slice(&data);
    let result = super::cf_data::CFDataCreate(env, kCFAllocatorDefault, bytes.cast_const(), length);
    env.mem.free(bytes.cast());
    result
}

fn CFReadStreamCreateWithBytesNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
    bytes_deallocator: CFAllocatorRef,
) -> CFReadStreamRef {
    if length < 0
        || (length > 0 && bytes.is_null())
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return nil;
    }
    let owns_bytes = if bytes_deallocator == kCFAllocatorDefault {
        true
    } else {
        let allocator = env.mem.read(bytes_deallocator);
        if allocator.is_null() {
            false
        } else if allocator.is_system_default() {
            true
        } else {
            return nil;
        }
    };
    let class = env
        .objc
        .get_known_class("_Podroid_CFMemoryReadStream", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(MemoryReadStream {
            bytes,
            length: length as u32,
            offset: 0,
            owns_bytes,
            status: NOT_OPEN,
            file_path: None,
        }),
        &mut env.mem,
    )
}

fn CFReadStreamCreateWithFile(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url: id,
) -> CFReadStreamRef {
    if url.is_null()
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return nil;
    }
    let is_file: bool = msg![env; url isFileURL];
    if !is_file {
        return nil;
    }
    let path: id = msg![env; url path];
    if path.is_null() {
        return nil;
    }
    let path = ns_string::to_rust_string(env, path).into_owned();
    let class = env
        .objc
        .get_known_class("_Podroid_CFMemoryReadStream", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(MemoryReadStream {
            bytes: ConstPtr::null(),
            length: 0,
            offset: 0,
            owns_bytes: false,
            status: NOT_OPEN,
            file_path: Some(path),
        }),
        &mut env.mem,
    )
}

fn CFReadStreamOpen(env: &mut Environment, stream: CFReadStreamRef) -> bool {
    let host = env.objc.borrow::<MemoryReadStream>(stream);
    if host.status != NOT_OPEN {
        return false;
    }
    let path = host.file_path.clone();
    if let Some(path) = path {
        let Ok(data) = env.fs.read(GuestPath::new(&path)) else {
            env.objc.borrow_mut::<MemoryReadStream>(stream).status = ERROR;
            return false;
        };
        let Ok(length) = u32::try_from(data.len()) else {
            env.objc.borrow_mut::<MemoryReadStream>(stream).status = ERROR;
            return false;
        };
        if length > 0 {
            let bytes: MutPtr<u8> = env.mem.alloc(length).cast();
            env.mem.bytes_at_mut(bytes, length).copy_from_slice(&data);
            let host = env.objc.borrow_mut::<MemoryReadStream>(stream);
            host.bytes = bytes.cast_const();
            host.length = length;
            host.owns_bytes = true;
        }
    }
    let host = env.objc.borrow_mut::<MemoryReadStream>(stream);
    host.status = OPEN;
    true
}

fn CFReadStreamClose(env: &mut Environment, stream: CFReadStreamRef) {
    env.objc.borrow_mut::<MemoryReadStream>(stream).status = CLOSED;
}

fn CFReadStreamGetStatus(env: &mut Environment, stream: CFReadStreamRef) -> i32 {
    env.objc.borrow::<MemoryReadStream>(stream).status
}

fn CFReadStreamHasBytesAvailable(env: &mut Environment, stream: CFReadStreamRef) -> bool {
    let host = env.objc.borrow::<MemoryReadStream>(stream);
    host.status == OPEN && host.offset < host.length
}

fn consume(
    env: &mut Environment,
    stream: CFReadStreamRef,
    maximum: CFIndex,
) -> (ConstPtr<u8>, CFIndex) {
    if maximum <= 0 {
        return (ConstPtr::null(), 0);
    }
    let host = env.objc.borrow_mut::<MemoryReadStream>(stream);
    if host.status != OPEN {
        return (ConstPtr::null(), 0);
    }
    let count = (maximum as u32).min(host.length - host.offset);
    if count == 0 {
        host.status = AT_END;
        return (ConstPtr::null(), 0);
    }
    let result = host.bytes + host.offset;
    host.offset += count;
    if host.offset == host.length {
        host.status = AT_END;
    }
    (result, count as CFIndex)
}

fn CFReadStreamGetBuffer(
    env: &mut Environment,
    stream: CFReadStreamRef,
    maximum: CFIndex,
    num_read: MutPtr<CFIndex>,
) -> ConstPtr<u8> {
    let status = CFReadStreamGetStatus(env, stream);
    if status != OPEN && status != AT_END {
        if !num_read.is_null() {
            env.mem.write(num_read, -1);
        }
        return ConstPtr::null();
    }
    let (result, count) = consume(
        env,
        stream,
        if maximum <= 0 { CFIndex::MAX } else { maximum },
    );
    if !num_read.is_null() {
        env.mem.write(num_read, count);
    }
    result
}

fn CFReadStreamRead(
    env: &mut Environment,
    stream: CFReadStreamRef,
    buffer: MutPtr<u8>,
    length: CFIndex,
) -> CFIndex {
    if length < 0 || (length > 0 && buffer.is_null()) {
        return -1;
    }
    let status = CFReadStreamGetStatus(env, stream);
    if status == AT_END {
        return 0;
    }
    if status != OPEN {
        return -1;
    }
    if length == 0 {
        return 0;
    }
    let (source, count) = consume(env, stream, length);
    if count > 0 {
        let data = env.mem.bytes_at(source, count as u32).to_vec();
        env.mem
            .bytes_at_mut(buffer, count as u32)
            .copy_from_slice(&data);
    }
    count
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFWriteStreamCreateWithBuffer(_, _, _)),
    export_c_func!(CFWriteStreamCreateWithAllocatedBuffers(_, _)),
    export_c_func!(CFWriteStreamCreateWithFile(_, _)),
    export_c_func!(CFWriteStreamOpen(_)),
    export_c_func!(CFWriteStreamClose(_)),
    export_c_func!(CFWriteStreamGetStatus(_)),
    export_c_func!(CFWriteStreamCanAcceptBytes(_)),
    export_c_func!(CFWriteStreamWrite(_, _, _)),
    export_c_func!(CFWriteStreamCopyProperty(_, _)),
    export_c_func!(CFWriteStreamSetProperty(_, _, _)),
    export_c_func!(CFReadStreamCreateWithBytesNoCopy(_, _, _, _)),
    export_c_func!(CFReadStreamCreateWithFile(_, _)),
    export_c_func!(CFReadStreamOpen(_)),
    export_c_func!(CFReadStreamClose(_)),
    export_c_func!(CFReadStreamGetStatus(_)),
    export_c_func!(CFReadStreamHasBytesAvailable(_)),
    export_c_func!(CFReadStreamGetBuffer(_, _, _)),
    export_c_func!(CFReadStreamRead(_, _, _)),
];
