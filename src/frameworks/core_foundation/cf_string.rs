/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFString` and `CFMutableString`.
//!
//! This is toll-free bridged to `NSString` and `NSMutableString` in
//! Apple's implementation. Here it is the same type.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_array::CFArrayRef;
use super::cf_dictionary::CFDictionaryRef;
use super::cf_locale::CFLocaleRef;
use super::CFRelease;
use super::CFTypeRef;
use super::{kCFNotFound, CFComparisonResult, CFIndex, CFOptionFlags, CFRange};
use crate::abi::{DotDotDot, VaList};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::NSNotFound;
use crate::frameworks::foundation::{ns_string, unichar, NSRange, NSUInteger};
use crate::libc::string::strlen;
use crate::mem::MutVoidPtr;
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr};
use crate::objc::nil;
use crate::objc::{id, msg, msg_class, release, retain};
use crate::Environment;

pub type CFStringRef = super::CFTypeRef;
pub type CFMutableStringRef = CFStringRef;

fn CFStringRetain(env: &mut Environment, string: CFStringRef) -> CFStringRef {
    if string.is_null() {
        string
    } else {
        retain(env, string)
    }
}

fn CFStringRelease(env: &mut Environment, string: CFStringRef) {
    if !string.is_null() {
        release(env, string);
    }
}

fn CFStringCreateArrayBySeparatingStrings(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFStringRef,
    separator: CFStringRef,
) -> CFArrayRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let array: id = msg![env; string componentsSeparatedByString:separator];
    retain(env, array)
}

fn CFStringCreateByCombiningStrings(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    array: CFArrayRef,
    separator: CFStringRef,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let string: id = msg![env; array componentsJoinedByString:separator];
    retain(env, string)
}

fn CFStringGetDoubleValue(env: &mut Environment, string: CFStringRef) -> f64 {
    msg![env; string doubleValue]
}

fn CFStringHasPrefix(env: &mut Environment, string: CFStringRef, prefix: CFStringRef) -> bool {
    msg![env; string hasPrefix:prefix]
}

fn CFStringInsert(
    env: &mut Environment,
    string: CFMutableStringRef,
    index: CFIndex,
    inserted: CFStringRef,
) {
    let index: NSUInteger = index.try_into().unwrap();
    () = msg![env; string insertString:inserted atIndex:index];
}

fn CFStringReplace(
    env: &mut Environment,
    string: CFMutableStringRef,
    range: CFRange,
    replacement: CFStringRef,
) {
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length: range.length.try_into().unwrap(),
    };
    () = msg![env; string replaceCharactersInRange:range withString:replacement];
}

fn CFStringReplaceAll(env: &mut Environment, string: CFMutableStringRef, replacement: CFStringRef) {
    () = msg![env; string setString:replacement];
}

fn CFStringCreateWithCharacters(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    chars: ConstPtr<unichar>,
    num_chars: CFIndex,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    let len: GuestUSize = num_chars
        .try_into()
        .expect("Negative CFString character count");
    let units = if len == 0 {
        Vec::new()
    } else {
        env.mem
            .bytes_at(chars.cast(), len.checked_mul(2).unwrap())
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect()
    };
    // Preserve UTF-16 units (including embedded NULs and unpaired surrogates)
    // and return a +1 object even for the empty string.
    ns_string::from_u16_vec(env, units)
}

pub type CFStringEncoding = u32;
pub const kCFStringEncodingMacRoman: CFStringEncoding = 0;
pub const kCFStringEncodingASCII: CFStringEncoding = 0x600;
pub const kCFStringEncodingUTF8: CFStringEncoding = 0x8000100;
pub const kCFStringEncodingUnicode: CFStringEncoding = 0x100;
pub const kCFStringEncodingUTF16: CFStringEncoding = kCFStringEncodingUnicode;
pub const kCFStringEncodingUTF16BE: CFStringEncoding = 0x10000100;
pub const kCFStringEncodingUTF16LE: CFStringEncoding = 0x14000100;
pub const kCFStringEncodingISOLatin1: CFStringEncoding = 0x0201;

fn CFStringIsEncodingAvailable(_env: &mut Environment, encoding: CFStringEncoding) -> bool {
    matches!(
        encoding,
        kCFStringEncodingMacRoman
            | kCFStringEncodingASCII
            | kCFStringEncodingUTF8
            | kCFStringEncodingUTF16
            | kCFStringEncodingUTF16BE
            | kCFStringEncodingUTF16LE
            | kCFStringEncodingISOLatin1
    )
}

fn CFStringGetSystemEncoding(_env: &mut Environment) -> CFStringEncoding {
    kCFStringEncodingUTF8
}

fn CFStringConvertEncodingToIANACharSetName(
    env: &mut Environment,
    encoding: CFStringEncoding,
) -> CFStringRef {
    let name = match encoding {
        kCFStringEncodingMacRoman => "macintosh",
        kCFStringEncodingASCII => "us-ascii",
        kCFStringEncodingUTF8 => "utf-8",
        kCFStringEncodingUTF16 => "utf-16",
        kCFStringEncodingUTF16BE => "utf-16be",
        kCFStringEncodingUTF16LE => "utf-16le",
        kCFStringEncodingISOLatin1 => "iso-8859-1",
        _ => return id::null(),
    };
    ns_string::get_static_str(env, name)
}

fn CFStringGetMaximumSizeForEncoding(
    _env: &mut Environment,
    length: CFIndex,
    encoding: CFStringEncoding,
) -> CFIndex {
    if length < 0 || !CFStringIsEncodingAvailable(_env, encoding) {
        return kCFNotFound;
    }
    let bytes_per_unit = match encoding {
        kCFStringEncodingUTF8 => 3,
        kCFStringEncodingUTF16 | kCFStringEncodingUTF16BE | kCFStringEncodingUTF16LE => 2,
        _ => 1,
    };
    length.checked_mul(bytes_per_unit).unwrap_or(kCFNotFound)
}

fn CFStringAppend(
    env: &mut Environment,
    the_string: CFMutableStringRef,
    appended_string: CFStringRef,
) {
    msg![env; the_string appendString:appended_string]
}

fn CFStringAppendCharacters(
    env: &mut Environment,
    string: CFMutableStringRef,
    chars: ConstPtr<unichar>,
    num_chars: CFIndex,
) {
    let num_chars: GuestUSize = num_chars.try_into().unwrap();
    let chars: Vec<unichar> = (0..num_chars).map(|i| env.mem.read(chars + i)).collect();
    let appended = ns_string::from_u16_vec(env, chars);
    () = msg![env; string appendString:appended];
    crate::objc::release(env, appended);
}

fn CFStringAppendCString(
    env: &mut Environment,
    string: CFMutableStringRef,
    c_string: ConstPtr<u8>,
    encoding: CFStringEncoding,
) {
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    // TODO: avoid copying
    let to_append: id = msg_class![env; NSString stringWithCString:c_string encoding:encoding];
    msg![env; string appendString:to_append]
}

fn CFStringAppendFormat(
    env: &mut Environment,
    string: CFMutableStringRef,
    // Apple's own docs say these are unimplemented!
    _format_options: CFDictionaryRef,
    format: CFStringRef,
    dots: DotDotDot,
) {
    let res = ns_string::with_format(env, format, dots.start());
    let to_append: id = ns_string::from_rust_string(env, res);
    msg![env; string appendString:to_append]
}

pub fn CFStringConvertEncodingToNSStringEncoding(
    _env: &mut Environment,
    encoding: CFStringEncoding,
) -> ns_string::NSStringEncoding {
    match encoding {
        kCFStringEncodingMacRoman => ns_string::NSMacOSRomanStringEncoding,
        kCFStringEncodingASCII => ns_string::NSASCIIStringEncoding,
        kCFStringEncodingUTF8 => ns_string::NSUTF8StringEncoding,
        kCFStringEncodingUTF16 => ns_string::NSUTF16StringEncoding,
        kCFStringEncodingUTF16BE => ns_string::NSUTF16BigEndianStringEncoding,
        kCFStringEncodingUTF16LE => ns_string::NSUTF16LittleEndianStringEncoding,
        kCFStringEncodingISOLatin1 => ns_string::NSISOLatin1StringEncoding,
        _ => unimplemented!("Unhandled: CFStringEncoding {:#x}", encoding),
    }
}
fn CFStringConvertNSStringEncodingToEncoding(
    _env: &mut Environment,
    encoding: ns_string::NSStringEncoding,
) -> CFStringEncoding {
    match encoding {
        ns_string::NSMacOSRomanStringEncoding => kCFStringEncodingMacRoman,
        ns_string::NSASCIIStringEncoding => kCFStringEncodingASCII,
        ns_string::NSUTF8StringEncoding => kCFStringEncodingUTF8,
        ns_string::NSUTF16StringEncoding => kCFStringEncodingUTF16,
        ns_string::NSUTF16BigEndianStringEncoding => kCFStringEncodingUTF16BE,
        ns_string::NSUTF16LittleEndianStringEncoding => kCFStringEncodingUTF16LE,
        ns_string::NSISOLatin1StringEncoding => kCFStringEncodingISOLatin1,
        _ => unimplemented!("Unhandled: NSStringEncoding {:#x}", encoding),
    }
}

fn CFStringCreateCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    the_string: CFStringRef,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    msg![env; the_string copy]
}

fn CFStringCreateMutable(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    max_length: CFIndex,
) -> CFMutableStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert_eq!(max_length, 0);
    msg_class![env; NSMutableString new]
}

fn CFStringCreateMutableCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    max_length: CFIndex,
    the_string: CFStringRef,
) -> CFMutableStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert_eq!(max_length, 0);
    msg![env; the_string mutableCopy]
}

pub(crate) fn CFStringCreateWithBytes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    num_bytes: CFIndex,
    encoding: CFStringEncoding,
    is_external: bool,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let Ok(length) = u32::try_from(num_bytes) else {
        return crate::objc::nil;
    };
    if length != 0 && bytes.is_null() {
        return crate::objc::nil;
    }
    let mut input = env.mem.bytes_at(bytes, length);
    match encoding {
        kCFStringEncodingUTF16 | kCFStringEncodingUTF16BE | kCFStringEncodingUTF16LE => {
            if !length.is_multiple_of(2) {
                return crate::objc::nil;
            }
            let mut big = encoding == kCFStringEncodingUTF16BE;
            if is_external && encoding == kCFStringEncodingUTF16 {
                if input.starts_with(&[0xfe, 0xff]) {
                    big = true;
                    input = &input[2..];
                } else if input.starts_with(&[0xff, 0xfe]) {
                    input = &input[2..];
                }
            }
            let units = input
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| {
                    if big {
                        u16::from_be_bytes(*pair)
                    } else {
                        u16::from_le_bytes(*pair)
                    }
                })
                .collect();
            ns_string::from_u16_vec(env, units)
        }
        kCFStringEncodingUTF8 => {
            if is_external && input.starts_with(&[0xef, 0xbb, 0xbf]) {
                input = &input[3..];
            }
            let Ok(text) = std::str::from_utf8(input) else {
                return crate::objc::nil;
            };
            let text = text.to_owned();
            ns_string::from_rust_string(env, text)
        }
        kCFStringEncodingASCII | kCFStringEncodingMacRoman | kCFStringEncodingISOLatin1 => {
            if encoding != kCFStringEncodingISOLatin1 && !input.is_ascii() {
                return crate::objc::nil;
            }
            let units = input.iter().map(|&b| u16::from(b)).collect();
            ns_string::from_u16_vec(env, units)
        }
        _ => crate::objc::nil,
    }
}

fn CFStringCreateWithCString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    c_string: ConstPtr<u8>,
    encoding: CFStringEncoding,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    let ns_string: id = msg_class![env; NSString alloc];
    msg![env; ns_string initWithCString:c_string encoding:encoding]
}

fn CFStringCreateWithCStringNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    c_string: ConstPtr<u8>,
    encoding: CFStringEncoding,
    deallocator: CFAllocatorRef,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    if !valid_string_deallocator(env, deallocator) {
        return crate::objc::nil;
    }
    let c_len: GuestUSize = strlen(env, c_string);
    // Docs of CFStringCreateWithCStringNoCopy says caller should never assume
    // that the object is using the external buffer (it could be copied or even
    // dumped). So we can "safely" invoke a method which does copy!
    let Ok(length) = CFIndex::try_from(c_len) else {
        return crate::objc::nil;
    };
    let result = CFStringCreateWithBytes(env, allocator, c_string, length, encoding, false);
    if !result.is_null() {
        dispose_string_buffer(env, c_string.cast_void(), deallocator);
    }
    result
}

fn valid_string_deallocator(env: &Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault || {
        let allocator = env.mem.read(allocator);
        allocator.is_null() || allocator.is_system_default()
    }
}

fn dispose_string_buffer(env: &mut Environment, buffer: ConstVoidPtr, allocator: CFAllocatorRef) {
    if !buffer.is_null()
        && (allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        env.mem.free(buffer.cast_mut());
    }
}

fn CFStringCreateWithCharactersNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    characters: ConstPtr<unichar>,
    length: CFIndex,
    deallocator: CFAllocatorRef,
) -> CFStringRef {
    if length < 0
        || (length > 0 && characters.is_null())
        || !valid_string_deallocator(env, deallocator)
    {
        return crate::objc::nil;
    }
    let result = CFStringCreateWithCharacters(env, allocator, characters, length);
    if !result.is_null() {
        dispose_string_buffer(env, characters.cast_void(), deallocator);
    }
    result
}

fn CFStringCreateWithBytesNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
    encoding: CFStringEncoding,
    external: bool,
    deallocator: CFAllocatorRef,
) -> CFStringRef {
    if length < 0
        || (length > 0 && bytes.is_null())
        || !CFStringIsEncodingAvailable(env, encoding)
        || !valid_string_deallocator(env, deallocator)
    {
        return crate::objc::nil;
    }
    let result = CFStringCreateWithBytes(env, allocator, bytes, length, encoding, external);
    if !result.is_null() {
        dispose_string_buffer(env, bytes.cast_void(), deallocator);
    }
    result
}

fn CFStringCreateWithFormat(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    format_options: CFDictionaryRef,
    format: CFStringRef,
    args: DotDotDot,
) -> CFStringRef {
    CFStringCreateWithFormatAndArguments(env, allocator, format_options, format, args.start())
}

fn CFStringCreateWithFormatAndArguments(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    // Apple's own docs say these are unimplemented!
    _format_options: CFDictionaryRef,
    format: CFStringRef,
    args: VaList,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let res = ns_string::with_format(env, format, args);
    ns_string::from_rust_string(env, res)
}

fn CFStringCreateWithSubstring(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    the_string: CFStringRef,
    range: CFRange,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length: range.length.try_into().unwrap(),
    };
    let res: id = msg![env; the_string substringWithRange:range];
    msg![env; res copy]
}

pub type CFStringCompareFlags = CFOptionFlags;

fn CFStringCompare(
    env: &mut Environment,
    a: CFStringRef,
    b: CFStringRef,
    flags: CFStringCompareFlags,
) -> CFComparisonResult {
    msg![env; a compare:b options:flags]
}

fn CFStringCompareWithOptions(
    env: &mut Environment,
    a: CFStringRef,
    b: CFStringRef,
    range: CFRange,
    flags: CFStringCompareFlags,
) -> CFComparisonResult {
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length: range.length.try_into().unwrap(),
    };
    // TODO: avoid copying
    let a_sub: id = msg![env; a substringWithRange:range];
    msg![env; a_sub compare:b options:flags]
}

fn CFStringDelete(env: &mut Environment, string: CFMutableStringRef, range: CFRange) {
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length: range.length.try_into().unwrap(),
    };
    msg![env; string deleteCharactersInRange:range]
}

fn CFStringGetCharacterAtIndex(
    env: &mut Environment,
    the_string: CFStringRef,
    idx: CFIndex,
) -> unichar {
    let idx: NSUInteger = idx.try_into().unwrap();
    msg![env; the_string characterAtIndex:idx]
}

fn CFStringGetCharacters(
    env: &mut Environment,
    string: CFStringRef,
    range: CFRange,
    buffer: MutPtr<unichar>,
) {
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length: range.length.try_into().unwrap(),
    };
    msg![env; string getCharacters:buffer range:range]
}

fn CFStringGetCharactersPtr(_env: &mut Environment, _the_string: CFStringRef) -> ConstPtr<unichar> {
    // NULL is expected if the function cannot provide a buffer of Unicode
    // characters `efficiently`. Moreover, the same doc claims that the caller
    // should not `count on receiving a non-NULL result from this function
    // under any circumstances`. Win-win situation, if you ask me!
    ConstPtr::null()
}

fn CFStringGetCStringPtr(
    env: &mut Environment,
    the_string: CFStringRef,
    encoding: CFStringEncoding,
) -> ConstPtr<u8> {
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    msg![env; the_string cStringUsingEncoding:encoding]
}

fn CFStringGetCString(
    env: &mut Environment,
    a: CFStringRef,
    buffer: MutPtr<u8>,
    buffer_size: CFIndex,
    encoding: CFStringEncoding,
) -> bool {
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    let buffer_size = buffer_size as NSUInteger;
    msg![env; a getCString:buffer maxLength:buffer_size encoding:encoding]
}

fn CFStringGetBytes(
    env: &mut Environment,
    string: CFStringRef,
    range: CFRange,
    encoding: CFStringEncoding,
    loss_byte: u8,
    is_external: bool,
    buffer: MutPtr<u8>,
    max_buf_len: CFIndex,
    used_buf_len: MutPtr<CFIndex>,
) -> CFIndex {
    assert_eq!(loss_byte, 0);
    assert!(!is_external); // TODO

    let range_len = range.length;
    let range = NSRange {
        location: range.location.try_into().unwrap(),
        length: range_len.try_into().unwrap(),
    };
    // TODO: avoid copying
    let substring: id = msg![env; string substringWithRange:range];

    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    let buffer_size: NSUInteger = max_buf_len.try_into().unwrap();
    let success: bool =
        ns_string::get_bytes_buffer_inner(env, substring, buffer, buffer_size, encoding, false);
    assert!(success); // TODO
    let length: NSUInteger = msg![env; substring length];
    assert_eq!(length, u32::try_from(range_len).unwrap());

    if !used_buf_len.is_null() {
        let result_bytes_length: NSUInteger =
            msg![env; substring lengthOfBytesUsingEncoding:encoding];
        env.mem
            .write(used_buf_len, result_bytes_length.try_into().unwrap());
    }

    length.try_into().unwrap()
}

fn CFStringGetFileSystemRepresentation(
    env: &mut Environment,
    string: CFStringRef,
    buffer: MutPtr<u8>,
    max_buflen: CFIndex,
) -> bool {
    let max_buflen: NSUInteger = max_buflen.try_into().unwrap();
    msg![env; string getFileSystemRepresentation:buffer maxLength:max_buflen]
}

fn CFStringGetLength(env: &mut Environment, the_string: CFStringRef) -> CFIndex {
    let length: NSUInteger = msg![env; the_string length];
    length.try_into().unwrap()
}

fn CFStringGetIntValue(env: &mut Environment, string: CFStringRef) -> i32 {
    // TODO: check for allowed characters
    msg![env; string intValue]
}

fn CFStringFind(
    env: &mut Environment,
    string: CFStringRef,
    to_find: CFStringRef,
    options: CFStringCompareFlags,
) -> CFRange {
    let range = CFRange {
        location: 0,
        length: CFStringGetLength(env, string),
    };
    find_in_range(env, string, to_find, range, options).unwrap_or(CFRange {
        location: kCFNotFound,
        length: 0,
    })
}

fn find_in_range(
    env: &mut Environment,
    string: CFStringRef,
    needle: CFStringRef,
    range: CFRange,
    options: CFStringCompareFlags,
) -> Option<CFRange> {
    if string.is_null() || needle.is_null() || options & !15 != 0 {
        return None;
    }
    let start = usize::try_from(range.location).ok()?;
    let length = usize::try_from(range.length).ok()?;
    let mut haystack = Vec::new();
    ns_string::for_each_code_unit(env, string, |_, c| haystack.push(c));
    let mut target = Vec::new();
    ns_string::for_each_code_unit(env, needle, |_, c| target.push(c));
    let end = start.checked_add(length)?;
    if end > haystack.len() || target.is_empty() || target.len() > length {
        return None;
    }
    let index = search_units(&haystack, &target, start, end, options)?;
    Some(CFRange {
        location: index as CFIndex,
        length: target.len() as CFIndex,
    })
}

fn search_units(
    haystack: &[u16],
    target: &[u16],
    start: usize,
    end: usize,
    options: CFStringCompareFlags,
) -> Option<usize> {
    if target.is_empty() || end > haystack.len() || start > end || target.len() > end - start {
        return None;
    }
    let last = end - target.len();
    let matches = |index: usize| {
        haystack[index..index + target.len()]
            .iter()
            .zip(target)
            .all(|(&a, &b)| {
                a == b
                    || (options & 1 != 0
                        && match (char::from_u32(a.into()), char::from_u32(b.into())) {
                            (Some(a), Some(b)) => a.to_lowercase().eq(b.to_lowercase()),
                            _ => false,
                        })
            })
    };
    let backwards = options & 4 != 0;
    if options & 8 != 0 {
        let at = if backwards { last } else { start };
        matches(at).then_some(at)
    } else if backwards {
        (start..=last).rev().find(|&i| matches(i))
    } else {
        (start..=last).find(|&i| matches(i))
    }
}

fn CFStringFindAndReplace(
    env: &mut Environment,
    string: CFMutableStringRef,
    needle: CFStringRef,
    replacement: CFStringRef,
    range: CFRange,
    options: CFStringCompareFlags,
) -> CFIndex {
    if string.is_null() || needle.is_null() || replacement.is_null() || options & !15 != 0 {
        return 0;
    }
    let (Ok(mut start), Ok(length)) = (
        usize::try_from(range.location),
        usize::try_from(range.length),
    ) else {
        return 0;
    };
    let Some(mut end) = start.checked_add(length) else {
        return 0;
    };
    let mut text = Vec::new();
    let mut target = Vec::new();
    let mut substitute = Vec::new();
    ns_string::for_each_code_unit(env, string, |_, c| text.push(c));
    ns_string::for_each_code_unit(env, needle, |_, c| target.push(c));
    ns_string::for_each_code_unit(env, replacement, |_, c| substitute.push(c));
    let mut positions = Vec::new();
    while let Some(index) = search_units(&text, &target, start, end, options) {
        positions.push(index);
        if options & 4 != 0 {
            end = index;
        } else {
            start = index + target.len();
        }
    }
    if positions.is_empty() {
        return 0;
    }
    positions.sort_unstable();
    let count = positions.len();
    let Some(size) = text.len().checked_sub(count * target.len()).and_then(|n| {
        count
            .checked_mul(substitute.len())
            .and_then(|extra| n.checked_add(extra))
    }) else {
        return 0;
    };
    if size > i32::MAX as usize {
        return 0;
    }
    let mut result = Vec::new();
    if result.try_reserve_exact(size).is_err() {
        return 0;
    }
    let mut cursor = 0;
    for index in positions {
        result.extend_from_slice(&text[cursor..index]);
        result.extend_from_slice(&substitute);
        cursor = index + target.len();
    }
    result.extend_from_slice(&text[cursor..]);
    let result = ns_string::from_u16_vec(env, result);
    let _: () = msg![env; string setString:result];
    release(env, result);
    count as CFIndex
}

fn CFStringPad(
    env: &mut Environment,
    string: CFMutableStringRef,
    pad: CFStringRef,
    length: CFIndex,
    index: CFIndex,
) {
    if string.is_null() || length < 0 {
        return;
    }
    let mut text = Vec::new();
    ns_string::for_each_code_unit(env, string, |_, c| text.push(c));
    let length = length as usize;
    if length < text.len() {
        text.truncate(length);
    } else if length > text.len() {
        if pad.is_null() || index < 0 {
            return;
        }
        let mut padding = Vec::new();
        ns_string::for_each_code_unit(env, pad, |_, c| padding.push(c));
        let mut index = index as usize;
        if index >= padding.len() || text.try_reserve_exact(length - text.len()).is_err() {
            return;
        }
        while text.len() < length {
            text.push(padding[index]);
            index = (index + 1) % padding.len();
        }
    } else {
        return;
    }
    let result = ns_string::from_u16_vec(env, text);
    let _: () = msg![env; string setString:result];
    release(env, result);
}

fn CFStringFindWithOptions(
    env: &mut Environment,
    string: CFStringRef,
    needle: CFStringRef,
    range: CFRange,
    options: CFStringCompareFlags,
    result: MutPtr<CFRange>,
) -> bool {
    if let Some(found) = find_in_range(env, string, needle, range, options) {
        if !result.is_null() {
            env.mem.write(result, found);
        }
        true
    } else {
        false
    }
}

fn CFStringLowercase(env: &mut Environment, string: CFMutableStringRef, _locale: CFLocaleRef) {
    if string.is_null() {
        return;
    }
    let lowercase: id = msg![env; string lowercaseString];
    msg![env; string setString:lowercase]
}

fn CFStringTrim(env: &mut Environment, string: CFMutableStringRef, trim: CFStringRef) {
    if string.is_null() || trim.is_null() {
        return;
    }
    let text = ns_string::to_rust_string(env, string);
    let trim = ns_string::to_rust_string(env, trim);
    if trim.is_empty() {
        return;
    }
    let trimmed = text
        .trim_start_matches(trim.as_ref())
        .trim_end_matches(trim.as_ref())
        .to_owned();
    let result = ns_string::from_rust_string(env, trimmed);
    let _: () = msg![env; string setString:result];
    release(env, result);
}

fn CFStringTrimWhitespace(env: &mut Environment, string: CFMutableStringRef) {
    if string.is_null() {
        return;
    }
    let text = ns_string::to_rust_string(env, string);
    let result = ns_string::from_rust_string(env, text.trim().to_owned());
    let _: () = msg![env; string setString:result];
    release(env, result);
}

fn CFStringCreateExternalRepresentation(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    string: CFStringRef,
    encoding: CFStringEncoding,
    loss_byte: u8,
) -> super::cf_data::CFDataRef {
    if string.is_null()
        || !(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default())
    {
        return crate::objc::nil;
    }
    let mut units = Vec::new();
    ns_string::for_each_code_unit(env, string, |_, c| units.push(c));
    let mut bytes = Vec::new();
    match encoding {
        kCFStringEncodingUTF16 | kCFStringEncodingUTF16LE | kCFStringEncodingUTF16BE => {
            if encoding == kCFStringEncodingUTF16 {
                bytes.extend_from_slice(&[0xff, 0xfe]);
            }
            for unit in units {
                bytes.extend_from_slice(&if encoding == kCFStringEncodingUTF16BE {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
        }
        kCFStringEncodingUTF8 | kCFStringEncodingASCII | kCFStringEncodingISOLatin1 => {
            for character in char::decode_utf16(units) {
                let character = match character {
                    Ok(character) => character,
                    Err(_) if loss_byte != 0 => {
                        bytes.push(loss_byte);
                        continue;
                    }
                    Err(_) => return crate::objc::nil,
                };
                if encoding == kCFStringEncodingUTF8 {
                    bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
                } else {
                    let maximum = if encoding == kCFStringEncodingASCII {
                        127
                    } else {
                        255
                    };
                    if character as u32 <= maximum {
                        bytes.push(character as u8);
                    } else if loss_byte != 0 {
                        bytes.push(loss_byte);
                    } else {
                        return crate::objc::nil;
                    }
                }
            }
        }
        _ => return crate::objc::nil,
    }
    let Ok(length) = u32::try_from(bytes.len()) else {
        return crate::objc::nil;
    };
    let Some(buffer) = env.mem.try_alloc(length) else {
        return crate::objc::nil;
    };
    env.mem
        .bytes_at_mut(buffer.cast(), length)
        .copy_from_slice(&bytes);
    let data: id = msg_class![env; NSData alloc];
    msg![env; data initWithBytesNoCopy:buffer length:length freeWhenDone:true]
}

fn CFStringHasSuffix(env: &mut Environment, the_string: CFStringRef, suffix: CFStringRef) -> bool {
    msg![env; the_string hasSuffix:suffix]
}

fn CFStringUppercase(env: &mut Environment, string: CFStringRef, _locale: CFLocaleRef) {
    // TODO: account for locale
    let uppercase: id = msg![env; string uppercaseString];
    msg![env; string setString:uppercase]
}

type ConstStr255Param = ConstPtr<u8>;
type StringPtr = MutPtr<u8>;

fn CFStringCreateWithPascalString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    p_str: ConstStr255Param,
    encoding: CFStringEncoding,
) -> CFStringRef {
    let len: CFIndex = env.mem.read(p_str).into();
    let res = CFStringCreateWithBytes(env, allocator, p_str + 1, len, encoding, false);
    assert_eq!(len, CFStringGetLength(env, res));
    log_dbg!(
        "CFStringCreateWithPascalString('{}')",
        ns_string::to_rust_string(env, res)
    );
    res
}

fn CFStringGetPascalString(
    env: &mut Environment,
    the_string: CFStringRef,
    buffer: StringPtr,
    buffer_size: CFIndex,
    encoding: CFStringEncoding,
) -> bool {
    log_dbg!(
        "CFStringGetPascalString('{}')",
        ns_string::to_rust_string(env, the_string)
    );
    let len = CFStringGetLength(env, the_string);
    // first byte of Pascal string is length
    assert!((len + 1) <= buffer_size);
    let len_char: u8 = len.try_into().unwrap();
    env.mem.write(buffer, len_char);
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    ns_string::get_bytes_buffer_inner(
        env,
        the_string,
        buffer + 1,
        len_char.into(),
        encoding,
        false,
    )
}

type CFStringNormalizationForm = CFIndex;

fn CFStringNormalize(
    env: &mut Environment,
    the_string: CFMutableStringRef,
    the_form: CFStringNormalizationForm,
) {
    let str = ns_string::to_rust_string(env, the_string);
    log!("TODO: CFStringNormalize('{}', {})", str, the_form);
    assert!(str.is_ascii()); // TODO
}

fn CFStringCapitalize(env: &mut Environment, string: CFMutableStringRef, _locale: CFLocaleRef) {
    if string.is_null() {
        return;
    }

    // See CFStringLowercase: locale-specific rules are not emulated.
    let capitalized: id = msg![env;
    string capitalizedString];
    () = msg![env; string setString:capitalized];
}

fn CFStringCompareWithOptionsAndLocale(
    env: &mut Environment,
    a: CFStringRef,
    b: CFStringRef,
    range: CFRange,
    flags: CFStringCompareFlags,
    locale: CFLocaleRef,
) -> CFComparisonResult {
    if a.is_null() || b.is_null() {
        return if a.is_null() && b.is_null() {
            0
        } else if a.is_null() {
            -1
        } else {
            1
        };
    }

    let ns_range = match safe_cf_range_to_ns_range(range) {
        Some(r) => r,
        None => return 0,
    };
    let a_sub: id = msg![env; a substringWithRange:ns_range];

    if locale.is_null() {
        msg![env;
        a_sub compare:b options:flags]
    } else {
        msg![env;
        a_sub compare:b options:flags range:(NSRange { location: 0, length: msg![env; b length] }) locale:locale]
    }
}

fn CFStringCreateMutableWithExternalCharactersNoCopy(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    chars: MutPtr<unichar>,
    num_chars: CFIndex,
    capacity: CFIndex,
    external_characters_allocator: CFAllocatorRef,
) -> CFMutableStringRef {
    if !validate_allocator(env, allocator) {
        return nil;
    }

    // Not fully supported - create a regular mutable copy
    let _ = (chars, external_characters_allocator);
    let string = CFStringCreateMutable(env, allocator, capacity);

    if !string.is_null() && !chars.is_null() && num_chars > 0 {
        let temp = CFStringCreateWithCharacters(env, allocator, chars.cast_const(), num_chars);
        if !temp.is_null() {
            () = msg![env; string appendString:temp];
            CFRelease(env, temp);
        }
    }

    string
}

fn CFStringFindCharacterFromSet(
    env: &mut Environment,
    string: CFStringRef,
    set: CFTypeRef, // CFCharacterSetRef
    range_to_search: CFRange,
    options: CFStringCompareFlags,
    result: MutPtr<CFRange>,
) -> bool {
    if string.is_null() || set.is_null() {
        return false;
    }

    let search_range = match safe_cf_range_to_ns_range(range_to_search) {
        Some(r) => r,
        None => return false,
    };
    let found_range: NSRange =
        msg![env; string rangeOfCharacterFromSet:set options:options range:search_range];

    if found_range.location == NSNotFound as NSUInteger {
        return false;
    }

    if !result.is_null() {
        let cf_range = CFRange {
            location: found_range.location.try_into().unwrap_or(kCFNotFound),
            length: found_range.length.try_into().unwrap_or(0),
        };
        env.mem.write(result, cf_range);
    }

    true
}

fn CFStringGetCharacterFromInlineBuffer(
    env: &mut Environment,
    buf: MutVoidPtr,
    idx: CFIndex,
) -> unichar {
    if buf.is_null() || idx < 0 {
        return 0;
    }

    // Apple's 32-bit layout of CFStringInlineBuffer (from CFString.h):
    //     UniChar buffer[32];
    //     CFStringRef theString;
    //     const UniChar *chars;
    //     CFRange rangeToBuffer; // two CFIndex fields
    //     CFIndex bufferIndex;
    //     CFIndex stringIndex;
    // The caller (usually inlined guest code) fills the cache itself, so we
    // just fetch the character from the referenced string, which is always
    // correct regardless of what is currently cached.
    const INLINE_BUFFER_STRING_OFFSET: GuestUSize = 32 * 2;

    let the_string_ptr: MutPtr<CFStringRef> =
        (buf.cast::<u8>() + INLINE_BUFFER_STRING_OFFSET).cast();
    let the_string: CFStringRef = env.mem.read(the_string_ptr);
    if the_string.is_null() {
        return 0;
    }

    let length = CFStringGetLength(env, the_string);
    if idx >= length {
        return 0;
    }

    let idx_u: NSUInteger = idx.try_into().unwrap();
    msg![env;
    the_string characterAtIndex:idx_u]
}

/// Returns the encoding in which the string is most efficiently stored.
///
/// CoreFoundation's `CFString` may hold its bytes as ASCII, UTF-16, or a
/// platform encoding; this getter lets callers grab those bytes without a
/// conversion. We bridge into `NSString`'s host object via
/// [`ns_string::fastest_encoding`] and translate the result back to a
/// `CFStringEncoding`.
fn CFStringGetFastestEncoding(env: &mut Environment, the_string: CFStringRef) -> CFStringEncoding {
    if the_string.is_null() {
        // Apple's real implementation also tolerates NULL by returning
        // `kCFStringEncodingASCII` rather than crashing.
        return kCFStringEncodingASCII;
    }
    let ns_enc = ns_string::fastest_encoding(env, the_string);
    CFStringConvertNSStringEncodingToEncoding(env, ns_enc)
}

fn CFStringGetMostCompatibleMacStringEncoding(
    _env: &mut Environment,
    encoding: CFStringEncoding,
) -> CFStringEncoding {
    // Return closest Mac encoding
    match encoding {
        kCFStringEncodingUTF8
        | kCFStringEncodingUTF16
        | kCFStringEncodingUTF16BE
        | kCFStringEncodingUTF16LE => kCFStringEncodingUTF8,
        kCFStringEncodingASCII => kCFStringEncodingASCII,
        _ => kCFStringEncodingMacRoman,
    }
}

/// Returns the smallest encoding that can losslessly represent the string.
///
/// Mirrors `CFStringGetSmallestEncoding`. ASCII-only content reports
/// `kCFStringEncodingASCII`; anything with non-ASCII code points reports
/// `kCFStringEncodingUTF8` (UTF-8 covers the full Unicode range).
fn CFStringGetSmallestEncoding(env: &mut Environment, the_string: CFStringRef) -> CFStringEncoding {
    if the_string.is_null() {
        return kCFStringEncodingASCII;
    }
    let ns_enc = ns_string::smallest_encoding(env, the_string);
    CFStringConvertNSStringEncodingToEncoding(env, ns_enc)
}

fn CFStringTransform(
    env: &mut Environment,
    string: CFMutableStringRef,
    _range: MutPtr<CFRange>,
    transform: CFStringRef,
    reverse: bool,
) -> bool {
    if string.is_null() || transform.is_null() {
        return false;
    }

    let transform_name = ns_string::to_rust_string(env, transform);
    log_dbg!(
        "CFStringTransform('{}', reverse={})",
        transform_name,
        reverse
    );

    // Apple's public constants (e.g. kCFStringTransformStripDiacritics) hold
    // names without a "StringTransform" prefix, but guest code may also pass
    // the prefixed or ICU-style spelling; accept all of them.
    let name = transform_name
        .strip_prefix("StringTransform")
        .unwrap_or(&transform_name);

    // Approximate but adequate for the transforms real apps actually use.
    // Transliterations we cannot perform (e.g. CJK -> Latin) are reported as
    // success with the string unchanged, so callers do not take error paths.
    match name {
        "StripDiacritics" | "StripCombiningMarks" | "Latin-ASCII" => {
            // NSDiacriticInsensitiveSearch strips accents/diacritics.
            let folded: id = msg![env;
            string
                stringByFoldingWithOptions:128 // NSCaseInsensitiveSearch + NSDiacriticInsensitiveSearch
                locale:nil];
            () = msg![env; string setString:folded];
            true
        }
        "ToLatin" | "Any-Latin" => {
            let content = ns_string::to_rust_string(env, string);
            if !content.bytes().all(|b| b < 0x80) {
                // Best-effort: fold away diacritics; leave other scripts
                // untouched.
                let folded: id = msg![env;
                string
                    stringByFoldingWithOptions:128 // NSCaseInsensitiveSearch + NSDiacriticInsensitiveSearch
                    locale:nil];
                () = msg![env; string setString:folded];
            }
            true
        }
        "Lower" => {
            let lowered: id = msg![env; string lowercaseString];
            () = msg![env; string setString:lowered];
            true
        }
        "Upper" => {
            let uppered: id = msg![env; string uppercaseString];
            () = msg![env; string setString:uppered];
            true
        }
        _ => {
            log_dbg!(
                "CFStringTransform: unsupported transform '{}'; returning false",
                name
            );
            false
        }
    }
}

fn safe_cf_range_to_ns_range(range: CFRange) -> Option<NSRange> {
    if range.location < 0 || range.length < 0 {
        return None;
    }
    Some(NSRange {
        location: range.location.try_into().ok()?,
        length: range.length.try_into().ok()?,
    })
}

fn validate_allocator(env: &mut Environment, allocator: CFAllocatorRef) -> bool {
    allocator == kCFAllocatorDefault
        || allocator.is_null()
        || env.mem.read(allocator).is_system_default()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFStringRetain(_)),
    export_c_func!(CFStringIsEncodingAvailable(_)),
    export_c_func!(CFStringGetSystemEncoding()),
    export_c_func!(CFStringConvertEncodingToIANACharSetName(_)),
    export_c_func!(CFStringGetMaximumSizeForEncoding(_, _)),
    export_c_func!(CFStringRelease(_)),
    export_c_func!(CFStringCreateArrayBySeparatingStrings(_, _, _)),
    export_c_func!(CFStringCreateByCombiningStrings(_, _, _)),
    export_c_func!(CFStringGetDoubleValue(_)),
    export_c_func!(CFStringHasPrefix(_, _)),
    export_c_func!(CFStringInsert(_, _, _)),
    export_c_func!(CFStringReplace(_, _, _)),
    export_c_func!(CFStringReplaceAll(_, _)),
    export_c_func!(CFStringAppend(_, _)),
    export_c_func!(CFStringAppendCharacters(_, _, _)),
    export_c_func!(CFStringAppendCString(_, _, _)),
    export_c_func!(CFStringAppendFormat(_, _, _, _)),
    export_c_func!(CFStringConvertEncodingToNSStringEncoding(_)),
    export_c_func!(CFStringConvertNSStringEncodingToEncoding(_)),
    export_c_func!(CFStringCreateCopy(_, _)),
    export_c_func!(CFStringCreateMutable(_, _)),
    export_c_func!(CFStringCreateMutableCopy(_, _, _)),
    export_c_func!(CFStringCreateWithBytes(_, _, _, _, _)),
    export_c_func!(CFStringCreateWithCString(_, _, _)),
    export_c_func!(CFStringCreateWithCStringNoCopy(_, _, _, _)),
    export_c_func!(CFStringCreateWithCharactersNoCopy(_, _, _, _)),
    export_c_func!(CFStringCreateWithBytesNoCopy(_, _, _, _, _, _)),
    export_c_func!(CFStringCreateWithCharacters(_, _, _)),
    export_c_func!(CFStringCreateWithFormat(_, _, _, _)),
    export_c_func!(CFStringCreateWithFormatAndArguments(_, _, _, _)),
    export_c_func!(CFStringCreateWithSubstring(_, _, _)),
    export_c_func!(CFStringCompare(_, _, _)),
    export_c_func!(CFStringCompareWithOptions(_, _, _, _)),
    export_c_func!(CFStringDelete(_, _)),
    export_c_func!(CFStringGetCharacterAtIndex(_, _)),
    export_c_func!(CFStringGetCharacters(_, _, _)),
    export_c_func!(CFStringGetCharactersPtr(_)),
    export_c_func!(CFStringGetCStringPtr(_, _)),
    export_c_func!(CFStringGetCString(_, _, _, _)),
    export_c_func!(CFStringGetBytes(_, _, _, _, _, _, _, _)),
    export_c_func!(CFStringGetFileSystemRepresentation(_, _, _)),
    export_c_func!(CFStringGetIntValue(_)),
    export_c_func!(CFStringGetLength(_)),
    export_c_func!(CFStringFind(_, _, _)),
    export_c_func!(CFStringFindWithOptions(_, _, _, _, _)),
    export_c_func!(CFStringFindAndReplace(_, _, _, _, _)),
    export_c_func!(CFStringPad(_, _, _, _)),
    export_c_func!(CFStringLowercase(_, _)),
    export_c_func!(CFStringTrim(_, _)),
    export_c_func!(CFStringTrimWhitespace(_)),
    export_c_func!(CFStringCreateExternalRepresentation(_, _, _, _)),
    export_c_func!(CFStringHasSuffix(_, _)),
    export_c_func!(CFStringUppercase(_, _)),
    export_c_func!(CFStringCreateWithPascalString(_, _, _)),
    export_c_func!(CFStringGetPascalString(_, _, _, _)),
    export_c_func!(CFStringNormalize(_, _)),
    export_c_func!(CFStringCapitalize(_, _)),
    export_c_func!(CFStringCompareWithOptionsAndLocale(_, _, _, _, _)),
    export_c_func!(CFStringCreateMutableWithExternalCharactersNoCopy(
        _,
        _,
        _,
        _,
        _
    )),
    export_c_func!(CFStringFindCharacterFromSet(_, _, _, _, _)),
    export_c_func!(CFStringGetCharacterFromInlineBuffer(_, _)),
    export_c_func!(CFStringGetFastestEncoding(_)),
    export_c_func!(CFStringGetMostCompatibleMacStringEncoding(_)),
    export_c_func!(CFStringGetSmallestEncoding(_)),
    export_c_func!(CFStringTransform(_, _, _, _)),
];
