/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Bridge from the emulated camera/mic stack (`AVCaptureDevice`,
//! `UIImagePickerController`, `AVAudioRecorder`, RemoteIO input) to *real*
//! host hardware on Android.
//!
//! All calls go through hand-rolled JNI (no `jni` crate dependency) into
//! static methods on `org.podroid.android.HostMedia` — see that file for
//! the Android side (Camera2 still capture, AudioRecord streaming).
//!
//! On other platforms (or when the permission is denied / hardware absent)
//! every function here returns the "not available" answer, so emulated apps
//! see an honest "no camera / no mic" device instead of a fake one.
//!
//! The `HostMedia` class reference and the method ID of every bridge method
//! are resolved once at startup by `populate_jni_cache`, on the real
//! SDLThread stack: guest code later calls this bridge from a coroutine
//! stack, where JNI *lookups* are not reliable (see that function).

// The camera half of the bridge has no callers yet.
#[allow(dead_code)]
#[cfg(target_os = "android")]
mod imp {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};
    use std::sync::OnceLock;

    extern "C" {
        // Exported by libSDL2.so on Android (SDL_system.h).
        fn SDL_AndroidGetJNIEnv() -> *mut c_void;
    }

    /// JNI function-table slot indices. The table starts with 4 reserved
    /// pointers, hence every index is `4 + <ordinal in jni.h>`.
    mod slots {
        pub const FIND_CLASS: usize = 6;
        pub const EXCEPTION_OCCURRED: usize = 15;
        pub const EXCEPTION_CLEAR: usize = 17;
        pub const NEW_GLOBAL_REF: usize = 21;
        pub const DELETE_LOCAL_REF: usize = 23;
        pub const GET_STATIC_METHOD_ID: usize = 113;
        pub const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
        pub const CALL_STATIC_BOOLEAN_METHOD_A: usize = 119;
        pub const CALL_STATIC_VOID_METHOD_A: usize = 143;
        pub const NEW_STRING_UTF: usize = 167;
        pub const GET_STRING_UTF_CHARS: usize = 169;
        pub const RELEASE_STRING_UTF_CHARS: usize = 170;
        pub const GET_ARRAY_LENGTH: usize = 171;
        pub const NEW_BYTE_ARRAY: usize = 176;
        pub const GET_BYTE_ARRAY_ELEMENTS: usize = 184;
        pub const RELEASE_BYTE_ARRAY_ELEMENTS: usize = 192;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    union JValue {
        l: *mut c_void,
        i: c_int,
        z: u8,
        _pad: u64,
    }

    struct Jni {
        env: *mut c_void,
    }

    impl Jni {
        fn attach() -> Option<Jni> {
            unsafe {
                let env = SDL_AndroidGetJNIEnv();
                if env.is_null() {
                    return None;
                }
                Some(Jni { env })
            }
        }

        /// Resolve a JNI function pointer from the function table reachable
        /// from `self.env`.
        ///
        /// This MUST go through `crate::android_jni`, which dereferences the
        /// `JNIEnv*` to reach `struct JNINativeInterface_`. Indexing `env`
        /// itself reads bytes that follow the 8-byte `JNIEnv` struct and
        /// returns them as "function pointers", so the very first call jumps
        /// to garbage — usually 0, killing the host process with
        /// `SIGSEGV at address 0x0`.
        ///
        /// [None] means the slot could not be resolved; every caller treats
        /// that as "host hardware unavailable" and returns its no-hardware
        /// answer, which is what these bridges return on other platforms too.
        fn slot<F>(&self, index: usize) -> Option<F> {
            // SAFETY: `self.env` came from SDL_AndroidGetJNIEnv() and was
            // checked for null in `attach()`; `index` is one of the `slots`
            // constants, whose numbers are taken from jni.h.
            unsafe { crate::android_jni::table_fn(self.env, index) }
        }

        fn exception_pending(&self) -> bool {
            let Some(f): Option<unsafe extern "C" fn(*mut c_void) -> *mut c_void> =
                self.slot(slots::EXCEPTION_OCCURRED)
            else {
                return false;
            };
            !unsafe { f(self.env) }.is_null()
        }

        fn clear_exception(&self) {
            let Some(f): Option<unsafe extern "C" fn(*mut c_void)> =
                self.slot(slots::EXCEPTION_CLEAR)
            else {
                return;
            };
            unsafe { f(self.env) }
        }

        fn find_host_media_class(&self) -> Option<*mut c_void> {
            let name = CString::new("org/podroid/android/HostMedia").ok()?;
            let Some(f): Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void> =
                self.slot(slots::FIND_CLASS)
            else {
                return None;
            };
            let class = unsafe { f(self.env, name.as_ptr()) };
            if class.is_null() {
                self.clear_exception();
                return None;
            }
            Some(class)
        }

        fn get_static_method(
            &self,
            class: *mut c_void,
            name: &str,
            sig: &str,
        ) -> Option<*mut c_void> {
            let name = CString::new(name).ok()?;
            let sig = CString::new(sig).ok()?;
            let Some(f): Option<
                unsafe extern "C" fn(
                    *mut c_void,
                    *mut c_void,
                    *const c_char,
                    *const c_char,
                ) -> *mut c_void,
            > = self.slot(slots::GET_STATIC_METHOD_ID) else {
                return None;
            };
            let method = unsafe { f(self.env, class, name.as_ptr(), sig.as_ptr()) };
            if method.is_null() {
                self.clear_exception();
                return None;
            }
            Some(method)
        }

        fn call_static_bool(
            &self,
            class: *mut c_void,
            method: *mut c_void,
            args: &[JValue],
        ) -> bool {
            let Some(f): Option<
                unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const JValue) -> u8,
            > = self.slot(slots::CALL_STATIC_BOOLEAN_METHOD_A) else {
                return false;
            };
            let r = unsafe { f(self.env, class, method, args.as_ptr()) };
            if self.exception_pending() {
                self.clear_exception();
            }
            r != 0
        }

        fn call_static_void(&self, class: *mut c_void, method: *mut c_void, args: &[JValue]) {
            let Some(f): Option<
                unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const JValue),
            > = self.slot(slots::CALL_STATIC_VOID_METHOD_A) else {
                return;
            };
            unsafe { f(self.env, class, method, args.as_ptr()) };
            if self.exception_pending() {
                self.clear_exception();
            }
        }

        fn call_static_object(
            &self,
            class: *mut c_void,
            method: *mut c_void,
            args: &[JValue],
        ) -> *mut c_void {
            let Some(f): Option<
                unsafe extern "C" fn(
                    *mut c_void,
                    *mut c_void,
                    *mut c_void,
                    *const JValue,
                ) -> *mut c_void,
            > = self.slot(slots::CALL_STATIC_OBJECT_METHOD_A) else {
                return std::ptr::null_mut();
            };
            let r = unsafe { f(self.env, class, method, args.as_ptr()) };
            if self.exception_pending() {
                self.clear_exception();
            }
            r
        }

        fn delete_local_ref(&self, obj: *mut c_void) {
            if obj.is_null() {
                return;
            }
            let Some(f): Option<unsafe extern "C" fn(*mut c_void, *mut c_void)> =
                self.slot(slots::DELETE_LOCAL_REF)
            else {
                return;
            };
            unsafe { f(self.env, obj) }
        }

        fn new_global_ref(&self, obj: *mut c_void) -> *mut c_void {
            if obj.is_null() {
                return std::ptr::null_mut();
            }
            let Some(f): Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void> =
                self.slot(slots::NEW_GLOBAL_REF)
            else {
                return std::ptr::null_mut();
            };
            unsafe { f(self.env, obj) }
        }

        /// `HostMedia` as a *global* reference: FindClass returns a local
        /// reference, which dies with the frame that created it, while the
        /// cache below has to stay valid for the whole process lifetime.
        fn host_media_global_class(&self) -> Option<*mut c_void> {
            let local = self.find_host_media_class()?;
            let global = self.new_global_ref(local);
            self.delete_local_ref(local);
            if global.is_null() {
                None
            } else {
                Some(global)
            }
        }

        fn jstring_to_rust(&self, js: *mut c_void) -> Option<String> {
            if js.is_null() {
                return None;
            }
            let Some(get): Option<
                unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> *const c_char,
            > = self.slot(slots::GET_STRING_UTF_CHARS) else {
                return None;
            };
            let Some(release): Option<
                unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_char),
            > = self.slot(slots::RELEASE_STRING_UTF_CHARS) else {
                return None;
            };
            let chars = unsafe { get(self.env, js, std::ptr::null_mut()) };
            if chars.is_null() {
                return None;
            }
            let s = unsafe { std::ffi::CStr::from_ptr(chars) }
                .to_string_lossy()
                .into_owned();
            unsafe { release(self.env, js, chars) };
            Some(s)
        }
    }

    /// The `HostMedia` class (as a global reference) plus the method ID of
    /// every static bridge method, resolved once at startup.
    struct JniCache {
        class: *mut c_void,
        has_camera: *mut c_void,
        take_photo: *mut c_void,
        has_microphone: *mut c_void,
        start_mic: *mut c_void,
        stop_mic: *mut c_void,
        read_mic_chunk: *mut c_void,
    }

    // SAFETY: the class is held as a global reference, which is valid from
    // any thread until the process exits, and jmethodIDs are process-wide
    // lifetime handles, so sharing this cache across threads is sound.
    unsafe impl Send for JniCache {}
    unsafe impl Sync for JniCache {}

    static CACHE: OnceLock<JniCache> = OnceLock::new();

    fn cached() -> Option<&'static JniCache> {
        CACHE.get()
    }

    /// Resolve the `HostMedia` class and every bridge method ID once, on the
    /// real SDLThread stack, before guest emulation begins.
    ///
    /// This is not an optimization. Guest code calls this bridge from a
    /// coroutine stack, and ART resolves `FindClass` through the class loader
    /// of the calling *managed* frame - there is none on a coroutine stack, so
    /// it falls back to the system class loader, which does not know the app's
    /// own classes. A lazy `FindClass("org/podroid/android/HostMedia")` from
    /// guest code therefore returns NULL, and every camera/microphone probe
    /// silently answers "not available" even on a device that has both.
    /// Calling the *resolved* methods from a coroutine stack is fine; only the
    /// lookups need this one. `android_web_view` caches its `MainActivity`
    /// lookups for exactly the same reason.
    ///
    /// Any failure is logged here, the cache stays empty, and the bridge then
    /// reports "not available" - the answer it gives on platforms that have no
    /// host bridge at all.
    pub fn populate_jni_cache() {
        fn method(jni: &Jni, class: *mut c_void, name: &str, sig: &str) -> Option<*mut c_void> {
            let mid = jni.get_static_method(class, name, sig);
            if mid.is_none() {
                log!(
                    "GetStaticMethodID(HostMedia.{}) failed; bridge disabled",
                    name
                );
            }
            mid
        }
        fn resolve(jni: &Jni) -> Option<JniCache> {
            let class = match jni.host_media_global_class() {
                Some(class) => class,
                None => {
                    log!("HostMedia class not found; camera/mic bridge disabled");
                    return None;
                }
            };
            Some(JniCache {
                class,
                has_camera: method(jni, class, "hasCamera", "(Z)Z")?,
                take_photo: method(jni, class, "takePhoto", "(Z)[B")?,
                has_microphone: method(jni, class, "hasMicrophone", "()Z")?,
                start_mic: method(jni, class, "startMic", "()Z")?,
                stop_mic: method(jni, class, "stopMic", "()V")?,
                read_mic_chunk: method(jni, class, "readMicChunk", "()[S")?,
            })
        }
        let Some(jni) = Jni::attach() else {
            log!("No JNIEnv from SDL; camera/microphone bridge disabled");
            return;
        };
        let Some(cache) = resolve(&jni) else {
            return;
        };
        // Ask the Java side what it can see on this device, so a "no camera /
        // no microphone" answer can be attributed from this log alone instead
        // of needing a logcat capture under the HostMedia tag. Not part of the
        // cache: it is a one-shot startup report.
        if let Some(diagnose) =
            jni.get_static_method(cache.class, "diagnose", "()Ljava/lang/String;")
        {
            let text = jni.call_static_object(cache.class, diagnose, &[]);
            if let Some(line) = jni.jstring_to_rust(text) {
                log!("HostMedia: {}", line);
            }
            jni.delete_local_ref(text);
        }
        log!("Native camera/microphone bridge ready");
        let _ = CACHE.set(cache);
    }

    fn bool_arg(v: bool) -> JValue {
        JValue {
            z: if v { 1 } else { 0 },
        }
    }

    pub fn has_camera(front: bool) -> bool {
        let Some(c) = cached() else {
            return false;
        };
        let Some(jni) = Jni::attach() else {
            return false;
        };
        jni.call_static_bool(c.class, c.has_camera, &[bool_arg(front)])
    }

    /// Take a still photo; returns JPEG bytes or None.
    pub fn take_photo(front: bool) -> Option<Vec<u8>> {
        let jni = Jni::attach()?;
        let c = cached()?;
        // Resolve every function pointer up front, so a slot that cannot be
        // resolved bails out before any JNI local reference exists.
        let len_f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int =
            jni.slot(slots::GET_ARRAY_LENGTH)?;
        // GetByteArrayElements(env, array, &isCopy) - the third argument is an
        // *out* parameter for "did JNI copy the bytes?", not a destination
        // buffer.
        let get_f: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> *mut u8 =
            jni.slot(slots::GET_BYTE_ARRAY_ELEMENTS)?;
        let rel_f: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8, c_int) =
            jni.slot(slots::RELEASE_BYTE_ARRAY_ELEMENTS)?;
        let arr = jni.call_static_object(c.class, c.take_photo, &[bool_arg(front)]);
        if arr.is_null() {
            // Java returns null for "permission denied, no such camera, or the
            // capture failed"; HostMedia logs which one to logcat.
            log!("android_media: the host camera returned no photo");
            return None;
        }
        let len = unsafe { len_f(jni.env, arr) };
        if len <= 0 {
            jni.delete_local_ref(arr);
            log!("android_media: the host camera returned an empty photo");
            return None;
        }
        let mut bytes = vec![0u8; len as usize];
        let mut is_copy: u8 = 0;
        let ptr = unsafe { get_f(jni.env, arr, &mut is_copy) };
        if ptr.is_null() {
            jni.delete_local_ref(arr);
            return None;
        }
        if ptr != bytes.as_mut_ptr() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, bytes.as_mut_ptr(), len as usize);
            }
        }
        unsafe { rel_f(jni.env, arr, ptr, 0) };
        jni.delete_local_ref(arr);
        Some(bytes)
    }

    pub fn has_microphone() -> bool {
        let Some(c) = cached() else {
            return false;
        };
        let Some(jni) = Jni::attach() else {
            return false;
        };
        jni.call_static_bool(c.class, c.has_microphone, &[])
    }

    pub fn start_mic() -> bool {
        let Some(c) = cached() else {
            return false;
        };
        let Some(jni) = Jni::attach() else {
            return false;
        };
        let started = jni.call_static_bool(c.class, c.start_mic, &[]);
        if !started {
            // Once per capture session, so this cannot flood the log.
            log!("android_media: the host refused to start microphone capture");
        }
        started
    }

    pub fn stop_mic() {
        let Some(c) = cached() else {
            return;
        };
        let Some(jni) = Jni::attach() else {
            return;
        };
        jni.call_static_void(c.class, c.stop_mic, &[]);
    }

    /// The most recent mic chunk: mono 16-bit LE PCM samples.
    pub fn read_mic_chunk() -> Vec<i16> {
        let Some(c) = cached() else {
            return Vec::new();
        };
        let Some(jni) = Jni::attach() else {
            return Vec::new();
        };
        let bytes = jni.call_static_object(c.class, c.read_mic_chunk, &[]);
        if bytes.is_null() {
            return Vec::new();
        }
        let Some(len_f): Option<unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int> =
            jni.slot(slots::GET_ARRAY_LENGTH)
        else {
            jni.delete_local_ref(bytes);
            return Vec::new();
        };
        // GetShortArrayRegion(env, array, start, len, buf) copies a short[]
        // region straight into our buffer; index 202 in
        // `struct JNINativeInterface_` (see jni.h).
        const GET_SHORT_ARRAY_REGION: usize = 202;
        let Some(region_f): Option<
            unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, c_int, *mut i16),
        > = jni.slot(GET_SHORT_ARRAY_REGION) else {
            jni.delete_local_ref(bytes);
            return Vec::new();
        };
        let len = unsafe { len_f(jni.env, bytes) };
        let mut out = Vec::new();
        if len > 0 {
            let mut samples = vec![0i16; len as usize];
            unsafe { region_f(jni.env, bytes, 0, len, samples.as_mut_ptr()) };
            out = samples;
        }
        jni.delete_local_ref(bytes);
        out
    }
}
#[cfg(not(target_os = "android"))]
mod imp {
    /// No host bridge on this platform, so nothing to resolve.
    pub fn populate_jni_cache() {}

    #[allow(dead_code)]
    pub fn has_camera(_front: bool) -> bool {
        false
    }
    #[allow(dead_code)]
    pub fn take_photo(_front: bool) -> Option<Vec<u8>> {
        None
    }
}

pub use imp::populate_jni_cache;

// The raw JNI mic bridge, for the shared implementation in
// `crate::audio::mic` (Android only).
#[cfg(target_os = "android")]
pub(crate) use imp::{
    has_microphone as jni_has_microphone, read_mic_chunk as jni_read_mic_chunk,
    start_mic as jni_start_mic, stop_mic as jni_stop_mic,
};

// Public microphone API: thin wrappers over the shared host implementation
// so existing callers (Audio Toolbox, AVFoundation) keep working unchanged.
pub fn has_microphone() -> bool {
    crate::audio::mic::has_microphone()
}
pub fn start_mic() -> bool {
    crate::audio::mic::start()
}
pub fn stop_mic() {
    crate::audio::mic::stop()
}
pub fn read_mic_chunk() -> Vec<i16> {
    crate::audio::mic::read_mic_chunk()
}

/// Sample rate of the shared host capture stream (mono 16-bit PCM); matches
/// `HostMedia.MIC_SAMPLE_RATE` on Android.
pub const MIC_SAMPLE_RATE: u32 = 44100;
