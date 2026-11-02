/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CFBundle`.
//!
//! This is not even toll-free bridged to `NSBundle` in Apple's implementation,
//! but here it is the same type.

use super::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use super::cf_array::CFArrayRef;
use super::cf_dictionary::CFDictionaryRef;
use super::cf_string::CFStringRef;
use super::cf_url::CFURLRef;
use super::CFTypeRef;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::foundation::ns_bundle::NSBundleHostObject;
use crate::frameworks::foundation::{ns_array, ns_string, NSUInteger};
use crate::objc::{id, msg, msg_class, nil, release, retain};
use crate::Environment;

const kCFBundleVersionKey: &str = "CFBundleVersion";
const kCFBundleExecutableKey: &str = "CFBundleExecutable";

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCFBundleVersionKey",
        HostConstant::NSString(kCFBundleVersionKey),
    ),
    (
        "_kCFBundleExecutableKey",
        HostConstant::NSString(kCFBundleExecutableKey),
    ),
    (
        "_kCFBundleIdentifierKey",
        HostConstant::NSString("CFBundleIdentifier"),
    ),
    ("_kCFBundleNameKey", HostConstant::NSString("CFBundleName")),
    (
        "_kCFBundleInfoDictionaryVersionKey",
        HostConstant::NSString("CFBundleInfoDictionaryVersion"),
    ),
    (
        "_kCFBundleLocalizationsKey",
        HostConstant::NSString("CFBundleLocalizations"),
    ),
    (
        "_kCFBundleDevelopmentRegionKey",
        HostConstant::NSString("CFBundleDevelopmentRegion"),
    ),
];

pub type CFBundleRef = CFTypeRef;

fn CFBundleGetMainBundle(env: &mut Environment) -> CFBundleRef {
    msg_class![env; NSBundle mainBundle]
}

fn CFBundleCreate(env: &mut Environment, allocator: CFAllocatorRef, url: CFURLRef) -> CFBundleRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default());
    if url == nil {
        return nil;
    }
    let bundle: id = msg_class![env; NSBundle bundleWithURL:url];
    retain(env, bundle)
}

fn CFBundleGetIdentifier(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    if bundle == nil {
        return nil;
    }
    let key = ns_string::get_static_str(env, "CFBundleIdentifier");
    CFBundleGetValueForInfoDictionaryKey(env, bundle, key)
}

fn CFBundleCopyBundleIdentifier(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    let identifier = CFBundleGetIdentifier(env, bundle);
    msg![env; identifier copy]
}

fn copy_info_string(env: &mut Environment, bundle: CFBundleRef, key: &'static str) -> CFStringRef {
    if bundle == nil {
        return nil;
    }
    let key = ns_string::get_static_str(env, key);
    let value = CFBundleGetValueForInfoDictionaryKey(env, bundle, key);
    msg![env; value copy]
}

fn CFBundleCopyShortVersionString(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    copy_info_string(env, bundle, "CFBundleShortVersionString")
}

fn CFBundleCopyBundleDevelopmentRegion(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    let region = copy_info_string(env, bundle, "CFBundleDevelopmentRegion");
    if region != nil {
        return region;
    }
    let english = ns_string::get_static_str(env, "en");
    msg![env; english copy]
}

fn CFBundleCopyDisplayName(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    for key in ["CFBundleDisplayName", "CFBundleName", "CFBundleExecutable"] {
        let value = copy_info_string(env, bundle, key);
        if value != nil {
            return value;
        }
    }
    let empty = ns_string::get_static_str(env, "");
    msg![env; empty copy]
}

fn CFBundleCopyHelpBookName(env: &mut Environment, bundle: CFBundleRef) -> CFStringRef {
    copy_info_string(env, bundle, "CFBundleHelpBookName")
}

fn CFBundleCopyResourceURLsOfType(
    env: &mut Environment,
    bundle: CFBundleRef,
    resource_type: CFStringRef,
    subdirectory: CFStringRef,
) -> CFArrayRef {
    if bundle == nil {
        return nil;
    }
    let paths: id =
        msg![env; bundle pathsForResourcesOfType:resource_type inDirectory:subdirectory];
    let count: NSUInteger = msg![env; paths count];
    let mut urls = Vec::new();
    for index in 0..count {
        let path: id = msg![env; paths objectAtIndex:index];
        let url: id = msg_class![env; NSURL alloc];
        let url: id = msg![env; url initFileURLWithPath:path];
        urls.push(url);
    }
    ns_array::from_vec(env, urls)
}

fn CFBundleCopyInfoDictionaryForURL(env: &mut Environment, url: CFURLRef) -> CFDictionaryRef {
    if url == nil {
        return nil;
    }
    let bundle: id = msg_class![env; NSBundle bundleWithURL:url];
    let dict: id = msg![env; bundle infoDictionary];
    msg![env; dict copy]
}

fn CFBundleCopyInfoDictionaryInDirectory(env: &mut Environment, url: CFURLRef) -> CFDictionaryRef {
    CFBundleCopyInfoDictionaryForURL(env, url)
}

fn CFBundleCopyResourceURLInDirectory(
    env: &mut Environment,
    url: CFURLRef,
    name: CFStringRef,
    resource_type: CFStringRef,
    subdirectory: CFStringRef,
) -> CFURLRef {
    if url == nil {
        return nil;
    }
    let bundle: id = msg_class![env; NSBundle bundleWithURL:url];
    if bundle == nil {
        return nil;
    }
    CFBundleCopyResourceURL(env, bundle, name, resource_type, subdirectory)
}

fn CFBundleGetInfoDictionary(env: &mut Environment, bundle: CFBundleRef) -> CFDictionaryRef {
    msg![env; bundle infoDictionary]
}

fn CFBundleGetValueForInfoDictionaryKey(
    env: &mut Environment,
    bundle: CFBundleRef,
    key: CFStringRef,
) -> CFTypeRef {
    let dict: id = msg![env; bundle infoDictionary];
    msg![env; dict objectForKey:key]
}

fn CFBundleGetVersionNumber(env: &mut Environment, bundle: CFBundleRef) -> u32 {
    let version_key: id = ns_string::get_static_str(env, kCFBundleVersionKey);
    let vers: id = CFBundleGetValueForInfoDictionaryKey(env, bundle, version_key);
    let vers_str = ns_string::to_rust_string(env, vers);
    log_dbg!("CFBundleGetVersionNumber {}", vers_str);

    let parts: Vec<&str> = vers_str.split('.').collect();
    assert!(parts.len() <= 3);

    let mut result: u32 = 1 << 15;
    let major: u32 = parts[0].parse().unwrap();
    assert!(major <= 99);
    result |= (major / 10) << 28;
    result |= (major % 10) << 24;
    if parts.len() >= 2 {
        let minor: u32 = parts[1].parse().unwrap();
        assert!(minor <= 9);
        result |= minor << 20;
    }
    if parts.len() == 3 {
        let bug_fix: u32 = parts[2].parse().unwrap();
        assert!(bug_fix <= 9);
        result |= bug_fix << 16;
    }
    result
}

fn CFBundleCopyBundleURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    let url: CFURLRef = msg![env; bundle bundleURL];
    msg![env; url copy]
}

fn CFBundleCopyExecutableURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    let url: id = msg![env; bundle executableURL];
    msg![env; url copy]
}

fn CFBundleCopyResourcesDirectoryURL(env: &mut Environment, bundle: CFBundleRef) -> CFURLRef {
    let url: CFURLRef = msg![env; bundle resourceURL];
    msg![env; url copy]
}

fn CFBundleCopyResourceURL(
    env: &mut Environment,
    bundle: CFBundleRef,
    resource_name: CFStringRef,
    resource_type: CFStringRef,
    sub_dir_name: CFStringRef,
) -> CFURLRef {
    let url: CFURLRef = msg![env; bundle URLForResource:resource_name
                                          withExtension:resource_type
                                           subdirectory:sub_dir_name];
    msg![env; url copy]
}

fn CFBundleCopyResourceURLForLocalization(
    env: &mut Environment,
    bundle: CFBundleRef,
    resource_name: CFStringRef,
    resource_type: CFStringRef,
    sub_dir_name: CFStringRef,
    localization_name: CFStringRef,
) -> CFURLRef {
    // Look in the requested localization's .lproj first, then fall back to
    // the normal resource lookup.
    if localization_name != nil {
        let loc = ns_string::to_rust_string(env, localization_name);
        let lproj_dir = if sub_dir_name != nil {
            let sub_dir = ns_string::to_rust_string(env, sub_dir_name);
            format!("{}/{}.lproj", sub_dir, loc)
        } else {
            format!("{}.lproj", loc)
        };
        let lproj_dir = ns_string::from_rust_string(env, lproj_dir);
        let url: CFURLRef = msg![env; bundle URLForResource:resource_name
                                              withExtension:resource_type
                                               subdirectory:lproj_dir];
        release(env, lproj_dir);
        if url != nil {
            return msg![env; url copy];
        }
    }
    CFBundleCopyResourceURL(env, bundle, resource_name, resource_type, sub_dir_name)
}

pub fn CFBundleCopyBundleLocalizations(env: &mut Environment, bundle: CFBundleRef) -> CFArrayRef {
    let bundle_localizations = env
        .objc
        .borrow_mut::<NSBundleHostObject>(bundle)
        .bundle
        .as_ref()
        .unwrap_or(&env.bundle)
        .bundle_localizations()
        .iter()
        .map(|value| value.as_string().unwrap().to_string())
        .collect::<Vec<String>>();
    let guest_bundle_localizations = bundle_localizations
        .iter()
        .map(|loc| ns_string::from_rust_string(env, loc.to_owned()))
        .collect::<Vec<id>>();
    let loc_array = ns_array::from_vec(env, guest_bundle_localizations);
    log_dbg!(
        "CFBundleCopyBundleLocalizations({:?}) => {:?} ({})",
        bundle,
        loc_array,
        bundle_localizations.join(", ")
    );
    loc_array
}

pub fn CFBundleCopyPreferredLocalizationsFromArray(
    env: &mut Environment,
    loc_array: CFArrayRef,
) -> CFArrayRef {
    let mut result = Vec::new();

    let preferred_languages: id = msg_class![env; NSLocale preferredLanguages];

    // Check if the user's preferred languages are in loc_array
    let loc_count: NSUInteger = msg![env; loc_array count];
    let pref_loc_count: NSUInteger = msg![env; preferred_languages count];
    for loc_index in 0..loc_count {
        let loc: id = msg![env; loc_array objectAtIndex:loc_index];
        for pref_loc_index in 0..pref_loc_count {
            let pref_loc: id = msg![env; preferred_languages objectAtIndex:pref_loc_index];
            let equal: bool = msg![env; loc isEqualToString:pref_loc];
            if equal {
                // If one of them is, add it to the array
                result.push(loc);
                retain(env, loc);
                break;
            }
        }
    }

    if loc_count > 0 {
        // Add the first element as fallback
        let first_loc: id = msg![env; loc_array objectAtIndex:(0 as NSUInteger)];
        result.push(first_loc);
        retain(env, first_loc);
    } else {
        // Behaviour was verified on macOS
        let en_loc = ns_string::get_static_str(env, "en");
        result.push(en_loc);
    };

    let result = ns_array::from_vec(env, result);
    log_dbg!(
        "CFBundleCopyPreferredLocalizationsFromArray({:?}) => {:?}",
        loc_array,
        result
    );
    result
}

fn CFBundleCopyLocalizedString(
    env: &mut Environment,
    bundle: CFBundleRef,
    key: CFStringRef,
    value: CFStringRef,
    table_name: CFStringRef,
) -> CFStringRef {
    let res = msg![env; bundle localizedStringForKey:key value:value table:table_name];
    msg![env; res copy]
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CFBundleGetMainBundle()),
    export_c_func!(CFBundleCreate(_, _)),
    export_c_func!(CFBundleGetIdentifier(_)),
    export_c_func!(CFBundleCopyBundleIdentifier(_)),
    export_c_func!(CFBundleCopyShortVersionString(_)),
    export_c_func!(CFBundleCopyBundleDevelopmentRegion(_)),
    export_c_func!(CFBundleCopyDisplayName(_)),
    export_c_func!(CFBundleCopyHelpBookName(_)),
    export_c_func!(CFBundleCopyResourceURLsOfType(_, _, _)),
    export_c_func!(CFBundleCopyInfoDictionaryForURL(_)),
    export_c_func!(CFBundleCopyInfoDictionaryInDirectory(_)),
    export_c_func!(CFBundleCopyResourceURLInDirectory(_, _, _, _)),
    export_c_func!(CFBundleGetInfoDictionary(_)),
    export_c_func!(CFBundleGetValueForInfoDictionaryKey(_, _)),
    export_c_func!(CFBundleGetVersionNumber(_)),
    export_c_func!(CFBundleCopyBundleURL(_)),
    export_c_func!(CFBundleCopyExecutableURL(_)),
    export_c_func!(CFBundleCopyResourcesDirectoryURL(_)),
    export_c_func!(CFBundleCopyResourceURL(_, _, _, _)),
    export_c_func!(CFBundleCopyResourceURLForLocalization(_, _, _, _, _)),
    export_c_func!(CFBundleCopyBundleLocalizations(_)),
    export_c_func!(CFBundleCopyPreferredLocalizationsFromArray(_)),
    export_c_func!(CFBundleCopyLocalizedString(_, _, _, _)),
];
