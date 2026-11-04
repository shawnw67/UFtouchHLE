/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! EAGL.

use super::software_vao::VertexArrayState;
use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_animation::ca_eagl_layer::{
    find_fullscreen_eagl_layer_with_overlays, get_pixels_vec_for_presenting, present_pixels,
};
use crate::frameworks::core_graphics::{CGFloat, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::NSUInteger;
use crate::gles::gles11_raw as gles11; // constants only
use crate::gles::gles11_raw::types::*;
use crate::gles::present::{present_frame, FpsCounter};
use crate::gles::{
    create_gles1_ctx, create_gles2_ctx, create_gles3_ctx, gles1_on_gl2, GLESContext, GLES,
};
use crate::mem::MutPtr;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
};
use crate::options::Options;
use crate::Environment;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// These are used by the EAGLDrawable protocol implemented by CAEAGLayer.
// Since these have the ABI of constant symbols rather than literal constants,
// the values shouldn't matter, and haven't been checked against real iPhone OS.
pub const kEAGLDrawablePropertyColorFormat: &str = "ColorFormat";
pub const kEAGLDrawablePropertyRetainedBacking: &str = "RetainedBacking";
pub const kEAGLColorFormatRGBA8: &str = "RGBA8";
pub const kEAGLColorFormatRGB565: &str = "RGB565";
/// `kEAGLColorFormatSRGBA8` — sRGB 8888 EAGL color format, added in
/// iOS 7. Apps that supply this string in `drawableProperties` are
/// requesting a sRGB-encoded color renderbuffer (per
/// `EAGLDrawable.h`).
pub const kEAGLColorFormatSRGBA8: &str = "SRGBA8";

pub const CONSTANTS: ConstantExports = &[
    (
        "_kEAGLDrawablePropertyColorFormat",
        HostConstant::NSString(kEAGLDrawablePropertyColorFormat),
    ),
    (
        "_kEAGLDrawablePropertyRetainedBacking",
        HostConstant::NSString(kEAGLDrawablePropertyRetainedBacking),
    ),
    (
        "_kEAGLColorFormatRGBA8",
        HostConstant::NSString(kEAGLColorFormatRGBA8),
    ),
    (
        "_kEAGLColorFormatRGB565",
        HostConstant::NSString(kEAGLColorFormatRGB565),
    ),
    (
        "_kEAGLColorFormatSRGBA8",
        HostConstant::NSString(kEAGLColorFormatSRGBA8),
    ),
];

type EAGLRenderingAPI = u32;
const kEAGLRenderingAPIOpenGLES1: EAGLRenderingAPI = 1;
const kEAGLRenderingAPIOpenGLES2: EAGLRenderingAPI = 2;
const kEAGLRenderingAPIOpenGLES3: EAGLRenderingAPI = 3;

/// Whether OpenGL ES 3.0 contexts can be created. On macOS, they could only
/// be emulated with OpenGL 3.3's core profile, which lacks too much of OpenGL
/// ES (e.g. client-side vertex arrays) for games to work.
fn es3_is_supported() -> bool {
    !cfg!(target_os = "macos")
}

/// Resolve the EAGL rendering API the host should actually create a context
/// for. When `prefer_gles2_context` is set and the app requested ES 1.1, we
/// transparently upgrade to ES 2.0 so apps that ask for ES 1.1 but drive
/// rendering with shader entry points (`glUseProgram`, `glCreateShader`, …)
/// route through the real native ES 2.0 backend instead of falling through
/// to the GLES 1.1-only stubs in `gles_generic`.
fn effective_eagl_api(
    requested: EAGLRenderingAPI,
    prefer_gles2_context: bool,
    force_gles1_context: bool,
) -> EAGLRenderingAPI {
    // Hardcoded driver pin: TOUCHHLE_FORCE_EAGL_API forces the reported/
    // effective EAGL rendering API (1, 2 or 3) regardless of what the guest
    // app requested. This pins the GPU driver surface the app sees, mirroring
    // TOUCHHLE_FORCE_GLES1 on the context-creation side.
    if let Ok(forced) = std::env::var("TOUCHHLE_FORCE_EAGL_API") {
        let forced = forced.trim();
        if let Ok(v) = forced.parse::<EAGLRenderingAPI>() {
            if (1..=3).contains(&v) && v != requested {
                log!(
                    "EAGL: TOUCHHLE_FORCE_EAGL_API active, overriding requested API {} with {}",
                    requested,
                    v
                );
                return v;
            }
        }
    }
    if force_gles1_context && requested != kEAGLRenderingAPIOpenGLES1 {
        log!(
            "EAGL: --force-gles1-context active, downgrading initWithAPI:{} (kEAGLRenderingAPIOpenGLES{}) to kEAGLRenderingAPIOpenGLES1",
            requested,
            requested
        );
        return kEAGLRenderingAPIOpenGLES1;
    }
    if prefer_gles2_context && requested == kEAGLRenderingAPIOpenGLES1 {
        log!(
            "EAGL: --prefer-gles2-context active, upgrading initWithAPI:{} \
             (kEAGLRenderingAPIOpenGLES1) to kEAGLRenderingAPIOpenGLES2",
            requested
        );
        return kEAGLRenderingAPIOpenGLES2;
    }
    requested
}

/// Host-side mirror of a few pieces of guest-visible OpenGL ES state.
///
/// The guest wrappers in [super::gles_guest] used to ask the host driver
/// (`glGetIntegerv`, `glGetBooleanv`, `glGetFloatv`, each followed by a
/// `glGetError()` to swallow the errors strict drivers raise for those
/// queries) on *every* `gl*Pointer` and draw call, just to learn state that
/// only the guest itself can change. On mobile drivers and on ANGLE those
/// round-trips are far from free, and they add up to thousands of extra GL
/// calls per frame in draw-heavy games. Tracking the state here instead makes
/// them disappear.
///
/// Everything in here is per-context (bindings and fixed-function state are
/// context state, not sharegroup state) and is updated by the guest wrappers
/// that change it. Host-side code that touches this state on the app's
/// context (the presenter) always restores what it found, so the mirror stays
/// valid across presents.
pub(super) struct GLShadowState {
    /// `GL_ARRAY_BUFFER_BINDING`.
    pub(super) array_buffer: GLuint,
    /// `GL_ELEMENT_ARRAY_BUFFER_BINDING`. `None` means "unknown, ask the
    /// driver": the element array binding is part of vertex array object
    /// state, so it is invalidated whenever the guest switches VAOs.
    pub(super) element_array_buffer: Option<GLuint>,
    /// `glIsEnabled(GL_FOG)`.
    pub(super) fog_enabled: bool,
    /// `GL_FOG_START` / `GL_FOG_END`.
    pub(super) fog_start: f32,
    pub(super) fog_end: f32,
    /// Whether `glEnableVertexAttribArray` was ever called on this context.
    /// Draw-call guards for generic vertex attributes are skipped entirely
    /// for the (very common) fixed-function-only apps that never use them.
    pub(super) generic_attribs_used: bool,
    /// Programs for which the guest app explicitly bound attribute locations
    /// via `glBindAttribLocation` before linking. For these, `glLinkProgram`
    /// must not force-rebind canonical attribute names, because that would
    /// override the app's own vertex layout (e.g. Gameloft's Jet engine).
    pub(super) guest_bound_attribs: HashMap<GLuint, std::collections::HashSet<String>>,
    /// Dimensions and format for PVRTC levels expanded to RGBA storage.
    pub(super) pvrtc_texture_levels: HashMap<(GLenum, GLuint, GLint), (GLsizei, GLsizei, GLenum)>,
    /// PVRTC levels whose RGBA storage has not been materialized yet. Set by
    /// the `glTexStorage2D` PVRTC emulation (immutable storage is skipped when
    /// the host can't take the PVRTC internalformat) and cleared once the
    /// level is first allocated via `glTexImage2D`, so subsequent updates can
    /// use `glTexSubImage2D`.
    pub(super) pvrtc_unallocated_levels: HashSet<(GLenum, GLuint, GLint)>,
    pub(super) current_vertex_array: GLuint,
    pub(super) software_vertex_arrays: HashSet<GLuint>,
    pub(super) software_vao_element_array_buffers: HashMap<GLuint, GLuint>,
    pub(super) next_software_vertex_array: GLuint,
    /// Client active texture unit (`GL_CLIENT_ACTIVE_TEXTURE - GL_TEXTURE0`).
    pub(super) client_active_texture: usize,
    /// Client array state of the currently bound vertex array object. Only
    /// meaningful with the software VAO fallback, see [super::software_vao].
    pub(super) vertex_array_state: VertexArrayState,
    /// Client array state of the other (software) vertex array objects.
    pub(super) software_vao_states: HashMap<GLuint, VertexArrayState>,
}
impl Default for GLShadowState {
    fn default() -> Self {
        GLShadowState {
            array_buffer: 0,
            element_array_buffer: Some(0),
            fog_enabled: false,
            // OpenGL ES 1.1 defaults.
            fog_start: 0.0,
            fog_end: 1.0,
            generic_attribs_used: false,
            guest_bound_attribs: HashMap::new(),
            pvrtc_texture_levels: HashMap::new(),
            pvrtc_unallocated_levels: HashSet::new(),
            current_vertex_array: 0,
            software_vertex_arrays: HashSet::new(),
            software_vao_element_array_buffers: HashMap::new(),
            next_software_vertex_array: 1,
            client_active_texture: 0,
            vertex_array_state: VertexArrayState::default(),
            software_vao_states: HashMap::new(),
        }
    }
}
impl GLShadowState {
    /// Forget everything that is stored in vertex array object state.
    pub(super) fn invalidate_vao_state(&mut self) {
        self.element_array_buffer = None;
    }
    /// Update the mirror after `glDeleteBuffers`: deleting a bound buffer
    /// resets the binding to zero.
    pub(super) fn on_buffers_deleted(&mut self, deleted: GLuint) {
        if deleted == 0 {
            return;
        }
        if self.array_buffer == deleted {
            self.array_buffer = 0;
        }
        if self.element_array_buffer == Some(deleted) {
            self.element_array_buffer = Some(0);
        }
        for buffer in self.software_vao_element_array_buffers.values_mut() {
            if *buffer == deleted {
                *buffer = 0;
            }
        }
    }
    pub(super) fn record_pvrtc_texture_level(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
    ) {
        self.pvrtc_texture_levels
            .insert((target, texture, level), (width, height, format));
    }
    pub(super) fn pvrtc_texture_level(
        &self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) -> Option<(GLsizei, GLsizei, GLenum)> {
        self.pvrtc_texture_levels
            .get(&(target, texture, level))
            .copied()
    }
    pub(super) fn forget_pvrtc_texture_level(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) {
        self.pvrtc_texture_levels.remove(&(target, texture, level));
        self.pvrtc_unallocated_levels
            .remove(&(target, texture, level));
    }
    pub(super) fn forget_pvrtc_texture(&mut self, texture: GLuint) {
        self.pvrtc_texture_levels.retain(|key, _| key.1 != texture);
        self.pvrtc_unallocated_levels.retain(|key| key.1 != texture);
    }
    /// Mark a PVRTC level as not yet materialized: its first upload must use
    /// `glTexImage2D` (which defines the level) instead of
    /// `glTexSubImage2D` (which requires the level to already exist).
    pub(super) fn record_pvrtc_unallocated_level(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) {
        self.pvrtc_unallocated_levels
            .insert((target, texture, level));
    }
    pub(super) fn pvrtc_level_unallocated(
        &self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) -> bool {
        self.pvrtc_unallocated_levels
            .contains(&(target, texture, level))
    }
    pub(super) fn mark_pvrtc_level_allocated(
        &mut self,
        target: GLenum,
        texture: GLuint,
        level: GLint,
    ) {
        self.pvrtc_unallocated_levels
            .remove(&(target, texture, level));
    }
    pub(super) fn record_element_array_buffer_binding(&mut self, buffer: GLuint) {
        self.element_array_buffer = Some(buffer);
        self.software_vao_element_array_buffers
            .insert(self.current_vertex_array, buffer);
    }
    /// Switch the software VAO binding to `array`. Returns the element array
    /// buffer to bind and, if the binding changed, the client array state the
    /// host context is in (the outgoing VAO's), which
    /// [super::software_vao::apply_vertex_array_state] must turn into
    /// [Self::vertex_array_state].
    pub(super) fn bind_software_vertex_array(
        &mut self,
        array: GLuint,
    ) -> (GLuint, Option<VertexArrayState>) {
        if array == self.current_vertex_array {
            return (self.bind_software_element_array_buffer(array), None);
        }
        let incoming = self.software_vao_states.remove(&array).unwrap_or_default();
        let outgoing = std::mem::replace(&mut self.vertex_array_state, incoming);
        self.software_vao_states
            .insert(self.current_vertex_array, outgoing.clone());
        (
            self.bind_software_element_array_buffer(array),
            Some(outgoing),
        )
    }
    fn bind_software_element_array_buffer(&mut self, array: GLuint) -> GLuint {
        if let Some(binding) = self.element_array_buffer {
            self.software_vao_element_array_buffers
                .insert(self.current_vertex_array, binding);
        }
        self.current_vertex_array = array;
        let binding = *self
            .software_vao_element_array_buffers
            .entry(array)
            .or_insert(0);
        self.element_array_buffer = Some(binding);
        binding
    }
    pub(super) fn generate_software_vertex_array(&mut self) -> GLuint {
        let mut name = self.next_software_vertex_array.max(1);
        while self.software_vertex_arrays.contains(&name) {
            name = name.wrapping_add(1).max(1);
        }
        self.next_software_vertex_array = name.wrapping_add(1).max(1);
        self.software_vertex_arrays.insert(name);
        self.software_vao_element_array_buffers
            .entry(name)
            .or_insert(0);
        name
    }
    pub(super) fn is_software_vertex_array(&self, array: GLuint) -> bool {
        array != 0 && self.software_vertex_arrays.contains(&array)
    }
    pub(super) fn delete_software_vertex_array(
        &mut self,
        array: GLuint,
    ) -> Option<(GLuint, Option<VertexArrayState>)> {
        if array == 0 || !self.software_vertex_arrays.remove(&array) {
            return None;
        }
        self.software_vao_element_array_buffers.remove(&array);
        if self.current_vertex_array == array {
            // Deleting the bound VAO reverts to the default one. Forget the
            // deleted VAO's element array binding before switching so that
            // it isn't saved under its name again.
            self.element_array_buffer = None;
            let (binding, outgoing) = self.bind_software_vertex_array(0);
            self.software_vao_states.remove(&array);
            Some((binding, outgoing))
        } else {
            self.software_vao_states.remove(&array);
            None
        }
    }
}

type MappedBuffer = (MutPtr<GLvoid>, Option<*mut GLvoid>, usize);

#[derive(Default)]
pub(super) struct EAGLContextHostObject {
    pub(super) gles_ctx: Option<Box<dyn GLESContext>>,
    /// Guest object addresses can be reused when a game session restarts.
    present_token: usize,
    /// See [GLShadowState].
    pub(super) shadow: GLShadowState,
    /// Which EAGL rendering API was requested. This influences how
    /// [super::gles_guest] dispatches calls and how the present-renderbuffer
    /// path saves and restores state.
    pub(super) api: EAGLRenderingAPI,
    /// The framebuffer that was bound when this context was created. On iOS,
    /// SDL2 draws the window through a framebuffer object of its own rather
    /// than framebuffer 0, so presenting has to target this one. Always 0 on
    /// other platforms.
    drawable_framebuffer: GLuint,
    /// Mapping of OpenGL ES renderbuffer names to `EAGLDrawable` instances
    /// (always `CAEAGLLayer*`). Retains the instance so it won't dangle.
    renderbuffer_drawable_bindings: Rc<RefCell<HashMap<GLuint, id>>>,
    fps_counter: Option<FpsCounter>,
    next_frame_due: Option<Instant>,
    /// Buffers the guest currently has mapped, keyed by target and buffer
    /// object name: `(guest mirror, driver mapping, size)`.
    ///
    /// The driver mapping is only set for `glMapBufferRange`, whose mapping is
    /// still open; `glMapBufferOES` keeps the data in the guest mirror alone
    /// and uploads it on unmap, so it never holds a driver pointer across a
    /// guest call (see `gles_guest::glMapBufferOES`).
    pub mapped_buffers: HashMap<(GLenum, GLuint), MappedBuffer>,
    /// Always [Some], except while it's in use (see `gles_guest`).
    pub matrix_palette: Option<Box<super::matrix_palette::State>>,
}
impl HostObject for EAGLContextHostObject {}

static NEXT_PRESENT_TOKEN: AtomicUsize = AtomicUsize::new(1);

/// Log the effective state of the `fix_texture_min_filter` option once,
/// the first time an EAGL context is created. This makes it easy to
/// diagnose "geometry rendering as flat black/white textures" reports
/// from a single log file: if the option is on we say so, if it's off
/// (e.g. because the user explicitly disabled it via
/// `--no-fix-texture-min-filter`) we say that too along with the
/// suggested switch to enable it. The log fires only once per process.
fn log_fix_min_filter_status(enabled: bool) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SEEN: AtomicBool = AtomicBool::new(false);
    if SEEN.swap(true, Ordering::Relaxed) {
        return;
    }
    if enabled {
        log!(
            "fix_texture_min_filter: ON (after every level-0 texture upload, \
             GL_TEXTURE_MIN_FILTER will be forced to GL_LINEAR if the guest \
             never set it). This avoids flat-black/white-textured geometry on \
             strict drivers (Mali, Adreno) for iOS games that don't ship \
             mipmaps. Disable with --no-fix-texture-min-filter if a game \
             genuinely needs mipmap minification."
        );
    } else {
        log!(
            "fix_texture_min_filter: OFF. If textured geometry renders as \
             flat black or white shapes on this device, enable the fix-up \
             with --fix-texture-min-filter (or remove \
             --no-fix-texture-min-filter from your options)."
        );
    }
}

/// Identifies the sharegroup of an EAGLContext. Contexts in the same
/// sharegroup share their renderbuffer drawable bindings.
pub(super) fn sharegroup_key(host_obj: &EAGLContextHostObject) -> usize {
    Rc::as_ptr(&host_obj.renderbuffer_drawable_bindings) as *const () as usize
}

/// Sizes of the textures [present_renderbuffer] keeps between frames.
#[derive(Default, Copy, Clone)]
pub(super) struct PresentTextureSizes {
    frame: Option<(GLsizei, GLsizei)>,
    upscaled: Option<(GLsizei, GLsizei)>,
    /// See [read_framebuffer] (0 if none yet).
    read_framebuffer: GLuint,
}

/// Get the framebuffer object used for reading from the app's renderbuffers,
/// creating it if it doesn't exist (yet). It's kept rather than deleted after
/// use, because that's faster, and because deleting framebuffers crashes the
/// Android emulator's OpenGL ES 1.1 implementation. (Unlike the objects
/// below, it can't just be given a reserved name, as that emulator only
/// accepts names from glGenFramebuffersOES.)
unsafe fn read_framebuffer(gles: &mut dyn GLES, cached: &mut GLuint) -> GLuint {
    if *cached == 0 || gles.IsFramebufferOES(*cached) == 0 {
        gles.GenFramebuffersOES(1, cached);
    }
    *cached
}

// Names of the objects [present_renderbuffer] keeps between frames, rather
// than creating new ones every frame (which is slow on some GPUs). They're in
// the app's context, so very high names are used: some apps don't call
// glGenTextures and just use small numbers as names.
const PRESENT_FRAME_TEXTURE: GLuint = 0xFFFF_FF01;
const PRESENT_UPSCALED_TEXTURE: GLuint = 0xFFFF_FF02;
const PRESENT_UPSCALED_FRAMEBUFFER: GLuint = 0xFFFF_FF03;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation EAGLContext: NSObject

+ (id)alloc {
    let host_object = Box::new(EAGLContextHostObject {
        gles_ctx: None,
        present_token: NEXT_PRESENT_TOKEN.fetch_add(1, Ordering::Relaxed),
        shadow: GLShadowState::default(),
        api: kEAGLRenderingAPIOpenGLES1,
        drawable_framebuffer: 0,
        renderbuffer_drawable_bindings: Rc::new(RefCell::new(HashMap::new())),
        fps_counter: None,
        next_frame_due: None,
        mapped_buffers: HashMap::new(),
        matrix_palette: Some(Default::default()),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)currentContext {
    env.framework_state.opengles.current_ctx_for_thread(env.current_thread).unwrap_or(nil)
}
+ (bool)setCurrentContext:(id)context { // EAGLContext*
    retain(env, context);

    let current_ctx = env.framework_state.opengles.current_ctx_for_thread(env.current_thread);

    if let Some(old_ctx) = std::mem::take(current_ctx) {
        release(env, old_ctx);
    }

    // reborrow
    let current_ctx = env.framework_state.opengles.current_ctx_for_thread(env.current_thread);

    if context != nil {
        *current_ctx = Some(context);
    }

    true
}

- (id)initWithAPI:(EAGLRenderingAPI)api sharegroup:(id)group {
    if api == kEAGLRenderingAPIOpenGLES3 && !es3_is_supported() {
        // Apps fall back to OpenGL ES 2.0, like on devices without ES 3.0.
        log!("App requested EAGL initWithAPI:3, returning nil as OpenGL ES 3.0 isn't supported on this host");
        return nil;
    }
    if api != kEAGLRenderingAPIOpenGLES1
        && api != kEAGLRenderingAPIOpenGLES2
        && api != kEAGLRenderingAPIOpenGLES3
    {
        log!(
            "App requested EAGL initWithAPI:{} sharegroup:{:?}, returning nil as we only support APIs 1, 2 and 3",
            api,
            group
        );
        return nil;
    }

    if group == nil {
        return msg![env; this initWithAPI:api];
    }

    let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
    let prev_context = env.objc.borrow_mut::<EAGLContextHostObject>(group).gles_ctx.as_mut().unwrap();

    // This is sort of a hack - we set the "current" context, then immediately
    // drop it. Since we know all the code between here and creating the new
    // context, we know that there won't be any context switches, so it's fine
    // to do this.
    {
        let _prev_ctx = prev_context.make_current(window);
    }
    env.window.as_mut().unwrap().set_share_with_current_context(true);

    let effective_api = effective_eagl_api(
        api,
        env.options.prefer_gles2_context,
        env.options.force_gles1_context,
    );

    let mut gles_ins = match effective_api {
        kEAGLRenderingAPIOpenGLES3 => create_gles3_ctx(env),
        kEAGLRenderingAPIOpenGLES2 => create_gles2_ctx(env),
        _ => create_gles1_ctx(env),
    };

    let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
    let drawable_framebuffer = {
        #[allow(unused_mut)]
        let mut gles_ctx = gles_ins.make_current(window);
        log!("Driver info: {}", unsafe { gles_ctx.driver_description() });
        log_fix_min_filter_status(env.options.fix_texture_min_filter);
        #[allow(unused_mut)]
        let mut framebuffer: GLint = 0;
        #[cfg(target_os = "ios")]
        unsafe {
            gles_ctx.GetIntegerv(gles11::FRAMEBUFFER_BINDING_OES, &mut framebuffer);
        }
        framebuffer as GLuint
    };

    let host_object = env.objc.borrow_mut::<EAGLContextHostObject>(this);
    host_object.gles_ctx = Some(gles_ins);
    host_object.api = effective_api;
    host_object.drawable_framebuffer = drawable_framebuffer;

    env.window.as_mut().unwrap().set_share_with_current_context(false);

    env.objc.borrow_mut::<EAGLContextHostObject>(this).renderbuffer_drawable_bindings = env.objc.borrow::<EAGLContextHostObject>(group).renderbuffer_drawable_bindings.clone();
    let buffers = env.objc.borrow::<EAGLContextHostObject>(group).matrix_palette.as_ref().unwrap().buffers.clone();
    env.objc.borrow_mut::<EAGLContextHostObject>(this).matrix_palette.as_mut().unwrap().buffers = buffers;
    this
}

- (id)initWithAPI:(EAGLRenderingAPI)api {
    if api == kEAGLRenderingAPIOpenGLES3 && !es3_is_supported() {
        // Apps fall back to OpenGL ES 2.0, like on devices without ES 3.0.
        log!("App requested EAGL initWithAPI:3, returning nil as OpenGL ES 3.0 isn't supported on this host");
        return nil;
    }
    if api != kEAGLRenderingAPIOpenGLES1
        && api != kEAGLRenderingAPIOpenGLES2
        && api != kEAGLRenderingAPIOpenGLES3
    {
        log!(
            "App requested EAGL initWithAPI:{}, returning nil as we only support APIs 1, 2 and 3",
            api
        );
        return nil;
    }

    let effective_api = effective_eagl_api(
        api,
        env.options.prefer_gles2_context,
        env.options.force_gles1_context,
    );

    let mut gles_ins = match effective_api {
        kEAGLRenderingAPIOpenGLES3 => create_gles3_ctx(env),
        kEAGLRenderingAPIOpenGLES2 => create_gles2_ctx(env),
        _ => create_gles1_ctx(env),
    };

    let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
    let drawable_framebuffer = {
        #[allow(unused_mut)]
        let mut gles_ctx = gles_ins.make_current(window);
        log!("Driver info: {}", unsafe { gles_ctx.driver_description() });
        log_fix_min_filter_status(env.options.fix_texture_min_filter);
        #[allow(unused_mut)]
        let mut framebuffer: GLint = 0;
        #[cfg(target_os = "ios")]
        unsafe {
            gles_ctx.GetIntegerv(gles11::FRAMEBUFFER_BINDING_OES, &mut framebuffer);
        }
        framebuffer as GLuint
    };

    let host_object = env.objc.borrow_mut::<EAGLContextHostObject>(this);
    host_object.gles_ctx = Some(gles_ins);
    host_object.api = effective_api;
    host_object.drawable_framebuffer = drawable_framebuffer;

    this
}

- (EAGLRenderingAPI)API {
    env.objc.borrow::<EAGLContextHostObject>(this).api
}

- (id)sharegroup {
    // We use object itself as the sharegroup.
    // Check initWithAPI:sharegroup: for more info
    this
}

- (())dealloc {
    env.framework_state.opengles.pending_errors.remove(&this);
    let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(this);
    for &(guest_buf, _host_buf, _size) in host_obj.mapped_buffers.values() {
        env.mem.free(guest_buf);
    }
    if Rc::strong_count(&host_obj.renderbuffer_drawable_bindings) == 1 {
        env.framework_state
            .opengles
            .present_texture_sizes
            .remove(&sharegroup_key(host_obj));
        let bindings = std::mem::take(&mut host_obj.renderbuffer_drawable_bindings);
        for (_renderbuffer, drawable) in bindings.take() {
            release(env, drawable);
        }
    }
    env.objc.dealloc_object(this, &mut env.mem);
}

- (bool)renderbufferStorage:(NSUInteger)target
               fromDrawable:(id)drawable { // EAGLDrawable (always CAEAGLayer*)
    log_dbg!("[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}]", target, drawable);

    if target != gles11::RENDERBUFFER_OES {
        log!(
            "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] invalid target; returning NO",
            target,
            drawable
        );
        return false;
    }

    // Apple's `EAGLContext` documentation for
    // `-renderbufferStorage:fromDrawable:` states that passing `nil` for the
    // drawable "deletes any earlier binding" of the currently bound
    // renderbuffer to a drawable and returns `YES`. touchHLE used to
    // assert against this case (`drawable != nil`), which crashed apps
    // that legitimately unbind during teardown (e.g. Resident Evil 4 —
    // HyperHLE log #5 — calls `renderbufferStorage:fromDrawable:nil`
    // while tearing down its EAGL surface during a scene transition).
    //
    // Spec behaviour: look up the currently bound renderbuffer (via
    // `RENDERBUFFER_BINDING_OES`), drop its entry from the
    // (renderbuffer -> drawable) map, and release the retained drawable.
    // No new storage is allocated in this case.
    if drawable == nil {
        let Some(window) = env.window.as_mut() else {
            log_dbg!(
                "[EAGLContext renderbufferStorage:{:#x} fromDrawable:nil] ignored in headless mode",
                target
            );
            return true;
        };
        let current_renderbuffer = {
            let Some(mut gles) = super::sync_context(
                &mut env.framework_state.opengles,
                &mut env.objc,
                window,
                env.current_thread,
            ) else {
                // No current EAGL context: there can't be a meaningful
                // renderbuffer binding to remove. Apple-style soft failure.
                log!(
                    "[EAGLContext renderbufferStorage:{:#x} fromDrawable:nil] \
                     called with no current GL context for thread {}; \
                     treating as no-op.",
                    target,
                    env.current_thread
                );
                return true;
            };
            let mut renderbuffer: gles11::types::GLint = 0;
            unsafe {
                gles.GetIntegerv(gles11::RENDERBUFFER_BINDING_OES, &mut renderbuffer);
            }
            renderbuffer as gles11::types::GLuint
        };

        let removed = {
            let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(this);
            host_obj
                .renderbuffer_drawable_bindings
                .borrow_mut()
                .remove(&current_renderbuffer)
        };
        if let Some(old_drawable) = removed {
            release(env, old_drawable);
        } else {
            log_dbg!(
                "[EAGLContext renderbufferStorage:{:#x} fromDrawable:nil]: \
                 no drawable was bound to renderbuffer {} — nothing to unbind.",
                target,
                current_renderbuffer
            );
        }
        return true;
    }

    let props: id = msg![env; drawable drawableProperties];

    let format_key = get_static_str(env, kEAGLDrawablePropertyColorFormat);
    let format_rgba8 = get_static_str(env, kEAGLColorFormatRGBA8);
    let format_rgb565 = get_static_str(env, kEAGLColorFormatRGB565);

    let format: id = msg![env; props objectForKey:format_key];
    // Theoretically this should map formats like:
    // - kColorFormatRGBA8 => RGBA8_OES
    // - kColorFormatRGB565 => RGB565_OES
    // However, the specification of EXT_framebuffer_object allows the
    // implementation to arbitrarily restrict which formats can be rendered to,
    // and it seems like RGB565 isn't supported, at least on a machine with
    // Intel HD Graphics 615 running macOS Monterey. I don't think RGBA8 is
    // guaranteed either, but it at least seems to work.
    if !msg![env; format isEqual:format_rgba8] && !msg![env; format isEqual:format_rgb565] {
        log!("[renderbufferStorage:{:?} fromDrawable:{:?}] Warning: unhandled format {:?}, using RGBA8", target, drawable, format);
    }
    let internalformat = gles11::RGBA8_OES;

    let (width, height) = {
        // ULTRAHLE_MINIONJUMP_RENDERBUFFER_BEGIN
        if matches!(
            env.bundle.bundle_identifier(),
            "com.apprisetec9.minionjump" | "com.risinghighapps.kingdomprincepro"
        ) {
            log!("UltraHLE MinionJump: forcing EAGL renderbuffer storage to 1024x768");
            (1024, 768)
        } else {
            let bounds: CGRect = msg![env; drawable bounds];
            let CGSize { width, height } = bounds.size;

            // Apple's `-renderbufferStorage:fromDrawable:` derives the
            // renderbuffer size from the CAEAGLayer's bounds. Some apps (e.g.
            // Beyond Gravity — HyperHLE log #1) momentarily present a layer
            // whose `bounds.size` is bogus (non-finite, negative, or zero)
            // during an unbind/rebind cycle while tearing down and recreating
            // their EAGL surface. touchHLE used to `assert!` the size was in a
            // sane range, which aborted the whole emulator. Real iOS never
            // crashes here — it simply allocates a renderbuffer sized to the
            // (screen-sized) drawable. Match that by falling back to the main
            // screen's bounds when the drawable reports an invalid size.
            let size_is_valid = |v: f32| v.is_finite() && (1.0..(u32::MAX as f32)).contains(&v);
            let (width, height) = if size_is_valid(width) && size_is_valid(height) {
                (width, height)
            } else {
                let screen: id = msg_class![env; UIScreen mainScreen];
                let screen_bounds: CGRect = msg![env; screen bounds];
                // Copy the fields out of the packed CGSize before using them
                // (taking a reference to a packed struct field is UB / a
                // compile error).
                let fallback_width = screen_bounds.size.width;
                let fallback_height = screen_bounds.size.height;
                log!(
                    "[renderbufferStorage:{:?} fromDrawable:{:?}] Warning: drawable \
                     reported invalid bounds size {}x{}; falling back to main screen \
                     bounds {}x{}",
                    target,
                    drawable,
                    width,
                    height,
                    fallback_width,
                    fallback_height
                );
                (fallback_width, fallback_height)
            };
            // A full-screen EAGL layer sized in points gets its backing
            // pixel resolution from the layer's `contentsScale` (which
            // init_common in ui_view.rs seeds with UIScreen.scale, and which
            // apps may override via setContentsScale:). Without honouring it,
            // retina devices (iPhone 4/5/5c, iPad 3/4/5, iPad mini 2/3,
            // iPod touch 4/5) would allocate a half-size renderbuffer and the
            // app would render zoomed-in and cropped. `scale_hack` is a
            // user-facing multiplier applied on top, as before.
            let contents_scale = {
                let layer_contents_scale: CGFloat = msg![env; drawable contentsScale];
                if layer_contents_scale.is_finite() && layer_contents_scale > 0.0 {
                    layer_contents_scale
                } else {
                    1.0
                }
            };

            let scale_hack = env.options.scale_hack.get();

            let mut width = (width * contents_scale).round() as u32 * scale_hack;
            let mut height = (height * contents_scale).round() as u32 * scale_hack;

            // If even the fallback produced a degenerate size, clamp to a
            // minimum 1x1 so the GL call below cannot receive a zero extent.
            width = width.max(1);
            height = height.max(1);
            // ... and to a sane maximum, so a bogus bounds/contentsScale/
            // scale-hack combination can't overflow the GLsizei conversion
            // below (a host panic) or ask the driver for gigabytes.
            const MAX_RENDERBUFFER_DIMENSION: u32 = 16384;
            if width > MAX_RENDERBUFFER_DIMENSION || height > MAX_RENDERBUFFER_DIMENSION {
                log!(
                    "[renderbufferStorage:{:?} fromDrawable:{:?}] Warning: clamping \
                     oversized renderbuffer {}x{} to at most {}x{}",
                    target,
                    drawable,
                    width,
                    height,
                    MAX_RENDERBUFFER_DIMENSION,
                    MAX_RENDERBUFFER_DIMENSION
                );
                width = width.min(MAX_RENDERBUFFER_DIMENSION);
                height = height.min(MAX_RENDERBUFFER_DIMENSION);
            }

            // RBSIZE-DIAG: log renderbuffer storage dimensions each time so a
            // mid-run resize (loading screen vs game scene) is visible.
            log_dbg!(
                "RBSIZE-DIAG renderbufferStorage drawable={:?} -> {}x{} (contents_scale={}, scale_hack={})",
                drawable, width, height, contents_scale, scale_hack
            );

            if std::env::var_os("TOUCHHLE_FORCE_LANDSCAPE_RENDERBUFFER").is_some() {
                let is_landscape = env
                    .window
                    .as_ref()
                    .map(|window| {
                        !matches!(
                            window.current_rotation(),
                            crate::window::DeviceOrientation::Portrait
                        )
                    })
                    .unwrap_or(false);

                if is_landscape && height > width {
                    log!(
                        "TOUCHHLE_FORCE_LANDSCAPE_RENDERBUFFER=1: swapping EAGL renderbuffer storage from {}x{} to {}x{}",
                        width,
                        height,
                        height,
                        width
                    );
                    std::mem::swap(&mut width, &mut height);
                } else {
                    log!(
                        "TOUCHHLE_FORCE_LANDSCAPE_RENDERBUFFER=1: keeping EAGL renderbuffer storage at {}x{} (is_landscape={})",
                        width,
                        height,
                        is_landscape
                    );
                }
            }

            (width, height)
        }
        // ULTRAHLE_MINIONJUMP_RENDERBUFFER_END
    };

    let guest_size = (width, height);
    let (width, height) = super::p2g_wide::drawable_size(env, width, height);

    // Apple's documentation states that the receiver must be the current
    // context when calling `renderbufferStorage:fromDrawable:`.  If no
    // context is current for this thread but `this` has a valid backing GLES
    // context, temporarily make `this` the current context so the GL call
    // succeeds, then restore the previous state.  This matches the behaviour
    // observed on real iOS where calling the method on a non-current context
    // still works as long as the receiver has been initialised.
    //
    // Note: `window` must be borrowed from `env` AFTER any calls to
    // `retain`/`release` because those also borrow `env` mutably.
    let prior_ctx = *env.framework_state.opengles.current_ctx_for_thread(env.current_thread);
    let needs_temp_current = prior_ctx.is_none() && {
        env.objc.borrow::<EAGLContextHostObject>(this).gles_ctx.is_some()
    };
    if needs_temp_current {
        log_dbg!(
            "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] \
             no current context for thread {}; temporarily making this context current.",
            target, drawable, env.current_thread
        );
        retain(env, this);
        *env.framework_state.opengles.current_ctx_for_thread(env.current_thread) = Some(this);
    }

    // Run the actual GL storage allocation inside a nested scope so that
    // `window` (which mutably borrows `env.window`) is dropped before we need
    // to call `retain`/`release` with a full `&mut env` borrow below.
    let renderbuffer_result: Option<u32> = {
        let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
        match super::sync_context(
            &mut env.framework_state.opengles,
            &mut env.objc,
            window,
            env.current_thread,
        ) {
            None => {
                log!(
                    "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] \
                     called with no current GL context for thread {}; failing \
                     the call instead of crashing.",
                    target,
                    drawable,
                    env.current_thread
                );
                None
            }
            Some(mut gles) => {
                // Clear any pre-existing error so we can detect failure of the
                // storage allocation reliably.
                unsafe { while gles.GetError() != gles11::NO_ERROR {} }
                unsafe { gles.RenderbufferStorageOES(target, internalformat, width.try_into().unwrap(), height.try_into().unwrap()); }
                let needs_fallback = unsafe { gles.GetError() != gles11::NO_ERROR };
                let alloc_ok = if needs_fallback {
                    // RGBA8 is optional in OpenGL ES 1.1 Common Profile
                    // (requires OES_rgb8_rgba8). Fall back to RGBA4 (0x8056)
                    // which is required by OES_framebuffer_object.
                    const GL_RGBA4: gles11::types::GLenum = 0x8056;
                    unsafe { gles.RenderbufferStorageOES(target, GL_RGBA4, width.try_into().unwrap(), height.try_into().unwrap()); }
                    unsafe { gles.GetError() == gles11::NO_ERROR }
                } else {
                    true
                };
                if !alloc_ok {
                    log!(
                        "[EAGLContext renderbufferStorage:{:#x} fromDrawable:{:?}] \
                         failed to allocate renderbuffer storage (tried RGBA8 and RGBA4)",
                        target,
                        drawable
                    );
                    None
                } else {
                    let mut renderbuffer: gles11::types::GLint = 0;
                    unsafe { gles.GetIntegerv(gles11::RENDERBUFFER_BINDING_OES, &mut renderbuffer); }
                    Some(renderbuffer as u32)
                }
            }
        }
    };

    // `window` borrow dropped here — safe to use `retain`/`release` again.
    // `None` means either no GL context was available, or storage allocation
    // failed.
    let renderbuffer = match renderbuffer_result {
        None => {
            if needs_temp_current {
                *env.framework_state.opengles.current_ctx_for_thread(env.current_thread) = None;
                release(env, this);
            }
            return false;
        }
        Some(rb) => rb,
    };

    super::p2g_wide::drawable(env, renderbuffer, guest_size, (width, height));

    // Restore the previous thread-local context (if we temporarily set `this`
    // as the current context earlier in this call).
    if needs_temp_current {
        *env.framework_state.opengles.current_ctx_for_thread(env.current_thread) = None;
        release(env, this);
    }

    retain(env, drawable);
    let host_obj = env.objc.borrow_mut::<EAGLContextHostObject>(this);
    let maybe_old_drawable = host_obj.renderbuffer_drawable_bindings.borrow_mut().insert(
        renderbuffer,
        drawable
    );
    if let Some(old_drawable) = maybe_old_drawable {
        release(env, old_drawable);
    }

    true
}


- (bool)presentRenderbuffer:(NSUInteger)target {
    if target != gles11::RENDERBUFFER_OES {
        log!("[EAGLContext presentRenderbuffer:{:#x}] invalid target; returning NO", target);
        return false;
    }
    // ES 2.0 and 3.0 contexts may have no fixed-function pipeline, which the
    // overlay compositor and the ES 1.1 presenter need.
    let programmable = env.objc.borrow::<EAGLContextHostObject>(this).api != kEAGLRenderingAPIOpenGLES1;

    let diagnostic_started = env.options.performance_test.map(|_| Instant::now());
    let mut diagnostic_slow = false;
    let mut diagnostic_readback = Duration::ZERO;

    // The presented frame should be displayed ASAP, but the next one must be
    // delayed, so this needs to be checked before returning.
    let sleep_for = limit_framerate(&mut env.objc.borrow_mut::<EAGLContextHostObject>(this).next_frame_due, &env.options);

    if env.options.print_fps {
        env
            .objc
            .borrow_mut::<EAGLContextHostObject>(this)
            .fps_counter
            .get_or_insert_with(FpsCounter::start)
            .count_frame(format_args!("EAGLContext {this:?}"), true);
    }

    // The fullscreen layer may have other layers on top of it (overlays).
    // Drawing them composites UIKit layers (and may call the app's drawing
    // code), which should only happen on the main thread, like normal
    // composition. Apps that present from another thread use the slow path
    // in that case.
    let found = find_fullscreen_eagl_layer_with_overlays(env);
    let found_layer = found.as_ref().map_or(nil, |found| found.layer);
    let (fullscreen_layer, quarter_turns, overlays) = match found {
        Some(found) if found.overlays.is_empty() || (env.current_thread == 0 && !programmable) => {
            (found.layer, found.quarter_turns, found.overlays)
        }
        _ => (nil, 0, Vec::new()),
    };

    // Unclear from documentation if this method requires the context to be
    // current, but it would be weird if it didn't?
    let window = env.window.as_mut().expect("OpenGL ES is not supported in headless mode");
    let Some(mut gles) = super::sync_context(&mut env.framework_state.opengles, &mut env.objc, window, env.current_thread) else {
        log_dbg!("presentRenderbuffer: no current context for thread {}", env.current_thread);
        return false;
    };

    let renderbuffer: GLuint = unsafe {
        let mut renderbuffer = 0;
        gles.GetIntegerv(gles11::RENDERBUFFER_BINDING_OES, &mut renderbuffer);
        renderbuffer as _
    };
    // ES 2.0 on OpenGL 2.1 (see GLES1OnGL2Context::new_programmable) renders
    // to a drawable of its own, so it can only be presented by reading back.
    let read_back_only = programmable && !gles.is_es2();

    std::mem::drop(gles);
    let fullscreen_layer = if read_back_only { nil } else { fullscreen_layer };

    let Some(&drawable) = env
        .objc
        .borrow::<EAGLContextHostObject>(this)
        .renderbuffer_drawable_bindings
        .borrow()
        .get(&renderbuffer) else {
        log_dbg!("Can't present a renderbuffer {:?} not bound to a drawable!", renderbuffer);
        return false;
    };

    // We're presenting to the opaque CAEAGLLayer that covers the screen.
    // We can use the fast path where we skip composition and present directly.
    if drawable == fullscreen_layer {
        log_dbg!(
            "Layer {:?} is the fullscreen layer, presenting renderbuffer {:?} directly (fast path).",
            drawable,
            renderbuffer,
        );
        // re-borrow
        unsafe {
            present_renderbuffer(
                env,
                quarter_turns,
                /* copy: */ true,
                /* swap: */ overlays.is_empty(),
            );
        }
        if !overlays.is_empty() {
            // Draw the layers on top of it, rather than taking the slow path
            // below.
            crate::frameworks::core_animation::present_overlays(env, &overlays);
            crate::frameworks::core_animation::note_overlay_present(env, drawable, this);
        }
    } else {
        if fullscreen_layer != nil && overlays.is_empty() {
            // If there's a single layer that covers the screen, and this isn't
            // it, there's no point in presenting the output because it won't be
            // seen. (If there are overlays, this might be one of them, e.g. a
            // second OpenGL ES view, so it's presented the slow way.) Using a
            // noisy log because it's a weird scenario and might indicate a
            // bug.
            log!(
                "Layer {:?} is not the fullscreen layer {:?}, skipping presentation of renderbuffer {:?}!",
                drawable,
                fullscreen_layer,
                renderbuffer,
            );
            if let Some(sleep_for) = sleep_for {
                env.sleep(sleep_for);
            }
            return true;
        }

        // The very slow and inefficient path: not only does glReadPixels()
        // block the thread until rendering finishes, but the result has to be
        // copied back to system RAM, and then will have to be copied to VRAM
        // again during composition.
        // find_fullscreen_eagl_layer_with_overlays() exists to avoid this.
        log_dbg!(
            "There is no fullscreen layer, presenting renderbuffer {:?} to layer {:?} by copying to RAM (slow path).",
            renderbuffer,
            drawable,
        );
        // If this is the full-screen layer (but it's presented from another
        // thread), the compositor must not redraw an older frame presented
        // the fast way instead of this one (see recomposite_overlays).
        if drawable == found_layer {
            crate::frameworks::core_animation::forget_overlay_present(env);
        }
        let pixels_vec = get_pixels_vec_for_presenting(env, drawable);
        let sharegroup = sharegroup_key(env.objc.borrow::<EAGLContextHostObject>(this));
        let mut sizes = env.framework_state.opengles.present_texture_sizes.get(&sharegroup).copied().unwrap_or_default();
        // re-borrow
        diagnostic_slow = true;
        let readback_started = diagnostic_started.map(|_| Instant::now());
        let (pixels_vec, width, height) = super::without_leaving_errors(env, |env| {
            let mut gles = super::sync_context(&mut env.framework_state.opengles, &mut env.objc, env.window.as_mut().unwrap(), env.current_thread).unwrap();
            unsafe {
                let framebuffer = read_framebuffer(gles.as_mut(), &mut sizes.read_framebuffer);
                read_renderbuffer(gles.as_mut(), pixels_vec, framebuffer)
            }
        });
        env.framework_state.opengles.present_texture_sizes.insert(sharegroup, sizes);
        if let Some(started) = readback_started {
            diagnostic_readback = started.elapsed();
        }
        present_pixels(env, drawable, pixels_vec, width, height);
    }

    if let Some(started) = diagnostic_started {
        crate::performance::frame(env, started, diagnostic_slow, diagnostic_readback, sleep_for);
    }
    if let Some(sleep_for) = sleep_for {
        env.sleep(sleep_for);
    }

    true
}

@end

};

/// Implement framerate limiting.
///
/// The real iPhone OS seems to force 60Hz v-sync in `presentRenderbuffer:`.
/// touchHLE does not force v-sync, and its users might not have 60Hz monitors
/// in any case, so to avoid excessive FPS or games running too fast, we need
/// to simulate it.
///
/// V-sync is essentially a limiter with no "slop", or allowance for frames
/// arriving late: if the frame misses a 60Hz interval, it must wait until the
/// next one. This is quite harsh: if frames consistently arrive very slightly
/// late, the framerate is halved!
///
/// Most games already use NSTimer, which is itself a v-sync-like limiter.
/// For the remainder, let's do something a bit kinder, for the benefit of users
/// with slow systems or which are using high scale hack settings: allow at most
/// an interval's worth of accumulated slop. Allowing infinite accumulation of
/// slop is not desirable, because if the game is running slowly for a long time
/// and suddenly speeds back up, it will then run too fast for a long time.
fn limit_framerate(next_frame_due: &mut Option<Instant>, options: &Options) -> Option<Duration> {
    let interval = {
        let fps = options.fps_limit?;
        1.0 / fps
    };
    let interval_rust = Duration::from_secs_f64(interval);

    let &mut Some(current_frame_due) = next_frame_due else {
        // First frame presented: no delay yet.
        *next_frame_due = Some(Instant::now() + interval_rust);
        return None;
    };

    let now = Instant::now();
    *next_frame_due = if now > current_frame_due + interval_rust {
        // Too much slop has accumulated. Make the next frame wait for the next
        // interval.
        log_dbg!("Too much slop accumulated, skipping an interval.");
        Some(
            current_frame_due
                + Duration::from_secs_f64(
                    interval * (((now - current_frame_due).as_secs_f64() / interval).ceil()),
                ),
        )
    } else {
        // Time next frame based on when the current frame was due, not
        // the current time, so as to allow some slop.
        Some(current_frame_due + interval_rust)
    };

    if now < current_frame_due {
        // Frame was presented early, delay it to maintain framerate limit.
        Some(current_frame_due.saturating_duration_since(now))
    } else {
        // Frame was presented on time or late, don't delay.
        None
    }
}

// These helper functions make the state backup code easier to read, but
// more importantly, they make it free of mutable variables that wouldn't
// get caught by Rust's unused variable warnings, which are useful to check
// we actually restore the stuff we back up.

unsafe fn get_ptr(gles: &mut dyn GLES, pname: GLenum) -> *const GLvoid {
    let mut ptr = std::ptr::null();
    gles.GetPointerv(pname, &mut ptr);
    ptr
}
// Safety: caller's responsibility to use appropriate N.
unsafe fn get_ints<const N: usize>(gles: &mut dyn GLES, pname: GLenum) -> [GLint; N] {
    let mut res = [0; N];
    gles.GetIntegerv(pname, res.as_mut_ptr());
    res
}
// Safety: caller's responsibility to only use this for scalars.
unsafe fn get_int(gles: &mut dyn GLES, pname: GLenum) -> GLint {
    get_ints::<1>(gles, pname)[0]
}
// Safety: caller's responsibility to use appropriate N.
unsafe fn get_tex_env_ints<const N: usize>(
    gles: &mut dyn GLES,
    target: GLenum,
    pname: GLenum,
) -> [GLint; N] {
    let mut res = [0; N];
    gles.GetTexEnviv(target, pname, res.as_mut_ptr());
    res
}
// Safety: caller's responsibility to only use this for scalars.
unsafe fn get_tex_env_int(gles: &mut dyn GLES, target: GLenum, pname: GLenum) -> GLint {
    get_tex_env_ints::<1>(gles, target, pname)[0]
}
// Safety: caller's responsibility to use appropriate N.
unsafe fn get_floats<const N: usize>(gles: &mut dyn GLES, pname: GLenum) -> [GLfloat; N] {
    let mut res = [0.0; N];
    gles.GetFloatv(pname, res.as_mut_ptr());
    res
}
unsafe fn get_renderbuffer_size(gles: &mut dyn GLES) -> (GLsizei, GLsizei) {
    let mut width: GLint = 0;
    let mut height: GLint = 0;
    gles.GetRenderbufferParameterivOES(
        gles11::RENDERBUFFER_OES,
        gles11::RENDERBUFFER_WIDTH_OES,
        &mut width,
    );
    gles.GetRenderbufferParameterivOES(
        gles11::RENDERBUFFER_OES,
        gles11::RENDERBUFFER_HEIGHT_OES,
        &mut height,
    );
    (width, height)
}

/// Copies the pixels in a renderbuffer bound to `GL_RENDERBUFFER_BINDING_OES`
/// (which should be provided by the app) to a provided [Vec], trying to avoid
/// noticeably modifying OpenGL ES state while doing so.
///
/// This uses `glReadPixels()`, with all the associated performance risks. Any
/// existing content in the [Vec] will bereplaced. The format is RGBA8.
/// The returned values are the [Vec], the width and height.
///
/// The provided context must be current.
unsafe fn read_renderbuffer(
    gles: &mut dyn GLES,
    mut pixel_buffer: Vec<u8>,
    src_framebuffer: GLuint,
) -> (Vec<u8>, u32, u32) {
    let renderbuffer: GLuint = get_int(gles, gles11::RENDERBUFFER_BINDING_OES) as _;
    let (width, height) = get_renderbuffer_size(gles);
    let width_u32: u32 = width.try_into().unwrap();
    let height_u32: u32 = height.try_into().unwrap();

    // To avoid confusing the guest app, we need to be able to undo any
    // state changes we make.
    let old_framebuffer: GLuint = get_int(gles, gles11::FRAMEBUFFER_BINDING_OES) as _;

    // Use a framebuffer to read from the renderbuffer
    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, src_framebuffer);
    gles.FramebufferRenderbufferOES(
        gles11::FRAMEBUFFER_OES,
        gles11::COLOR_ATTACHMENT0_OES,
        gles11::RENDERBUFFER_OES,
        renderbuffer,
    );

    // Read the pixels
    let size = (width_u32 as usize)
        .checked_mul(height_u32 as usize)
        .unwrap()
        .checked_mul(4)
        .unwrap();
    pixel_buffer.clear();
    pixel_buffer.reserve_exact(size);
    let before = Instant::now();
    gles.ReadPixels(
        0,
        0,
        width,
        height,
        gles11::RGBA,
        gles11::UNSIGNED_BYTE,
        pixel_buffer.as_mut_ptr() as *mut _,
    );
    log_dbg!(
        "glReadPixels(0, 0, {}, {}, …) took {:?}",
        width,
        height,
        Instant::now().saturating_duration_since(before)
    );
    pixel_buffer.set_len(size);
    // Restore the framebuffer binding
    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, old_framebuffer);

    (pixel_buffer, width_u32, height_u32)
}

/// Draw the frame most recently presented by `context` to the window again,
/// without swapping the window's buffers, so that the layers on top of it can
/// be redrawn when the app isn't presenting new frames (see
/// [crate::frameworks::core_animation::present_overlays]). Returns false if
/// that isn't possible.
pub fn redraw_last_frame(env: &mut Environment, context: id, quarter_turns: u8) -> bool {
    // The context may have been destroyed since.
    if !env
        .objc
        .get_host_object(context)
        .is_some_and(|host_obj| host_obj.as_any().is::<EAGLContextHostObject>())
    {
        return false;
    }
    // Apps may no longer have the context current (e.g. while paused), so it
    // is made current temporarily. The OpenGL ES context that is actually
    // current is synced again before the app next uses OpenGL ES.
    let previous = env
        .framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread)
        .replace(context);
    let result = unsafe {
        present_renderbuffer(
            env,
            quarter_turns,
            /* copy: */ false,
            /* swap: */ false,
        )
    };
    *env.framework_state
        .opengles
        .current_ctx_for_thread(env.current_thread) = previous;
    result
}

/// Copies the pixels in a renderbuffer bound to `GL_RENDERBUFFER_BINDING_OES`
/// (which should be provided by the app) to a texture and presents it with
/// [present_frame], trying to avoid noticeably modifying OpenGL ES state while
/// doing so. The front and back buffers are then swapped.
///
/// If `copy` is false, the frame presented last time is presented again
/// (see [redraw_last_frame]); returns false if there isn't one. If `swap` is
/// false, the window's buffers aren't swapped afterwards, so more can be
/// drawn on top (see [crate::frameworks::core_animation::present_overlays]).
unsafe fn present_renderbuffer(
    env: &mut Environment,
    quarter_turns: u8,
    copy: bool,
    swap: bool,
) -> bool {
    super::without_leaving_errors(env, |env| {
        present_renderbuffer_inner(env, quarter_turns, copy, swap)
    })
}
unsafe fn present_renderbuffer_inner(
    env: &mut Environment,
    quarter_turns: u8,
    copy: bool,
    swap: bool,
) -> bool {
    // Save these for when we need to draw the frame
    let viewport = env.window.as_mut().unwrap().viewport();
    let pixel_filter =
        env.window.as_ref().unwrap().filter_mode() == crate::options::FilterMode::Pixel;
    // The layer may be rotated by quarter turns to cover the screen (see
    // find_fullscreen_eagl_layer_with_overlays), in addition to the rotation
    // for the device orientation.
    let rotation_matrix = env.window.as_mut().unwrap().rotation_matrix().multiply(
        &crate::matrix::Matrix::z_rotation((quarter_turns as f32) * std::f32::consts::FRAC_PI_2),
    );
    // If more is drawn on top afterwards, that draws the cursor instead, so
    // it's on top and not drawn twice.
    let virtual_cursor_visible_at = if swap {
        env.window.as_mut().unwrap().virtual_cursor_visible_at()
    } else {
        None
    };
    let sharp_upscale = env.window.as_ref().unwrap().sharp_upscale_factor();

    let (sharegroup, window_framebuffer, context_token) = {
        let ctx = env
            .framework_state
            .opengles
            .current_ctx_for_thread(env.current_thread)
            .unwrap();
        let host_obj = env.objc.borrow::<EAGLContextHostObject>(ctx);
        (
            sharegroup_key(host_obj),
            host_obj.drawable_framebuffer,
            host_obj.present_token,
        )
    };
    let mut sizes = env
        .framework_state
        .opengles
        .present_texture_sizes
        .get(&sharegroup)
        .copied()
        .unwrap_or_default();
    if !copy && sizes.frame.is_none() {
        return false;
    }

    let Some(gles_ctx) = super::get_thread_context(
        &mut env.framework_state.opengles,
        &mut env.objc,
        env.current_thread,
    ) else {
        return false;
    };

    let mut gles_boxed = gles_ctx.make_current(env.window.as_mut().unwrap());
    let gles = gles_boxed.as_mut();

    // A real OpenGL ES 2.0/3.0 (or OpenGL 3.3 core) context has no
    // fixed-function pipeline, so it's presented with a small shader instead.
    if gles.is_es2() && !gles.is_translator() {
        if !copy {
            return false;
        }
        let renderbuffer: GLuint = get_int(gles, gles11::RENDERBUFFER_BINDING_OES) as _;
        invalidate_present_cache_for_context(context_token);
        present_renderbuffer_es2(
            gles,
            renderbuffer,
            window_framebuffer,
            viewport,
            rotation_matrix,
            virtual_cursor_visible_at,
            /* present_finish: */ false,
        );
        std::mem::drop(gles_boxed);
        if swap {
            env.window.as_mut().unwrap().swap_window();
        }
        return true;
    }

    // We can't directly copy the content of the renderbuffer to the default
    // framebuffer (the window), but if we attach it to a framebuffer object, we
    // can use glCopyTexImage2D() to copy it to a texture, which we can then
    // draw to the default framebuffer via a textured quad, which can be
    // rotated, scaled or letterboxed as appropriate.

    // To avoid confusing the guest app, we need to be able to undo any
    // state changes we make.
    let old_framebuffer: GLuint = get_int(gles, gles11::FRAMEBUFFER_BINDING_OES) as _;
    let old_texture_2d: GLuint = get_int(gles, gles11::TEXTURE_BINDING_2D) as _;

    let texture = PRESENT_FRAME_TEXTURE;
    let (width, height) = if copy {
        let renderbuffer: GLuint = get_int(gles, gles11::RENDERBUFFER_BINDING_OES) as _;
        let (width, height) = get_renderbuffer_size(gles);

        // Use a framebuffer to read from the renderbuffer
        let src_framebuffer = read_framebuffer(gles, &mut sizes.read_framebuffer);
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, src_framebuffer);
        gles.FramebufferRenderbufferOES(
            gles11::FRAMEBUFFER_OES,
            gles11::COLOR_ATTACHMENT0_OES,
            gles11::RENDERBUFFER_OES,
            renderbuffer,
        );

        // Copy the pixels in the framebuffer to a texture. The texture is
        // reused if it has the right size (glIsTexture() checks it still
        // exists).
        let reuse = sizes.frame == Some((width, height)) && gles.IsTexture(texture) != 0;
        gles.BindTexture(gles11::TEXTURE_2D, texture);
        if reuse {
            gles.CopyTexSubImage2D(gles11::TEXTURE_2D, 0, 0, 0, 0, 0, width, height);
        } else {
            gles.CopyTexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGB as _,
                0,
                0,
                width,
                height,
                0,
            );
            sizes.frame = Some((width, height));
        }

        // Set the framebuffer binding back to zero, so rendering will go to
        // the default framebuffer (the window).
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, 0);
        (width, height)
    } else {
        // Present the copy made last time again.
        if gles.IsTexture(texture) == 0 {
            return false;
        }
        gles.BindTexture(gles11::TEXTURE_2D, texture);
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, 0);
        sizes.frame.unwrap()
    };
    // The texture will not have any mip levels so we must ensure the filter
    // does not use them, else rendering will fail.
    gles.TexParameteri(
        gles11::TEXTURE_2D,
        gles11::TEXTURE_MIN_FILTER,
        if pixel_filter {
            gles11::NEAREST
        } else {
            gles11::LINEAR
        } as _,
    );
    // The default magnification filter is LINEAR.
    if pixel_filter {
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_MAG_FILTER,
            gles11::NEAREST as _,
        );
    }

    // Reset various things that could affect the quad or virtual cursor we're
    // going to draw. Back up the old state while doing so, so it can be
    // restored later. The app's subsequent drawing will be messed up if we
    // don't restore it.
    let old_arrays = {
        let mut old_arrays = [gles11::FALSE; gles1_on_gl2::ARRAYS.len()];
        for (is_enabled, info) in old_arrays.iter_mut().zip(gles1_on_gl2::ARRAYS.iter()) {
            gles.GetBooleanv(info.name, is_enabled);
            gles.DisableClientState(info.name);
        }
        old_arrays
    };
    let old_capabilities = {
        let mut old_capabilities = [gles11::FALSE; gles1_on_gl2::CAPABILITIES.len()];
        for (is_enabled, &name) in old_capabilities
            .iter_mut()
            .zip(gles1_on_gl2::CAPABILITIES.iter())
        {
            gles.GetBooleanv(name, is_enabled);
            gles.Disable(name);
        }
        old_capabilities
    };
    let old_matrix_mode: GLenum = get_int(gles, gles11::MATRIX_MODE) as _;
    for mode in [gles11::MODELVIEW, gles11::PROJECTION, gles11::TEXTURE] {
        gles.MatrixMode(mode);
        gles.PushMatrix();
        gles.LoadIdentity();
    }
    let old_color: [GLfloat; 4] = get_floats(gles, gles11::CURRENT_COLOR);
    gles.Color4f(1.0, 1.0, 1.0, 1.0);

    // Back up other things that will be modified while drawing.
    let old_viewport: (GLint, GLint, GLsizei, GLsizei) = {
        let [x, y, width, height] = get_ints(gles, gles11::VIEWPORT);
        (x, y, width as _, height as _)
    };
    let old_clear_color: [GLfloat; 4] = get_floats(gles, gles11::COLOR_CLEAR_VALUE);
    let old_array_buffer: GLuint = get_int(gles, gles11::ARRAY_BUFFER_BINDING) as _;
    let old_vertex_array_binding: GLuint = get_int(gles, gles11::VERTEX_ARRAY_BUFFER_BINDING) as _;
    let old_vertex_array_size: GLint = get_int(gles, gles11::VERTEX_ARRAY_SIZE);
    let old_vertex_array_type: GLenum = get_int(gles, gles11::VERTEX_ARRAY_TYPE) as _;
    let old_vertex_array_stride: GLsizei = get_int(gles, gles11::VERTEX_ARRAY_STRIDE) as _;
    let old_vertex_array_pointer = get_ptr(gles, gles11::VERTEX_ARRAY_POINTER);
    let old_tex_coord_array_binding: GLuint =
        get_int(gles, gles11::TEXTURE_COORD_ARRAY_BUFFER_BINDING) as _;
    let old_tex_coord_array_size: GLint = get_int(gles, gles11::TEXTURE_COORD_ARRAY_SIZE);
    let old_tex_coord_array_type: GLenum = get_int(gles, gles11::TEXTURE_COORD_ARRAY_TYPE) as _;
    let old_tex_coord_array_stride: GLsizei =
        get_int(gles, gles11::TEXTURE_COORD_ARRAY_STRIDE) as _;
    let old_tex_coord_array_pointer = get_ptr(gles, gles11::TEXTURE_COORD_ARRAY_POINTER);
    let old_blend_sfactor: GLenum = get_int(gles, gles11::BLEND_SRC) as _;
    let old_blend_dfactor: GLenum = get_int(gles, gles11::BLEND_DST) as _;

    let old_tex_env_mode = get_tex_env_int(gles, gles11::TEXTURE_ENV, gles11::TEXTURE_ENV_MODE);
    // if the mode is REPLACE, we don't have to reset the other texture
    // environment values
    let tex_env_mode_arr = [gles11::REPLACE; 1];
    gles.TexEnviv(
        gles11::TEXTURE_ENV,
        gles11::TEXTURE_ENV_MODE,
        tex_env_mode_arr.as_ptr().cast(),
    );

    // For sharp filtering, first scale the frame up by a whole number with
    // nearest-neighbour filtering, then draw that with bilinear filtering
    // (see crate::options::FilterMode::Sharp).
    if sharp_upscale > 1 {
        // Keep the intermediate texture a reasonable size.
        let upscale = (sharp_upscale as GLsizei)
            .min(4096 / width.max(1))
            .min(4096 / height.max(1))
            .max(1);
        let (up_width, up_height) = (width * upscale, height * upscale);
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_MIN_FILTER,
            gles11::NEAREST as _,
        );
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_MAG_FILTER,
            gles11::NEAREST as _,
        );

        let up_texture = PRESENT_UPSCALED_TEXTURE;
        let reuse =
            sizes.upscaled == Some((up_width, up_height)) && gles.IsTexture(up_texture) != 0;
        gles.BindTexture(gles11::TEXTURE_2D, up_texture);
        if !reuse {
            gles.TexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGBA as _,
                up_width,
                up_height,
                0,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            sizes.upscaled = Some((up_width, up_height));
        }
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_MIN_FILTER,
            gles11::LINEAR as _,
        );
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_MAG_FILTER,
            gles11::LINEAR as _,
        );
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, PRESENT_UPSCALED_FRAMEBUFFER);
        gles.FramebufferTexture2DOES(
            gles11::FRAMEBUFFER_OES,
            gles11::COLOR_ATTACHMENT0_OES,
            gles11::TEXTURE_2D,
            up_texture,
            0,
        );

        gles.BindTexture(gles11::TEXTURE_2D, texture);
        present_frame(
            gles,
            (0, 0, up_width as u32, up_height as u32),
            crate::matrix::Matrix::identity(),
            None,
            /* clear: */ true,
        );

        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, 0);
        gles.BindTexture(gles11::TEXTURE_2D, up_texture);
    }

    // Draw the quad
    present_frame(
        gles,
        viewport,
        rotation_matrix,
        virtual_cursor_visible_at,
        /* clear: */ true,
    );

    env.framework_state
        .opengles
        .present_texture_sizes
        .insert(sharegroup, sizes);

    // Restore all the state saved before rendering
    for (&is_enabled, info) in old_arrays.iter().zip(gles1_on_gl2::ARRAYS.iter()) {
        match is_enabled {
            gles11::TRUE => gles.EnableClientState(info.name),
            gles11::FALSE => gles.DisableClientState(info.name),
            _ => unreachable!(),
        }
    }
    for (&is_enabled, &name) in old_capabilities
        .iter()
        .zip(gles1_on_gl2::CAPABILITIES.iter())
    {
        match is_enabled {
            gles11::TRUE => gles.Enable(name),
            gles11::FALSE => gles.Disable(name),
            _ => unreachable!(),
        }
    }
    for mode in [gles11::MODELVIEW, gles11::PROJECTION, gles11::TEXTURE] {
        gles.MatrixMode(mode);
        gles.PopMatrix();
    }
    gles.MatrixMode(old_matrix_mode);
    gles.Color4f(old_color[0], old_color[1], old_color[2], old_color[3]);
    gles.Viewport(
        old_viewport.0,
        old_viewport.1,
        old_viewport.2,
        old_viewport.3,
    );
    gles.ClearColor(
        old_clear_color[0],
        old_clear_color[1],
        old_clear_color[2],
        old_clear_color[3],
    );
    // GL_ARRAY_BUFFER is implicitly used by the Pointer functions but is also
    // an independent binding.
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_vertex_array_binding);
    gles.VertexPointer(
        old_vertex_array_size,
        old_vertex_array_type,
        old_vertex_array_stride,
        old_vertex_array_pointer,
    );
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_tex_coord_array_binding);
    gles.TexCoordPointer(
        old_tex_coord_array_size,
        old_tex_coord_array_type,
        old_tex_coord_array_stride,
        old_tex_coord_array_pointer,
    );
    gles.BindBuffer(gles11::ARRAY_BUFFER, old_array_buffer);
    gles.BlendFunc(old_blend_sfactor, old_blend_dfactor);

    let old_tex_env_mode_arr = [old_tex_env_mode; 1];
    gles.TexEnviv(
        gles11::TEXTURE_ENV,
        gles11::TEXTURE_ENV_MODE,
        old_tex_env_mode_arr.as_ptr().cast(),
    );

    std::mem::drop(gles_boxed);

    // SDL2's documentation warns 0 should be bound to the draw framebuffer
    // when swapping the window, so this is the perfect moment.
    if swap {
        env.window.as_mut().unwrap().swap_window();
    }

    let mut gles_boxed = gles_ctx.make_current(env.window.as_mut().unwrap());
    let gles = gles_boxed.as_mut();

    // Restore the other bindings
    gles.BindTexture(gles11::TEXTURE_2D, old_texture_2d);
    gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, old_framebuffer);

    // { let err = gles.GetError(); if err != 0 { panic!("{:#x}", err); } }
    true
}

/// Shader-based variant of the renderbuffer presenter, used when the
/// underlying driver is a real OpenGL ES 2.0 driver (no fixed-function
/// pipeline available).
///
/// This is intentionally simpler than the fixed-function version: we save the
/// minimum amount of ES 2.0 state, draw the textured quad with a small
/// dedicated shader program, and restore. The app's matrices, vertex pointers
/// etc. are not part of ES 2.0 state and thus need no save/restore.
unsafe fn present_renderbuffer_es2(
    gles: &mut dyn GLES,
    renderbuffer: GLuint,
    window_framebuffer: GLuint,
    viewport: (i32, i32, u32, u32),
    rotation_matrix: crate::matrix::Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
    present_finish: bool,
) {
    use crate::gles::gles2_raw as gles2;

    // Save state we are about to clobber
    let mut old_program: GLint = 0;
    gles.GetIntegerv(gles2::CURRENT_PROGRAM, &mut old_program);
    let mut old_array_buffer: GLint = 0;
    gles.GetIntegerv(gles2::ARRAY_BUFFER_BINDING, &mut old_array_buffer);
    let mut old_elem_buffer: GLint = 0;
    gles.GetIntegerv(gles2::ELEMENT_ARRAY_BUFFER_BINDING, &mut old_elem_buffer);
    let mut old_active_texture: GLint = 0;
    gles.GetIntegerv(gles2::ACTIVE_TEXTURE, &mut old_active_texture);
    let mut old_active_texture_binding: GLint = 0;
    gles.GetIntegerv(gles2::TEXTURE_BINDING_2D, &mut old_active_texture_binding);
    gles.ActiveTexture(gles2::TEXTURE0);
    let mut old_texture0_binding: GLint = 0;
    gles.GetIntegerv(gles2::TEXTURE_BINDING_2D, &mut old_texture0_binding);
    gles.ActiveTexture(old_active_texture as GLenum);
    let mut old_framebuffer: GLint = 0;
    gles.GetIntegerv(gles2::FRAMEBUFFER_BINDING, &mut old_framebuffer);
    let mut old_viewport = [0i32; 4];
    gles.GetIntegerv(gles2::VIEWPORT, old_viewport.as_mut_ptr());
    let mut old_clear_color = [0.0f32; 4];
    gles.GetFloatv(gles2::COLOR_CLEAR_VALUE, old_clear_color.as_mut_ptr());
    let mut old_color_mask = [0u8; 4];
    gles.GetBooleanv(gles2::COLOR_WRITEMASK, old_color_mask.as_mut_ptr());
    let mut old_depth_mask = 0u8;
    gles.GetBooleanv(gles2::DEPTH_WRITEMASK, &mut old_depth_mask);
    let mut old_stencil_mask: GLint = 0;
    gles.GetIntegerv(gles2::STENCIL_WRITEMASK, &mut old_stencil_mask);
    let depth_test_was_on = gles.IsEnabled(gles2::DEPTH_TEST) != 0;
    let stencil_test_was_on = gles.IsEnabled(gles2::STENCIL_TEST) != 0;
    let cull_was_on = gles.IsEnabled(gles2::CULL_FACE) != 0;
    let blend_was_on = gles.IsEnabled(gles2::BLEND) != 0;
    let scissor_was_on = gles.IsEnabled(gles2::SCISSOR_TEST) != 0;

    // Resolve the renderbuffer supplied by `-presentRenderbuffer:` into a
    // texture with cached ES 2.0 objects. Passing this explicit object avoids
    // trusting a guest's later renderbuffer binding.
    let (width, height) = {
        let mut w: GLint = 0;
        let mut h: GLint = 0;
        gles.GetRenderbufferParameteriv(gles2::RENDERBUFFER, gles2::RENDERBUFFER_WIDTH, &mut w);
        gles.GetRenderbufferParameteriv(gles2::RENDERBUFFER, gles2::RENDERBUFFER_HEIGHT, &mut h);
        (w, h)
    };

    let present_objects = ensure_present_objects(gles);
    if width > 0 && height > 0 {
        // Prefer the application's already-bound FBO as the copy source.
        // Switching away from it before CopyTexSubImage2D can discard
        // unresolved tile data on Adreno, leaving the presentation texture
        // black even though the guest just rendered a valid frame. Keep the
        // cached source FBO as a fallback only when the bound FBO cannot be
        // verified to contain this drawable.
        let (attached_renderbuffer, framebuffer_status) = if old_framebuffer != 0 {
            let mut attached = 0;
            gles.GetFramebufferAttachmentParameteriv(
                gles2::FRAMEBUFFER,
                gles2::COLOR_ATTACHMENT0,
                gles2::FRAMEBUFFER_ATTACHMENT_OBJECT_NAME,
                &mut attached,
            );
            (
                attached as GLuint,
                gles.CheckFramebufferStatus(gles2::FRAMEBUFFER),
            )
        } else {
            (0, gles2::FRAMEBUFFER_COMPLETE)
        };
        let use_bound_framebuffer = old_framebuffer != 0
            && attached_renderbuffer == renderbuffer
            && framebuffer_status == gles2::FRAMEBUFFER_COMPLETE;
        if !use_bound_framebuffer {
            // Resolve the guest's current draw target before switching to the
            // cached source FBO. Otherwise a tile-based driver can discard the
            // frame while the fallback FBO is being bound.
            gles.Finish();
            let source_renderbuffer = PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.get());
            gles.BindFramebuffer(gles2::FRAMEBUFFER, present_objects.framebuffer);
            if source_renderbuffer != renderbuffer {
                gles.FramebufferRenderbuffer(
                    gles2::FRAMEBUFFER,
                    gles2::COLOR_ATTACHMENT0,
                    gles2::RENDERBUFFER,
                    renderbuffer,
                );
                PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.set(renderbuffer));
            }
            static LOGGED_FALLBACK: std::sync::Once = std::sync::Once::new();
            LOGGED_FALLBACK.call_once(|| {
                log!(
                    "GLES2 presenter: no suitable guest FBO is bound; using cached source FBO fallback"
                )
            });
        } else {
            static LOGGED_APP_FBO: std::sync::Once = std::sync::Once::new();
            LOGGED_APP_FBO.call_once(|| {
                log!(
                    "GLES2 presenter: copying from the guest's bound FBO to preserve Adreno tile contents"
                )
            });
        }

        // Copy into preallocated texture storage. CopyTexImage2D reallocates
        // that storage on every frame, while CopyTexSubImage2D does not.
        //
        // glCopyTexSubImage2D from the bound framebuffer is ordered after the
        // app's draws by the driver, so no explicit sync is needed here. A
        // glFinish() at this point drains the whole GPU pipeline every frame
        // (the most expensive thing a presenter can do on a tile-based GPU),
        // so it is opt-in: --present-finish / TOUCHHLE_PRESENT_FINISH=1.
        if present_finish {
            gles.Finish();
        }
        // The guest's scissor test clips glCopyTexSubImage2D readouts the
        // same way it clips the ES 1.1 path's copies (2D engines that keep
        // scissor enabled at present time produce glitched frames). Save the
        // enable state, copy with the test disabled, restore.
        let es2_scissor_was_on = gles.IsEnabled(gles2::SCISSOR_TEST) != 0;
        gles.Disable(gles2::SCISSOR_TEST);
        gles.ActiveTexture(gles2::TEXTURE0);
        gles.BindTexture(gles2::TEXTURE_2D, present_objects.texture);
        let texture_size = PRESENT_TEXTURE_SIZE.with(|cell| cell.get());
        if texture_size != Some((width, height)) {
            gles.TexImage2D(
                gles2::TEXTURE_2D,
                0,
                gles2::RGBA as GLint,
                width,
                height,
                0,
                gles2::RGBA,
                gles2::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            PRESENT_TEXTURE_SIZE.with(|cell| cell.set(Some((width, height))));
        }
        gles.CopyTexSubImage2D(gles2::TEXTURE_2D, 0, 0, 0, 0, 0, width, height);
        if es2_scissor_was_on {
            gles.Enable(gles2::SCISSOR_TEST);
        }
        gles.BindFramebuffer(gles2::FRAMEBUFFER, old_framebuffer as _);
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| log!("GLES2 presenter: cached FBO and glCopyTexSubImage2D fast path"));
    }

    gles.ActiveTexture(gles2::TEXTURE0);
    gles.BindTexture(gles2::TEXTURE_2D, present_objects.texture);
    gles.BindBuffer(gles2::ARRAY_BUFFER, present_objects.quad_vbo);
    gles.BindFramebuffer(gles2::FRAMEBUFFER, window_framebuffer);

    // Configure the destination viewport (the window) and clear.
    gles.Viewport(
        viewport.0 as _,
        viewport.1 as _,
        viewport.2 as _,
        viewport.3 as _,
    );
    gles.ClearColor(0.0, 0.0, 0.0, 1.0);
    gles.Disable(gles2::DEPTH_TEST);
    gles.Disable(gles2::STENCIL_TEST);
    gles.Disable(gles2::CULL_FACE);
    gles.Disable(gles2::BLEND);
    gles.Disable(gles2::SCISSOR_TEST);
    gles.ColorMask(gles2::TRUE, gles2::TRUE, gles2::TRUE, gles2::TRUE);
    gles.DepthMask(gles2::TRUE);
    gles.StencilMask(!0);
    // The window has no depth/stencil attachments (see window.rs) and the
    // quad is drawn with depth/stencil testing off; only colour needs
    // clearing (for the letterbox area).
    gles.Clear(gles2::COLOR_BUFFER_BIT);

    // Compile the present shader program once and cache it. If the shader
    // fails to compile/link (e.g. on a host with a buggy GLSL ES driver),
    // skip the present pass instead of crashing — the previous frame
    // remains on screen and the app continues to run.
    let Some(program) = ensure_present_program(gles) else {
        log!("Warning: present_renderbuffer_es2: present shader unavailable, skipping frame.");
        gles.UseProgram(if old_program > 0 {
            old_program as GLuint
        } else {
            0
        });
        gles.BindBuffer(gles2::ARRAY_BUFFER, old_array_buffer as GLuint);
        gles.BindBuffer(gles2::ELEMENT_ARRAY_BUFFER, old_elem_buffer as GLuint);
        gles.BindFramebuffer(gles2::FRAMEBUFFER, old_framebuffer as GLuint);
        gles.ActiveTexture(gles2::TEXTURE0);
        gles.BindTexture(gles2::TEXTURE_2D, old_texture0_binding as GLuint);
        gles.ActiveTexture(old_active_texture as GLenum);
        gles.BindTexture(gles2::TEXTURE_2D, old_active_texture_binding as GLuint);
        gles.Viewport(
            old_viewport[0],
            old_viewport[1],
            old_viewport[2],
            old_viewport[3],
        );
        gles.ClearColor(
            old_clear_color[0],
            old_clear_color[1],
            old_clear_color[2],
            old_clear_color[3],
        );
        gles.ColorMask(
            old_color_mask[0],
            old_color_mask[1],
            old_color_mask[2],
            old_color_mask[3],
        );
        gles.DepthMask(old_depth_mask);
        gles.StencilMask(old_stencil_mask as _);
        if depth_test_was_on {
            gles.Enable(gles2::DEPTH_TEST);
        }
        if stencil_test_was_on {
            gles.Enable(gles2::STENCIL_TEST);
        }
        if cull_was_on {
            gles.Enable(gles2::CULL_FACE);
        }
        if blend_was_on {
            gles.Enable(gles2::BLEND);
        }
        if scissor_was_on {
            gles.Enable(gles2::SCISSOR_TEST);
        }
        return;
    };

    // The presenter only changes the two attributes it owns. Querying all 16
    // slots on every frame is unnecessary driver traffic.
    let attribute_slots = [program.a_pos as GLuint, program.a_uv as GLuint];
    let mut attrib_was_enabled = [0u8; 2];
    for (slot, &attribute) in attrib_was_enabled.iter_mut().zip(attribute_slots.iter()) {
        let mut v: GLint = 0;
        gles.GetVertexAttribiv(attribute, gles2::VERTEX_ATTRIB_ARRAY_ENABLED, &mut v);
        *slot = v as u8;
    }

    gles.UseProgram(program.program);
    gles.Uniform1i(program.u_tex, 0);
    // Keep rotation around the center of the texture. Applying a raw
    // 0..1-space rotation sends one or both axes negative for landscape
    // orientations; with CLAMP_TO_EDGE that samples only the border texel and
    // is indistinguishable from a black frame on strict Adreno drivers.
    let r = crate::matrix::Matrix::<4>::from(&rotation_matrix);
    let to_center = crate::matrix::Matrix::<4>::translate_3d(-0.5, -0.5, 0.0);
    let from_center = crate::matrix::Matrix::<4>::translate_3d(0.5, 0.5, 0.0);
    let m = to_center.multiply(&r).multiply(&from_center);
    let cols = m.columns();
    gles.UniformMatrix4fv(
        program.u_tex_mat,
        1,
        gles2::FALSE,
        cols.as_ptr() as *const _,
    );

    // The full-screen quad lives in `present_objects.quad_vbo`, uploaded
    // above. Core-profile desktop GL (and the GL-on-Vulkan Zink driver used
    // by Winlator on Android) reject client-side vertex arrays with
    // GL_INVALID_OPERATION, so we must source the quad from a real buffer
    // object — never from a host-memory pointer.
    gles.BindBuffer(gles2::ARRAY_BUFFER, present_objects.quad_vbo);
    gles.EnableVertexAttribArray(program.a_pos as _);
    gles.EnableVertexAttribArray(program.a_uv as _);
    gles.VertexAttribPointer(
        program.a_pos as _,
        2,
        gles2::FLOAT,
        gles2::FALSE,
        16,
        std::ptr::null(),
    );
    gles.VertexAttribPointer(
        program.a_uv as _,
        2,
        gles2::FLOAT,
        gles2::FALSE,
        16,
        8usize as *const _,
    );
    gles.DrawArrays(gles2::TRIANGLES, 0, 6);

    // Optional: virtual cursor.
    if let Some((cx, cy, pressed)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = cx - vx as f32;
        let y = cy - vy as f32;
        let radius = 10.0_f32;
        // Build quad in NDC.
        let mut q: [f32; 24] = [
            -1.0, -1.0, 0.0, 0.0, 1.0, -1.0, 1.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 0.0,
            1.0, 1.0, 1.0, 1.0, -1.0, 1.0, 0.0, 1.0,
        ];
        for i in (0..q.len()).step_by(4) {
            q[i] = (q[i] * radius + x) / (vw as f32 / 2.0) - 1.0;
            q[i + 1] = 1.0 - (q[i + 1] * radius + y) / (vh as f32 / 2.0);
        }
        // Use a solid black quasi-shadow via a separate program, but to keep
        // things simple just sample our present texture with very low alpha
        // — skip for now if no separate cursor shader.
        let _ = pressed;
    }

    // Restore vertex attribute enabled state so the app's next draw works.
    for (&attribute, &was) in attribute_slots.iter().zip(attrib_was_enabled.iter()) {
        if was != 0 {
            gles.EnableVertexAttribArray(attribute);
        } else {
            gles.DisableVertexAttribArray(attribute);
        }
    }

    // Restore state we touched
    gles.UseProgram(if old_program > 0 {
        old_program as GLuint
    } else {
        0
    });
    gles.BindBuffer(gles2::ARRAY_BUFFER, old_array_buffer as GLuint);
    gles.BindBuffer(gles2::ELEMENT_ARRAY_BUFFER, old_elem_buffer as GLuint);
    gles.BindFramebuffer(gles2::FRAMEBUFFER, old_framebuffer as GLuint);
    gles.ActiveTexture(gles2::TEXTURE0);
    gles.BindTexture(gles2::TEXTURE_2D, old_texture0_binding as GLuint);
    gles.ActiveTexture(old_active_texture as GLenum);
    gles.BindTexture(gles2::TEXTURE_2D, old_active_texture_binding as GLuint);
    gles.Viewport(
        old_viewport[0],
        old_viewport[1],
        old_viewport[2] as _,
        old_viewport[3] as _,
    );
    gles.ClearColor(
        old_clear_color[0],
        old_clear_color[1],
        old_clear_color[2],
        old_clear_color[3],
    );
    gles.ColorMask(
        old_color_mask[0],
        old_color_mask[1],
        old_color_mask[2],
        old_color_mask[3],
    );
    gles.DepthMask(old_depth_mask);
    gles.StencilMask(old_stencil_mask as _);
    if depth_test_was_on {
        gles.Enable(gles2::DEPTH_TEST);
    }
    if stencil_test_was_on {
        gles.Enable(gles2::STENCIL_TEST);
    }
    if cull_was_on {
        gles.Enable(gles2::CULL_FACE);
    }
    if blend_was_on {
        gles.Enable(gles2::BLEND);
    }
    if scissor_was_on {
        gles.Enable(gles2::SCISSOR_TEST);
    }

    // The renderbuffer present is performed by us (the emulator), not by the
    // guest app. On a real iPhone OS device the equivalent work happens
    // inside `-[EAGLContext presentRenderbuffer:]` / the windowserver, behind
    // a context the app never error-checks. Our present path runs in the
    // *same* host GL context and therefore shares a single `glGetError` error
    // queue with the guest. Any error our own present operations might queue
    // (e.g. a benign INVALID_OPERATION from a strict desktop GL / Zink core
    // profile that the guest's lenient iOS PowerVR/Adreno driver would never
    // raise) would otherwise be read back by the guest's *next* GL call and
    // misattributed to the app — this is exactly the per-frame
    // `OpenGLES error 0x0502 in .../GlesHelper.mm` flood seen on Unity titles.
    // Drain the queue so the guest only ever observes errors it actually
    // caused, matching the real-device contract.
    while gles.GetError() != 0 {}
}

#[derive(Copy, Clone)]
struct PresentProgram {
    program: GLuint,
    a_pos: GLint,
    a_uv: GLint,
    u_tex: GLint,
    u_tex_mat: GLint,
}

/// Reusable GL objects for the ES 2.0 present path. Allocating and freeing a
/// framebuffer object and a texture on *every* presented frame is very
/// expensive on tile-based mobile GPUs (e.g. ARM Mali, Qualcomm Adreno):
/// object creation/teardown forces driver-side synchronisation and breaks
/// the driver's ability to pipeline frames, which shows up as severe stutter
/// in 60 FPS games (notably Unity titles, which present through this path).
/// Caching the objects and reusing them across frames removes that per-frame
/// churn entirely. Texture storage is allocated only when its dimensions
/// change; steady-state frames use `glCopyTexSubImage2D`.
#[derive(Copy, Clone)]
struct PresentObjects {
    framebuffer: GLuint,
    texture: GLuint,
    quad_vbo: GLuint,
}

/// One flag per entry of [gles1_on_gl2::CAPABILITIES]: does the driver reject
/// the enum?
type RejectedCaps = [bool; gles1_on_gl2::CAPABILITIES.len()];

thread_local! {
    static PRESENT_PROGRAM: std::cell::Cell<Option<PresentProgram>> =
        const { std::cell::Cell::new(None) };
    static PRESENT_OBJECTS: std::cell::Cell<Option<PresentObjects>> =
        const { std::cell::Cell::new(None) };
    static PRESENT_SOURCE_RENDERBUFFER: std::cell::Cell<GLuint> =
        const { std::cell::Cell::new(0) };
    static PRESENT_TEXTURE_SIZE: std::cell::Cell<Option<(GLint, GLint)>> =
        const { std::cell::Cell::new(None) };
    // Cached texture for the ES 1.1 (fixed-function) present path. Keeping it
    // alive avoids per-frame allocation and storage redefinition.
    static PRESENT_ES1_TEXTURE: std::cell::Cell<Option<(GLuint, GLint, GLint)>> =
        const { std::cell::Cell::new(None) };
    // Which entries of gles1_on_gl2::CAPABILITIES the driver rejects
    // (GL_INVALID_ENUM on glGetBooleanv). Probed once per context by the ES
    // 1.1 present path.
    static PRESENT_ES1_REJECTED_CAPS: std::cell::Cell<Option<RejectedCaps>> =
        const { std::cell::Cell::new(None) };
    /// GL object names are context-local unless contexts share a sharegroup.
    /// Keep every presenter cache tied to the EAGLContext host object so a game
    /// switching contexts cannot use another context's program/FBO/texture.
    static PRESENT_CONTEXT_TOKEN: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// Drop presenter object names whenever the current EAGL context changes.
/// The caches are thread-local for the normal fast path, but OpenGL names are
/// only valid in the context (or sharegroup) that created them.
fn invalidate_present_cache_for_context(context_token: usize) {
    let changed = PRESENT_CONTEXT_TOKEN.with(|cell| {
        let changed = cell.get() != Some(context_token);
        cell.set(Some(context_token));
        changed
    });
    if changed {
        PRESENT_PROGRAM.with(|cell| cell.set(None));
        PRESENT_OBJECTS.with(|cell| cell.set(None));
        PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.set(0));
        PRESENT_TEXTURE_SIZE.with(|cell| cell.set(None));
        PRESENT_ES1_TEXTURE.with(|cell| cell.set(None));
        PRESENT_ES1_REJECTED_CAPS.with(|cell| cell.set(None));
        log!("EAGL presenter: invalidated cached GL objects after EAGL context switch");
    }
}

unsafe fn ensure_present_program(gles: &mut dyn GLES) -> Option<PresentProgram> {
    use crate::gles::gles2_raw as gles2;
    if let Some(p) = PRESENT_PROGRAM.with(|c| c.get()) {
        return Some(p);
    }

    let vs_src = b"\
        attribute vec2 aPos;\n\
        attribute vec2 aUV;\n\
        uniform mat4 uTexMat;\n\
        varying vec2 vUV;\n\
        void main() {\n\
            gl_Position = vec4(aPos, 0.0, 1.0);\n\
            vUV = (uTexMat * vec4(aUV, 0.0, 1.0)).xy;\n\
        }\0";
    // Alpha is forced to 1.0: the window surface may have an alpha channel
    // that the OS compositor honours (Android SurfaceFlinger), and an app's
    // renderbuffer alpha is meaningless for an opaque CAEAGLLayer.
    let fs_src = b"\
        precision mediump float;\n\
        varying vec2 vUV;\n\
        uniform sampler2D uTex;\n\
        void main() {\n\
            gl_FragColor = vec4(texture2D(uTex, vUV).rgb, 1.0);\n\
        }\0";

    let vs = gles.CreateShader(gles2::VERTEX_SHADER);
    let vs_ptr = vs_src.as_ptr() as *const _;
    let vs_len = (vs_src.len() - 1) as GLint;
    gles.ShaderSource(vs, 1, &vs_ptr, &vs_len);
    gles.CompileShader(vs);
    let mut ok: GLint = 0;
    gles.GetShaderiv(vs, gles2::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut len: GLsizei = 0;
        gles.GetShaderInfoLog(vs, 1024, &mut len, buf.as_mut_ptr() as *mut _);
        let s =
            std::str::from_utf8(std::slice::from_raw_parts(buf.as_ptr(), len as _)).unwrap_or("?");
        log!("Warning: present_es2 vertex shader compile failed: {s}");
        gles.DeleteShader(vs);
        return None;
    }

    let fs = gles.CreateShader(gles2::FRAGMENT_SHADER);
    let fs_ptr = fs_src.as_ptr() as *const _;
    let fs_len = (fs_src.len() - 1) as GLint;
    gles.ShaderSource(fs, 1, &fs_ptr, &fs_len);
    gles.CompileShader(fs);
    gles.GetShaderiv(fs, gles2::COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut len: GLsizei = 0;
        gles.GetShaderInfoLog(fs, 1024, &mut len, buf.as_mut_ptr() as *mut _);
        let s =
            std::str::from_utf8(std::slice::from_raw_parts(buf.as_ptr(), len as _)).unwrap_or("?");
        log!("Warning: present_es2 fragment shader compile failed: {s}");
        gles.DeleteShader(vs);
        gles.DeleteShader(fs);
        return None;
    }

    let prog = gles.CreateProgram();
    gles.AttachShader(prog, vs);
    gles.AttachShader(prog, fs);
    // Bind to high attribute slots so we never collide with the app's
    // attribute layout (which typically starts at 0).
    gles.BindAttribLocation(prog, 6, c"aPos".as_ptr() as *const _);
    gles.BindAttribLocation(prog, 7, c"aUV".as_ptr() as *const _);
    gles.LinkProgram(prog);
    gles.GetProgramiv(prog, gles2::LINK_STATUS, &mut ok);
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut len: GLsizei = 0;
        gles.GetProgramInfoLog(prog, 1024, &mut len, buf.as_mut_ptr() as *mut _);
        let s =
            std::str::from_utf8(std::slice::from_raw_parts(buf.as_ptr(), len as _)).unwrap_or("?");
        log!("Warning: present_es2 program link failed: {s}");
        gles.DeleteShader(vs);
        gles.DeleteShader(fs);
        gles.DeleteProgram(prog);
        return None;
    }

    let a_pos = gles.GetAttribLocation(prog, c"aPos".as_ptr() as *const _);
    let a_uv = gles.GetAttribLocation(prog, c"aUV".as_ptr() as *const _);
    let u_tex = gles.GetUniformLocation(prog, c"uTex".as_ptr() as *const _);
    let u_tex_mat = gles.GetUniformLocation(prog, c"uTexMat".as_ptr() as *const _);

    let result = PresentProgram {
        program: prog,
        a_pos,
        a_uv,
        u_tex,
        u_tex_mat,
    };
    PRESENT_PROGRAM.with(|c| c.set(Some(result)));
    Some(result)
}

/// Returns the reusable framebuffer + texture used by
/// [present_renderbuffer_es2] to resolve the guest renderbuffer into a
/// presentable texture.
///
/// These objects used to be created with `glGenFramebuffers` / `glGenTextures`
/// and destroyed with `glDeleteFramebuffers` / `glDeleteTextures` on *every*
/// presented frame. On tile-based mobile GPUs (ARM Mali, Qualcomm Adreno,
/// PowerVR) allocating and freeing driver objects every frame is surprisingly
/// expensive — it forces driver-side bookkeeping and synchronization that can
/// dominate frame time, which is why a powerful Helio G99 (Mali-G57) could run
/// a Unity game far more slowly than a real iPhone 4S, where iOS forces 60Hz
/// v-sync and never does this churn. Caching the objects per thread and reusing
/// them every frame removes that churn entirely.
unsafe fn ensure_present_objects(gles: &mut dyn GLES) -> PresentObjects {
    if let Some(o) = PRESENT_OBJECTS.with(|c| c.get()) {
        return o;
    }

    let mut framebuffer: GLuint = 0;
    gles.GenFramebuffers(1, &mut framebuffer);
    let mut texture: GLuint = 0;
    gles.GenTextures(1, &mut texture);
    let mut quad_vbo: GLuint = 0;
    gles.GenBuffers(1, &mut quad_vbo);
    gles.BindBuffer(crate::gles::gles2_raw::ARRAY_BUFFER, quad_vbo);
    #[rustfmt::skip]
    let verts: [f32; 24] = [
        -1.0, -1.0, 0.0, 0.0,
         1.0, -1.0, 1.0, 0.0,
        -1.0,  1.0, 0.0, 1.0,
         1.0, -1.0, 1.0, 0.0,
         1.0,  1.0, 1.0, 1.0,
        -1.0,  1.0, 0.0, 1.0,
    ];
    gles.BufferData(
        crate::gles::gles2_raw::ARRAY_BUFFER,
        std::mem::size_of_val(&verts) as isize,
        verts.as_ptr().cast(),
        crate::gles::gles2_raw::STATIC_DRAW,
    );
    gles.ActiveTexture(crate::gles::gles2_raw::TEXTURE0);
    gles.BindTexture(crate::gles::gles2_raw::TEXTURE_2D, texture);
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_MIN_FILTER,
        crate::gles::gles2_raw::LINEAR as _,
    );
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_MAG_FILTER,
        crate::gles::gles2_raw::LINEAR as _,
    );
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_WRAP_S,
        crate::gles::gles2_raw::CLAMP_TO_EDGE as _,
    );
    gles.TexParameteri(
        crate::gles::gles2_raw::TEXTURE_2D,
        crate::gles::gles2_raw::TEXTURE_WRAP_T,
        crate::gles::gles2_raw::CLAMP_TO_EDGE as _,
    );

    let result = PresentObjects {
        framebuffer,
        texture,
        quad_vbo,
    };
    PRESENT_SOURCE_RENDERBUFFER.with(|cell| cell.set(0));
    PRESENT_TEXTURE_SIZE.with(|cell| cell.set(None));
    PRESENT_OBJECTS.with(|c| c.set(Some(result)));
    result
}

pub fn EAGLGetVersion(env: &mut Environment, major: MutPtr<u32>, minor: MutPtr<u32>) {
    let version_major: u32 = 1;
    let version_minor: u32 = 1;

    if !major.is_null() {
        env.mem.write(major, version_major);
    }
    if !minor.is_null() {
        env.mem.write(minor, version_minor);
    }

    log!(
        "EAGLGetVersion called: major={}, minor={}",
        version_major,
        version_minor
    );
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(EAGLGetVersion(_, _))];
