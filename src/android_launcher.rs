/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Native functions for the Android launcher (LauncherActivity.java).
//!
//! The launcher is a normal Android activity, so it can use the whole screen
//! at its real resolution, unlike the app picker
//! (src/environment/app_picker.rs), which is an emulated iPhone OS app. It
//! runs in a different process from touchHLE proper, where SDL isn't
//! initialized, so nothing here may use SDL (e.g.
//! [crate::paths::user_data_base_path]).

use crate::bundle::Bundle;
use crate::fs::BundleData;
use jni_sys::{jbyteArray, jclass, jobject, jsize, jstring, JNIEnv};
use std::ffi::{c_char, c_int, c_void, CStr, CString, OsStr};
use std::path::Path;

unsafe fn get_string(env: *mut JNIEnv, string: jstring) -> String {
    let chars = ((**env).GetStringUTFChars.unwrap())(env, string, std::ptr::null_mut());
    if chars.is_null() {
        return String::new();
    }
    let result = CStr::from_ptr(chars).to_string_lossy().into_owned();
    ((**env).ReleaseStringUTFChars.unwrap())(env, string, chars);
    result
}

/// Strings are returned as UTF-8 byte arrays, because JNI's `NewStringUTF`
/// uses "modified UTF-8", which can't represent e.g. emoji the normal way.
unsafe fn new_byte_array(env: *mut JNIEnv, bytes: &[u8]) -> jbyteArray {
    let len: jsize = bytes.len().try_into().unwrap();
    let array = ((**env).NewByteArray.unwrap())(env, len);
    if !array.is_null() {
        ((**env).SetByteArrayRegion.unwrap())(env, array, 0, len, bytes.as_ptr().cast());
    }
    array
}

/// Lists the apps in `apps_dir`. Their icons are written to `icons_dir` as
/// raw premultiplied RGBA pixels. Returns one line per app, with tab-separated
/// fields: path, display name, icon file (or empty), icon width, icon height,
/// bundle identifier (the name of the app's folder in `Podroid_sandbox`).
#[no_mangle]
pub unsafe extern "system" fn Java_org_podroid_android_LauncherActivity_nativeScanApps(
    env: *mut JNIEnv,
    _class: jclass,
    apps_dir: jstring,
    icons_dir: jstring,
    languages: jstring,
) -> jbyteArray {
    crate::log::LOG_TO_FILE.store(false, std::sync::atomic::Ordering::Relaxed);

    let apps_dir = get_string(env, apps_dir);
    let icons_dir = get_string(env, icons_dir);
    let languages: Vec<String> = get_string(env, languages)
        .split(',')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect();

    let result = std::panic::catch_unwind(|| {
        scan_apps(Path::new(&apps_dir), Path::new(&icons_dir), &languages)
    })
    .unwrap_or_default();
    new_byte_array(env, result.as_bytes())
}

fn scan_apps(apps_dir: &Path, icons_dir: &Path, languages: &[String]) -> String {
    let _ = std::fs::create_dir_all(icons_dir);
    let Ok(entries) = std::fs::read_dir(apps_dir) else {
        return String::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension() == Some(OsStr::new("app"))
                || path.extension() == Some(OsStr::new("ipa"))
        })
        .collect();
    paths.sort();

    let mut apps = Vec::new();
    for (i, path) in paths.iter().enumerate() {
        // A broken app shouldn't stop the others from being listed.
        let Ok(Some(line)) = std::panic::catch_unwind(|| scan_app(path, icons_dir, i, languages))
        else {
            log!(
                "Warning: couldn't open app bundle {} (skipping)",
                path.display()
            );
            continue;
        };
        apps.push(line);
    }
    apps.sort_by_key(|(name, _)| name.to_uppercase());
    apps.into_iter()
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Returns the display name (for sorting) and the line for [scan_apps].
fn scan_app(
    path: &Path,
    icons_dir: &Path,
    index: usize,
    languages: &[String],
) -> Option<(String, String)> {
    let (bundle, fs) = BundleData::open_any(path)
        .and_then(|data| Bundle::new_bundle_and_fs_from_host_path(data, true))
        .ok()?;
    let clean = |s: &str| s.replace(['\t', '\n', '\r'], " ");
    let name = clean(&bundle.localized_display_name(&fs, languages));

    let mut icon_fields = "\t\t0\t0".to_string();
    if let Ok(icon) = bundle.load_icon(&fs) {
        let (width, height) = icon.dimensions();
        let pixels: Vec<u8> = icon
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| {
                let a = p[3] as u32;
                let premultiply = |c: u8| ((c as u32 * a + 127) / 255) as u8;
                [
                    premultiply(p[0]),
                    premultiply(p[1]),
                    premultiply(p[2]),
                    p[3],
                ]
            })
            .collect();
        let icon_path = icons_dir.join(format!("{index}.rgba"));
        if std::fs::write(&icon_path, pixels).is_ok() {
            icon_fields = format!("\t{}\t{}\t{}", icon_path.display(), width, height);
        }
    }

    let line = format!(
        "{}\t{}{}\t{}",
        clean(&path.to_string_lossy()),
        name,
        icon_fields,
        clean(bundle.bundle_identifier()),
    );
    Some((name, line))
}

#[link(name = "android")]
extern "C" {
    fn AAssetManager_fromJava(env: *mut JNIEnv, asset_manager: jobject) -> *mut c_void;
    fn AAssetManager_open(
        manager: *mut c_void,
        filename: *const c_char,
        mode: c_int,
    ) -> *mut c_void;
    fn AAsset_getLength64(asset: *mut c_void) -> i64;
    fn AAsset_read(asset: *mut c_void, buf: *mut c_void, count: usize) -> c_int;
    fn AAsset_close(asset: *mut c_void);
}

/// Reads a file from the APK's assets (where touchHLE's bundled files are).
unsafe fn read_asset(manager: *mut c_void, path: &str) -> String {
    let path = CString::new(path).unwrap();
    const AASSET_MODE_BUFFER: c_int = 3;
    let asset = AAssetManager_open(manager, path.as_ptr(), AASSET_MODE_BUFFER);
    assert!(!asset.is_null(), "Couldn't open asset {path:?}");
    let mut bytes = vec![0u8; AAsset_getLength64(asset).try_into().unwrap()];
    let mut done = 0;
    while done < bytes.len() {
        let count = AAsset_read(asset, bytes[done..].as_mut_ptr().cast(), bytes.len() - done);
        if count <= 0 {
            break;
        }
        done += count as usize;
    }
    AAsset_close(asset);
    bytes.truncate(done);
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The copyright, authorship and license information (`--copyright`).
/// `assets` is the `AssetManager`.
#[no_mangle]
pub unsafe extern "system" fn Java_org_podroid_android_LauncherActivity_nativeCopyrightText(
    env: *mut JNIEnv,
    _class: jclass,
    assets: jobject,
) -> jbyteArray {
    let manager = AAssetManager_fromJava(env, assets);
    let text = std::panic::catch_unwind(|| {
        crate::licenses::get_text_with_reader(&|path| unsafe { read_asset(manager, path) })
    })
    .unwrap_or_else(|_| "(Couldn't load the license text.)".to_string());
    new_byte_array(env, text.as_bytes())
}

/// The version, as shown in the app picker.
#[no_mangle]
pub unsafe extern "system" fn Java_org_podroid_android_LauncherActivity_nativeVersion(
    env: *mut JNIEnv,
    _class: jclass,
) -> jbyteArray {
    new_byte_array(env, crate::VERSION.as_bytes())
}

/// Copies a Java `byte[]`, which may be null.
unsafe fn get_byte_array(env: *mut JNIEnv, array: jbyteArray) -> Option<Vec<u8>> {
    if array.is_null() {
        return None;
    }
    let len = ((**env).GetArrayLength.unwrap())(env, array);
    let mut bytes = vec![0u8; len.try_into().unwrap()];
    ((**env).GetByteArrayRegion.unwrap())(env, array, 0, len, bytes.as_mut_ptr().cast());
    Some(bytes)
}

/// Converts a Monster Hunter Portable 2nd G save from the PSP for the iOS
/// version (see `mhp2g-save-tools/`). `file` is `MHP2NDG.BIN`, `sfo` the
/// `PARAM.SFO` next to it (or null) and `current` the app's current
/// `savedata.dat` (or null), whose settings are kept.
///
/// On success, returns 0, the length of the next part as a little-endian
/// u32, the names of the characters in the save (UTF-8, one per line) and the
/// new `savedata.dat`. On failure, returns 1 and an error message (UTF-8).
#[no_mangle]
pub unsafe extern "system" fn Java_org_podroid_android_LauncherActivity_nativeImportPspSave(
    env: *mut JNIEnv,
    _class: jclass,
    file: jbyteArray,
    sfo: jbyteArray,
    current: jbyteArray,
) -> jbyteArray {
    let file = get_byte_array(env, file).unwrap_or_default();
    let sfo = get_byte_array(env, sfo);
    let current = get_byte_array(env, current);
    let result = std::panic::catch_unwind(|| {
        mhp2g_save::import_psp(&file, sfo.as_deref(), current.as_deref()).map_err(|e| e.to_string())
    })
    .unwrap_or_else(|_| Err("internal error".to_string()));
    let out = match result {
        Ok(import) => {
            let names = import.characters.join("\n");
            let mut out = vec![0];
            out.extend_from_slice(&u32::try_from(names.len()).unwrap().to_le_bytes());
            out.extend_from_slice(names.as_bytes());
            out.extend_from_slice(&import.file);
            out
        }
        Err(message) => [&[1], message.as_bytes()].concat(),
    };
    new_byte_array(env, &out)
}

/// Request an audio snapshot from the game's UI thread without accessing
/// emulated state or OpenAL from JNI.
#[no_mangle]
pub extern "system" fn Java_org_podroid_android_MainActivity_nativeMarkAudioNoise(
    _env: *mut JNIEnv,
    _this: jobject,
) {
    crate::audio::sound_trace::request_capture();
}
