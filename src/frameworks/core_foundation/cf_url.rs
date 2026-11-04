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
    kCFStringEncodingASCII, kCFStringEncodingUTF8, CFStringEncoding, CFStringRef,
};
use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str, to_rust_string};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::{ConstPtr, MutPtr, Ptr};
use crate::objc::{id, msg, msg_class, nil, release, retain};
use crate::Environment;

pub type CFURLRef = super::CFTypeRef;

fn CFURLRetain(env: &mut Environment, url: CFURLRef) -> CFURLRef {
    retain(env, url)
}

fn CFURLRelease(env: &mut Environment, url: CFURLRef) {
    release(env, url);
}

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
    let url: id = if resolve_against_base {
        msg![env; url absoluteURL]
    } else {
        url
    };
    let buffer_size: NSUInteger = buffer_size.try_into().unwrap();

    msg![env; url getFileSystemRepresentation:buffer
                                    maxLength:buffer_size]
}

fn CFURLGetString(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    msg![env; url relativeString]
}

fn CFURLCopyAbsoluteURL(env: &mut Environment, url: CFURLRef) -> CFURLRef {
    let absolute: id = msg![env; url absoluteURL];
    retain(env, absolute)
}

fn CFURLGetBytes(
    env: &mut Environment,
    url: CFURLRef,
    buffer: MutPtr<u8>,
    length: CFIndex,
) -> CFIndex {
    if url == nil || length < 0 {
        return -1;
    }
    let string = CFURLGetString(env, url);
    if string == nil {
        return -1;
    }
    let bytes = to_rust_string(env, string);
    let Ok(size) = CFIndex::try_from(bytes.len()) else {
        return -1;
    };
    if buffer.is_null() {
        return size;
    }
    if length < size {
        return -1;
    }
    env.mem
        .bytes_at_mut(buffer, size as u32)
        .copy_from_slice(bytes.as_bytes());
    size
}

pub fn CFURLCreateFromFileSystemRepresentation(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    buffer: ConstPtr<u8>,
    buffer_size: CFIndex,
    is_directory: bool,
) -> CFURLRef {
    CFURLCreateFromFileSystemRepresentationRelativeToBase(
        env,
        allocator,
        buffer,
        buffer_size,
        is_directory,
        nil,
    )
}

fn CFURLCreateFromFileSystemRepresentationRelativeToBase(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    buffer: ConstPtr<u8>,
    length: CFIndex,
    directory: bool,
    base: CFURLRef,
) -> CFURLRef {
    let string = super::cf_string::CFStringCreateWithBytes(
        env,
        allocator,
        buffer,
        length,
        kCFStringEncodingUTF8,
        false,
    );
    if string == nil {
        return nil;
    }
    let result = CFURLCreateWithFileSystemPathRelativeToBase(
        env,
        allocator,
        string,
        kCFURLPOSIXPathStyle,
        directory,
        base,
    );
    release(env, string);
    result
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
    if encoding != kCFStringEncodingASCII && encoding != kCFStringEncodingUTF8 {
        return nil;
    }

    // TODO: interpret percent escape sequences using encoding as well
    if length < 0 || (length > 0 && url_bytes.is_null()) {
        return nil;
    }
    let string = super::cf_string::CFStringCreateWithBytes(
        env, allocator, url_bytes, length, encoding, false,
    );

    if string.is_null() {
        return nil;
    }

    // The bytes describe a URL, not necessarily a filesystem path.
    let url: id = msg_class![env; NSURL alloc];
    let res = msg![env; url initWithString:string relativeToURL:base_url];
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
    let url: id = msg_class![env; NSURL alloc];
    msg![env; url initWithString:url_string relativeToURL:base_url]
}

fn CFURLCreateWithFileSystemPathRelativeToBase(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    path: CFStringRef,
    style: CFURLPathStyle,
    directory: bool,
    base: CFURLRef,
) -> CFURLRef {
    if path == nil || style != kCFURLPOSIXPathStyle {
        return nil;
    }
    let text = to_rust_string(env, path);
    if base == nil || text.starts_with('/') {
        return CFURLCreateWithFileSystemPath(env, allocator, path, style, directory);
    }
    let is_file: bool = msg![env; base isFileURL];
    if !is_file {
        return nil;
    }
    let mut escaped = add_percent_escapes(&text, "", "%?#;:");
    if directory && !escaped.ends_with('/') {
        escaped.push('/');
    }
    let string = from_rust_string(env, escaped);
    let result = CFURLCreateWithString(env, allocator, string, base);
    release(env, string);
    result
}

fn CFURLCreateAbsoluteURLWithBytes(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    bytes: ConstPtr<u8>,
    length: CFIndex,
    encoding: CFStringEncoding,
    base: CFURLRef,
    _compatibility: bool,
) -> CFURLRef {
    // Both entry modes currently use the hierarchical reference resolver;
    // historical compatibility-only parsing differences remain unsupported.
    let relative = CFURLCreateWithBytes(env, allocator, bytes, length, encoding, base);
    if relative == nil {
        return nil;
    }
    let absolute = CFURLCopyAbsoluteURL(env, relative);
    release(env, relative);
    absolute
}

pub fn CFURLCopyPathExtension(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    let path = msg![env; url path];
    let ext = msg![env; path pathExtension];
    msg![env; ext copy]
}

fn CFURLCopyLastPathComponent(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    let path: id = msg![env; url path];
    let component: id = msg![env; path lastPathComponent];
    msg![env; component copy]
}

fn CFURLGetBaseURL(env: &mut Environment, url: CFURLRef) -> CFURLRef {
    msg![env; url baseURL]
}

fn CFURLCopyScheme(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    let scheme: CFStringRef = msg![env; url scheme];
    msg![env; scheme copy]
}

fn CFURLCopyHostName(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    let host: CFStringRef = msg![env; url host];
    msg![env; host copy]
}

fn CFURLCopyNetLocation(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    match crate::frameworks::foundation::ns_url::cf_authority(env, url) {
        Some(authority) => from_rust_string(env, authority),
        None => nil,
    }
}

fn CFURLCopyUserName(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    match crate::frameworks::foundation::ns_url::cf_user_and_password(env, url).0 {
        Some(user) => from_rust_string(env, user),
        None => nil,
    }
}

fn CFURLCopyPassword(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    match crate::frameworks::foundation::ns_url::cf_user_and_password(env, url).1 {
        Some(password) => from_rust_string(env, password),
        None => nil,
    }
}

fn CFURLCopyResourceSpecifier(env: &mut Environment, url: CFURLRef) -> CFStringRef {
    match crate::frameworks::foundation::ns_url::cf_resource_components(env, url).1 {
        Some(resource) => from_rust_string(env, resource),
        None => nil,
    }
}

fn CFURLCopyParameterString(
    env: &mut Environment,
    url: CFURLRef,
    leave_escaped: CFStringRef,
) -> CFStringRef {
    let Some(parameter) = crate::frameworks::foundation::ns_url::cf_resource_components(env, url).0
    else {
        return nil;
    };
    let parameter = from_rust_string(env, parameter);
    let result = CFURLCreateStringByReplacingPercentEscapesUsingEncoding(
        env,
        kCFAllocatorDefault,
        parameter,
        leave_escaped,
        kCFStringEncodingUTF8,
    );
    release(env, parameter);
    result
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

fn CFURLCreateCopyAppendingPathExtension(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url: CFURLRef,
    extension: CFStringRef,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    if url.is_null() || extension.is_null() {
        return nil;
    }
    let is_file: bool = msg![env; url isFileURL];
    if !is_file {
        return nil;
    }
    let path: id = msg![env; url path];
    let path: id = msg![env; path stringByAppendingPathExtension:extension];
    if path.is_null() {
        return nil;
    }
    let result: id = msg_class![env; NSURL alloc];
    msg![env; result initFileURLWithPath:path]
}

fn CFURLCreateCopyDeletingPathExtension(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    url: CFURLRef,
) -> CFURLRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    if url.is_null() {
        return nil;
    }
    let is_file: bool = msg![env; url isFileURL];
    if !is_file {
        return nil;
    }
    let path: id = msg![env; url path];
    let path: id = msg![env; path stringByDeletingPathExtension];
    let result: id = msg_class![env; NSURL alloc];
    msg![env; result initFileURLWithPath:path]
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
    export_c_func!(CFURLCreateWithFileSystemPathRelativeToBase(_, _, _, _, _)),
    export_c_func!(CFURLCreateFromFileSystemRepresentationRelativeToBase(
        _,
        _,
        _,
        _,
        _
    )),
    export_c_func!(CFURLCreateAbsoluteURLWithBytes(_, _, _, _, _, _)),
    export_c_func!(CFURLGetString(_)),
    export_c_func!(CFURLCopyAbsoluteURL(_)),
    export_c_func!(CFURLGetBytes(_, _, _)),
    export_c_func!(CFURLRetain(_)),
    export_c_func!(CFURLRelease(_)),
    export_c_func!(CFURLGetBaseURL(_)),
    export_c_func!(CFURLCopyLastPathComponent(_)),
    export_c_func!(CFURLCreateCopyAppendingPathExtension(_, _, _)),
    export_c_func!(CFURLCreateCopyDeletingPathExtension(_, _)),
    export_c_func!(CFURLGetFileSystemRepresentation(_, _, _, _)),
    export_c_func!(CFURLCreateFromFileSystemRepresentation(_, _, _, _)),
    export_c_func!(CFURLCreateWithBytes(_, _, _, _, _)),
    export_c_func!(CFURLCreateWithFileSystemPath(_, _, _, _)),
    export_c_func!(CFURLCreateWithString(_, _, _)),
    export_c_func!(CFURLCopyPathExtension(_)),
    export_c_func!(CFURLCopyScheme(_)),
    export_c_func!(CFURLCopyHostName(_)),
    export_c_func!(CFURLCopyNetLocation(_)),
    export_c_func!(CFURLCopyUserName(_)),
    export_c_func!(CFURLCopyPassword(_)),
    export_c_func!(CFURLCopyResourceSpecifier(_)),
    export_c_func!(CFURLCopyParameterString(_, _)),
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
