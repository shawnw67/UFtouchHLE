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
mod gles2_guest;
mod gles_guest;
mod matrix_palette;
pub(crate) mod p2g_wide;
pub(crate) use p2g_wide::validate as validate_p2g_wide;

pub use eagl::redraw_last_frame;

use Podroid_gl_bindings::gles11::types::{GLenum, GLuint};

use crate::mem::ConstPtr;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/OpenGLES.framework/OpenGLES",
    aliases: &[],
    class_exports: &[eagl::CLASSES],
    constant_exports: &[eagl::CONSTANTS],
    function_exports: &[gles_guest::FUNCTIONS, gles2_guest::FUNCTIONS],
};

#[derive(Default)]
pub struct State {
    pub(crate) p2g_wide: p2g_wide::State,
    /// ES 2 contexts currently use the readback compositor, which keeps the
    /// app's shaders and vertex attributes separate from presentation.
    pub(crate) programmable_contexts: std::collections::HashSet<crate::objc::id>,
    /// Current EAGLContext for each thread
    current_ctxs: std::collections::HashMap<crate::ThreadId, Option<crate::objc::id>>,
    strings_cache: std::collections::HashMap<GLenum, ConstPtr<u8>>,
    programmable_strings_cache: std::collections::HashMap<GLenum, ConstPtr<u8>>,
    /// Sizes of the textures used by `eagl::present_renderbuffer` to present
    /// frames, so they can be reused. Keyed by sharegroup (see
    /// [eagl::sharegroup_key]), since that's what texture names belong to.
    present_texture_sizes: std::collections::HashMap<usize, eagl::PresentTextureSizes>,
    /// Errors the app hasn't retrieved with `glGetError` yet, kept for each
    /// context while touchHLE uses it (see [without_leaving_errors]).
    pending_errors: std::collections::HashMap<crate::objc::id, GLenum>,
    /// Textures allocated without data, which the app may render to, keyed by
    /// sharegroup and texture name (see `scale_render_target` in `gles_guest`).
    render_target_textures:
        std::collections::HashMap<(usize, GLuint), gles_guest::RenderTargetTexture>,
}
impl State {
    fn current_ctx_for_thread(&mut self, thread: crate::ThreadId) -> &mut Option<crate::objc::id> {
        self.current_ctxs.entry(thread).or_insert(None);
        self.current_ctxs.get_mut(&thread).unwrap()
    }
}

fn sync_context<'objc, 'win: 'objc>(
    state: &mut State,
    objc: &'objc mut crate::objc::ObjC,
    window: &'win mut crate::window::Window,
    current_thread: crate::ThreadId,
) -> Box<dyn crate::gles::GLES + 'objc> {
    let gles_ctx = get_thread_context(state, objc, current_thread);
    gles_ctx.make_current(window)
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
    let mut gles = sync_context(
        &mut env.framework_state.opengles,
        &mut env.objc,
        env.window
            .as_mut()
            .expect("OpenGL ES is not supported in headless mode"),
        env.current_thread,
    );
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

fn get_thread_context<'objc>(
    state: &mut State,
    objc: &'objc mut crate::objc::ObjC,
    current_thread: crate::ThreadId,
) -> &'objc mut dyn crate::gles::GLESContext {
    let current_ctx = state.current_ctx_for_thread(current_thread);
    let host_obj = objc.borrow_mut::<eagl::EAGLContextHostObject>(current_ctx.unwrap());
    let gles_ctx = host_obj.gles_ctx.as_deref_mut().unwrap();
    gles_ctx
}
