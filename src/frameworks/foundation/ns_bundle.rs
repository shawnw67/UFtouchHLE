/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSBundle`.

mod strings;

use super::{ns_string, NSUInteger};
use crate::bundle::Bundle;
use crate::frameworks::core_foundation::cf_bundle::{
    CFBundleCopyBundleLocalizations, CFBundleCopyPreferredLocalizationsFromArray,
};
use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::fs::GuestPath;
use crate::mem::ConstVoidPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, Class, ClassExports,
    HostObject, NSZonePtr,
};
use crate::Environment;
use std::collections::{HashMap, HashSet};

fn bundle_child_path(env: &mut Environment, bundle: id, child: &str, directory: bool) -> id {
    let root: id = msg![env; bundle bundlePath];
    if root == nil {
        return nil;
    }
    let root = to_rust_string(env, root);
    let path = format!("{}/{child}", root.trim_end_matches('/'));
    let exists = if directory {
        env.fs.is_dir(GuestPath::new(&path))
    } else {
        env.fs.is_file(GuestPath::new(&path))
    };
    if !exists {
        return nil;
    }
    let path = from_rust_string(env, path);
    autorelease(env, path)
}

fn bundle_child_url(env: &mut Environment, bundle: id, child: &str, directory: bool) -> id {
    let path = bundle_child_path(env, bundle, child, directory);
    if path == nil {
        return nil;
    }
    msg_class![env; NSURL fileURLWithPath:path isDirectory:directory]
}

// Should be ISO 639-1 (or ISO 639-2) compliant
// Legacy projects use language names while newer ones use language code lprojs
// TODO: complete this list or use some crate for mapping
pub(crate) const LANG_ID_TO_LANG_PROJ: &[(&str, &[&str])] = &[
    ("da", &["Danish.lproj", "da.lproj"]),
    ("nl", &["Dutch.lproj", "nl.lproj"]),
    ("en", &["English.lproj", "en.lproj"]),
    ("fi", &["Finnish.lproj", "fi.lproj"]),
    ("fr", &["French.lproj", "fr.lproj"]),
    ("de", &["German.lproj", "de.lproj"]),
    ("it", &["Italian.lproj", "it.lproj"]),
    ("ja", &["Japanese.lproj", "ja.lproj"]),
    ("ko", &["Korean.lproj", "ko.lproj"]),
    ("no", &["Norwegian.lproj", "no.lproj"]),
    ("pt", &["Portuguese.lproj", "pt.lproj"]),
    ("ru", &["Russian.lproj", "ru.lproj"]),
    ("es", &["Spanish.lproj", "es.lproj"]),
    ("sv", &["Swedish.lproj", "sv.lproj"]),
    ("zh-Hans", &["zh-Hans.lproj", "zh_CN.lproj"]),
    ("zh-Hant", &["zh-Hant.lproj", "zh_TW.lproj"]),
];

#[derive(Default)]
pub struct State {
    main_bundle: Option<id>,
    localization_tables: HashMap<id, id>, // NSString* to NSDictionary*
}

pub struct NSBundleHostObject {
    /// If this is [None], the [Bundle] used is the main bundle's, which is
    /// stored in [crate::Environment], not here. This is also the case for
    /// other bundles created with `initWithPath:` (usually resource bundles
    /// inside the main bundle), which is an approximation.
    pub bundle: Option<Bundle>,
    /// NSString with bundle path.
    bundle_path: id,
    /// NSString with bundle identifier.
    bundle_identifier: id,
    /// NSURL with bundle path. [None] if not created yet.
    bundle_url: Option<id>,
    /// `NSDictionary*` for the `Info.plist` content. [None] if not created yet.
    info_dictionary: Option<id>,
}
impl HostObject for NSBundleHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSBundle: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = NSBundleHostObject {
        bundle: None,
        bundle_path: nil,
        bundle_identifier: nil,
        bundle_url: None,
        info_dictionary: None,
    };
    env.objc.alloc_object(this, Box::new(host_object), &mut env.mem)
}

+ (id)bundleWithPath:(id)path { // NSString*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithPath:path];
    autorelease(env, new)
}

+ (id)bundleWithURL:(id)url { // NSURL*
    if url == nil { return nil; }
    let is_file: bool = msg![env; url isFileURL];
    if !is_file { return nil; }
    let path: id = msg![env; url path];
    msg![env; this bundleWithPath:path]
}

+ (id)mainBundle {
    if let Some(bundle) = env.framework_state.foundation.ns_bundle.main_bundle {
        bundle
    } else {
        let new = msg_class![env; _Podroid_NSBundle_Static alloc];
        env.framework_state.foundation.ns_bundle.main_bundle = Some(new);
        new
   }
}

+ (id)preferredLocalizationsFromArray:(id)localizations_array { // NSArray<NSString *> *
    let preferredLocalizations = CFBundleCopyPreferredLocalizationsFromArray(env, localizations_array);
    autorelease(env, preferredLocalizations)
}

- (id)initWithPath:(id)path { // NSString*
    // Only directories can be bundles.
    let mut is_dir = false;
    let exists: bool = if path == nil {
        false
    } else {
        let file_manager: id = msg_class![env; NSFileManager defaultManager];
        let is_dir_ptr = env.mem.alloc_and_write(false);
        let exists: bool = msg![env; file_manager fileExistsAtPath:path isDirectory:is_dir_ptr];
        is_dir = env.mem.read(is_dir_ptr);
        env.mem.free(is_dir_ptr.cast());
        exists
    };
    if !exists || !is_dir {
        log!(
            "Warning: [NSBundle initWithPath:{:?}]: not a directory, returning nil",
            path
        );
        release(env, this);
        return nil;
    }

    let path: id = msg![env; path copy];
    env.objc.borrow_mut::<NSBundleHostObject>(this).bundle_path = path;
    let info_dict: id = msg![env; this infoDictionary];
    if info_dict != nil {
        let key = ns_string::get_static_str(env, "CFBundleIdentifier");
        let identifier: id = msg![env; info_dict objectForKey:key];
        retain(env, identifier);
        env.objc.borrow_mut::<NSBundleHostObject>(this).bundle_identifier = identifier;
    }
    this
}

- (())dealloc {
    let &NSBundleHostObject {
        bundle: _,
        bundle_path,
        bundle_identifier,
        bundle_url,
        info_dictionary,
    } = env.objc.borrow(this);
    // The main bundle is never deallocated, so these were all retained by
    // initWithPath:.
    release(env, bundle_path);
    release(env, bundle_identifier);
    if let Some(bundle_url) = bundle_url {
        release(env, bundle_url);
    }
    if let Some(info_dictionary) = info_dictionary {
        release(env, info_dictionary);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)bundlePath {
    env.objc.borrow::<NSBundleHostObject>(this).bundle_path
}
- (id)bundleIdentifier {
    env.objc.borrow::<NSBundleHostObject>(this).bundle_identifier
}
- (id)bundleURL {
    if let Some(url) = env.objc.borrow::<NSBundleHostObject>(this).bundle_url {
        url
    } else {
        let bundle_path: id = msg![env; this bundlePath];
        let new: id = msg_class![env; NSURL alloc];
        let new: id = msg![env; new initFileURLWithPath:bundle_path];
        env.objc.borrow_mut::<NSBundleHostObject>(this).bundle_url = Some(new);
        new
    }
}

- (id)loadNibNamed:(id)name // NSString*
             owner:(id)owner
           options:(id)options { // NSDictionary<UINibOptionsKey, id> *
    if options != nil {
        let options_count: NSUInteger = msg![env; options count];
        // TODO: support options
        assert_eq!(options_count, 0);
    }

    let nib : id = msg_class![env; UINib nibWithNibName:name bundle:this];
    let arr = msg![env; nib instantiateWithOwner:owner options:nil];

    // Need to filter out the File's Owner or any proxy objects
    let ui_proxy_class: Class = msg_class![env; UIProxyObject class];
    let res = msg_class![env; NSMutableArray new];
    let count: NSUInteger = msg![env; arr count];
    for i in 0..count {
        let curr_object: id = msg![env; arr objectAtIndex:i];
        if curr_object == owner || msg![env; curr_object isKindOfClass:ui_proxy_class] {
            continue;
        }
        () = msg![env; res addObject:curr_object];
    }
    let res_imm = msg![env; res copy];
    release(env, res);
    autorelease(env, res_imm)
}

- (id)resourcePath {
    // This seems to be the same as the bundle path. The iPhone OS bundle
    // structure is a lot flatter than the macOS one.
    msg![env; this bundlePath]
}
- (id)resourceURL {
    // This seems to be the same as the bundle path. The iPhone OS bundle
    // structure is a lot flatter than the macOS one.
    msg![env; this bundleURL]
}

- (id)privateFrameworksURL { bundle_child_url(env, this, "Frameworks", true) }
- (id)sharedFrameworksURL { bundle_child_url(env, this, "SharedFrameworks", true) }
- (id)sharedSupportURL { bundle_child_url(env, this, "SharedSupport", true) }
- (id)builtInPlugInsURL { bundle_child_url(env, this, "PlugIns", true) }
- (id)pathForAuxiliaryExecutable:(id)name {
    if name == nil { return nil; }
    let name = to_rust_string(env, name);
    bundle_child_path(env, this, &name, false)
}
- (id)URLForAuxiliaryExecutable:(id)name {
    if name == nil { return nil; }
    let name = to_rust_string(env, name);
    bundle_child_url(env, this, &name, false)
}

- (id)executablePath {
    let exec_path_str = env.bundle.executable_path().as_str().to_string();
    let exec_path = from_rust_string(env, exec_path_str);
    autorelease(env, exec_path)
}
- (id)executableURL {
    // TODO: cache result
    let exec_path: id = msg![env; this executablePath];
    msg_class![env; NSURL fileURLWithPath:exec_path]
}

- (id)pathForResource:(id)name // NSString*
               ofType:(id)extension // NSString*
          inDirectory:(id)directory { // NSString*
    assert!(name != nil); // TODO

    // TODO: cache result of lookups

    let path = path_for_resource_helper(env, this, name, nil, directory, extension);
    if path != nil {
        return path
    }

    // Try preferred languages in order of preference
    // TODO: Support both Region-specific and Language-specific
    // localized resources
    let langs: id = msg_class![env; NSLocale preferredLanguages];
    let lang_count: NSUInteger = msg![env; langs count];
    let mut unknown_codes = HashSet::new();
    for i in 0..lang_count {
        let lang_code: id = msg![env; langs objectAtIndex:i];
        let lang_code = ns_string::to_rust_string(env, lang_code); // TODO: avoid copy
        if let Some(&(_, lprojs)) = LANG_ID_TO_LANG_PROJ.iter().find(|&&(code, _)| code == lang_code) {
            for lproj in lprojs {
                let lproj: id = ns_string::get_static_str(env, lproj);
                let localized_path = path_for_resource_helper(env, this, name, lproj, directory, extension);
                if localized_path != nil {
                    return localized_path;
                }
            }
        } else {
            unknown_codes.insert(lang_code);
        }
    }

    // TODO: Support look up for device specific resources, e.g. ~iphone

    // As a last resort, fallback to English
    // TODO: fallback to a development language (CFBundleDevelopmentRegion from
    // Info.plist)
    if !unknown_codes.is_empty() {
        log!("TODO: language codes {:?} aren't mapped to a language name, falling back to English", unknown_codes);
    }

    for lproj in ["English.lproj", "en.lproj"] {
        let lproj: id = ns_string::get_static_str(env, lproj);
        let path = path_for_resource_helper(env, this, name, lproj, directory, extension);
        if path != nil {
            return path;
        }
    }
    nil
}
- (id)pathForResource:(id)name // NSString*
               ofType:(id)extension { // NSString*
    msg![env; this pathForResource:name ofType:extension inDirectory:nil]
}

- (id)pathsForResourcesOfType:(id)extension inDirectory:(id)directory {
    let root: id = msg![env; this resourcePath];
    let root: id = if directory == nil { root } else {
        msg![env; root stringByAppendingPathComponent:directory]
    };
    let root = to_rust_string(env, root).to_string();
    let extension = if extension == nil { None } else {
        Some(to_rust_string(env, extension).to_string())
    };
    let root = crate::fs::GuestPath::new(&root);
    let paths = env.fs.enumerate_recursive(root).unwrap_or_default();
    let paths: Vec<_> = paths.into_iter().filter_map(|relative| {
        let path = root.join(relative.as_str());
        if !env.fs.is_file(&path) { return None; }
        if let Some(extension) = &extension {
            if !relative.as_str().ends_with(&format!(".{extension}")) { return None; }
        }
        Some(path.as_str().to_string())
    }).collect();
    let paths = paths.into_iter().map(|path| from_rust_string(env, path)).collect();
    let array = super::ns_array::from_vec(env, paths);
    autorelease(env, array)
}
- (id)URLForResource:(id)name // NSString*
       withExtension:(id)extension // NSString *
        subdirectory:(id)subpath { // NSString *
    let path_string: id = msg![env; this pathForResource:name
                                                 ofType:extension
                                            inDirectory:subpath];
    if path_string == nil {
        return nil;
    }
    let path_url: id = msg_class![env; NSURL alloc];
    let path_url: id = msg![env; path_url initFileURLWithPath:path_string];
    autorelease(env, path_url)
}
- (id)URLForResource:(id)name // NSString*
       withExtension:(id)extension { // NSString *
    msg![env; this URLForResource:name withExtension:extension subdirectory:nil]
}

- (id)localizedStringForKey:(id)key
                      value:(id)value
                      table:(id)table_name {
    log_dbg!("localizedStringForKey key:'{}' value:'{}' table:'{}'",
            if key == nil { std::borrow::Cow::from("(null)") } else { ns_string::to_rust_string(env, key) },
            if value == nil { std::borrow::Cow::from("(null)") } else { ns_string::to_rust_string(env, value) },
            if table_name == nil { std::borrow::Cow::from("(null)") } else { ns_string::to_rust_string(env, table_name) }
    );
    let empty_str: id = ns_string::get_static_str(env, "");
    if key == nil {
        if value == nil {
            return empty_str;
        }
        return value;
    }
    let name = if table_name == nil || msg![env; table_name isEqualToString:empty_str] {
        ns_string::get_static_str(env, "Localizable")
    } else {
        table_name
    };
    // Only the main bundle's tables are cached.
    let is_main_bundle = Some(this) == env.framework_state.foundation.ns_bundle.main_bundle;
    let dict = if !is_main_bundle {
        let extension = ns_string::get_static_str(env, "strings");
        let dict_url: id = msg![env; this URLForResource:name withExtension:extension];
        if dict_url == nil {
            nil
        } else {
            let dict: id = msg_class![env; NSDictionary dictionaryWithContentsOfURL:dict_url];
            if dict != nil {
                dict
            } else {
                load_strings_as_standard_format(env, dict_url)
            }
        }
    } else if let Some(&table_dict) = env.framework_state.foundation.ns_bundle.localization_tables.get(&name) {
        table_dict
    } else {
        let extension = ns_string::get_static_str(env, "strings");
        let dict_url: id = msg![env; this URLForResource:name withExtension:extension];
        if dict_url == nil {
            log!("Warning: Unable to locate localization table named '{}', caching as nil", to_rust_string(env, name));
            retain(env, name);
            env.framework_state.foundation.ns_bundle.localization_tables.insert(name, nil);
            nil
        } else {
            let dict = {
                // First, try to load as property list format
                let dict: id = msg_class![env; NSDictionary dictionaryWithContentsOfURL:dict_url];
                if dict != nil {
                    dict
                } else {
                    // Else, load as standard format
                    load_strings_as_standard_format(env, dict_url)
                }
            };
            retain(env, name);
            retain(env, dict);
            env.framework_state.foundation.ns_bundle.localization_tables.insert(name, dict);
            dict
        }
    };
    let res: id = msg![env; dict objectForKey:key];
    if res == nil {
        if value == nil || msg![env; value isEqualToString:empty_str] {
            return key;
        }
        return value;
    }
    log_dbg!("localizedStringForKey res => {:?}", ns_string::to_rust_string(env, res));
    res
}

- (id)infoDictionary {
    let &NSBundleHostObject {
        bundle_path,
        info_dictionary,
        ..
    } = env.objc.borrow(this);
    if let Some(dict) = info_dictionary {
        return dict;
    }

    let plist_path = ns_string::get_static_str(env, "Info.plist");
    let plist_path: id = msg![env; bundle_path stringByAppendingPathComponent:plist_path];
    let dict: id = msg_class![env; NSDictionary alloc];
    let dict: id = msg![env; dict initWithContentsOfFile:plist_path];
    env.objc.borrow_mut::<NSBundleHostObject>(this).info_dictionary = Some(dict);
    dict
}

- (id)objectForInfoDictionaryKey:(id)key {
    let info_dict = msg![env; this infoDictionary];
    // TODO: return the localized value of a key when one is available
    msg![env; info_dict objectForKey:key]
}

- (id)localizations {
    let localizations = CFBundleCopyBundleLocalizations(env, this);
    autorelease(env, localizations)
}

- (id)preferredLocalizations {
    let loc_array = CFBundleCopyBundleLocalizations(env, this);

    let preferred_localizations = CFBundleCopyPreferredLocalizationsFromArray(env, loc_array);
    autorelease(env, preferred_localizations)
}

// TODO: constructors, more accessors

@end

// Private static implementation of NSBundle, used for the main bundle
// allocation. This is needed because some apps (e.g. Ovenbreak)
// attempts to release it.
@implementation _Podroid_NSBundle_Static: NSBundle

+ (id)allocWithZone:(NSZonePtr)_zone {
    let bundle_path = env.bundle.bundle_path().as_str().to_string();
    let bundle_path = ns_string::from_rust_string(env, bundle_path);
    let bundle_identifier = env.bundle.bundle_identifier().to_string();
    let bundle_identifier = ns_string::from_rust_string(env, bundle_identifier);
    let host_object = NSBundleHostObject {
        bundle: None,
        bundle_path,
        bundle_identifier,
        bundle_url: None,
        info_dictionary: None,
    };
    env.objc.alloc_object(
        this,
        Box::new(host_object),
        &mut env.mem
    )
}

- (id) retain { this }
- (()) release {}
- (id) autorelease { this }

@end

};

fn path_for_resource_helper(
    env: &mut Environment,
    bundle: id,
    name: id,
    lproj: id,
    directory: id,
    extension: id,
) -> id {
    let mut path: id = msg![env; bundle resourcePath];
    if lproj != nil {
        path = msg![env; path stringByAppendingPathComponent:lproj];
    }
    if directory != nil {
        path = msg![env; path stringByAppendingPathComponent:directory];
    }
    path = msg![env; path stringByAppendingPathComponent:name];
    if extension != nil {
        path = msg![env; path stringByAppendingPathExtension:extension];
    }
    let file_manager: id = msg_class![env; NSFileManager defaultManager];
    let file_exists: bool = msg![env; file_manager fileExistsAtPath:path];
    if file_exists {
        return path;
    }
    nil
}

/// Helper function which loads a `strings` file from an `dict_url` and parses
/// it as standard format - one or more key-value pairs along with optional
/// comments. Returned dictionary is autoreleased, so it's a responsibility of
/// the caller to retain it.
/// [String Resources reference](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/LoadingResources/Strings/Strings.html#//apple_ref/doc/uid/10000051i-CH6)
pub(crate) fn load_strings_as_standard_format(env: &mut Environment, dict_url: id) -> id {
    let res: id = msg_class![env; NSMutableDictionary new];
    // TODO: avoid loading whole file in memory
    let data: id = msg_class![env; NSData dataWithContentsOfURL:dict_url];
    if data == nil {
        release(env, res);
        return nil;
    }
    let length: NSUInteger = msg![env; data length];
    if length == 0 {
        return autorelease(env, res);
    }
    let bytes: ConstVoidPtr = msg![env; data bytes];
    let input = env.mem.bytes_at(bytes.cast(), length);
    let utf16 = input.starts_with(&[0xfe, 0xff]) || input.starts_with(&[0xff, 0xfe]);
    let encoding = if utf16 {
        crate::frameworks::core_foundation::cf_string::kCFStringEncodingUTF16
    } else {
        crate::frameworks::core_foundation::cf_string::kCFStringEncodingUTF8
    };
    let Ok(length) = i32::try_from(length) else {
        release(env, res);
        return nil;
    };
    let strings_str = crate::frameworks::core_foundation::cf_string::CFStringCreateWithBytes(
        env,
        crate::frameworks::core_foundation::cf_allocator::kCFAllocatorDefault,
        bytes.cast(),
        length,
        encoding,
        true,
    );
    if strings_str == nil {
        release(env, res);
        return nil;
    }

    let mut units = Vec::new();
    ns_string::for_each_code_unit(env, strings_str, |_, unit| units.push(unit));
    release(env, strings_str);
    let Some(entries) = strings::parse(&units) else {
        release(env, res);
        return nil;
    };
    for (key, value) in entries {
        let key = ns_string::from_u16_vec(env, key);
        let value = ns_string::from_u16_vec(env, value);
        let _: () = msg![env; res setObject:value forKey:key];
        release(env, key);
        release(env, value);
    }
    let result: id = msg![env; res copy];
    release(env, res);
    autorelease(env, result)
}
