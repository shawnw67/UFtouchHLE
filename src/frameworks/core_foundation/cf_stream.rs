/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Core Foundation memory streams.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::{CFIndex, CFTypeRef};
use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::{ns_run_loop, ns_string};
use crate::fs::{GuestFile, GuestOpenOptions, GuestPath};
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::objc::{id, msg, nil, objc_classes, release, retain, ClassExports, HostObject};
use crate::Environment;
use std::io::Write;

type CFReadStreamRef = CFTypeRef;
type CFWriteStreamRef = CFTypeRef;
const NOT_OPEN: i32 = 0;
const OPEN: i32 = 2;
const AT_END: i32 = 5;
const CLOSED: i32 = 6;
const ERROR: i32 = 7;
const EVENT_OPEN: u32 = 1;
const EVENT_READABLE: u32 = 2;
const EVENT_WRITABLE: u32 = 4;
const EVENT_ERROR: u32 = 8;
const EVENT_END: u32 = 16;

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct StreamClientContext {
    version: CFIndex,
    info: MutVoidPtr,
    retain: GuestFunction,
    release: GuestFunction,
    copy_description: GuestFunction,
}
unsafe impl SafeRead for StreamClientContext {}

#[derive(Clone, Copy)]
struct StreamClient {
    events: u32,
    callback: GuestFunction,
    info: MutVoidPtr,
    release: GuestFunction,
}

#[derive(Default)]
struct StreamEvents {
    client: Option<StreamClient>,
    pending: u32,
    run_loops: Vec<id>,
}

fn release_client(env: &mut Environment, client: Option<StreamClient>) {
    if let Some(client) = client {
        if !client.release.to_ptr().is_null() {
            let _: () = client.release.call_from_host(env, (client.info,));
        }
    }
}

struct MemoryReadStream {
    bytes: ConstPtr<u8>,
    length: u32,
    offset: u32,
    owns_bytes: bool,
    status: i32,
    file_path: Option<String>,
    events: StreamEvents,
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
    events: StreamEvents,
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
    let (bytes, owns, client) = (host.bytes, host.owns_bytes, host.events.client);
    if owns && !bytes.is_null() { env.mem.free(bytes.cast_mut().cast()); }
    release_client(env, client);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end
@implementation _Podroid_CFMemoryWriteStream: NSObject
- (())dealloc {
    let client = env.objc.borrow::<MemoryWriteStream>(this).events.client;
    release_client(env, client);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end
};

fn events_mut(env: &mut Environment, stream: id, is_write: bool) -> &mut StreamEvents {
    if is_write {
        &mut env.objc.borrow_mut::<MemoryWriteStream>(stream).events
    } else {
        &mut env.objc.borrow_mut::<MemoryReadStream>(stream).events
    }
}

fn set_client(
    env: &mut Environment,
    stream: id,
    is_write: bool,
    mask: u32,
    callback: GuestFunction,
    context: ConstPtr<StreamClientContext>,
) -> bool {
    if stream.is_null() {
        return false;
    }
    let new = if mask == 0 || callback.to_ptr().is_null() {
        None
    } else {
        let (info, release) = if context.is_null() {
            (MutVoidPtr::null(), GuestFunction::null_ptr())
        } else {
            let value = env.mem.read(context);
            if value.version != 0 {
                return false;
            }
            let retain_callback = value.retain;
            let info = if retain_callback.to_ptr().is_null() {
                value.info
            } else {
                retain_callback.call_from_host(env, (value.info,))
            };
            (info, value.release)
        };
        Some(StreamClient {
            events: mask,
            callback,
            info,
            release,
        })
    };
    let old = std::mem::replace(&mut events_mut(env, stream, is_write).client, new);
    release_client(env, old);
    true
}

fn CFReadStreamSetClient(
    env: &mut Environment,
    stream: CFReadStreamRef,
    mask: u32,
    callback: GuestFunction,
    context: ConstPtr<StreamClientContext>,
) -> bool {
    set_client(env, stream, false, mask, callback, context)
}

fn CFWriteStreamSetClient(
    env: &mut Environment,
    stream: CFWriteStreamRef,
    mask: u32,
    callback: GuestFunction,
    context: ConstPtr<StreamClientContext>,
) -> bool {
    set_client(env, stream, true, mask, callback, context)
}

fn schedule(env: &mut Environment, stream: id, run_loop: id, mode: id, is_write: bool) {
    if stream.is_null()
        || run_loop.is_null()
        || mode.is_null()
        || events_mut(env, stream, is_write)
            .run_loops
            .contains(&run_loop)
    {
        return;
    }
    retain(env, stream);
    events_mut(env, stream, is_write).run_loops.push(run_loop);
    ns_run_loop::add_cf_stream(env, run_loop, stream, is_write);
}

fn unschedule(env: &mut Environment, stream: id, run_loop: id, is_write: bool) {
    if stream.is_null() || run_loop.is_null() {
        return;
    }
    let events = events_mut(env, stream, is_write);
    if let Some(index) = events.run_loops.iter().position(|&item| item == run_loop) {
        events.run_loops.remove(index);
        ns_run_loop::remove_cf_stream(env, run_loop, stream, is_write);
        release(env, stream);
    }
}

fn CFReadStreamScheduleWithRunLoop(
    env: &mut Environment,
    stream: CFReadStreamRef,
    run_loop: id,
    mode: id,
) {
    schedule(env, stream, run_loop, mode, false);
}
fn CFReadStreamUnscheduleFromRunLoop(
    env: &mut Environment,
    stream: CFReadStreamRef,
    run_loop: id,
    _mode: id,
) {
    unschedule(env, stream, run_loop, false);
}
fn CFWriteStreamScheduleWithRunLoop(
    env: &mut Environment,
    stream: CFWriteStreamRef,
    run_loop: id,
    mode: id,
) {
    schedule(env, stream, run_loop, mode, true);
}
fn CFWriteStreamUnscheduleFromRunLoop(
    env: &mut Environment,
    stream: CFWriteStreamRef,
    run_loop: id,
    _mode: id,
) {
    unschedule(env, stream, run_loop, true);
}

fn unschedule_all(env: &mut Environment, stream: id, is_write: bool) {
    retain(env, stream);
    while let Some(run_loop) = events_mut(env, stream, is_write).run_loops.last().copied() {
        unschedule(env, stream, run_loop, is_write);
    }
    release(env, stream);
}

pub fn perform_events(env: &mut Environment, run_loop: id, stream: id, is_write: bool) {
    let events = events_mut(env, stream, is_write);
    if !events.run_loops.contains(&run_loop) {
        return;
    }
    let Some(client) = events.client else {
        return;
    };
    let ready = events.pending & client.events;
    if ready == 0 {
        return;
    }
    let event = 1 << ready.trailing_zeros();
    events.pending &= !event;
    let _: () = client
        .callback
        .call_from_host(env, (stream, event, client.info));
}

fn allocate_write_stream(env: &mut Environment, buffer: WriteBuffer) -> CFWriteStreamRef {
    let class = env
        .objc
        .get_known_class("_Podroid_CFMemoryWriteStream", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(MemoryWriteStream {
            buffer,
            status: NOT_OPEN,
            events: StreamEvents::default(),
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
            let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
            host.status = ERROR;
            host.events.pending |= EVENT_ERROR;
            return false;
        };
        let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
        if let WriteBuffer::File { file, .. } = &mut host.buffer {
            *file = Some(opened);
        }
    }
    let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
    host.status = OPEN;
    host.events.pending |= EVENT_OPEN | EVENT_WRITABLE;
    true
}

fn CFWriteStreamClose(env: &mut Environment, stream: CFWriteStreamRef) {
    let host = env.objc.borrow_mut::<MemoryWriteStream>(stream);
    if let WriteBuffer::File { file, .. } = &mut host.buffer {
        *file = None;
    }
    host.status = CLOSED;
    unschedule_all(env, stream, true);
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
                host.events.pending &= !EVENT_WRITABLE;
                host.events.pending |= EVENT_END;
            } else {
                host.events.pending |= EVENT_WRITABLE;
            }
            count as CFIndex
        }
        WriteBuffer::Allocated(buffer) => {
            if buffer.try_reserve(data.len()).is_err() {
                host.status = ERROR;
                host.events.pending |= EVENT_ERROR;
                return -1;
            }
            buffer.extend_from_slice(&data);
            host.events.pending |= EVENT_WRITABLE;
            length
        }
        WriteBuffer::File { file, .. } => match file.as_mut().unwrap().write(&data) {
            Ok(count) => {
                host.events.pending |= EVENT_WRITABLE;
                count as CFIndex
            }
            Err(_) => {
                host.status = ERROR;
                host.events.pending |= EVENT_ERROR;
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
            events: StreamEvents::default(),
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
            events: StreamEvents::default(),
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
            let host = env.objc.borrow_mut::<MemoryReadStream>(stream);
            host.status = ERROR;
            host.events.pending |= EVENT_ERROR;
            return false;
        };
        let Ok(length) = u32::try_from(data.len()) else {
            let host = env.objc.borrow_mut::<MemoryReadStream>(stream);
            host.status = ERROR;
            host.events.pending |= EVENT_ERROR;
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
    host.events.pending |= EVENT_OPEN;
    if host.length > 0 {
        host.events.pending |= EVENT_READABLE;
    } else {
        host.events.pending |= EVENT_END;
    }
    true
}

fn CFReadStreamClose(env: &mut Environment, stream: CFReadStreamRef) {
    env.objc.borrow_mut::<MemoryReadStream>(stream).status = CLOSED;
    unschedule_all(env, stream, false);
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
        host.events.pending &= !EVENT_READABLE;
        host.events.pending |= EVENT_END;
        return (ConstPtr::null(), 0);
    }
    let result = host.bytes + host.offset;
    host.offset += count;
    if host.offset == host.length {
        host.status = AT_END;
        host.events.pending &= !EVENT_READABLE;
        host.events.pending |= EVENT_END;
    } else {
        host.events.pending |= EVENT_READABLE;
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
    export_c_func!(CFReadStreamSetClient(_, _, _, _)),
    export_c_func!(CFWriteStreamSetClient(_, _, _, _)),
    export_c_func!(CFReadStreamScheduleWithRunLoop(_, _, _)),
    export_c_func!(CFReadStreamUnscheduleFromRunLoop(_, _, _)),
    export_c_func!(CFWriteStreamScheduleWithRunLoop(_, _, _)),
    export_c_func!(CFWriteStreamUnscheduleFromRunLoop(_, _, _)),
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
