/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CAEAGLLayer`.

use super::ca_layer::CALayerHostObject;
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::{CGPoint, CGRect, CGSize};
use crate::matrix::Matrix;
use crate::objc::{id, msg, msg_class, objc_classes, Class, ClassExports};
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CAEAGLLayer: CALayer

// EAGLDrawable implementation (the only one)

- (id)drawableProperties {
    // FIXME: do we need to return an empty dictionary rather than nil?
    env.objc.borrow::<CALayerHostObject>(this).drawable_properties
}

- (())setDrawableProperties:(id)props { // NSDictionary<NSString*, id>*
    let props: id = msg![env; props copy];
    env.objc.borrow_mut::<CALayerHostObject>(this).drawable_properties = props;
}

@end

};

/// If `transform` is a rotation by a whole number of quarter turns (e.g. a
/// landscape app's view rotated to cover the portrait screen), returns that
/// number (0 to 3, clockwise on screen).
fn quarter_turns(transform: &CGAffineTransform) -> Option<u8> {
    const EPSILON: f32 = 1e-5;
    let close = |a: f32, b: f32| (a - b).abs() < EPSILON;
    let &CGAffineTransform { a, b, c, d, tx, ty } = transform;
    if !close(tx, 0.0) || !close(ty, 0.0) {
        return None;
    }
    [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)]
        .iter()
        .position(|&(cos, sin)| close(a, cos) && close(b, sin) && close(c, -sin) && close(d, cos))
        .map(|turns| turns as u8)
}

/// See [find_fullscreen_eagl_layer_with_overlays].
pub struct FullscreenEaglLayer {
    /// The `CAEAGLLayer`.
    pub layer: id,
    /// Total rotation of the layer, in quarter turns clockwise on screen.
    pub quarter_turns: u8,
    /// Visible layers that are drawn on top of the `CAEAGLLayer` (e.g. UIKit
    /// buttons over a game's OpenGL ES view), in drawing order, each with the
    /// cumulative transform of its superlayer (as used for composition).
    pub overlays: Vec<(id, Matrix<4>)>,
}

/// Find an opaque `CAEAGLLayer` that covers the whole screen (possibly rotated
/// by quarter turns), so that everything drawn before it is hidden by it. The
/// layers drawn after it, if any, are returned as overlays.
///
/// To avoid a state management nightmare, we want to have an internal OpenGL ES
/// context for compositing, separate from any OpenGL ES contexts the app uses
/// for its rendering. When we have a `CAEAGLLayer` though, we need to transfer
/// a rendered frame from the app's context to the compositor's context, and
/// unfortunately the most practical way to do this is `glReadPixels()`, which
/// is highly inefficient. To make things efficient, then, we have a shortcut:
/// if there's a single full-screen layer, we skip transferring between contexts
/// and present it directly from the app's context, and then draw any overlays
/// on top of it. This function is used to determine when that will happen.
pub fn find_fullscreen_eagl_layer_with_overlays(
    env: &mut Environment,
) -> Option<FullscreenEaglLayer> {
    if env.options.force_composition {
        return None;
    }

    let windows = env.framework_state.uikit.ui_view.ui_window.windows.clone();
    // Assumes the windows in the list are ordered back-to-front. Lower windows
    // are hidden by the layer if one is found in the top window.
    // TODO: this may not be correct once we support windowLevel.
    let top_window = windows
        .into_iter()
        .rev()
        .find(|&window| !msg![env; window isHidden])?;

    let screen_bounds: CGRect = {
        let screen: id = msg_class![env; UIScreen mainScreen];
        msg![env; screen bounds]
    };

    let root_layer: id = msg![env; top_window layer];
    let ca_eagl_layer_class: Class = msg_class![env; CAEAGLLayer class];
    search_for_fullscreen_eagl_layer(
        env,
        ca_eagl_layer_class,
        root_layer,
        screen_bounds.size,
        Matrix::identity(),
        0,
    )
}

/// Recursive part of [find_fullscreen_eagl_layer_with_overlays]. `parent_size`
/// is the size of the superlayer's bounds, `parent_transform` its cumulative
/// transform and `parent_turns` its total rotation.
fn search_for_fullscreen_eagl_layer(
    env: &mut Environment,
    ca_eagl_layer_class: Class,
    layer: id,
    parent_size: CGSize,
    parent_transform: Matrix<4>,
    parent_turns: u8,
) -> Option<FullscreenEaglLayer> {
    let host_obj: &CALayerHostObject = env.objc.borrow(layer);

    // This is stricter than it should be. In theory we should accumulate
    // the transforms and handle different anchor points etc, but real apps
    // probably only use these common cases: no transform, or a rotation
    // by quarter turns (with the bounds' width and height swapped, so the
    // layer still covers the screen exactly).
    let turns = quarter_turns(&host_obj.affine_transform)?;
    let expected_size = if turns % 2 == 1 {
        CGSize {
            width: parent_size.height,
            height: parent_size.width,
        }
    } else {
        parent_size
    };
    if host_obj.bounds.size != expected_size
        || host_obj.bounds.origin != (CGPoint { x: 0.0, y: 0.0 })
        || host_obj.anchor_point != (CGPoint { x: 0.5, y: 0.5 })
        || host_obj.position
            != (CGPoint {
                x: parent_size.width / 2.0,
                y: parent_size.height / 2.0,
            })
        || host_obj.hidden
        || host_obj.opacity != 1.0
    {
        return None;
    }
    let total_turns = (parent_turns + turns) % 4;
    // Same as in composition::composite_layer_recursive.
    let transform = <Matrix<4> as From<_>>::from(host_obj.superlayer_to_layer_transform())
        .multiply(&parent_transform);

    // Layers that are hidden or fully transparent don't affect what's shown
    // (apps often keep e.g. a hidden menu above their OpenGL view), so they're
    // skipped.
    let visible_sublayers: Vec<id> = host_obj
        .sublayers
        .iter()
        .copied()
        .filter(|&sublayer| {
            let sublayer = env.objc.borrow::<CALayerHostObject>(sublayer);
            !sublayer.hidden && sublayer.opacity > 0.0
        })
        .collect();
    let opaque = host_obj.opaque;

    if opaque && msg![env; layer isKindOfClass:ca_eagl_layer_class] {
        // Everything drawn after this layer is an overlay: its own sublayers.
        return Some(FullscreenEaglLayer {
            layer,
            quarter_turns: total_turns,
            overlays: visible_sublayers
                .into_iter()
                .map(|sublayer| (sublayer, transform))
                .collect(),
        });
    }

    // Look for the layer among the sublayers, starting from the top one (the
    // last one).
    // TODO: this is not correct once we support zPosition.
    for (i, &sublayer) in visible_sublayers.iter().enumerate().rev() {
        if let Some(mut found) = search_for_fullscreen_eagl_layer(
            env,
            ca_eagl_layer_class,
            sublayer,
            expected_size,
            transform,
            total_turns,
        ) {
            // The sublayers above it are drawn after it.
            found.overlays.extend(
                visible_sublayers[i + 1..]
                    .iter()
                    .map(|&sublayer| (sublayer, transform)),
            );
            return Some(found);
        }
    }
    None
}

/// For use by `EAGLContext` when presenting to a `CAEAGLLayer`:
/// [std::mem::take]s the buffer used to hold the pixels. It should be passed
/// back to [present_pixels] once it has been filled.
pub fn get_pixels_vec_for_presenting(env: &mut Environment, layer: id) -> Vec<u8> {
    env.objc
        .borrow_mut::<CALayerHostObject>(layer)
        .presented_pixels
        .take()
        .map(|(vec, _width, _height)| vec)
        .unwrap_or_default()
}

/// For use by `EAGLContext` when presenting to a `CAEAGLLayer`: provide the new
/// frame rendered by the app, so it can be used when compositing. The buffer
/// should have been obtained with [get_pixels_vec_for_presenting] before
/// filling. The data must be in RGBA8 format.
pub fn present_pixels(env: &mut Environment, layer: id, pixels: Vec<u8>, width: u32, height: u32) {
    let host_obj = env.objc.borrow_mut::<CALayerHostObject>(layer);
    host_obj.presented_pixels = Some((pixels, width, height));
    host_obj.gles_texture_is_up_to_date = false;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quarter_turns() {
        let rotation = |angle: f32| {
            let (sin, cos) = angle.sin_cos();
            CGAffineTransform {
                a: cos,
                b: sin,
                c: -sin,
                d: cos,
                tx: 0.0,
                ty: 0.0,
            }
        };
        use std::f32::consts::FRAC_PI_2;
        assert_eq!(quarter_turns(&rotation(0.0)), Some(0));
        // Not exact in floating point, as apps' rotations won't be either.
        assert_eq!(quarter_turns(&rotation(FRAC_PI_2)), Some(1));
        assert_eq!(quarter_turns(&rotation(2.0 * FRAC_PI_2)), Some(2));
        assert_eq!(quarter_turns(&rotation(3.0 * FRAC_PI_2)), Some(3));
        assert_eq!(quarter_turns(&rotation(-FRAC_PI_2)), Some(3));
        assert_eq!(quarter_turns(&rotation(0.5)), None);
        let mut translated = rotation(FRAC_PI_2);
        translated.tx = 10.0;
        assert_eq!(quarter_turns(&translated), None);
        let mut scaled = rotation(0.0);
        scaled.a = 2.0;
        assert_eq!(quarter_turns(&scaled), None);
    }
}
