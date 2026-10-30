/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFURL`.
//!
//! This is toll-free bridged to `NSURL` in Apple's implementation. Here it is
//! the same type.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::CFIndex;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_string::{
    kCFStringEncodingASCII, kCFStringEncodingUTF8, CFStringConvertEncodingToNSStringEncoding,
    CFStringEncoding, CFStringRef,
};
use crate::frameworks::foundation::ns_string::{
    from_rust_string, get_static_str, to_rust_string, NSUTF8StringEncoding,
};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::{ConstPtr, MutPtr, Ptr};
use crate::objc::{id, msg, msg_class, nil, release};
use crate::Environment;

pub type CFURLRef = super::CFTypeRef;

type CFURLPathStyle = CFIndex;
const kCFURLPOSIXPathStyle: CFURLPathStyle = 0;
#[allow(dead_code)]
const kCFURLHFSPathStyle: CFURLPathStyle = 1;
#[allow(dead_code)]
const kCFURLWindowsPathStyle: CFURLPathStyle = 2;

pub fn CFURLGetFileSystemRepresentation(
    env: &mut Environment,
    url: CFURLRef,
    resolve_against_base: bool,
    buffer: MutPtr<u8>,
    buffer_size: CFIndex,
) -> bool {
    if resolve_against_base {
        // this function usually called to resolve resources from the main
        // bundle
        // thus, the url should already be an absolute path name
        // TODO: use absoluteURL instead once implemented
        let path = msg![env; url path];
        // TODO: avoid copy
        assert!(to_rust_string(env, path).starts_with('/'));
    }
    let buffer_size: NSUInteger = buffer_size.try_into().unwrap();

    msg![env; url getFileSystemRepresentation:buffer
                                    maxLength:buffer_size]
}

pub fn CFURLCreateFromFileSystemRepresentation(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    buffer: ConstPtr<u8>,
    buffer_size: CFIndex,
    is_directory: bool,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented

    let buffer_size: NSUInteger = buffer_size.try_into().unwrap();

    let string: id = msg_class![env; NSString alloc];
    let string: id = msg![env; string initWithBytes:buffer
                                             length:buffer_size
                                           encoding:NSUTF8StringEncoding];

    let url: id = msg_class![env; NSURL alloc];
    let res = msg![env; url initFileURLWithPath:string isDirectory:is_directory];
    release(env, string);
    res
}

fn CFURLCreateWithBytes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url_bytes: ConstPtr<u8>,
    length: CFIndex,
    encoding: CFStringEncoding,
    base_url: CFURLRef,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert_eq!(encoding, kCFStringEncodingASCII); // TODO
    assert!(base_url.is_null()); // TODO

    // TODO: interpret percent escape sequences using encoding as well
    let encoding = CFStringConvertEncodingToNSStringEncoding(env, encoding);
    if length <= 0 || url_bytes.is_null() {
        return Ptr::null();
    }
    let length: NSUInteger = length.try_into().unwrap();

    let string: id = msg_class![env; NSString alloc];
    let string: id = msg![env; string initWithBytes:url_bytes
                                             length:length
                                           encoding:encoding];

    if string.is_null() {
        return nil;
    }

    // The bytes describe a URL, not necessarily a filesystem path.
    let url: id = msg_class![env; NSURL alloc];
    let res = msg![env; url initWithString:string];
    release(env, string);
    res
}

fn CFURLCreateWithFileSystemPath(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    file_path: CFStringRef,
    style: CFURLPathStyle,
    is_directory: bool,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert_eq!(style, kCFURLPOSIXPathStyle);
    let url: id = msg_class![env; NSURL alloc];
    msg![env; url initFileURLWithPath:file_path isDirectory:is_directory]
}

fn CFURLCreateWithString(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url_string: CFStringRef,
    base_url: CFURLRef,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert!(base_url.is_null()); // TODO
    let url: id = msg_class![env; NSURL alloc];
    msg![env; url initWithString:url_string]
}

pub fn CFURLCopyPathExtension(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    let path = msg![env; url path];
    let ext = msg![env; path pathExtension];
    msg![env; ext copy]
}

fn CFURLCopyScheme(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    let scheme: CFStringRef = msg![env; url scheme];
    msg![env; scheme copy]
}

fn CFURLCopyHostName(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    let host: CFStringRef = msg![env; url host];
    msg![env; host copy]
}

fn CFURLCopyPath(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    match crate::frameworks::foundation::ns_url::cf_escaped_path(env, url) {
        Some(path) => from_rust_string(env, path),
        None => nil,
    }
}

fn CFURLCanBeDecomposed(env: &mut Environment, url: CFURLRef) -> bool {
    crate::frameworks::foundation::ns_url::cf_escaped_path(env, url).is_some()
}

fn CFURLCopyStrictPath(
    env: &mut Environment,
    url: CFURLRef,
    is_absolute: MutPtr<u8>,
) -> CFStringRef {
    let path = crate::frameworks::foundation::ns_url::cf_escaped_path(env, url);
    let absolute = path.as_deref().is_some_and(|path| path.starts_with('/'));
    if !is_absolute.is_null() {
        env.mem.write(is_absolute, u8::from(absolute));
    }
    match path {
        Some(path) if !path.is_empty() => {
            let strict = if absolute { &path[1..] } else { &path };
            from_rust_string(env, strict.to_owned())
        }
        _ => nil,
    }
}

fn CFURLCopyQueryString(
    env: &mut Environment,
    url: CFURLRef,
    leave_escaped: CFStringRef,
) -> CFStringRef {
    let query: CFStringRef = msg![env; url query];
    if query.is_null() {
        return nil;
    }
    CFURLCreateStringByReplacingPercentEscapesUsingEncoding(
        env,
        kCFAllocatorDefault,
        query,
        leave_escaped,
        kCFStringEncodingUTF8,
    )
}

fn CFURLCopyFragment(
    env: &mut Environment,
    url: CFURLRef,
    leave_escaped: CFStringRef,
) -> CFStringRef {
    let fragment: CFStringRef = msg![env; url fragment];
    if fragment.is_null() {
        return nil;
    }
    CFURLCreateStringByReplacingPercentEscapesUsingEncoding(
        env,
        kCFAllocatorDefault,
        fragment,
        leave_escaped,
        kCFStringEncodingUTF8,
    )
}

fn CFURLGetPortNumber(env: &mut Environment, url: CFURLRef) -> i32 {
    let port: id = msg![env; url port];
    if port.is_null() {
        -1
    } else {
        msg![env; port intValue]
    }
}

fn CFURLCopyFileSystemPath(
    env: &mut Environment,
    url: CFURLRef,
    style: CFURLPathStyle,
) -> CFStringRef {
    assert_eq!(style, kCFURLPOSIXPathStyle);
    let path: CFStringRef = msg![env; url path];
    msg![env; path copy]
}

fn CFURLCreateCopyAppendingPathComponent(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url: CFURLRef,
    path_component: CFStringRef,
    is_directory: bool,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let new_url =
        msg![env; url URLByAppendingPathComponent:path_component isDirectory:is_directory];
    msg![env; new_url copy]
}

fn CFURLCreateCopyDeletingLastPathComponent(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url: CFURLRef,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let new_url = msg![env; url URLByDeletingLastPathComponent];
    msg![env; new_url copy]
}

fn CFURLHasDirectoryPath(env: &mut Environment, url: CFURLRef) -> bool {
    assert!(!url.is_null());

    if let Some(directory) = crate::frameworks::foundation::ns_url::cf_file_directory_flag(env, url)
    {
        return directory;
    }
    if let Some(path) = crate::frameworks::foundation::ns_url::cf_escaped_path(env, url) {
        let last = path.rsplit('/').next().unwrap_or("");
        return path.ends_with('/') || last == "." || last == "..";
    }

    let path = msg![env; url path];
    if msg![env; path isEqual:(get_static_str(env, "//"))] {
        // Special case
        return false;
    }
    // Note: cannot use `lastPathComponent` here!
    let components: id = msg![env; path pathComponents];
    let count: NSUInteger = msg![env; components count];
    if count == 0 {
        return false;
    }
    let last: id = msg![env; components objectAtIndex:(count - 1)];
    msg![env; last isEqual:(get_static_str(env, "/"))]
        || msg![env; last isEqual:(get_static_str(env, "."))]
        || msg![env; last isEqual:(get_static_str(env, ".."))]
}

/// Deprecated in iOS 7, but used by some older apps to load resource files.
/// Only reading the data is supported; properties are not.
fn CFURLCreateDataAndPropertiesFromResource(
    env: &mut Environment,
    _allocator: CFAllocatorRef,
    url: CFURLRef,
    resource_data: MutPtr<id>,
    properties: MutPtr<id>,
    _desired_properties: id,
    error_code: MutPtr<i32>,
) -> bool {
    const kCFURLResourceNotFoundError: i32 = -12;

    let data: id = if url.is_null() || resource_data.is_null() {
        nil
    } else {
        let data: id = msg_class![env; NSData alloc];
        msg![env; data initWithContentsOfURL:url]
    };
    let success = !data.is_null();
    if !resource_data.is_null() {
        env.mem.write(resource_data, data);
    }
    if !properties.is_null() {
        env.mem.write(properties, nil);
    }
    if !error_code.is_null() {
        let code = if success || resource_data.is_null() {
            0
        } else {
            kCFURLResourceNotFoundError
        };
        env.mem.write(error_code, code);
    }
    success || resource_data.is_null()
}

/// Characters that may appear in a URL as they are: RFC 2396's unreserved and
/// reserved characters, plus `%` and `#`, which CF also leaves alone.
fn is_legal_url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-_.~!*'();:@&=+$,/?%#".contains(c)
}

pub(crate) fn add_percent_escapes(string: &str, leave_unescaped: &str, to_escape: &str) -> String {
    let mut escaped = String::with_capacity(string.len());
    for c in string.chars() {
        let escape = if is_legal_url_char(c) {
            to_escape.contains(c)
        } else {
            !leave_unescaped.contains(c)
        };
        if escape {
            let mut utf8 = [0u8; 4];
            for byte in c.encode_utf8(&mut utf8).bytes() {
                escaped.push_str(&format!("%{:02X}", byte));
            }
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// Replaces percent escapes, except those of the characters in
/// `leave_escaped`. Returns [None] for a malformed escape, or if the escapes
/// don't decode to UTF-8.
fn replace_percent_escapes(string: &str, leave_escaped: &str) -> Option<String> {
    let bytes = string.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            decoded.push(bytes[i]);
            i += 1;
            continue;
        }
        let group_start = i;
        let mut group = Vec::new();
        while i < bytes.len() && bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            group.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        }
        let group_str = std::str::from_utf8(&group).ok()?;
        for (start, character) in group_str.char_indices() {
            let end = start + character.len_utf8();
            if leave_escaped.contains(character) {
                decoded.extend_from_slice(&bytes[group_start + start * 3..group_start + end * 3]);
            } else {
                decoded.extend_from_slice(&group[start..end]);
            }
        }
    }
    String::from_utf8(decoded).ok()
}

fn optional_rust_string(env: &mut Environment, string: CFStringRef) -> Option<String> {
    (!string.is_null()).then(|| to_rust_string(env, string).into_owned())
}

fn CFURLCreateStringByAddingPercentEscapes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    original_string: CFStringRef,
    characters_to_leave_unescaped: CFStringRef,
    legal_url_characters_to_be_escaped: CFStringRef,
    encoding: CFStringEncoding,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert!(encoding == kCFStringEncodingUTF8 || encoding == kCFStringEncodingASCII); // TODO
    let original = to_rust_string(env, original_string).into_owned();
    let leave_unescaped = optional_rust_string(env, characters_to_leave_unescaped);
    let to_escape = optional_rust_string(env, legal_url_characters_to_be_escaped);
    let escaped = add_percent_escapes(
        &original,
        leave_unescaped.as_deref().unwrap_or(""),
        to_escape.as_deref().unwrap_or(""),
    );
    from_rust_string(env, escaped)
}

fn CFURLCreateStringByReplacingPercentEscapesUsingEncoding(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    original_string: CFStringRef,
    characters_to_leave_escaped: CFStringRef,
    encoding: CFStringEncoding,
) -> CFStringRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    assert!(encoding == kCFStringEncodingUTF8 || encoding == kCFStringEncodingASCII); // TODO

    // NULL means that no escapes are replaced.
    let Some(leave_escaped) = optional_rust_string(env, characters_to_leave_escaped) else {
        return msg![env; original_string copy];
    };
    let original = to_rust_string(env, original_string).into_owned();
    match replace_percent_escapes(&original, &leave_escaped) {
        Some(replaced) => from_rust_string(env, replaced),
        None => Ptr::null(),
    }
}

fn CFURLCreateStringByReplacingPercentEscapes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    original_string: CFStringRef,
    characters_to_leave_escaped: CFStringRef,
) -> CFStringRef {
    CFURLCreateStringByReplacingPercentEscapesUsingEncoding(
        env,
        allocator,
        original_string,
        characters_to_leave_escaped,
        kCFStringEncodingUTF8,
    )
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFURLGetFileSystemRepresentation(_, _, _, _)),
    export_c_func!(CFURLCreateFromFileSystemRepresentation(_, _, _, _)),
    export_c_func!(CFURLCreateWithBytes(_, _, _, _, _)),
    export_c_func!(CFURLCreateWithFileSystemPath(_, _, _, _)),
    export_c_func!(CFURLCreateWithString(_, _, _)),
    export_c_func!(CFURLCopyPathExtension(_)),
    export_c_func!(CFURLCopyScheme(_)),
    export_c_func!(CFURLCopyHostName(_)),
    export_c_func!(CFURLCopyPath(_)),
    export_c_func!(CFURLCanBeDecomposed(_)),
    export_c_func!(CFURLCopyStrictPath(_, _)),
    export_c_func!(CFURLCopyQueryString(_, _)),
    export_c_func!(CFURLCopyFragment(_, _)),
    export_c_func!(CFURLGetPortNumber(_)),
    export_c_func!(CFURLCopyFileSystemPath(_, _)),
    export_c_func!(CFURLCreateCopyAppendingPathComponent(_, _, _, _)),
    export_c_func!(CFURLCreateCopyDeletingLastPathComponent(_, _)),
    export_c_func!(CFURLHasDirectoryPath(_)),
    export_c_func!(CFURLCreateDataAndPropertiesFromResource(_, _, _, _, _, _)),
    export_c_func!(CFURLCreateStringByAddingPercentEscapes(_, _, _, _, _)),
    export_c_func!(CFURLCreateStringByReplacingPercentEscapes(_, _, _)),
    export_c_func!(CFURLCreateStringByReplacingPercentEscapesUsingEncoding(
        _,
        _,
        _,
        _
    )),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percent_escapes() {
        assert_eq!(
            add_percent_escapes("a b/é?x=1#f", "", ""),
            "a%20b/%C3%A9?x=1#f"
        );
        assert_eq!(add_percent_escapes("a b&c=d", " ", "&="), "a b%26c%3Dd");
        assert_eq!(
            replace_percent_escapes("a%20b%2Fc%C3%A9", "").as_deref(),
            Some("a b/cé")
        );
        assert_eq!(
            replace_percent_escapes("a%20b%2Fc", "/").as_deref(),
            Some("a b%2Fc")
        );
        assert_eq!(replace_percent_escapes("100%", ""), None);
        assert_eq!(
            replace_percent_escapes("%c3%a9%2F%E4%B8%AD", "é/").as_deref(),
            Some("%c3%a9%2F中")
        );
        assert_eq!(replace_percent_escapes("%C3%FF", "é"), None);
    }
}
