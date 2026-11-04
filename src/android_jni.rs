/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Minimal hand-rolled JNI plumbing shared by the Android bridges.
//!
//! touchHLE reaches the Android framework from two places (`android_media`
//! and `android_web_view`) without depending on the `jni` crate, so both walk
//! the JNI function table by hand. That walk used to be duplicated, and the
//! two copies disagreed about what a `JNIEnv*` is:
//!
//! ```c
//! struct _JNIEnv {           // what `SDL_AndroidGetJNIEnv()` returns
//!     const struct JNINativeInterface_ *functions;
//! };
//! ```
//!
//! `android_web_view` dereferenced `env` to reach `functions` and indexed
//! that; `android_media` indexed `env` itself. The latter reads whatever
//! happens to follow the 8-byte `JNIEnv` struct in ART's per-thread heap and
//! then *calls it as a function pointer*. Those bytes are very often zero, so
//! the first camera/microphone query — e.g.
//! `+[UIImagePickerController isSourceTypeAvailable:…SourceTypeCamera]` —
//! killed the whole host process with `SIGSEGV at address 0x0`, with nothing
//! in the log but the FATAL SIGNAL marker (no panic, no guest fault).
//!
//! Keeping the dereference in one place means the two bridges cannot drift
//! apart again, and because the helper only does plain pointer arithmetic it
//! also compiles — and is unit-tested — on desktop platforms, where the
//! bridges themselves are inert no-ops.

use std::os::raw::c_void;

/// Resolve the raw function pointer held in slot `index` of the JNI function
/// table (`struct JNINativeInterface_`) reachable from `env`.
///
/// Returns [None] when `env` is null, when the function-table pointer it
/// holds is null, or when the slot itself is null. Callers must treat [None]
/// as "this bridge is unavailable" and fall back to their no-hardware answer
/// rather than calling through a null pointer.
///
/// # Safety
///
/// `env` must be either null or a valid `JNIEnv*` for the calling thread (as
/// returned by `SDL_AndroidGetJNIEnv()`), and `index` must be a valid slot of
/// `struct JNINativeInterface_` - reading out of bounds of the table is
/// undefined behaviour, exactly as in C.
// Only the Android bridges call this; on other platforms it exists so that
// the unit tests below can check the table walk.
#[allow(dead_code)]
pub unsafe fn table_slot(env: *mut c_void, index: usize) -> Option<*mut c_void> {
    if env.is_null() {
        return None;
    }
    // One dereference: `env` points at a struct whose only field is the
    // pointer to the function table. This is the `(*env)->Fn(env, ...)` idiom
    // from C. Indexing `env` itself would read JNIEnv-internal/neighbouring
    // bytes and hand them back as if they were function pointers.
    let table = unsafe { *(env as *mut *mut c_void) };
    if table.is_null() {
        return None;
    }
    let slot = unsafe { *((table as *mut *mut c_void).add(index)) };
    if slot.is_null() {
        return None;
    }
    Some(slot)
}

/// Like [`table_slot`], but reinterprets the resolved pointer as a function of
/// type `F` (one of the `unsafe extern "C"` signatures from jni.h).
///
/// # Safety
///
/// Same requirements as [`table_slot`], plus: `F` must be the C signature of
/// the function that really lives in slot `index`. A wrong `index` silently
/// resolves to a *different* JNI function with a different signature, which
/// corrupts the process when called; the slot numbers are documented in
/// jni.h and in the `slots` modules of the two bridges.
#[allow(dead_code)]
pub unsafe fn table_fn<F>(env: *mut c_void, index: usize) -> Option<F> {
    let slot = unsafe { table_slot(env, index) }?;
    // SAFETY: `slot` is a non-null pointer to a C function, and the caller
    // guarantees `F` matches its signature. `transmute_copy` is used (rather
    // than `transmute`) because `F` is a generic type parameter whose size
    // the compiler cannot relate to `*mut c_void` symbolically; both are one
    // pointer wide.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&slot) })
}

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn some_jni_fn() {}

    #[test]
    fn resolves_through_the_function_table() {
        let want = some_jni_fn as usize as *mut c_void;
        // Slot 0 deliberately left null, slot 1 populated: `FindClass`-style
        // indices are never the first entry (slots 0-3 are reserved).
        let mut table = [std::ptr::null_mut::<c_void>(), want];
        let mut env_struct: *mut c_void = table.as_mut_ptr() as *mut c_void;
        let env = &mut env_struct as *mut *mut c_void as *mut c_void;

        assert_eq!(unsafe { table_slot(env, 1) }, Some(want));
        // A null slot must be reported, not called.
        assert_eq!(unsafe { table_slot(env, 0) }, None);
    }

    #[test]
    fn rejects_null_env_and_null_table() {
        let mut table = [std::ptr::null_mut::<c_void>()];
        let mut env_struct: *mut c_void = table.as_mut_ptr() as *mut c_void;
        let env = &mut env_struct as *mut *mut c_void as *mut c_void;

        assert_eq!(unsafe { table_slot(std::ptr::null_mut(), 0) }, None);
        // env is valid but its `functions` pointer is null.
        let mut null_table: *mut c_void = std::ptr::null_mut();
        let null_env = &mut null_table as *mut *mut c_void as *mut c_void;
        assert_eq!(unsafe { table_slot(null_env, 0) }, None);
        assert_eq!(unsafe { table_fn::<unsafe extern "C" fn()>(env, 0) }, None);
    }

    /// The regression this module exists for: indexing the `JNIEnv` struct
    /// instead of the table it points at. With a table pointer followed by
    /// zeros (what ART's heap looks like in practice), the buggy walk reads a
    /// null slot and the bridge calls address 0 — a host SIGSEGV. The fixed
    /// walk finds the real function.
    #[test]
    fn indexing_the_env_struct_is_not_indexing_the_table() {
        let want = some_jni_fn as usize as *mut c_void;
        // env_struct[0] = &table, env_struct[1..] = zeros (heap padding).
        let mut env_words = [std::ptr::null_mut::<c_void>(); 8];
        let mut table = [std::ptr::null_mut::<c_void>(), want];
        env_words[0] = table.as_mut_ptr() as *mut c_void;
        let env = env_words.as_mut_ptr() as *mut c_void;

        // What the buggy `let table = env as *mut *mut c_void` did:
        let buggy = unsafe { *((env as *mut *mut c_void).add(1)) };
        assert_eq!(buggy, std::ptr::null_mut());

        // What the fixed walk does:
        assert_eq!(unsafe { table_slot(env, 1) }, Some(want));
        assert!(unsafe { table_fn::<unsafe extern "C" fn()>(env, 1) }.is_some());
    }
}
