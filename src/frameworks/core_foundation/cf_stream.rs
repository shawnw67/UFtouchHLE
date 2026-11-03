/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Core Foundation memory streams.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFIndex, CFTypeRef};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::ns_string;
use crate::fs::GuestPath;
use crate::mem::{ConstPtr, MutPtr};
use crate::objc::{id, msg, nil, objc_classes, ClassExports, HostObject};
use crate::Environment;

type CFReadStreamRef = CFTypeRef;
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
};

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
    export_c_func!(CFReadStreamCreateWithBytesNoCopy(_, _, _, _)),
    export_c_func!(CFReadStreamCreateWithFile(_, _)),
    export_c_func!(CFReadStreamOpen(_)),
    export_c_func!(CFReadStreamClose(_)),
    export_c_func!(CFReadStreamGetStatus(_)),
    export_c_func!(CFReadStreamHasBytesAvailable(_)),
    export_c_func!(CFReadStreamGetBuffer(_, _, _)),
    export_c_func!(CFReadStreamRead(_, _, _)),
];
