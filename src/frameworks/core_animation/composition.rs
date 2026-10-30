/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The implementation of layer compositing.
//!
//! This is completely original; I don't think Apple document how this works and
//! I haven't attempted to reverse-engineer the details. As such, it probably
//! diverges wildly from what the real iPhone OS does.
#![allow(clippy::zero_ptr)] // alas, as you know, opengl

use super::ca_eagl_layer::{find_fullscreen_eagl_layer_with_overlays, FullscreenEaglLayer};
use super::ca_layer::CALayerHostObject;
use crate::frameworks::core_animation::animation;
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::cg_color::CGColorHostObject;
use crate::frameworks::core_graphics::{cg_bitmap_context, cg_image, CGFloat, CGRect};
use crate::gles::gles11_raw as gles11; // constants only
use crate::gles::gles11_raw::types::*;
use crate::gles::present::{present_frame, FpsCounter};
use crate::gles::GLES; // constants only
use crate::image::Image;
use crate::matrix::Matrix;
use crate::mem::SafeWrite;
use crate::objc::{id, msg, msg_class, nil, ObjC};
use crate::Environment;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct State {
    /// Texture, framebuffer and size (width, height) of the render target.
    texture_framebuffer: Option<(GLuint, GLuint, (u32, u32))>,
    recomposite_next: Option<Instant>,
    fps_counter: Option<FpsCounter>,
    misc_gl_objects: Option<MiscGlObjects>,
    /// Whether layer textures are sampled with nearest-neighbour filtering
    /// (see [crate::options::FilterMode]).
    layer_nearest: bool,
    /// Whether to scale up layer contents drawn on the CPU with Scale2x (see
    /// [crate::options::FilterMode::Hq]).
    layer_scale2x: bool,
    /// Buffer for the Scale2x output, reused to avoid reallocation.
    scale2x_buffer: Vec<u8>,
    /// Size of each layer texture's storage, so it can be updated with
    /// glTexSubImage2D when the size is unchanged (cheaper on mobile GPUs).
    layer_texture_sizes: crate::fast_hash::FastHashMap<GLuint, (u32, u32)>,
    /// Textures of deallocated layers, deleted at the next composition (when
    /// the right GL context is current).
    textures_to_delete: Vec<GLuint>,
    /// Fingerprint (see [frame_fingerprint]) of the last composited frame,
    /// used to skip compositing frames that would be identical. `None` if the
    /// window might not show that frame any more.
    last_frame_fingerprint: Option<u64>,
    /// When a frame was last actually composited.
    last_composited: Option<Instant>,
    /// Set while compositing layers that will be drawn on top of an OpenGL ES
    /// frame (see [present_overlays]).
    overlay_mode: bool,
    /// The full-screen `CAEAGLLayer` and `EAGLContext` that last presented a
    /// frame with overlays (see [present_overlays]), and when.
    last_overlay_present: Option<(id, id, Instant)>,
    /// Like `last_frame_fingerprint`, but for the overlays when they were last
    /// redrawn while the app wasn't presenting new frames.
    last_overlay_fingerprint: Option<u64>,
    /// When the overlays were last redrawn while the app wasn't presenting new
    /// frames.
    last_overlay_redraw: Option<Instant>,
    /// Fingerprint of the overlays in the render target, if it contains them
    /// (see [present_overlays]).
    overlay_target_fingerprint: Option<u64>,
}

/// Called when an app presents a frame the slow way (read back from the GPU),
/// so that [recomposite_overlays] doesn't redraw an older one instead.
pub fn forget_overlay_present(env: &mut Environment) {
    env.framework_state
        .core_animation
        .composition
        .last_overlay_present = None;
}

/// Called after an app presented a frame from a full-screen `CAEAGLLayer`
/// with overlays (see [present_overlays]).
pub fn note_overlay_present(env: &mut Environment, layer: id, context: id) {
    env.framework_state
        .core_animation
        .composition
        .last_overlay_present = Some((layer, context, Instant::now()));
}

/// Called when a layer with a texture is deallocated.
pub(super) fn queue_texture_deletion(env: &mut Environment, texture: GLuint) {
    env.framework_state
        .core_animation
        .composition
        .textures_to_delete
        .push(texture);
}

struct MiscGlObjects {
    /// Texture containing a single rounded corner.
    rounded_corner_texture: GLuint,
    /// [BASIC_SQUARE_POINTS], used as both vertex and texture co-ords for
    /// drawing simple textured quads.
    basic_square_buffer: GLuint,
    /// [FLIPPED_SQUARE_POINTS], used as texture co-ords for some textured
    /// quads.
    flipped_square_buffer: GLuint,
    /// 9-patch rounded corner texture co-ords (always the same).
    rounded_vertex_buffer: GLuint,
    /// 9-patch rounded corner vertex co-ords (varies with ratio of corner
    /// radius to overall rectangle size).
    rounded_tex_coord_buffer: GLuint,
    /// Index buffer for 9-patch (first 6 elements can be used for square).
    index_buffer: GLuint,
}

unsafe fn load_matrix(gles: &mut dyn GLES, matrix: Matrix<4>) {
    gles.LoadMatrixf(matrix.columns().as_ptr() as *const _);
}

/// For use by `NSRunLoop`: call this 60 times per second. Composites the app's
/// visible layers (i.e. UI) and presents it to the screen. Does nothing if
/// composition isn't in use or it's too soon (the latter check is skipped if
/// `force` is set to [true]).
///
/// Returns the time a recomposite is due, if any.
pub fn recomposite_if_necessary(env: &mut Environment, force: bool) -> Option<Instant> {
    let mut animation_state = animation::State::default();
    let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
    if !windows.iter().any(|&window| !msg![env; window isHidden]) {
        log_dbg!("No visible windows, skipping composition");
        env.framework_state
            .core_animation
            .composition
            .last_frame_fingerprint = None;
        return None;
    }

    if env.window.as_ref().is_some_and(|w| w.is_minimized()) {
        // Nothing would be shown, and on Android the surface may be gone.
        env.framework_state
            .core_animation
            .composition
            .last_frame_fingerprint = None;
        return None;
    }

    match find_fullscreen_eagl_layer_with_overlays(env) {
        Some(found) if found.overlays.is_empty() => {
            // No composition done, EAGLContext will present directly.
            log_dbg!("Using CAEAGLLayer fast path, skipping composition");
            env.framework_state
                .core_animation
                .composition
                .last_frame_fingerprint = None;
            return None;
        }
        Some(found) => {
            if let Some(next) = recomposite_overlays(env, &found, force) {
                return next;
            }
            // Otherwise, composite normally.
        }
        None => (),
    }

    let now = Instant::now();
    let interval = 1.0 / 60.0; // 60Hz
    let new_recomposite_next = if let Some(recomposite_next) = env
        .framework_state
        .core_animation
        .composition
        .recomposite_next
    {
        if !force && recomposite_next > now {
            log_dbg!("Not recompositing yet, wait {:?}", recomposite_next - now);
            return Some(recomposite_next);
        }

        // See NSTimer implementation for a discussion of what this does.
        let overdue_by = now.duration_since(recomposite_next);
        log_dbg!("Recompositing, overdue by {:?}", overdue_by);
        // TODO: Use `.div_duration_f64()` once that is stabilized.
        let advance_by = (overdue_by.as_secs_f64() / interval).max(1.0).ceil();
        assert!(advance_by == (advance_by as u32) as f64);
        let advance_by = advance_by as u32;
        if advance_by > 1 {
            log_dbg!("Warning: compositor is lagging. It is overdue by {}s and has missed {} interval(s)!", overdue_by.as_secs_f64(), advance_by - 1);
        }
        let advance_by = Duration::from_secs_f64(interval)
            .checked_mul(advance_by)
            .unwrap();
        Some(recomposite_next.checked_add(advance_by).unwrap())
    } else {
        Some(now.checked_add(Duration::from_secs_f64(interval)).unwrap())
    };
    env.framework_state
        .core_animation
        .composition
        .recomposite_next = new_recomposite_next;

    let window_layers: Vec<id> = windows
        .into_iter()
        .map(|window| {
            let layer: id = msg![env; window layer];
            // Ensure layer bitmaps are up to date.
            display_layers(env, layer);
            layer
        })
        .collect();

    let (params, filter_mode) = prepare_render_params(env);
    let RenderParams {
        fb_size: (fb_width, fb_height),
        ref present_frame_args,
        ..
    } = params;

    // If nothing has changed since the last frame, compositing again would
    // produce an identical image, so skip it. Many apps only redraw a few
    // times per second, so this saves a lot of GPU work (and battery).
    let fingerprint = frame_fingerprint(
        &env.objc,
        &window_layers,
        (fb_width, fb_height, filter_mode as u8),
        present_frame_args,
    );
    let needs_redraw = env.window().take_needs_redraw();
    let composition = &mut env.framework_state.core_animation.composition;
    // As a precaution against any change that the fingerprint misses, never
    // skip compositing for more than a second.
    let recently_composited = composition
        .last_composited
        .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1));
    if !force
        && !needs_redraw
        && recently_composited
        && fingerprint.is_some()
        && fingerprint == composition.last_frame_fingerprint
    {
        log_dbg!("Nothing changed, skipping composition");
        return new_recomposite_next;
    }
    composition.last_composited = Some(now);

    // Count only frames that are actually composited (not skipped).
    if env.options.print_fps {
        composition
            .fps_counter
            .get_or_insert_with(FpsCounter::start)
            .count_frame(format_args!("Core Animation compositor"), true);
    }

    // TODO: draw status bar if it's not hidden

    let window_layers_for_fingerprint = window_layers.clone();
    let roots: Vec<(id, Matrix<4>)> = window_layers
        .into_iter()
        .map(|layer| (layer, Matrix::identity()))
        .collect();
    unsafe {
        render_and_present(
            env,
            &mut animation_state,
            &roots,
            &params,
            /* overlay: */ false,
            /* composite: */ true,
        );
    }

    animation_state.update_started_and_finished_animations(env);

    // Compositing updates the layers' textures, so the fingerprint to compare
    // the next frame with must be taken afterwards.
    env.framework_state
        .core_animation
        .composition
        .last_frame_fingerprint = frame_fingerprint(
        &env.objc,
        &window_layers_for_fingerprint,
        (fb_width, fb_height, filter_mode as u8),
        &params.present_frame_args,
    );

    new_recomposite_next
}

/// For when the app's full-screen `CAEAGLLayer` has other layers on top of it:
/// when the app presents a frame, it's drawn directly and the overlays are
/// drawn on top (see [present_overlays]), so nothing needs to be done here,
/// unless the app has stopped presenting frames while the overlays change
/// (e.g. a UIKit menu over a paused game). In that case the last frame and the
/// overlays are redrawn. Returns [None] if normal composition is needed
/// instead, otherwise what [recomposite_if_necessary] should return.
fn recomposite_overlays(
    env: &mut Environment,
    found: &FullscreenEaglLayer,
    force: bool,
) -> Option<Option<Instant>> {
    let now = Instant::now();
    let next = Some(now + Duration::from_secs_f64(1.0 / 60.0));
    let (layer, context, last_present) = env
        .framework_state
        .core_animation
        .composition
        .last_overlay_present?;
    if layer != found.layer {
        return None;
    }
    // If the app has presented a frame in about the last frame interval, the
    // overlays were drawn with it. Otherwise (the app has stopped presenting
    // frames, or presents them less often than the overlays change) they are
    // redrawn here if they changed.
    if !force && now.duration_since(last_present) < Duration::from_millis(25) {
        return Some(next);
    }

    // Ensure layer bitmaps are up to date before taking the fingerprint.
    for &(overlay, _) in &found.overlays {
        display_layers(env, overlay);
    }
    let (params, filter_mode) = prepare_render_params(env);
    let overlay_layers: Vec<id> = found.overlays.iter().map(|&(layer, _)| layer).collect();
    let target = (
        params.fb_size.0,
        params.fb_size.1,
        filter_mode as u8 | (found.quarter_turns << 4),
    );
    let fingerprint = frame_fingerprint(
        &env.objc,
        &overlay_layers,
        target,
        &params.present_frame_args,
    );
    let needs_redraw = env.window().take_needs_redraw();
    let composition = &env.framework_state.core_animation.composition;
    let recently_redrawn = composition
        .last_overlay_redraw
        .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1));
    if !force
        && !needs_redraw
        && recently_redrawn
        && fingerprint.is_some()
        && fingerprint == composition.last_overlay_fingerprint
    {
        return Some(next);
    }

    if !crate::frameworks::opengles::redraw_last_frame(env, context, found.quarter_turns) {
        return None;
    }
    present_overlays(env, &found.overlays);
    let composition = &mut env.framework_state.core_animation.composition;
    composition.last_overlay_redraw = Some(now);
    composition.last_overlay_fingerprint = frame_fingerprint(
        &env.objc,
        &overlay_layers,
        target,
        &params.present_frame_args,
    );
    Some(next)
}

/// Work out the parameters for compositing and presenting a frame, and
/// update the compositor's filtering settings.
fn prepare_render_params(env: &mut Environment) -> (RenderParams, crate::options::FilterMode) {
    let screen_bounds: CGRect = {
        let screen: id = msg_class![env; UIScreen mainScreen];
        msg![env; screen bounds]
    };
    let scale_hack: u32 = env.options.scale_hack.get();
    // For sharp filtering, the screen is rendered at a whole multiple of its
    // size (with nearest-neighbour filtering of the layers) before the final
    // bilinear scaling. This is invisible to the app.
    let upscale = env.window().sharp_upscale_factor();
    let filter_mode = env.window().filter_mode();
    let layer_nearest = filter_mode != crate::options::FilterMode::Smooth;
    let final_nearest = filter_mode == crate::options::FilterMode::Pixel;
    env.framework_state.core_animation.composition.layer_nearest = layer_nearest;
    env.framework_state.core_animation.composition.layer_scale2x =
        filter_mode == crate::options::FilterMode::Hq;
    let fb_width = screen_bounds.size.width as u32 * scale_hack * upscale;
    let fb_height = env
        .options
        .p2g_wide_width
        .filter(|_| env.options.p2g_wide_screen)
        .unwrap_or(screen_bounds.size.height as u32)
        * scale_hack
        * upscale;
    let present_frame_args = (
        env.window().viewport(),
        env.window().rotation_matrix(),
        env.window().virtual_cursor_visible_at(),
    );
    (
        RenderParams {
            fb_size: (fb_width, fb_height),
            screen_bounds,
            final_nearest,
            present_frame_args,
        },
        filter_mode,
    )
}

/// Draw `overlays` (layers drawn on top of a full-screen OpenGL ES layer, see
/// [super::ca_eagl_layer::find_fullscreen_eagl_layer_with_overlays]) on top of
/// the OpenGL ES frame that has just been drawn to the window, then swap the
/// window's buffers. This avoids having to read the frame back from the GPU
/// and composite it, which is slow.
pub fn present_overlays(env: &mut Environment, overlays: &[(id, Matrix<4>)]) {
    let mut animation_state = animation::State::default();
    // Ensure layer bitmaps are up to date.
    for &(layer, _) in overlays {
        display_layers(env, layer);
    }
    let (params, filter_mode) = prepare_render_params(env);

    // Usually the overlays don't change from frame to frame, so the render
    // target can be presented again without compositing them again.
    let overlays_fingerprint = |env: &Environment| {
        use std::hash::{Hash, Hasher};
        let layers: Vec<id> = overlays.iter().map(|&(layer, _)| layer).collect();
        let fingerprint = frame_fingerprint(
            &env.objc,
            &layers,
            (params.fb_size.0, params.fb_size.1, filter_mode as u8),
            &params.present_frame_args,
        )?;
        let mut hasher = crate::fast_hash::FxHasher::default();
        fingerprint.hash(&mut hasher);
        for (_, transform) in overlays {
            for column in transform.columns() {
                for value in column {
                    value.to_bits().hash(&mut hasher);
                }
            }
        }
        Some(hasher.finish())
    };
    let fingerprint = overlays_fingerprint(env);
    let composition = &env.framework_state.core_animation.composition;
    let reuse = fingerprint.is_some()
        && fingerprint == composition.overlay_target_fingerprint
        && composition
            .texture_framebuffer
            .is_some_and(|(_, _, size)| size == params.fb_size);

    unsafe {
        render_and_present(
            env,
            &mut animation_state,
            overlays,
            &params,
            /* overlay: */ true,
            /* composite: */ !reuse,
        );
    }
    animation_state.update_started_and_finished_animations(env);
    // Compositing updates the layers' textures, so the fingerprint to compare
    // with next time must be taken afterwards.
    let fingerprint = overlays_fingerprint(env);
    let composition = &mut env.framework_state.core_animation.composition;
    composition.overlay_target_fingerprint = fingerprint;
    // What's in the window is no longer a normally composited frame.
    composition.last_frame_fingerprint = None;
}

/// What [render_and_present] needs to know about the screen.
struct RenderParams {
    /// Size of the render target.
    fb_size: (u32, u32),
    /// Bounds of the app's screen (`UIScreen`).
    screen_bounds: CGRect,
    /// Whether to use nearest-neighbour filtering when presenting.
    final_nearest: bool,
    present_frame_args: PresentFrameArgs,
}

/// Composite `roots` (each with the cumulative transform of its superlayer,
/// see [composite_layer_recursive]) into the render target and draw it to the
/// window, then swap the window's buffers.
///
/// If `overlay` is set, the render target is cleared to transparent rather
/// than black, and it is drawn on top of what's already in the window's
/// framebuffer (see [present_overlays]). If `composite` is false, the render
/// target's existing contents are presented again instead.
unsafe fn render_and_present(
    env: &mut Environment,
    animation_state: &mut animation::State,
    roots: &[(id, Matrix<4>)],
    params: &RenderParams,
    overlay: bool,
    composite: bool,
) {
    let &RenderParams {
        fb_size: (fb_width, fb_height),
        screen_bounds,
        final_nearest,
        ref present_frame_args,
    } = params;

    let window = env.window.as_mut().unwrap();
    let mut gles = window.make_internal_gl_ctx_current();

    let composition = &mut env.framework_state.core_animation.composition;
    for texture in std::mem::take(&mut composition.textures_to_delete) {
        composition.layer_texture_sizes.remove(&texture);
        unsafe { gles.DeleteTextures(1, &texture) };
    }

    // Set up GL objects needed for render-to-texture. We could draw directly
    // to the screen instead, but this way we can reuse the code for scaling and
    // rotating the screen and drawing the virtual cursor.
    // The render target needs resizing if the size changed (e.g. the window
    // was resized or rotated while using sharp filtering). This resizes the
    // texture in place rather than deleting the framebuffer and making a new
    // one, because deleting a framebuffer crashes Android emulator's OpenGL ES
    // 1.1 implementation (in GLClientState::removeFramebuffers()), which
    // happened when an app started in a different orientation than the
    // launcher's.
    if let Some((texture, framebuffer, size)) = env
        .framework_state
        .core_animation
        .composition
        .texture_framebuffer
    {
        if size != (fb_width, fb_height) {
            unsafe {
                gles.BindTexture(gles11::TEXTURE_2D, texture);
                gles.TexImage2D(
                    gles11::TEXTURE_2D,
                    0,
                    gles11::RGBA as _,
                    fb_width as _,
                    fb_height as _,
                    0,
                    gles11::RGBA,
                    gles11::UNSIGNED_BYTE,
                    std::ptr::null(),
                );
                gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, framebuffer);
                gles.FramebufferTexture2DOES(
                    gles11::FRAMEBUFFER_OES,
                    gles11::COLOR_ATTACHMENT0_OES,
                    gles11::TEXTURE_2D,
                    texture,
                    0,
                );
                warn_on_gl_error(gles.as_mut(), "resizing the render target");
            }
            env.framework_state
                .core_animation
                .composition
                .texture_framebuffer = Some((texture, framebuffer, (fb_width, fb_height)));
        }
    }
    let texture = if let Some((texture, framebuffer, _)) = env
        .framework_state
        .core_animation
        .composition
        .texture_framebuffer
    {
        unsafe {
            gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, framebuffer);
        };
        texture
    } else {
        let mut texture = 0;
        let mut framebuffer = 0;
        unsafe {
            gles.GenTextures(1, &mut texture);
            gles.BindTexture(gles11::TEXTURE_2D, texture);
            gles.TexImage2D(
                gles11::TEXTURE_2D,
                0,
                gles11::RGBA as _,
                fb_width as _,
                fb_height as _,
                0,
                gles11::RGBA,
                gles11::UNSIGNED_BYTE,
                std::ptr::null(),
            );
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

            gles.GenFramebuffersOES(1, &mut framebuffer);
            gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, framebuffer);
            gles.FramebufferTexture2DOES(
                gles11::FRAMEBUFFER_OES,
                gles11::COLOR_ATTACHMENT0_OES,
                gles11::TEXTURE_2D,
                texture,
                0,
            );
            warn_on_gl_error(gles.as_mut(), "creating the render target");
            let status = gles.CheckFramebufferStatusOES(gles11::FRAMEBUFFER_OES);
            if status != gles11::FRAMEBUFFER_COMPLETE_OES {
                log!(
                    "Warning: compositor render target is incomplete ({:#x})",
                    status
                );
            }
        }
        env.framework_state
            .core_animation
            .composition
            .texture_framebuffer = Some((texture, framebuffer, (fb_width, fb_height)));
        texture
    };

    // Set up various other GL objects that will be reused on every frame.
    let misc_gl_objects = env
        .framework_state
        .core_animation
        .composition
        .misc_gl_objects
        .get_or_insert_with(|| {
            let dimension = 512usize; // way larger than any reasonable corner
            let mut image = Image::from_pixel_vec(
                vec![255u8; dimension * dimension * 4],
                (dimension as _, dimension as _),
            );
            image.round_corners(dimension as _, /* four_corners: */ false, /* add_sheen: */ false);

            let mut rounded_corner_texture = 0;
            unsafe {
                gles.GenTextures(1, &mut rounded_corner_texture);
                gles.BindTexture(gles11::TEXTURE_2D, rounded_corner_texture);
                // GENERATE_MIPMAP must be set before the texture upload.
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::GENERATE_MIPMAP,
                    gles11::TRUE as _,
                );
                upload_rgba8_pixels(gles.as_mut(), image.pixels(), (dimension as _, dimension as _));
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_MIN_FILTER,
                    gles11::LINEAR_MIPMAP_LINEAR as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_S,
                    gles11::CLAMP_TO_EDGE as _,
                );
                gles.TexParameteri(
                    gles11::TEXTURE_2D,
                    gles11::TEXTURE_WRAP_T,
                    gles11::CLAMP_TO_EDGE as _,
                );
            }

            let [basic_square_buffer, flipped_square_buffer, rounded_vertex_buffer, rounded_tex_coord_buffer, index_buffer] = unsafe {
                let mut array_buffers = [0; 5];
                gles.GenBuffers(5, array_buffers.as_mut_ptr());
                array_buffers
            };
            unsafe {
                gles.BindBuffer(gles11::ARRAY_BUFFER, basic_square_buffer);
                upload_slice(gles.as_mut(), gles11::ARRAY_BUFFER, &BASIC_SQUARE_POINTS, gles11::STATIC_DRAW);
                gles.BindBuffer(gles11::ARRAY_BUFFER, flipped_square_buffer);
                upload_slice(gles.as_mut(), gles11::ARRAY_BUFFER, &FLIPPED_SQUARE_POINTS, gles11::STATIC_DRAW);
                gles.BindBuffer(gles11::ARRAY_BUFFER, rounded_vertex_buffer);
                upload_slice(gles.as_mut(), gles11::ARRAY_BUFFER, &[0f32; FLOATS_PER_9PATCH], gles11::DYNAMIC_DRAW);
                gles.BindBuffer(gles11::ARRAY_BUFFER, rounded_tex_coord_buffer);
                upload_slice(
                    gles.as_mut(),
                    gles11::ARRAY_BUFFER,
                    &make_9patch_coords([0.0, 1.0, 1.0, 0.0], [0.0, 1.0, 1.0, 0.0]),
                    gles11::STATIC_DRAW,
                );
                // Prevent accidental subsequent use.
                gles.BindBuffer(gles11::ARRAY_BUFFER, 0);

                gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, index_buffer);
                upload_slice(gles.as_mut(), gles11::ELEMENT_ARRAY_BUFFER, &make_9patch_indices(), gles11::STATIC_DRAW);
                // Prevent accidental subsequent use.
                gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, 0);
            }

            MiscGlObjects {
                rounded_corner_texture,
                basic_square_buffer,
                flipped_square_buffer,
                rounded_vertex_buffer,
                rounded_tex_coord_buffer,
                index_buffer,
            }
        });

    // Clear the framebuffer and set up state to prepare for rendering
    if composite {
        gles.Viewport(0, 0, fb_width as _, fb_height as _);
        if overlay {
            gles.ClearColor(0.0, 0.0, 0.0, 0.0);
        } else {
            gles.ClearColor(0.0, 0.0, 0.0, 1.0);
        }
        gles.Clear(gles11::COLOR_BUFFER_BIT);
        gles.Color4f(1.0, 1.0, 1.0, 1.0);

        gles.MatrixMode(gles11::PROJECTION);
        // Scale down screen-space to normalized device co-ordinates, shift the
        // origin to be at the top-left rather than the center, and flip the
        // Y axis (OpenGL's points up, Core Animation's points down).
        // Using the projection matrix for this is more convenient than adding
        // an extra multiply to composite_layer_recursive.
        load_matrix(
            gles.as_mut(),
            Matrix::from(&Matrix::scale_2d(
                2.0 / screen_bounds.size.width,
                -2.0 / screen_bounds.size.height,
            ))
            .multiply(&Matrix::translate_3d(-1.0, 1.0, 0.0)),
        );
        gles.MatrixMode(gles11::MODELVIEW);
        gles.LoadIdentity();

        // One index buffer to rule them all
        gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, misc_gl_objects.index_buffer);
    }
    std::mem::drop(gles);

    env.framework_state.core_animation.composition.overlay_mode = overlay;
    if !overlay {
        // The render target won't contain the overlays any more.
        env.framework_state
            .core_animation
            .composition
            .overlay_target_fingerprint = None;
    }
    for &(root_layer, cumulative_transform) in roots.iter().filter(|_| composite) {
        // Here's where the actual drawing happens
        composite_layer_recursive(
            env,
            animation_state,
            root_layer,
            cumulative_transform,
            /* opacity: */ 1.0,
        );
    }
    env.framework_state.core_animation.composition.overlay_mode = false;

    // Re-borrow
    let window = env.window.as_mut().unwrap();
    let mut gles = window.make_internal_gl_ctx_current();

    // Clean up some GL state
    unsafe {
        gles.Viewport(0, 0, fb_width as _, fb_height as _);
        gles.Color4f(1.0, 1.0, 1.0, 1.0);
        gles.Disable(gles11::BLEND);
        gles.MatrixMode(gles11::PROJECTION);
        gles.LoadIdentity();
        gles.MatrixMode(gles11::MODELVIEW);
        gles.LoadIdentity();
        gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
        gles.BindBuffer(gles11::ELEMENT_ARRAY_BUFFER, 0);
        warn_on_gl_error(gles.as_mut(), "compositing");
    }

    // Present our rendered frame (bound to TEXTURE_2D). This copies it to the
    // default framebuffer (0) so we need to unbind our internal framebuffer.
    unsafe {
        gles.BindTexture(gles11::TEXTURE_2D, texture);
        let filter = if final_nearest {
            gles11::NEAREST
        } else {
            gles11::LINEAR
        };
        gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MIN_FILTER, filter as _);
        gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MAG_FILTER, filter as _);
        gles.BindFramebufferOES(gles11::FRAMEBUFFER_OES, 0);
        if overlay {
            // Draw on top of what's already there (the app's OpenGL ES frame).
            gles.Enable(gles11::BLEND);
            gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        }
        present_frame(
            gles.as_mut(),
            present_frame_args.0,
            present_frame_args.1,
            present_frame_args.2,
            /* clear: */ !overlay,
        );
        if overlay {
            gles.Disable(gles11::BLEND);
        }
    }
    std::mem::drop(gles);
    window.swap_window();
}

/// Viewport, rotation matrix and virtual cursor state, as passed to
/// [present_frame].
type PresentFrameArgs = ((i32, i32, u32, u32), Matrix<2>, Option<(f32, f32, bool)>);

/// Compute a hash of everything that determines what a composited frame looks
/// like: the layer trees' properties that [composite_layer_recursive] uses,
/// whether their textures need updating, and the presentation parameters.
/// Returns [None] if the frame must be composited regardless, because there
/// are animations in progress.
fn frame_fingerprint(
    objc: &ObjC,
    window_layers: &[id],
    target: (u32, u32, u8),
    present_frame_args: &PresentFrameArgs,
) -> Option<u64> {
    use std::hash::{Hash, Hasher};

    fn hash_f32s(hasher: &mut crate::fast_hash::FxHasher, values: &[f32]) {
        for value in values {
            value.to_bits().hash(hasher);
        }
    }

    fn traverse(objc: &ObjC, layer: id, hasher: &mut crate::fast_hash::FxHasher) -> Option<()> {
        let host_obj = objc.borrow::<CALayerHostObject>(layer);
        if !host_obj.animations.is_empty() || !host_obj.anonymous_animations.is_empty() {
            return None;
        }
        layer.hash(hasher);
        host_obj.hidden.hash(hasher);
        if host_obj.hidden {
            return Some(());
        }
        let CGRect { origin, size } = host_obj.bounds;
        let CGAffineTransform { a, b, c, d, tx, ty } = host_obj.affine_transform;
        hash_f32s(
            hasher,
            &[
                origin.x,
                origin.y,
                size.width,
                size.height,
                host_obj.position.x,
                host_obj.position.y,
                host_obj.anchor_point.x,
                host_obj.anchor_point.y,
                a,
                b,
                c,
                d,
                tx,
                ty,
                host_obj.opacity,
                host_obj.corner_radius,
            ],
        );
        if let Some(color) = &host_obj.background_color {
            hash_f32s(hasher, &[color.r, color.g, color.b, color.a]);
        } else {
            hash_f32s(hasher, &[-1.0]);
        }
        host_obj.opaque.hash(hasher);
        host_obj.contents.hash(hasher);
        host_obj.cg_context.hash(hasher);
        host_obj.presented_pixels.is_some().hash(hasher);
        host_obj.gles_texture.hash(hasher);
        host_obj.gles_texture_is_up_to_date.hash(hasher);
        host_obj.sublayers.len().hash(hasher);
        for &sublayer in &host_obj.sublayers {
            traverse(objc, sublayer, hasher)?;
        }
        Some(())
    }

    let mut hasher = crate::fast_hash::FxHasher::default();
    target.hash(&mut hasher);
    let (viewport, rotation_matrix, virtual_cursor) = present_frame_args;
    viewport.hash(&mut hasher);
    hash_f32s(&mut hasher, &rotation_matrix.columns().concat());
    if let &Some((x, y, pressed)) = virtual_cursor {
        hash_f32s(&mut hasher, &[x, y]);
        pressed.hash(&mut hasher);
    }
    window_layers.len().hash(&mut hasher);
    for &layer in window_layers {
        traverse(objc, layer, &mut hasher)?;
    }
    Some(hasher.finish())
}

/// Call `displayIfNeeded` on all relevant layers in the tree, so their bitmaps
/// are up to date before compositing.
fn display_layers(env: &mut Environment, root_layer: id) {
    // Tell layers to redraw themselves if needed.

    fn traverse(objc: &ObjC, layer: id, layers_needing_display: &mut Vec<id>) {
        let host_obj = objc.borrow::<CALayerHostObject>(layer);
        if host_obj.hidden {
            return;
        }
        if host_obj.needs_display {
            layers_needing_display.push(layer);
        }
        for &layer in &host_obj.sublayers {
            traverse(objc, layer, layers_needing_display);
        }
    }

    let mut layers_needing_display = Vec::new();
    traverse(&env.objc, root_layer, &mut layers_needing_display);

    for layer in layers_needing_display {
        () = msg![env; layer displayIfNeeded];
    }
}

/// Traverses the layer tree and draws each layer.
unsafe fn composite_layer_recursive(
    env: &mut Environment,
    animation_state: &mut animation::State,
    layer: id,
    cumulative_transform: Matrix<4>,
    opacity: CGFloat,
) {
    // TODO: this can't handle zPosition among other things, but it is not
    //       supported yet :)
    // TODO: back-to-front drawing is not efficient, could we use front-to-back?

    // This is both acting as the presentationLayer and the private render layer
    // It might need to be reworked in the future into a guest presentationLayer
    let host_obj = animation_state.create_presentation_layer(env, layer);

    if host_obj.hidden {
        return;
    }

    let window = env.window.as_mut().unwrap();
    let mut gles = window.make_internal_gl_ctx_current();

    // Compose the expanded game render target across the whole output. Native
    // UIKit layers (including movies) retain the centered 16:9 viewport too.
    if env.options.p2g_wide_screen {
        if let Some((_, _, (width, height))) = env
            .framework_state
            .core_animation
            .composition
            .texture_framebuffer
        {
            let long = env.options.p2g_wide_width.unwrap();
            let expanded = host_obj
                .presented_pixels
                .as_ref()
                .is_some_and(|(_, w, h)| *w as u64 * 320 == *h as u64 * long as u64 && w > h);
            let safe_height = width * 568 / 320;
            if expanded {
                gles.Viewport(0, 0, width as _, height as _);
            } else {
                gles.Viewport(
                    0,
                    ((height - safe_height) / 2) as _,
                    width as _,
                    safe_height as _,
                );
            }
        }
    }
    let opacity = opacity * host_obj.opacity;
    let cumulative_transform = {
        let CALayerHostObject { bounds, .. } = host_obj;

        // Update the transform to match this layer's co-ordinate space.
        let cumulative_transform =
            <Matrix<4> as From<_>>::from(host_obj.superlayer_to_layer_transform())
                .multiply(&cumulative_transform);

        // Reposition and scale the unit quad (see ARRAY_BUFFER binding)
        // so it will have the right size in this layer's co-ordinate space.
        gles.MatrixMode(gles11::MODELVIEW);
        load_matrix(
            gles.as_mut(),
            Matrix::<4>::from(&Matrix::scale_2d(bounds.size.width, bounds.size.height))
                .multiply(&Matrix::translate_3d(bounds.origin.x, bounds.origin.y, 0.0))
                .multiply(&cumulative_transform),
        );

        cumulative_transform
    };

    // Draw background color, if any
    let have_background = if let Some(background_color) = host_obj.background_color {
        let misc = env
            .framework_state
            .core_animation
            .composition
            .misc_gl_objects
            .as_ref()
            .unwrap();

        let CGColorHostObject { r, g, b, a, .. } = background_color;
        gles.Color4f(
            r * a * opacity,
            g * a * opacity,
            b * a * opacity,
            a * opacity,
        );
        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);

        let radius = host_obj.corner_radius;
        if radius == 0.0 {
            gles.Disable(gles11::TEXTURE_2D);
            gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);

            gles.EnableClientState(gles11::VERTEX_ARRAY);
            gles.BindBuffer(gles11::ARRAY_BUFFER, misc.basic_square_buffer);
            gles.VertexPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

            gles.DrawElements(
                gles11::TRIANGLES,
                SQUARE_INDICES.len() as _,
                gles11::UNSIGNED_BYTE,
                0 as *const GLvoid,
            );
        } else {
            gles.Enable(gles11::TEXTURE_2D);
            gles.BindTexture(gles11::TEXTURE_2D, misc.rounded_corner_texture);
            gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
            gles.BindBuffer(gles11::ARRAY_BUFFER, misc.rounded_tex_coord_buffer);
            gles.TexCoordPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

            gles.EnableClientState(gles11::VERTEX_ARRAY);
            gles.BindBuffer(gles11::ARRAY_BUFFER, misc.rounded_vertex_buffer);
            upload_slice(
                gles.as_mut(),
                gles11::ARRAY_BUFFER,
                &make_9patch_coords(
                    [
                        0.0,
                        (radius / host_obj.bounds.size.width).min(0.5),
                        (1.0 - radius / host_obj.bounds.size.width).max(0.5),
                        1.0,
                    ],
                    [
                        0.0,
                        (radius / host_obj.bounds.size.height).min(0.5),
                        (1.0 - radius / host_obj.bounds.size.height).max(0.5),
                        1.0,
                    ],
                ),
                gles11::DYNAMIC_DRAW,
            );
            gles.VertexPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

            gles.DrawElements(
                gles11::TRIANGLES,
                INDICES_PER_9PATCH as _,
                gles11::UNSIGNED_BYTE,
                0 as *const GLvoid,
            );
        };

        true
    } else {
        false
    };

    let need_texture = host_obj.presented_pixels.is_some()
        || host_obj.contents != nil
        || host_obj.cg_context.is_some();
    let need_update = need_texture && !host_obj.gles_texture_is_up_to_date;

    if need_texture {
        if let Some(texture) = host_obj.gles_texture {
            gles.BindTexture(gles11::TEXTURE_2D, texture);
        } else {
            assert!(!host_obj.gles_texture_is_up_to_date);
            let mut texture = 0;
            gles.GenTextures(1, &mut texture);
            gles.BindTexture(gles11::TEXTURE_2D, texture);
            // Update original layer texture
            env.objc.borrow_mut::<CALayerHostObject>(layer).gles_texture = Some(texture);
        }
    }

    // Update original layer texture with CAEAGLLayer pixels (slow path), if any
    if need_update {
        let original_host_obj = env.objc.borrow_mut::<CALayerHostObject>(layer);
        if let Some((ref mut pixels, width, height)) = original_host_obj.presented_pixels {
            let texture = original_host_obj.gles_texture.unwrap();
            let state = &mut env.framework_state.core_animation.composition;
            // GPU-rendered frames already have their intended resolution.
            // Scale2x is for CPU-drawn artwork, not a second pass over 3D.
            upload_layer_pixels(
                gles.as_mut(),
                state,
                texture,
                pixels,
                (width, height),
                false,
            );
        }
    }

    // Update texture with CGImageRef or CGContextRef pixels, if any
    if need_update {
        if host_obj.contents != nil {
            let image = cg_image::borrow_image(&env.objc, host_obj.contents);

            // No special handling for opacity is needed here: the alpha channel
            // on an image is meaningful and won't be ignored.
            let texture = env
                .objc
                .borrow::<CALayerHostObject>(layer)
                .gles_texture
                .unwrap();
            let state = &mut env.framework_state.core_animation.composition;
            upload_layer_pixels(
                gles.as_mut(),
                state,
                texture,
                image.pixels(),
                image.dimensions(),
                true,
            );
        } else if let Some(cg_context) = host_obj.cg_context {
            // Make sure this is in sync with the code in ca_layer.rs that
            // sets up the context!
            let (width, height, pixels) =
                cg_bitmap_context::get_rgba8_pixels(&env.objc, &env.mem, cg_context);
            let texture = env
                .objc
                .borrow::<CALayerHostObject>(layer)
                .gles_texture
                .unwrap();
            let state = &mut env.framework_state.core_animation.composition;
            upload_layer_pixels(
                gles.as_mut(),
                state,
                texture,
                &pixels,
                (width, height),
                true,
            );
        }
    }

    if need_update {
        // Update original layer field
        env.objc
            .borrow_mut::<CALayerHostObject>(layer)
            .gles_texture_is_up_to_date = true;
    }

    // Draw texture, if any
    if need_texture {
        let filter = if env.framework_state.core_animation.composition.layer_nearest {
            gles11::NEAREST
        } else {
            gles11::LINEAR
        };
        gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MIN_FILTER, filter as _);
        gles.TexParameteri(gles11::TEXTURE_2D, gles11::TEXTURE_MAG_FILTER, filter as _);

        let misc = env
            .framework_state
            .core_animation
            .composition
            .misc_gl_objects
            .as_ref()
            .unwrap();

        gles.Color4f(opacity, opacity, opacity, opacity);
        let draw_opaque = opacity == 1.0 && host_obj.opaque && !have_background;
        if draw_opaque {
            gles.Disable(gles11::BLEND);
        } else {
            gles.Enable(gles11::BLEND);
            gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        }

        gles.EnableClientState(gles11::VERTEX_ARRAY);
        gles.BindBuffer(gles11::ARRAY_BUFFER, misc.basic_square_buffer);
        gles.VertexPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);

        gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
        // Normal images and layer backing stores have top-to-bottom row order;
        // only CAEAGLLayer pixels (from OpenGL ES) are bottom-to-top and
        // need their UVs flipped.
        gles.BindBuffer(
            gles11::ARRAY_BUFFER,
            if host_obj.contents != nil || host_obj.cg_context.is_some() {
                misc.basic_square_buffer
            } else {
                misc.flipped_square_buffer
            },
        );
        gles.TexCoordPointer(2, gles11::FLOAT, 0, 0 as *const GLvoid);
        // Opaque EAGL layers ignore framebuffer alpha. Do this during
        // sampling instead of rewriting every pixel on the CPU each frame.
        let ignore_texture_alpha = host_obj.opaque && host_obj.presented_pixels.is_some();
        if ignore_texture_alpha {
            gles.TexEnvi(
                gles11::TEXTURE_ENV,
                gles11::TEXTURE_ENV_MODE,
                gles11::COMBINE as _,
            );
            gles.TexEnvi(
                gles11::TEXTURE_ENV,
                gles11::COMBINE_RGB,
                gles11::MODULATE as _,
            );
            gles.TexEnvi(gles11::TEXTURE_ENV, gles11::SRC0_RGB, gles11::TEXTURE as _);
            gles.TexEnvi(
                gles11::TEXTURE_ENV,
                gles11::SRC1_RGB,
                gles11::PRIMARY_COLOR as _,
            );
            gles.TexEnvi(
                gles11::TEXTURE_ENV,
                gles11::COMBINE_ALPHA,
                gles11::REPLACE as _,
            );
            gles.TexEnvi(
                gles11::TEXTURE_ENV,
                gles11::SRC0_ALPHA,
                gles11::PRIMARY_COLOR as _,
            );
        }
        gles.Enable(gles11::TEXTURE_2D);
        gles.DrawElements(
            gles11::TRIANGLES,
            SQUARE_INDICES.len() as _,
            gles11::UNSIGNED_BYTE,
            0 as *const GLvoid,
        );

        if ignore_texture_alpha {
            gles.TexEnvi(
                gles11::TEXTURE_ENV,
                gles11::TEXTURE_ENV_MODE,
                gles11::MODULATE as _,
            );
        }

        // The alpha channel of an opaque layer's texture is meant to be
        // ignored. That doesn't matter when compositing onto black, but
        // overlays are drawn on top of an OpenGL ES frame afterwards (see
        // present_overlays), so make the layer's area fully opaque.
        if draw_opaque && env.framework_state.core_animation.composition.overlay_mode {
            gles.ColorMask(gles11::FALSE, gles11::FALSE, gles11::FALSE, gles11::TRUE);
            gles.Disable(gles11::TEXTURE_2D);
            gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
            gles.Color4f(1.0, 1.0, 1.0, 1.0);
            gles.DrawElements(
                gles11::TRIANGLES,
                SQUARE_INDICES.len() as _,
                gles11::UNSIGNED_BYTE,
                0 as *const GLvoid,
            );
            gles.ColorMask(gles11::TRUE, gles11::TRUE, gles11::TRUE, gles11::TRUE);
        }
    }
    std::mem::drop(gles);

    // avoid holding mutable borrow while recursing
    let original_host_obj = env.objc.borrow_mut::<CALayerHostObject>(layer);
    for &child_layer in &original_host_obj.sublayers.clone() {
        // TODO: clipping/masksToBounds support
        composite_layer_recursive(
            env,
            animation_state,
            child_layer,
            cumulative_transform,
            opacity,
        )
    }
}

const FLOATS_PER_POINT: usize = 2;
const BASIC_SQUARE_POINTS: [f32; 4 * FLOATS_PER_POINT] = [0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0];
const SQUARE_INDICES: [u8; 6] = [0, 1, 2, 2, 1, 3];
const FLIPPED_SQUARE_POINTS: [f32; 4 * FLOATS_PER_POINT] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0];
const FLOATS_PER_9PATCH: usize = BASIC_SQUARE_POINTS.len() * 3 * 3;
const INDICES_PER_9PATCH: usize = SQUARE_INDICES.len() * 3 * 3;

fn make_9patch_coords(x_edges: [f32; 4], y_edges: [f32; 4]) -> [f32; FLOATS_PER_9PATCH] {
    let mut out_points = [0.0; FLOATS_PER_9PATCH];
    #[allow(clippy::chunks_exact_to_as_chunks)]
    for (i, out_points_chunk) in out_points
        .chunks_exact_mut(BASIC_SQUARE_POINTS.len())
        .enumerate()
    {
        let (x, y) = (i % 3, i / 3);

        for (dst_xy, src_xy) in out_points_chunk
            .chunks_exact_mut(2)
            .zip(BASIC_SQUARE_POINTS.chunks_exact(2))
        {
            let (x1, x2) = (x_edges[x], x_edges[x + 1]);
            let (y1, y2) = (y_edges[y], y_edges[y + 1]);
            dst_xy[0] = x1 + src_xy[0] * (x2 - x1);
            dst_xy[1] = y1 + src_xy[1] * (y2 - y1);
        }
    }
    out_points
}

fn make_9patch_indices() -> [u8; INDICES_PER_9PATCH] {
    let mut out_indices = [0; SQUARE_INDICES.len() * 3 * 3];
    #[allow(clippy::chunks_exact_to_as_chunks)]
    for (i, out_indices_chunk) in out_indices
        .chunks_exact_mut(SQUARE_INDICES.len())
        .enumerate()
    {
        for (out_index, in_index) in out_indices_chunk
            .iter_mut()
            .zip(SQUARE_INDICES.iter().copied())
        {
            *out_index = in_index + i as u8 * (BASIC_SQUARE_POINTS.len() / FLOATS_PER_POINT) as u8;
        }
    }
    out_indices
}

unsafe fn upload_slice<T: SafeWrite>(
    gles: &mut dyn GLES,
    target: GLenum,
    data: &[T],
    usage: GLenum,
) {
    gles.BufferData(
        target,
        std::mem::size_of_val(data) as _,
        data.as_ptr() as *const _,
        usage,
    )
}

/// Log OpenGL ES errors in the compositor. They used to be fatal, but they
/// don't have to be, and on Android the OpenGL ES context can be in a bad
/// state after touchHLE returns from the background. Crashing then would lose
/// the app's unsaved progress, whereas a glitch might only last one frame.
unsafe fn warn_on_gl_error(gles: &mut dyn GLES, doing: &str) {
    let error = gles.GetError();
    if error != 0 {
        log!("Warning: OpenGL ES error {:#x} while {}", error, doing);
        debug_assert!(false, "OpenGL ES error in compositor");
    }
}

/// [upload_rgba8_pixels] for a layer's texture (which must be bound), reusing
/// the texture's storage if the size hasn't changed, and applying Scale2x if
/// enabled.
unsafe fn upload_layer_pixels(
    gles: &mut dyn GLES,
    state: &mut State,
    texture: GLuint,
    pixels: &[u8],
    dimensions: (u32, u32),
    allow_scale2x: bool,
) {
    // Images that are already large aren't pixel art that needs scaling, and
    // doubling them could exceed the GPU's maximum texture size.
    const SCALE2X_MAX_SIZE: u32 = 1024;
    let (pixels, dimensions) = if allow_scale2x
        && state.layer_scale2x
        && dimensions.0 <= SCALE2X_MAX_SIZE
        && dimensions.1 <= SCALE2X_MAX_SIZE
    {
        super::scale2x::scale2x_rgba8(pixels, dimensions, &mut state.scale2x_buffer);
        (
            &state.scale2x_buffer[..],
            (dimensions.0 * 2, dimensions.1 * 2),
        )
    } else {
        (pixels, dimensions)
    };
    if state.layer_texture_sizes.get(&texture) == Some(&dimensions) {
        gles.TexSubImage2D(
            gles11::TEXTURE_2D,
            0,
            0,
            0,
            dimensions.0 as _,
            dimensions.1 as _,
            gles11::RGBA,
            gles11::UNSIGNED_BYTE,
            pixels.as_ptr() as *const _,
        );
    } else {
        upload_rgba8_pixels(gles, pixels, dimensions);
        state.layer_texture_sizes.insert(texture, dimensions);
    }
}

unsafe fn upload_rgba8_pixels(gles: &mut dyn GLES, pixels: &[u8], dimensions: (u32, u32)) {
    gles.TexImage2D(
        gles11::TEXTURE_2D,
        0,
        gles11::RGBA as _,
        dimensions.0 as _,
        dimensions.1 as _,
        0,
        gles11::RGBA,
        gles11::UNSIGNED_BYTE,
        pixels.as_ptr() as *const _,
    );
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
}
