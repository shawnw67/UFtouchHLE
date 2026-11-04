/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSData` and `NSMutableData`.

use super::ns_string::{self, to_rust_string};
use super::{NSRange, NSUInteger};
use crate::frameworks::foundation::ns_keyed_unarchiver::decode_current_data;
use crate::fs::GuestPath;
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};
use crate::{msg_class, Environment};

pub(super) struct NSDataHostObject {
    pub(super) bytes: MutVoidPtr,
    pub(super) length: NSUInteger,
    free_when_done: bool,
}
impl HostObject for NSDataHostObject {}

const NSDataBase64Encoding64CharacterLineLength: NSUInteger = 1 << 0;
const NSDataBase64Encoding76CharacterLineLength: NSUInteger = 1 << 1;
const NSDataBase64EncodingEndLineWithCarriageReturn: NSUInteger = 1 << 4;
const NSDataBase64EncodingEndLineWithLineFeed: NSUInteger = 1 << 5;

/// Standard Base64 (RFC 4648, `+/` alphabet, `=` padding), as produced by
/// Apple's `-base64EncodedDataWithOptions:` and
/// `-base64EncodedStringWithOptions:`. With a line length option the output
/// is split into lines joined by the requested line ending (CR LF if none is
/// given), with no ending after the last line.
/// <https://developer.apple.com/documentation/foundation/nsdatabase64encodingoptions>
fn base64_encode_with_options(input: &[u8], options: NSUInteger) -> Vec<u8> {
    let encoded = base64_encode(input);
    let line_length = if options & NSDataBase64Encoding64CharacterLineLength != 0 {
        64
    } else if options & NSDataBase64Encoding76CharacterLineLength != 0 {
        76
    } else {
        return encoded;
    };
    let mut line_ending = Vec::new();
    if options & NSDataBase64EncodingEndLineWithCarriageReturn != 0 {
        line_ending.push(b'\r');
    }
    if options & NSDataBase64EncodingEndLineWithLineFeed != 0 {
        line_ending.push(b'\n');
    }
    if line_ending.is_empty() {
        line_ending.extend_from_slice(b"\r\n");
    }
    encoded
        .chunks(line_length)
        .collect::<Vec<_>>()
        .join(&line_ending[..])
}

fn base64_encode(input: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(ALPHABET[(b0 >> 2) as usize]);
        out.push(ALPHABET[(((b0 & 0x03) << 4) | (b1 >> 4)) as usize]);
        if chunk.len() > 1 {
            out.push(ALPHABET[(((b1 & 0x0f) << 2) | (b2 >> 6)) as usize]);
        } else {
            out.push(b'=');
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(b2 & 0x3f) as usize]);
        } else {
            out.push(b'=');
        }
    }
    out
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// NSData doesn't seem to be an abstract class?
@implementation NSData: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSDataHostObject {
        bytes: Ptr::null(),
        length: 0,
        free_when_done: true,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)data {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new init];
    autorelease(env, new)
}

+ (id)dataWithBytesNoCopy:(MutVoidPtr)bytes
                   length:(NSUInteger)length {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithBytesNoCopy:bytes length:length];
    autorelease(env, new)
}

+ (id)dataWithBytesNoCopy:(MutVoidPtr)bytes
                   length:(NSUInteger)length
             freeWhenDone:(bool)free_when_done {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithBytesNoCopy:bytes length:length freeWhenDone:free_when_done];
    autorelease(env, new)
}

+ (id)dataWithBytes:(ConstVoidPtr)bytes
             length:(NSUInteger)length {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithBytes:bytes length:length];
    autorelease(env, new)
}

+ (id)dataWithContentsOfFile:(id)path {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithContentsOfFile:path];
    autorelease(env, new)
}

+ (id)dataWithContentsOfMappedFile:(id)path {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithContentsOfMappedFile:path];
    autorelease(env, new)
}

+ (id)dataWithContentsOfURL:(id)url {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithContentsOfURL:url];
    autorelease(env, new)
}

+ (id)dataWithData:(id)data {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithData:data];
    autorelease(env, new)
}

// Calling the standard `init` is also allowed, in which case we just get data
// of size 0.

- (id)initWithBytesNoCopy:(MutVoidPtr)bytes
                   length:(NSUInteger)length {
    msg![env; this initWithBytesNoCopy:bytes length:length freeWhenDone:true]
}

- (id)initWithBytesNoCopy:(MutVoidPtr)bytes
                   length:(NSUInteger)length
             freeWhenDone:(bool)free_when_done {
    let host_object = env.objc.borrow_mut::<NSDataHostObject>(this);
    assert!(host_object.bytes.is_null() && host_object.length == 0);
    host_object.bytes = bytes;
    host_object.length = length;
    host_object.free_when_done = free_when_done;
    this
}

- (id)initWithBytes:(ConstVoidPtr)bytes
              length:(NSUInteger)length {
    let host_object = env.objc.borrow_mut::<NSDataHostObject>(this);
    assert!(host_object.bytes.is_null() && host_object.length == 0);
    let alloc = env.mem.alloc(length);
    env.mem.memmove(alloc, bytes, length);
    host_object.bytes = alloc;
    host_object.length = length;
    this
}

- (id)initWithData:(id)data {
    let bytes: ConstVoidPtr = msg![env; data bytes];
    let length: NSUInteger = msg![env; data length];
    msg![env; this initWithBytes:bytes length:length]
}

- (id)initWithContentsOfURL:(id)url { // NSURL *
    if msg![env; url isFileURL] {
        let ns_path: id = msg![env; url path];
        let path = to_rust_string(env, ns_path);
        assert!(path.starts_with("/")); // TODO
        msg![env; this initWithContentsOfFile:ns_path]
    } else {
        let absolute_str: id = msg![env; url absoluteString];
        let path = to_rust_string(env, absolute_str);
        assert!(path.starts_with("http"));
        log!("TODO: ignoring [(NSData*){:?} initWithContentsOfURL:{:?}]", this, path);
        release(env, this);
        nil
    }
}

- (id)initWithContentsOfFile:(id)path {
    if path == nil {
        return nil;
    }
    let path = to_rust_string(env, path);
    log_dbg!("[(NSData*){:?} initWithContentsOfFile:{:?}]", this, path);
    let Ok(bytes) = env.fs.read(GuestPath::new(&path)) else {
        release(env, this);
        return nil;
    };
    let size = bytes.len().try_into().unwrap();
    let alloc = env.mem.alloc(size);
    let slice = env.mem.bytes_at_mut(alloc.cast(), size);
    slice.copy_from_slice(&bytes);

    let host_object = env.objc.borrow_mut::<NSDataHostObject>(this);
    host_object.bytes = alloc;
    host_object.length = size;
    this
}

- (id)initWithContentsOfMappedFile:(id)path {
    log_dbg!("[NSData initWithContentsOfMappedFile:] not using memory mapping");
    msg![env; this initWithContentsOfFile:path]
}

// FIXME: writes should be atomic
- (bool)writeToFile:(id)path // NSString*
         atomically:(bool)_use_aux_file {
    let file = to_rust_string(env, path);
    log_dbg!("[(NSData*){:?} writeToFile:{:?} atomically:_]", this, file);
    let host_object = env.objc.borrow::<NSDataHostObject>(this);
    // Mem::bytes_at() panics when the pointer is NULL, but NSData's pointer can
    // be NULL if the length is 0.
    let slice = if host_object.length == 0 {
        &[]
    } else {
        env.mem.bytes_at(host_object.bytes.cast(), host_object.length)
    };
    env.fs.write(GuestPath::new(&file), slice).is_ok()
}

- (())dealloc {
    let &NSDataHostObject { bytes, free_when_done, .. } = env.objc.borrow(this);
    if !bytes.is_null() && free_when_done {
        env.mem.free(bytes);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

// NSCopying implementation
- (id)copyWithZone:(NSZonePtr)_zone {
    retain(env, this)
}

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    release(env, this);
    // Note: Assuming NSKeyedUnarchiver as coder here
    decode_current_data(env, coder, /* is_mutable: */ true)
}

- (id)mutableCopyWithZone:(NSZonePtr)_zone {
    let bytes: ConstVoidPtr = msg![env; this bytes];
    let length: NSUInteger = msg![env; this length];
    let new = msg_class![env; NSMutableData alloc];
    msg![env; new initWithBytes:bytes length:length]
}

- (ConstVoidPtr)bytes {
    env.objc.borrow::<NSDataHostObject>(this).bytes.cast_const()
}
- (NSUInteger)length {
    env.objc.borrow::<NSDataHostObject>(this).length
}

- (id)base64EncodedDataWithOptions:(NSUInteger)options {
    let source = {
        let host = env.objc.borrow::<NSDataHostObject>(this);
        if host.length == 0 || host.bytes.is_null() {
            Vec::new()
        } else {
            env.mem
                .bytes_at(host.bytes.cast_const().cast(), host.length)
                .to_vec()
        }
    };
    let encoded = base64_encode_with_options(&source, options);
    let len = encoded.len() as NSUInteger;
    if len == 0 {
        return msg_class![env; NSData data];
    }
    let buffer: MutVoidPtr = env.mem.alloc(len);
    env.mem
        .bytes_at_mut(buffer.cast(), len)
        .copy_from_slice(&encoded);
    msg_class![env; NSData dataWithBytesNoCopy:buffer length:len freeWhenDone:true]
}

// https://developer.apple.com/documentation/foundation/nsdata/1413546-base64encodedstringwithoptions
- (id)base64EncodedStringWithOptions:(NSUInteger)options { // NSString*
    let data: id = msg![env; this base64EncodedDataWithOptions:options];
    let encoded = {
        let bytes: ConstVoidPtr = msg![env; data bytes];
        let length: NSUInteger = msg![env; data length];
        if length == 0 {
            String::new()
        } else {
            // Base64 output is always ASCII.
            String::from_utf8(env.mem.bytes_at(bytes.cast(), length).to_vec()).unwrap()
        }
    };
    let string = ns_string::from_rust_string(env, encoded);
    autorelease(env, string)
}

- (bool)isEqualToData:(id)other {
    if other == nil { return false; }
    let left_length: NSUInteger = msg![env; this length];
    let right_length: NSUInteger = msg![env; other length];
    if left_length != right_length { return false; }
    if left_length == 0 { return true; }
    let left: ConstVoidPtr = msg![env; this bytes];
    let right: ConstVoidPtr = msg![env; other bytes];
    env.mem.bytes_at(left.cast(), left_length) == env.mem.bytes_at(right.cast(), right_length)
}

- (bool)isEqual:(id)other {
    if this == other { return true; }
    let class = env.objc.get_known_class("NSData", &mut env.mem);
    let is_data: bool = msg![env; other isKindOfClass:class];
    is_data && msg![env; this isEqualToData:other]
}

- (NSUInteger)hash {
    let length: NSUInteger = msg![env; this length];
    let bytes: ConstVoidPtr = msg![env; this bytes];
    if length == 0 { return 0x811c9dc5; }
    env.mem.bytes_at(bytes.cast(), length).iter().fold(0x811c9dc5u32, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x01000193)
    })
}

- (())getBytes:(MutPtr<u8>)buffer length:(NSUInteger)length {
    let length = length.min(env.objc.borrow::<NSDataHostObject>(this).length);
    let range = NSRange { location: 0, length };
    msg![env; this getBytes:buffer range:range]
}

- (())getBytes:(MutPtr<u8>)buffer range:(NSRange)range {
    let &NSDataHostObject { bytes, length, .. } = env.objc.borrow(this);
    // TODO: throw NSRangeException if out-of-range instead of panic?
    assert!(range.location <= length && range.length <= length - range.location);
    if range.length == 0 {
        return;
    }
    env.mem.memmove(
        buffer.cast(),
        bytes.cast_const() + range.location,
        range.length,
    );
}

- (())getBytes:(MutPtr<u8>)buffer {
    let &NSDataHostObject { bytes, length, .. } = env.objc.borrow(this);
    env.mem.memmove(
        buffer.cast(),
        bytes.cast_const(),
        length,
    );
}

@end

@implementation NSMutableData: NSData

+ (id)data {
    msg![env; this dataWithCapacity:0u32]
}

+ (id)dataWithCapacity:(NSUInteger)capacity {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCapacity:capacity];
    autorelease(env, new)
}

+ (id)dataWithLength:(NSUInteger)length {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithLength:length];
    autorelease(env, new)
}

- (id)initWithCapacity:(NSUInteger)_capacity {
    msg![env; this init]
}

- (id)initWithLength:(NSUInteger)length {
    let host_object = env.objc.borrow_mut::<NSDataHostObject>(this);
    assert!(host_object.bytes.is_null() && host_object.length == 0);
    let alloc = env.mem.calloc(length);
    host_object.bytes = alloc;
    host_object.length = length;
    this
}

- (id)copyWithZone:(NSZonePtr)_zone {
    let bytes: ConstVoidPtr = msg![env; this bytes];
    let length: NSUInteger = msg![env; this length];
    let new = msg_class![env; NSData alloc];
    msg![env; new initWithBytes:bytes length:length]
}

- (())increaseLengthBy:(NSUInteger)add_len {
    let &NSDataHostObject { bytes, length, .. } = env.objc.borrow(this);
    let new_len = length.checked_add(add_len).expect("NSData length overflow");
    let new_bytes = env.mem.realloc(bytes, new_len);
    if add_len != 0 {
        env.mem.bytes_at_mut(new_bytes.cast(), new_len)[length as usize..].fill(0);
    }
    let host = env.objc.borrow_mut::<NSDataHostObject>(this);
    host.length = new_len;
    host.bytes = new_bytes;
    log_dbg!("increaseLengthBy bytes {:?}, new_bytes {:?}; length {}, new_len {}", bytes, new_bytes, length, new_len);
}

- (())appendData:(id)other_data { // NSData *
    let other_bytes: ConstVoidPtr = msg![env; other_data bytes];
    let other_bytes: ConstPtr<u8> = other_bytes.cast();
    let other_length: NSUInteger = msg![env; other_data length];
    log_dbg!("appendData other_data {:?}, other_bytes {:?}, other_length {}", other_data, other_bytes, other_length);
    msg![env; this appendBytes:other_bytes length:other_length]
}

- (())appendBytes:(ConstPtr<u8>)append_bytes length:(NSUInteger)append_length {
    if append_length == 0 {
        return;
    }
    let old_len = env.objc.borrow::<NSDataHostObject>(this).length;
    let old_bytes = env.objc.borrow::<NSDataHostObject>(this).bytes;
    let source_start = u64::from(append_bytes.to_bits());
    let source_end = source_start + u64::from(append_length);
    let old_start = u64::from(old_bytes.to_bits());
    let old_end = old_start + u64::from(old_len);
    let overlap = source_start < old_end && old_start < source_end;
    let saved = overlap.then(|| env.mem.bytes_at(append_bytes, append_length).to_vec());
    () = msg![env; this increaseLengthBy:append_length];
    let &NSDataHostObject { bytes, length, .. } = env.objc.borrow(this);
    log_dbg!("appendBytes old_len {}, append_length {}, length {}", old_len, append_length, length);
    log_dbg!("appendBytes old_bytes {:?}, append_bytes {:?}, bytes {:?}", old_bytes, append_bytes, bytes);
    if let Some(saved) = saved {
        env.mem.bytes_at_mut((bytes + old_len).cast(), append_length).copy_from_slice(&saved);
    } else {
        env.mem.memmove(bytes + old_len, append_bytes.cast(), append_length);
    }
}

- (MutVoidPtr)mutableBytes {
    let host_obj = env.objc.borrow_mut::<NSDataHostObject>(this);
    host_obj.bytes
}

- (())setLength:(NSUInteger)new_length {
    let &NSDataHostObject {bytes, length, .. } = env.objc.borrow(this);
    let new_bytes = env.mem.realloc(bytes, new_length);
    if new_length > length {
        env.mem.bytes_at_mut(new_bytes.cast(), new_length)[length as usize..].fill(0);
    }
    let host = env.objc.borrow_mut::<NSDataHostObject>(this);
    host.length = new_length;
    host.bytes = new_bytes;
    log_dbg!("setLength bytes {:?}, new_bytes {:?}; length {}, new_len {}", bytes, new_bytes, length, new_length);
}

@end

};

pub fn to_rust_slice(env: &mut Environment, data: id) -> &[u8] {
    let borrowed_data = env.objc.borrow::<NSDataHostObject>(data);
    assert!(!borrowed_data.bytes.is_null() && borrowed_data.length != 0);
    env.mem
        .bytes_at(borrowed_data.bytes.cast(), borrowed_data.length)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_line_length_options() {
        let input = [0u8; 60]; // 80 Base64 characters
        let plain = base64_encode_with_options(&input, 0);
        assert_eq!(plain.len(), 80);
        assert!(!plain.contains(&b'\r') && !plain.contains(&b'\n'));

        let crlf = base64_encode_with_options(&input, NSDataBase64Encoding64CharacterLineLength);
        assert_eq!(crlf, [&plain[..64], b"\r\n", &plain[64..]].concat());

        let lf = base64_encode_with_options(
            &input,
            NSDataBase64Encoding76CharacterLineLength | NSDataBase64EncodingEndLineWithLineFeed,
        );
        assert_eq!(lf, [&plain[..76], b"\n", &plain[76..]].concat());

        // No line ending after a final full line.
        let short =
            base64_encode_with_options(&[0u8; 48], NSDataBase64Encoding64CharacterLineLength);
        assert_eq!(short.len(), 64);
    }
}
