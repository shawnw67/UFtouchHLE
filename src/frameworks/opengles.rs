/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! OpenGL ES and EAGL.
//!
//! This module is specific to OpenGL ES's role as a part of the iPhone OS API
//! surface. See [crate::gles] for other uses and a discussion of the broader
//! topic.

mod eagl;
mod gles_guest;
pub use gles_guest::run_queued_calls;
mod matrix_palette;
pub(crate) mod p2g_wide;
mod software_vao;
pub(crate) use p2g_wide::validate as validate_p2g_wide;

pub use eagl::redraw_last_frame;

use Podroid_gl_bindings::gles11::types::{GLenum, GLint, GLsizei, GLuint};

use crate::mem::ConstPtr;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/OpenGLES.framework/OpenGLES",
    aliases: &[],
    class_exports: &[eagl::CLASSES],
    constant_exports: &[eagl::CONSTANTS],
    function_exports: &[gles_guest::FUNCTIONS, eagl::FUNCTIONS],
};

#[derive(Default)]
pub struct State {
    pub(crate) p2g_wide: p2g_wide::State,
    /// Current EAGLContext for each thread
    current_ctxs: std::collections::HashMap<crate::ThreadId, Option<crate::objc::id>>,
    /// `glGetString()` results cache, keyed by `(is_es2, name)` so that an
    /// ES 2.0 context does not return the ES 1.1 `OpenGL ES-CM 1.1` version
    /// string (Bad Piggies / Unity 3.5 checks the version string to decide
    /// which renderer code path to use — mismatching the API leaves Unity in
    /// a half-initialised ES 2.0 path that emits torn / overlapping frames).
    strings_cache: std::collections::HashMap<(bool, GLenum), ConstPtr<u8>>,
    /// Guest `GLsync` handles (`glFenceSync` / `glFenceSyncAPPLE`). Host
    /// `GLsync` values are pointers and don't fit in a 32-bit guest value,
    /// so the guest gets an opaque handle into this table instead.
    sync_objects: std::collections::HashMap<GLuint, gles_guest::GuestSyncObject>,
    /// Next candidate guest `GLsync` handle (0 is never handed out).
    next_sync_handle: GLuint,
    /// See [gles_guest]'s `bioshock_framebuffer`.
    bioshock_view_framebuffer: GLuint,
    /// Guest buffer for [gles_guest::run_queued_calls], and its size.
    queued_gl_buffer: Option<(crate::mem::MutVoidPtr, u32)>,
    /// The framebuffer object the guest currently has bound, per thread.
    bound_framebuffers: std::collections::HashMap<crate::ThreadId, GLuint>,
    /// Framebuffer objects whose colour attachment is a *texture*.
    ///
    /// The scale hack enlarges renderbuffers (both the EAGL drawable's and any
    /// the app allocates via `glRenderbufferStorage`), so scaling a viewport
    /// aimed at those is correct. Textures are never enlarged, so a viewport
    /// aimed at a texture-backed framebuffer must be left alone — otherwise
    /// the app draws `scale_hack` times too large into an unscaled target and
    /// you see a magnified bottom-left corner (Flappy Bird renders through a
    /// 288x512 texture FBO and did exactly that).
    texture_backed_framebuffers: std::collections::HashSet<GLuint>,
    /// The last viewport the guest asked for, in guest (unscaled) co-ordinates.
    ///
    /// The GL viewport is global state rather than per-framebuffer, and apps
    /// are free to call `glViewport` *before* binding the framebuffer they mean
    /// it for (Flappy Bird does). Whether the scale hack applies therefore
    /// cannot be decided when `glViewport` is called — we remember the
    /// request and re-apply it with the right scaling whenever the binding
    /// changes.
    guest_viewports: std::collections::HashMap<crate::ThreadId, (GLint, GLint, GLsizei, GLsizei)>,
    /// Sizes of the textures used by `eagl::present_renderbuffer` to present
    /// frames, so they can be reused. Keyed by sharegroup (see
    /// [eagl::sharegroup_key]), since that's what texture names belong to.
    present_texture_sizes: std::collections::HashMap<usize, eagl::PresentTextureSizes>,
    /// Errors the app hasn't retrieved with `glGetError` yet, kept for each
    /// context while touchHLE uses it (see [without_leaving_errors]).
    pending_errors: std::collections::HashMap<crate::objc::id, GLenum>,
}
impl State {
    fn current_ctx_for_thread(&mut self, thread: crate::ThreadId) -> &mut Option<crate::objc::id> {
        // PERF: this runs on every guest GL call; one hash lookup, not two.
        self.current_ctxs.entry(thread).or_insert(None)
    }

    /// Get the guest buffer for queued GL call data, big enough for `size`.
    pub fn queued_gl_data(
        &mut self,
        mem: &mut crate::mem::Mem,
        size: usize,
    ) -> crate::mem::MutVoidPtr {
        let size = (size as u32).max(0x10000);
        match self.queued_gl_buffer {
            Some((buffer, buffer_size)) if buffer_size >= size => buffer,
            old => {
                if let Some((buffer, _)) = old {
                    mem.free(buffer);
                }
                let buffer = mem.alloc(size);
                self.queued_gl_buffer = Some((buffer, size));
                buffer
            }
        }
    }

    pub fn set_bound_framebuffer(&mut self, thread: crate::ThreadId, framebuffer: GLuint) {
        self.bound_framebuffers.insert(thread, framebuffer);
    }

    /// Record what kind of colour attachment a framebuffer was given, so
    /// `glViewport` can tell whether the scale hack applies to it.
    pub fn set_colour_attachment_is_texture(&mut self, thread: crate::ThreadId, is_texture: bool) {
        let framebuffer = self
            .bound_framebuffers
            .get(&thread)
            .copied()
            .unwrap_or_default();
        // Framebuffer 0 is the window/drawable itself and is always scaled.
        if framebuffer == 0 {
            return;
        }
        if is_texture {
            self.texture_backed_framebuffers.insert(framebuffer);
        } else {
            self.texture_backed_framebuffers.remove(&framebuffer);
        }
    }

    /// Whether the scale hack should be applied to `glViewport` right now.
    ///
    /// Defaults to `true` (the historical behaviour) unless we positively know
    /// the bound framebuffer is texture-backed, so anything we failed to track
    /// keeps working exactly as before.
    pub fn set_guest_viewport(
        &mut self,
        thread: crate::ThreadId,
        viewport: (GLint, GLint, GLsizei, GLsizei),
    ) {
        self.guest_viewports.insert(thread, viewport);
    }

    pub fn guest_viewport(
        &self,
        thread: crate::ThreadId,
    ) -> Option<(GLint, GLint, GLsizei, GLsizei)> {
        self.guest_viewports.get(&thread).copied()
    }

    pub fn scale_hack_applies_to_viewport(&self, thread: crate::ThreadId) -> bool {
        let framebuffer = self
            .bound_framebuffers
            .get(&thread)
            .copied()
            .unwrap_or_default();
        !self.texture_backed_framebuffers.contains(&framebuffer)
    }
}

/// Bind the calling thread's current EAGL context to the host window and
/// return a [`crate::gles::GLES`] wrapper for it.
///
/// Returns `None` when the calling thread has no current EAGL context (i.e.
/// `[EAGLContext setCurrentContext:]` was never called with a non-`nil`
/// argument on this thread, or it was reset to `nil`). Apple's
/// implementation simply ignores GL calls in that state instead of crashing
/// the process — see Apple's "OpenGL ES Programming Guide for iOS",
/// section "Context Lifecycle Management": *"If the current context is
/// `nil`, OpenGL ES commands silently fail"*.
///
/// Previously this used `.unwrap()` on the thread-local context, which
/// turned a perfectly recoverable guest-side state error into an emulator
/// panic. HyperHLE log #5 reproduced this when a worker thread issued GL
/// calls between `[EAGLContext setCurrentContext:nil]` and a later
/// `setCurrentContext:` on the main thread.
fn sync_context<'objc, 'win: 'objc>(
    state: &mut State,
    objc: &'objc mut crate::objc::ObjC,
    window: &'win mut crate::window::Window,
    current_thread: crate::ThreadId,
) -> Option<Box<dyn crate::gles::GLES + 'objc>> {
    let gles_ctx = get_thread_context(state, objc, current_thread)?;
    Some(gles_ctx.make_current(window))
}

/// Runs `f`, which uses the current context for touchHLE's own purposes
/// (e.g. presenting a frame), without leaving OpenGL ES errors for the app.
///
/// An app can't tell apart errors from its own calls and from touchHLE's, and
/// some react to them: if presenting leaves an error (as on Huawei's OpenGL ES
/// implementation), Resident Evil: Mercenaries VS. stops drawing its player
/// character. So errors from `f` are discarded, and errors the app already
/// had are kept for its next `glGetError` call.
fn without_leaving_errors<U>(
    env: &mut crate::Environment,
    f: impl FnOnce(&mut crate::Environment) -> U,
) -> U {
    let Some(ctx) = *env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
    else {
        return f(env);
    };
    let app_error = take_errors(env);
    let res = f(env);
    let own_error = take_errors(env);
    if own_error != 0 {
        log_dbg!(
            "Discarded error {:#x} from touchHLE's own OpenGL ES use",
            own_error
        );
    }
    if app_error != 0 {
        env.framework_state
            .opengles
            .pending_errors
            .entry(ctx)
            .or_insert(app_error);
    }
    res
}

/// Clears the error flags of the current context, returning the first error.
fn take_errors(env: &mut crate::Environment) -> GLenum {
    let Some(mut gles) = sync_context(
        &mut env.framework_state.opengles,
        &mut env.objc,
        env.window
            .as_mut()
            .expect("OpenGL ES is not supported in headless mode"),
        env.current_thread,
    ) else {
        return 0;
    };
    let mut first = 0;
    // There can be several error flags, but not endlessly many.
    for _ in 0..16 {
        let error = unsafe { gles.GetError() };
        if error == 0 {
            break;
        }
        if first == 0 {
            first = error;
        }
    }
    first
}

/// Look up the current [`crate::gles::GLESContext`] for the given thread.
///
/// Returns `None` when the thread has no current EAGL context bound, or
/// when that context was created without a backing GLES driver (e.g.
/// headless / unsupported `EAGLRenderingAPI`). Both are recoverable —
/// callers should skip the GL operation rather than abort the emulator.
fn get_thread_context<'objc>(
    state: &mut State,
    objc: &'objc mut crate::objc::ObjC,
    current_thread: crate::ThreadId,
) -> Option<&'objc mut dyn crate::gles::GLESContext> {
    // Note: `Box<dyn GLESContext>` carries the default `'static` trait-object
    // bound, so the returned mutable reference re-borrows from the
    // (`'objc`) ObjC store without imposing a tighter lifetime.
    let current_ctx = (*state.current_ctx_for_thread(current_thread))?;
    let host_obj = objc.borrow_mut::<eagl::EAGLContextHostObject>(current_ctx);
    let gles_ctx: &mut (dyn crate::gles::GLESContext + 'static) =
        host_obj.gles_ctx.as_deref_mut()?;
    Some(gles_ctx as &mut dyn crate::gles::GLESContext)
}
