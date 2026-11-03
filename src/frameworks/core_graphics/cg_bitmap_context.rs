/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGBitmapContext.h`

use super::cg_affine_transform::{CGAffineTransform, CGAffineTransformIdentity};
use super::cg_color_space::{
    kCGColorSpaceGenericGray, kCGColorSpaceGenericRGB, CGColorSpaceHostObject, CGColorSpaceRef,
};
use super::cg_context::{
    kCGBlendModeCopy, kCGBlendModeDarken, kCGBlendModeLighten, kCGBlendModeMultiply,
    kCGBlendModeNormal, kCGBlendModeScreen, CGBlendMode, CGContextHostObject, CGContextRef,
    CGContextSubclass,
};
use super::cg_gradient::{
    kCGGradientDrawsAfterEndLocation, kCGGradientDrawsBeforeStartLocation,
    CGGradientDrawingOptions, CGGradientHostObject, CGGradientRef,
};
use super::cg_image::{
    self, kCGBitmapAlphaInfoMask, kCGBitmapByteOrderMask, kCGImageAlphaFirst, kCGImageAlphaLast,
    kCGImageAlphaNone, kCGImageAlphaNoneSkipFirst, kCGImageAlphaNoneSkipLast, kCGImageAlphaOnly,
    kCGImageAlphaPremultipliedFirst, kCGImageAlphaPremultipliedLast, kCGImageByteOrder32Big,
    kCGImageByteOrder32Little, kCGImageByteOrderDefault, CGBitmapInfo, CGImageAlphaInfo,
    CGImageRef,
};
use super::cg_path::{self, Path};
use super::{CGFloat, CGPoint, CGRect, CGSize};
use crate::dyld::{export_c_func, FunctionExports};
use crate::image::{gamma_decode, gamma_decode_u8, gamma_encode, Image};
use crate::mem::{GuestUSize, Mem, MutVoidPtr, Ptr};
use crate::objc::ObjC;
use crate::Environment;

#[derive(Copy, Clone)]
pub(super) struct CGBitmapContextData {
    pub(super) data: MutVoidPtr,
    pub(super) data_is_owned: bool,
    width: GuestUSize,
    height: GuestUSize,
    bits_per_component: GuestUSize,
    bytes_per_row: GuestUSize,
    color_space: &'static str,
    alpha_info: CGImageAlphaInfo,
    /// If true, 32-bit pixels are stored little-endian (e.g. ARGB is stored as
    /// BGRA in memory), as with `kCGBitmapByteOrder32Little`.
    little_endian: bool,
    /// Cached result of [pixel_offsets] and [bytes_per_pixel], which are
    /// needed for every pixel drawn. See [CGBitmapContextData::with_layout].
    layout: Option<PixelLayout>,
    /// If true, drawing co-ordinates already point down (as for a layer's
    /// backing store, where UIKit's flip is implied), so CG's y=0 maps to the
    /// first row in memory. This matches the memory layout on iOS, where the
    /// first row of a layer's backing store is its top edge.
    y_down: bool,
}

pub fn CGBitmapContextCreate(
    env: &mut Environment,
    data: MutVoidPtr,
    width: GuestUSize,
    height: GuestUSize,
    bits_per_component: GuestUSize,
    bytes_per_row: GuestUSize,
    color_space: CGColorSpaceRef,
    bitmap_info: u32,
) -> CGContextRef {
    assert!(bits_per_component == 8); // TODO: support other bit depths

    let color_space = env.objc.borrow::<CGColorSpaceHostObject>(color_space).name;

    let component_count = match color_space {
        kCGColorSpaceGenericRGB => components_for_rgb(bitmap_info).unwrap(),
        kCGColorSpaceGenericGray => components_for_gray(bitmap_info).unwrap(),
        _ => unimplemented!("support other color spaces"),
    };

    let (data, data_is_owned, bytes_per_row) = if data.is_null() {
        let bytes_per_row = if bytes_per_row == 0 {
            width.checked_mul(component_count).unwrap()
        } else {
            bytes_per_row
        };
        let total_size = bytes_per_row.checked_mul(height).unwrap();
        let data = env.mem.alloc(total_size);
        (data, true, bytes_per_row)
    } else {
        assert!(bytes_per_row != 0);
        (data, false, bytes_per_row)
    };

    let host_object = CGContextHostObject {
        subclass: CGContextSubclass::CGBitmapContext(
            CGBitmapContextData {
                data,
                data_is_owned,
                width,
                height,
                bits_per_component,
                bytes_per_row,
                color_space,
                alpha_info: bitmap_info & kCGBitmapAlphaInfoMask,
                little_endian: bitmap_info & kCGBitmapByteOrderMask == kCGImageByteOrder32Little,
                layout: None,
                y_down: false,
            }
            .with_layout(),
        ),
        // TODO: is this the correct default?
        rgb_fill_color: (0.0, 0.0, 0.0, 0.0),
        font: Ptr::null(),
        font_size: 14.0,
        transform: CGAffineTransformIdentity,
        blend_mode: kCGBlendModeNormal,
        text_transform: None,
        text_drawing_mode: super::cg_context::kCGTextFill,
        state_stack: Vec::new(),
        path: Path::default(),
        clip: Vec::new(),
        // TODO: is this the correct default?
        rgb_stroke_color: (0.0, 0.0, 0.0, 1.0),
        alpha: 1.0,
        line_width: 1.0,
    };
    let isa = env.objc.get_known_class("_Podroid_CGContext", &mut env.mem);
    env.objc
        .alloc_object(isa, Box::new(host_object), &mut env.mem)
}

fn CGBitmapContextGetData(env: &mut Environment, context: CGContextRef) -> MutVoidPtr {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.data
}

pub fn CGBitmapContextGetWidth(env: &mut Environment, context: CGContextRef) -> GuestUSize {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.width
}

pub fn CGBitmapContextGetHeight(env: &mut Environment, context: CGContextRef) -> GuestUSize {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.height
}

fn CGBitmapContextGetBytesPerRow(env: &mut Environment, context: CGContextRef) -> GuestUSize {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    bitmap_data.bytes_per_row
}

pub fn CGBitmapContextCreateImage(env: &mut Environment, context: CGContextRef) -> CGImageRef {
    // TODO: Image::from_pixel_vec() should not exist, and this function should
    // support any bitmap format.
    let (width, height, pixels) = get_rgba8_pixels(&env.objc, &env.mem, context);
    cg_image::from_image(env, Image::from_pixel_vec(pixels, (width, height)))
}

/// Get a copy of a 32-bit RGB bitmap context's pixels, converted to RGBA8
/// (big-endian) order if needed.
pub fn get_rgba8_pixels(
    objc: &ObjC,
    mem: &Mem,
    context: CGContextRef,
) -> (GuestUSize, GuestUSize, Vec<u8>) {
    let host_obj = objc.borrow::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    assert!(
        bitmap_data.bits_per_component == 8
            && bitmap_data.bytes_per_row == bitmap_data.width * 4
            && bitmap_data.color_space == kCGColorSpaceGenericRGB
            && bytes_per_pixel(&bitmap_data) == 4
    );
    let bytes = mem.bytes_at(
        bitmap_data.data.cast(),
        bitmap_data.bytes_per_row * bitmap_data.height,
    );
    let offsets = pixel_offsets(&bitmap_data);
    let pixels = if offsets == (0, 1, 2, Some(3)) {
        bytes.to_vec()
    } else {
        let (r, g, b, a) = offsets;
        let mut out = vec![0u8; bytes.len()];
        for (o, p) in out
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(bytes.as_chunks::<4>().0)
        {
            *o = [p[r], p[g], p[b], a.map_or(255, |a| p[a])];
        }
        out
    };
    (bitmap_data.width, bitmap_data.height, pixels)
}

fn components_for_rgb(bitmap_info: CGBitmapInfo) -> Result<GuestUSize, ()> {
    let byte_order = bitmap_info & kCGBitmapByteOrderMask;
    if byte_order != kCGImageByteOrderDefault
        && byte_order != kCGImageByteOrder32Big
        && byte_order != kCGImageByteOrder32Little
    {
        return Err(()); // TODO: handle other byte orders
    }

    let alpha_info = bitmap_info & kCGBitmapAlphaInfoMask;
    if (alpha_info | byte_order) != bitmap_info {
        return Err(()); // TODO: handle other cases (float)
    }
    if byte_order == kCGImageByteOrder32Little
        && matches!(alpha_info, kCGImageAlphaNone | kCGImageAlphaOnly)
    {
        return Err(()); // 32-bit byte order needs 32-bit pixels
    }
    match alpha_info & kCGBitmapAlphaInfoMask {
        kCGImageAlphaNone => Ok(3), // RGB
        kCGImageAlphaPremultipliedLast
        | kCGImageAlphaPremultipliedFirst
        | kCGImageAlphaLast
        | kCGImageAlphaFirst
        | kCGImageAlphaNoneSkipLast
        | kCGImageAlphaNoneSkipFirst => Ok(4), // RGBA/ARGB/RGBX/XRGB
        kCGImageAlphaOnly => Ok(1), // A
        _ => Err(()),               // unknown values
    }
}

fn components_for_gray(bitmap_info: CGBitmapInfo) -> Result<GuestUSize, ()> {
    let byte_order = bitmap_info & kCGBitmapByteOrderMask;
    if byte_order != kCGImageByteOrderDefault && byte_order != kCGImageByteOrder32Big {
        return Err(()); // TODO: handle other byte orders
    }

    let alpha_info = bitmap_info & kCGBitmapAlphaInfoMask;
    if (alpha_info | byte_order) != bitmap_info {
        return Err(()); // TODO: handle other cases (float)
    }
    match alpha_info & kCGBitmapAlphaInfoMask {
        kCGImageAlphaNone => Ok(1), // gray
        kCGImageAlphaPremultipliedLast
        | kCGImageAlphaPremultipliedFirst
        | kCGImageAlphaLast
        | kCGImageAlphaFirst
        | kCGImageAlphaNoneSkipLast
        | kCGImageAlphaNoneSkipFirst => Ok(2), // gray + alpha
        kCGImageAlphaOnly => Ok(1), // A
        _ => Err(()),               // unknown values
    }
}

fn bytes_per_pixel(data: &CGBitmapContextData) -> GuestUSize {
    if let Some(layout) = data.layout {
        return layout.bytes_per_pixel;
    }
    let &CGBitmapContextData {
        bits_per_component,
        color_space,
        alpha_info,
        ..
    } = data;
    assert!(bits_per_component == 8);
    match color_space {
        kCGColorSpaceGenericRGB => components_for_rgb(alpha_info).unwrap(),
        kCGColorSpaceGenericGray => components_for_gray(alpha_info).unwrap(),
        _ => unimplemented!("support other color spaces"),
    }
}

fn get_pixels<'a>(data: &CGBitmapContextData, mem: &'a mut Mem) -> &'a mut [u8] {
    let pixel_data_size = data.height.checked_mul(data.bytes_per_row).unwrap();
    mem.bytes_at_mut(data.data.cast(), pixel_data_size)
}

/// Blends two RGBA non gamma-encoded values, with straight alpha,
/// using a blend mode.
fn blend_straight(
    bg: (f32, f32, f32, f32),
    fg: (f32, f32, f32, f32),
    blend_mode: CGBlendMode,
) -> (f32, f32, f32, f32) {
    assert_eq!(blend_mode, kCGBlendModeNormal); // TODO
    if fg.3 == 0.0 {
        // If fg.3 == 0.0 we attempt to blend fully transparent color.
        bg
    } else {
        let neg_fg_a = 1.0 - fg.3;
        let new_a = fg.3 + bg.3 * neg_fg_a; // Can't be 0 if fg.3 != 0
        (
            (fg.0 * fg.3 + bg.0 * bg.3 * neg_fg_a) / new_a,
            (fg.1 * fg.3 + bg.1 * bg.3 * neg_fg_a) / new_a,
            (fg.2 * fg.3 + bg.2 * bg.3 * neg_fg_a) / new_a,
            new_a,
        )
    }
}

/// Blends two RGBA non gamma-encoded values, with premultiplied alpha,
/// using a blend mode.
/// See [Blending](https://www.w3.org/TR/compositing-1/#blending) for details.
fn blend_premultiplied(
    bg: (f32, f32, f32, f32),
    fg: (f32, f32, f32, f32),
    blend_mode: CGBlendMode,
) -> (f32, f32, f32, f32) {
    if blend_mode == kCGBlendModeCopy {
        return fg;
    }
    // Blend
    let blend_res = match blend_mode {
        kCGBlendModeNormal => (bg.3 * fg.0, bg.3 * fg.1, bg.3 * fg.2),
        kCGBlendModeMultiply => (bg.0 * fg.0, bg.1 * fg.1, bg.2 * fg.2),
        kCGBlendModeScreen => (
            fg.3 * bg.0 + bg.3 * fg.0 - bg.0 * fg.0,
            fg.3 * bg.1 + bg.3 * fg.1 - bg.1 * fg.1,
            fg.3 * bg.2 + bg.3 * fg.2 - bg.2 * fg.2,
        ),
        kCGBlendModeDarken => (
            (fg.3 * bg.0).min(bg.3 * fg.0),
            (fg.3 * bg.1).min(bg.3 * fg.1),
            (fg.3 * bg.2).min(bg.3 * fg.2),
        ),
        kCGBlendModeLighten => (
            (fg.3 * bg.0).max(bg.3 * fg.0),
            (fg.3 * bg.1).max(bg.3 * fg.1),
            (fg.3 * bg.2).max(bg.3 * fg.2),
        ),
        _ => unimplemented!("blend mode {}", blend_mode),
    };
    // Compose
    let neg_bg_a = 1.0 - bg.3;
    let neg_fg_a = 1.0 - fg.3;
    (
        fg.0 * neg_bg_a + blend_res.0 + bg.0 * neg_fg_a,
        fg.1 * neg_bg_a + blend_res.1 + bg.1 * neg_fg_a,
        fg.2 * neg_bg_a + blend_res.2 + bg.2 * neg_fg_a,
        fg.3 + bg.3 * neg_fg_a,
    )
}

/// per component offsets (r, g, b, a)
/// Byte offsets of the R, G, B and (if present) A components within a pixel,
/// and the size of a pixel in bytes.
#[derive(Copy, Clone)]
struct PixelLayout {
    offsets: (usize, usize, usize, Option<usize>),
    bytes_per_pixel: GuestUSize,
}

impl CGBitmapContextData {
    /// Fill in the cached [PixelLayout]. Must be called after creating or
    /// changing the format fields.
    fn with_layout(mut self) -> Self {
        self.layout = None;
        self.layout = Some(PixelLayout {
            offsets: pixel_offsets(&self),
            bytes_per_pixel: bytes_per_pixel(&self),
        });
        self
    }
}

fn pixel_offsets(data: &CGBitmapContextData) -> (usize, usize, usize, Option<usize>) {
    if let Some(layout) = data.layout {
        return layout.offsets;
    }
    let (r, g, b, a) = pixel_offsets_big_endian(data);
    if data.little_endian {
        // 32-bit pixel stored little-endian: byte order is reversed.
        (3 - r, 3 - g, 3 - b, a.map(|a| 3 - a))
    } else {
        (r, g, b, a)
    }
}
fn pixel_offsets_big_endian(data: &CGBitmapContextData) -> (usize, usize, usize, Option<usize>) {
    match data.color_space {
        kCGColorSpaceGenericRGB => {
            match data.alpha_info {
                kCGImageAlphaNone => (0, 1, 2, None),
                kCGImageAlphaPremultipliedLast | kCGImageAlphaLast => (0, 1, 2, Some(3)),
                kCGImageAlphaPremultipliedFirst | kCGImageAlphaFirst => (1, 2, 3, Some(0)),
                kCGImageAlphaNoneSkipLast => (0, 1, 2, None),
                kCGImageAlphaNoneSkipFirst => (1, 2, 3, None),
                kCGImageAlphaOnly => (0, 0, 0, Some(0)),
                _ => unreachable!(), // checked by bytes_per_pixel
            }
        }
        kCGColorSpaceGenericGray => {
            // TODO: this is probably isn't doing RGB to grayscale conversion
            // properly
            match data.alpha_info {
                kCGImageAlphaNone => (0, 0, 0, None),
                kCGImageAlphaPremultipliedLast | kCGImageAlphaLast => (0, 0, 0, Some(1)),
                kCGImageAlphaPremultipliedFirst | kCGImageAlphaFirst => (1, 1, 1, Some(0)),
                kCGImageAlphaNoneSkipLast => (0, 0, 0, None),
                kCGImageAlphaNoneSkipFirst => (1, 1, 1, None),
                kCGImageAlphaOnly => (0, 0, 0, Some(0)),
                _ => unreachable!(), // checked by bytes_per_pixel
            }
        }
        _ => unimplemented!(),
    }
}

/// Equivalent to `(gamma_encode(v) * 255.0) as u8`, but without `powf`: finds
/// the largest 8-bit value whose decoded intensity doesn't exceed `v`.
fn gamma_encode_u8(v: f32) -> u8 {
    static THRESHOLDS: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    let t = THRESHOLDS.get_or_init(|| {
        // The smallest input encoding to each value, so that rounding
        // matches the direct calculation.
        std::array::from_fn(|i| {
            if i == 0 {
                return f32::NEG_INFINITY;
            }
            let mut lo = gamma_decode((i as f32 - 1.0) / 255.0);
            let mut hi = gamma_decode((i as f32 + 0.5) / 255.0).min(1.0);
            for _ in 0..40 {
                let mid = (lo + hi) / 2.0;
                if (gamma_encode(mid) * 255.0) as u8 >= i as u8 {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            hi
        })
    });
    if v.is_nan() || v <= 0.0 {
        return (gamma_encode(v.max(0.0)) * 255.0) as u8;
    }
    // number of thresholds <= v, minus one
    (t.partition_point(|&th| th <= v) - 1) as u8
}

/// Get gamma-decoded RGBA value.
fn get_pixel(
    data: &CGBitmapContextData,
    pixels: &mut [u8],
    first_component_idx: usize,
) -> (f32, f32, f32, f32) {
    let pixel_offset = pixel_offsets(data);
    (
        gamma_decode_u8(pixels[first_component_idx + pixel_offset.0]),
        gamma_decode_u8(pixels[first_component_idx + pixel_offset.1]),
        gamma_decode_u8(pixels[first_component_idx + pixel_offset.2]),
        if let Some(alpha_offest) = pixel_offset.3 {
            pixels[first_component_idx + alpha_offest] as f32 / 255.0
        } else {
            1.0
        },
    )
}

fn put_pixel(
    data: &CGBitmapContextData,
    pixels: &mut [u8],
    coords: (i32, i32),
    pixel: (CGFloat, CGFloat, CGFloat, CGFloat),
    blend: bool,
    blend_mode: CGBlendMode,
) {
    let (x, y) = coords;
    if x < 0 || y < 0 {
        return;
    }
    let (x, y) = (x as GuestUSize, y as GuestUSize);
    if x >= data.width || y >= data.height {
        return;
    }

    // CG's co-ordinate system puts the origin in the bottom-left corner, but it
    // *seems* like the rows are nonetheless in top-to-bottom order?
    let y = if data.y_down { y } else { data.height - 1 - y };

    let pixel_size = bytes_per_pixel(data);
    let first_component_idx = (y * data.bytes_per_row + x * pixel_size) as usize;

    let bg_pixel = get_pixel(data, pixels, first_component_idx);

    // Blending like this must be done in linear RGB, so this must come before
    // gamma encoding.
    let (r, g, b, a) = if blend {
        match data.alpha_info {
            kCGImageAlphaLast | kCGImageAlphaFirst => blend_straight(bg_pixel, pixel, blend_mode),
            kCGImageAlphaPremultipliedLast | kCGImageAlphaPremultipliedFirst => {
                blend_premultiplied(bg_pixel, pixel, blend_mode)
            }
            kCGImageAlphaOnly => (
                pixel.0,
                pixel.1,
                pixel.2,
                pixel.3 + bg_pixel.3 * (1.0 - pixel.3),
            ),
            _ => {
                assert_eq!(blend_mode, kCGBlendModeNormal); // TODO
                pixel
            }
        }
    } else {
        pixel
    };

    // Alpha is always linear.
    let pixel_offset = pixel_offsets(data);
    match data.alpha_info {
        kCGImageAlphaOnly => {
            pixels[first_component_idx] = (a * 255.0) as u8;
        }
        _ => {
            pixels[first_component_idx + pixel_offset.0] = gamma_encode_u8(r);
            pixels[first_component_idx + pixel_offset.1] = gamma_encode_u8(g);
            pixels[first_component_idx + pixel_offset.2] = gamma_encode_u8(b);
            if let Some(alpha_offset) = pixel_offset.3 {
                pixels[first_component_idx + alpha_offset] = (a * 255.0) as u8;
            }
        }
    }
}

/// Abstract interface for use by host code that wants to draw in a bitmap
/// context.
pub struct CGBitmapContextDrawer<'a> {
    bitmap_info: CGBitmapContextData,
    rgb_fill_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    blend_mode: CGBlendMode,
    alpha: CGFloat,
    transform: CGAffineTransform,
    pixels: &'a mut [u8],
}
impl CGBitmapContextDrawer<'_> {
    pub fn new<'a>(
        objc: &ObjC,
        mem: &'a mut Mem,
        context: CGContextRef,
    ) -> CGBitmapContextDrawer<'a> {
        let &CGContextHostObject {
            subclass: CGContextSubclass::CGBitmapContext(bitmap_info),
            rgb_fill_color,
            transform,
            blend_mode,
            alpha,
            ..
        } = objc.borrow(context);

        let pixels = get_pixels(&bitmap_info, mem);

        CGBitmapContextDrawer {
            bitmap_info,
            rgb_fill_color,
            blend_mode,
            alpha,
            transform,
            pixels,
        }
    }

    pub fn width(&self) -> GuestUSize {
        self.bitmap_info.width
    }
    pub fn height(&self) -> GuestUSize {
        self.bitmap_info.height
    }
    /// Get the current fill color. The returned color is linear RGB, not sRGB.
    /// It has premultiplied alpha if the context does.
    pub fn rgb_fill_color(&self) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
        let multiply_by = match self.bitmap_info.alpha_info {
            kCGImageAlphaPremultipliedLast | kCGImageAlphaPremultipliedFirst => {
                self.rgb_fill_color.3
            }
            _ => 1.0,
        };
        // Multiplying before decoding matches the Simulator's output.
        (
            gamma_decode(self.rgb_fill_color.0 * multiply_by),
            gamma_decode(self.rgb_fill_color.1 * multiply_by),
            gamma_decode(self.rgb_fill_color.2 * multiply_by),
            self.rgb_fill_color.3, // alpha is always linear
        )
    }
    /// Set the pixel at `coords` to `color`. `color` must be linear RGB, not
    /// sRGB! Note that `coords` are absolute: you must do transformation
    /// yourself.
    pub fn put_pixel(
        &mut self,
        coords: (i32, i32),
        mut color: (CGFloat, CGFloat, CGFloat, CGFloat),
        blend: bool,
    ) {
        if blend && self.alpha < 1.0 {
            if matches!(
                self.bitmap_info.alpha_info,
                kCGImageAlphaPremultipliedLast | kCGImageAlphaPremultipliedFirst
            ) {
                color.0 = gamma_decode(gamma_encode(color.0) * self.alpha);
                color.1 = gamma_decode(gamma_encode(color.1) * self.alpha);
                color.2 = gamma_decode(gamma_encode(color.2) * self.alpha);
            }
            color.3 *= self.alpha;
        }
        put_pixel(
            &self.bitmap_info,
            self.pixels,
            coords,
            color,
            blend,
            self.blend_mode,
        )
    }

    /// Index of the first byte of the pixel at in-bounds absolute `(x, y)`.
    fn pixel_index(&self, x: i32, y: i32) -> usize {
        let data = &self.bitmap_info;
        let (x, y) = (x as GuestUSize, y as GuestUSize);
        let y = if data.y_down { y } else { data.height - 1 - y };
        (y * data.bytes_per_row + x * bytes_per_pixel(data)) as usize
    }

    /// The byte offsets within a pixel that drawing writes to.
    fn component_offsets(&self) -> Vec<usize> {
        let data = &self.bitmap_info;
        if data.alpha_info == kCGImageAlphaOnly {
            return vec![0];
        }
        let (r, g, b, a) = pixel_offsets(data);
        let mut offsets = vec![r, g, b];
        offsets.extend(a);
        offsets.sort_unstable();
        offsets.dedup();
        offsets
    }

    /// If the current transform has no rotation or skew, returns the pixel
    /// columns and rows covered by `untransformed_rect`. The covered pixels are
    /// then exactly those [Self::iter_transformed_pixels] would produce (the
    /// same calculation is done, just once per column and row).
    fn axis_aligned_coverage(&self, untransformed_rect: CGRect) -> Option<(Vec<i32>, Vec<i32>)> {
        if self.transform.b != 0.0 || self.transform.c != 0.0 {
            return None;
        }
        let bounding_rect = self.transform.apply_to_rect(untransformed_rect);
        let x_start = bounding_rect.origin.x.round().max(0.0) as GuestUSize;
        let y_start = bounding_rect.origin.y.round().max(0.0) as GuestUSize;
        let x_end = (bounding_rect.origin.x + bounding_rect.size.width)
            .round()
            .min(self.width() as f32) as GuestUSize;
        let y_end = (bounding_rect.origin.y + bounding_rect.size.height)
            .round()
            .min(self.height() as f32) as GuestUSize;
        let inverse_transform = self.transform.invert();
        let within = |v: f32, origin: f32, size: f32| (0.0..1.0).contains(&((v - origin) / size));
        // With no rotation or skew, the x result doesn't depend on y and vice
        // versa, so any fixed value can be used for the other co-ordinate.
        let xs = (x_start..x_end)
            .filter(|&x| {
                let p = inverse_transform.apply_to_point(CGPoint {
                    x: x as f32 + 0.5,
                    y: y_start as f32 + 0.5,
                });
                within(
                    p.x,
                    untransformed_rect.origin.x,
                    untransformed_rect.size.width,
                )
            })
            .map(|x| x as i32)
            .collect();
        let ys = (y_start..y_end)
            .filter(|&y| {
                let p = inverse_transform.apply_to_point(CGPoint {
                    x: x_start as f32 + 0.5,
                    y: y as f32 + 0.5,
                });
                within(
                    p.y,
                    untransformed_rect.origin.y,
                    untransformed_rect.size.height,
                )
            })
            .map(|y| y as i32)
            .collect();
        Some((xs, ys))
    }

    /// Takes a [CGRect] and applies the current transform to it, and iterates
    /// over the transformed, clipped, absolute integer pixel co-ordinates in
    /// raster order for the target bitmap while providing floating-point
    /// co-ordinates from (0,0) to (1,1) as a reference for sampling e.g. a
    /// texture.
    pub fn iter_transformed_pixels(
        &self,
        untransformed_rect: CGRect,
    ) -> impl Iterator<Item = ((i32, i32), (f32, f32))> {
        let bounding_rect = self.transform.apply_to_rect(untransformed_rect);

        let x_start = bounding_rect.origin.x.round().max(0.0) as GuestUSize;
        let y_start = bounding_rect.origin.y.round().max(0.0) as GuestUSize;
        let x_end = (bounding_rect.origin.x + bounding_rect.size.width)
            .round()
            .min(self.width() as f32) as GuestUSize;
        let y_end = (bounding_rect.origin.y + bounding_rect.size.height)
            .round()
            .min(self.height() as f32) as GuestUSize;

        let inverse_transform = self.transform.invert();

        // TODO: Doing a matrix multiply per-pixel is not optimally efficient.
        // A scanline rasterizer would be better, though we should probably use
        // an existing library for this.
        (y_start..y_end).flat_map(move |y| {
            (x_start..x_end).flat_map(move |x| {
                let untransformed = inverse_transform.apply_to_point(CGPoint {
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                });
                let x_within =
                    (untransformed.x - untransformed_rect.origin.x) / untransformed_rect.size.width;
                let y_within = (untransformed.y - untransformed_rect.origin.y)
                    / untransformed_rect.size.height;
                if !(0.0..1.0).contains(&x_within) || !(0.0..1.0).contains(&y_within) {
                    None
                } else {
                    Some(((x as i32, y as i32), (x_within, y_within)))
                }
            })
        })
    }
}

#[cfg(test)]
#[test]
fn test_iter_transformed_pixels() {
    use super::CGSize;

    fn make_context(
        width: GuestUSize,
        height: GuestUSize,
        transform: CGAffineTransform,
    ) -> CGBitmapContextDrawer<'static> {
        CGBitmapContextDrawer {
            bitmap_info: CGBitmapContextData {
                data: crate::mem::Ptr::null(),
                data_is_owned: false,
                width,
                height,
                bits_per_component: 8,
                bytes_per_row: 3 * width,
                color_space: "kCGColorSpaceGenericRGB",
                alpha_info: 0,
                little_endian: false,
                layout: None,
                y_down: false,
            }
            .with_layout(),
            rgb_fill_color: (0.0, 0.0, 0.0, 0.0),
            blend_mode: kCGBlendModeNormal,
            alpha: 1.0,
            transform,
            pixels: &mut [],
        }
    }

    let square_2x2_at_0_0 = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 2.0,
            height: 2.0,
        },
    };
    let square_2x2_at_2_2 = CGRect {
        origin: CGPoint { x: 2.0, y: 2.0 },
        size: CGSize {
            width: 2.0,
            height: 2.0,
        },
    };
    let square_4x4_at_0_0 = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 4.0,
            height: 4.0,
        },
    };

    let upright_square_2x2_at_0_0 = [
        ((0, 0), (0.25, 0.25)),
        ((1, 0), (0.75, 0.25)),
        ((0, 1), (0.25, 0.75)),
        ((1, 1), (0.75, 0.75)),
    ];
    let inverted_square_2x2_at_0_0 = [
        ((0, 0), (0.75, 0.75)),
        ((1, 0), (0.25, 0.75)),
        ((0, 1), (0.75, 0.25)),
        ((1, 1), (0.25, 0.25)),
    ];
    let corner_pixel_of_upright_square_2x2_at_1_1 = [((1, 1), (0.25, 0.25))];

    // Constructed by hand so the results are precise
    let rotation_by_180deg = CGAffineTransform {
        a: -1.0,
        c: 0.0,
        b: 0.0,
        d: -1.0,
        tx: 0.0,
        ty: 0.0,
    };

    assert!(make_context(2, 2, CGAffineTransformIdentity)
        .iter_transformed_pixels(square_2x2_at_0_0)
        .eq(upright_square_2x2_at_0_0.into_iter()));
    assert!(
        make_context(2, 2, CGAffineTransform::make_translation(-2.0, -2.0))
            .iter_transformed_pixels(square_2x2_at_2_2)
            .eq(upright_square_2x2_at_0_0.into_iter())
    );
    assert!(
        make_context(2, 2, CGAffineTransform::make_translation(-1.0, -1.0))
            .iter_transformed_pixels(square_2x2_at_2_2)
            .eq(corner_pixel_of_upright_square_2x2_at_1_1.into_iter())
    );
    assert!(make_context(2, 2, CGAffineTransform::make_scale(0.5, 0.5))
        .iter_transformed_pixels(square_4x4_at_0_0)
        .eq(upright_square_2x2_at_0_0.into_iter()));
    assert!(make_context(2, 2, rotation_by_180deg.translate(-2.0, -2.0))
        .iter_transformed_pixels(square_2x2_at_0_0)
        .eq(inverted_square_2x2_at_0_0.into_iter()));
}

#[cfg(test)]
#[test]
fn test_axis_aligned_coverage_matches_iter_transformed_pixels() {
    use super::CGSize;
    let mut seed: u32 = 12345;
    let mut rnd = move |lo: f32, hi: f32| {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        lo + (hi - lo) * ((seed >> 8) as f32 / (1u32 << 24) as f32)
    };
    for i in 0..3000 {
        let pick = |v: f32, choices: &[f32]| choices[(v as usize) % choices.len()];
        let sx = pick(rnd(0.0, 100.0), &[1.0, -1.0, 2.0, 0.5, 1.5, -2.0, 3.0]);
        let sy = pick(rnd(0.0, 100.0), &[1.0, -1.0, 2.0, 0.5, 1.5, -2.0, 3.0]);
        let (tx, ty) = if i % 2 == 0 {
            (rnd(-20.0, 60.0).round(), rnd(-20.0, 60.0).round())
        } else {
            (rnd(-20.0, 60.0), rnd(-20.0, 60.0))
        };
        let transform = CGAffineTransform {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            tx,
            ty,
        };
        let rect = CGRect {
            origin: CGPoint {
                x: rnd(-30.0, 40.0),
                y: rnd(-30.0, 40.0),
            },
            size: CGSize {
                width: rnd(0.5, 50.0),
                height: rnd(0.5, 50.0),
            },
        };
        let drawer = CGBitmapContextDrawer {
            bitmap_info: CGBitmapContextData {
                data: crate::mem::Ptr::null(),
                data_is_owned: false,
                width: 40,
                height: 30,
                bits_per_component: 8,
                bytes_per_row: 4 * 40,
                color_space: "kCGColorSpaceGenericRGB",
                alpha_info: kCGImageAlphaPremultipliedLast,
                little_endian: false,
                layout: None,
                y_down: false,
            }
            .with_layout(),
            rgb_fill_color: (0.0, 0.0, 0.0, 0.0),
            blend_mode: kCGBlendModeNormal,
            alpha: 1.0,
            transform,
            pixels: &mut [],
        };
        let expected: Vec<(i32, i32)> = drawer
            .iter_transformed_pixels(rect)
            .map(|(c, _)| c)
            .collect();
        let (xs, ys) = drawer.axis_aligned_coverage(rect).unwrap();
        let actual: Vec<(i32, i32)> = ys
            .iter()
            .flat_map(|&y| xs.iter().map(move |&x| (x, y)))
            .collect();
        assert_eq!(expected, actual, "transform {transform:?} rect {rect:?}");
    }
}

#[cfg(test)]
#[test]
fn test_gamma_lookup_tables_match_direct_calculation() {
    for i in 0..=255u8 {
        assert_eq!(gamma_decode_u8(i), gamma_decode(i as f32 / 255.0));
    }
    let mut mismatches = 0;
    for i in 0..=1_000_000u32 {
        let v = i as f32 / 1_000_000.0;
        if gamma_encode_u8(v) != (gamma_encode(v) * 255.0) as u8 {
            mismatches += 1;
        }
    }
    for v in [-1.0f32, 0.0, 1.0, 1.5, 100.0] {
        assert_eq!(
            gamma_encode_u8(v),
            (gamma_encode(v.max(0.0)) * 255.0) as u8,
            "{v}"
        );
    }
    assert_eq!(mismatches, 0);
}

/// Implementation of `CGContextFillRect` (`clear` == [false]) and
/// `CGContextClearRect` (`clear` == [true]) for `CGBitmapContext`.
pub(super) fn fill_rect(env: &mut Environment, context: CGContextRef, rect: CGRect, clear: bool) {
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let color = if clear {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        drawer.rgb_fill_color()
    };
    // When the result doesn't depend on what's already there (clearing, or an
    // opaque fill in the normal blend mode), compute the pixel's bytes once
    // and copy them, rather than blending and gamma-encoding every pixel.
    // This matters a lot: layers are cleared before every redraw.
    let independent_of_bg =
        clear || (color.3 >= 1.0 && drawer.alpha >= 1.0 && drawer.blend_mode == kCGBlendModeNormal);
    if independent_of_bg {
        if let Some((xs, ys)) = drawer.axis_aligned_coverage(rect) {
            let Some(&first_y) = ys.first() else { return };
            let Some(&first_x) = xs.first() else { return };
            // Draw one pixel normally to get its bytes, then copy them.
            drawer.put_pixel((first_x, first_y), color, /* blend: */ !clear);
            let idx = drawer.pixel_index(first_x, first_y);
            let bytes: Vec<(usize, u8)> = drawer
                .component_offsets()
                .into_iter()
                .map(|o| (o, drawer.pixels[idx + o]))
                .collect();
            let bpp = bytes_per_pixel(&drawer.bitmap_info) as usize;
            let contiguous = xs.last().unwrap() - first_x + 1 == xs.len() as i32;
            let whole_pixel = bytes.len() == bpp;
            let all_zero = bytes.iter().all(|&(_, v)| v == 0);
            let mut pixel_bytes = vec![0u8; bpp];
            for &(offset, value) in &bytes {
                pixel_bytes[offset] = value;
            }
            for &y in &ys {
                if contiguous {
                    // The pixels of a row are next to each other in memory.
                    let start = drawer.pixel_index(first_x, y);
                    let row = &mut drawer.pixels[start..start + xs.len() * bpp];
                    if whole_pixel && all_zero {
                        row.fill(0);
                    } else if whole_pixel {
                        for pixel in row.chunks_exact_mut(bpp) {
                            pixel.copy_from_slice(&pixel_bytes);
                        }
                    } else {
                        for pixel in row.chunks_exact_mut(bpp) {
                            for &(offset, value) in &bytes {
                                pixel[offset] = value;
                            }
                        }
                    }
                } else {
                    for &x in &xs {
                        let idx = drawer.pixel_index(x, y);
                        for &(offset, value) in &bytes {
                            drawer.pixels[idx + offset] = value;
                        }
                    }
                }
            }
            return;
        }
    }
    // TODO: correct anti-aliasing
    for ((x, y), _) in drawer.iter_transformed_pixels(rect).collect::<Vec<_>>() {
        drawer.put_pixel((x, y), color, /* blend: */ !clear)
    }
}

pub(super) fn fill_ellipse(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let color = drawer.rgb_fill_color();
    // TODO: correct anti-aliasing
    for ((x, y), (u, v)) in drawer.iter_transformed_pixels(rect) {
        let (du, dv) = (u - 0.5, v - 0.5);
        if du * du + dv * dv <= 0.25 {
            drawer.put_pixel((x, y), color, /* blend: */ true)
        }
    }
}

/// Visits the device pixels inside `area` (in device space) and the context's
/// clip, with each pixel's center. `coverage` gives how much of the pixel to
/// draw at that point (0 to 1), and the color to draw it with, which is
/// non-premultiplied sRGB.
fn draw_area(
    env: &mut Environment,
    context: CGContextRef,
    area: Option<CGRect>,
    coverage: impl Fn(CGPoint) -> Option<(CGFloat, (CGFloat, CGFloat, CGFloat, CGFloat))>,
) {
    let clip = env.objc.borrow::<CGContextHostObject>(context).clip.clone();
    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let premultiply = matches!(
        drawer.bitmap_info.alpha_info,
        kCGImageAlphaPremultipliedLast | kCGImageAlphaPremultipliedFirst
    );
    let mut bounds = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: drawer.width() as f32,
            height: drawer.height() as f32,
        },
    };
    for rect in area.into_iter().chain(clip.iter().map(|path| {
        path.bounds().unwrap_or(CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 0.0,
                height: 0.0,
            },
        })
    })) {
        bounds = intersect_device_rect(bounds, rect);
    }
    let x_start = bounds.origin.x.floor().max(0.0) as i32;
    let y_start = bounds.origin.y.floor().max(0.0) as i32;
    let x_end = (bounds.origin.x + bounds.size.width).ceil() as i32;
    let y_end = (bounds.origin.y + bounds.size.height).ceil() as i32;
    for y in y_start..y_end {
        for x in x_start..x_end {
            let center = CGPoint {
                x: x as f32 + 0.5,
                y: y as f32 + 0.5,
            };
            if !clip.iter().all(|path| path.contains(center, false)) {
                continue;
            }
            let Some((coverage, (r, g, b, a))) = coverage(center) else {
                continue;
            };
            let a = a * coverage.clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let m = if premultiply { a } else { 1.0 };
            let color = (
                gamma_decode(r * m),
                gamma_decode(g * m),
                gamma_decode(b * m),
                a,
            );
            drawer.put_pixel((x, y), color, /* blend: */ true);
        }
    }
}

/// Shared implementation of `CGContextDrawLinearGradient` and
/// `CGContextDrawRadialGradient`. `t_at` gives the gradient position at a
/// point in user space, if the gradient covers it.
pub(super) fn draw_gradient(
    env: &mut Environment,
    context: CGContextRef,
    gradient: CGGradientRef,
    options: CGGradientDrawingOptions,
    t_at: impl Fn(CGPoint) -> Option<CGFloat>,
) {
    let inverse_transform = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .transform
        .invert();
    let stops = env.objc.borrow::<CGGradientHostObject>(gradient).clone();
    // Gradients fill the whole clip area.
    draw_area(env, context, None, |center| {
        let t = t_at(inverse_transform.apply_to_point(center))?;
        if (t < 0.0 && options & kCGGradientDrawsBeforeStartLocation == 0)
            || (t > 1.0 && options & kCGGradientDrawsAfterEndLocation == 0)
        {
            return None;
        }
        Some((1.0, stops.color_at(t.clamp(0.0, 1.0))))
    });
}

/// Implementation of `CGContextFillPath` and similar for `CGBitmapContext`.
pub(super) fn fill_path(env: &mut Environment, context: CGContextRef, path: &Path, even_odd: bool) {
    let color = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .rgb_fill_color;
    // TODO: anti-aliasing
    draw_area(env, context, path.bounds(), |center| {
        path.contains(center, even_odd).then_some((1.0, color))
    });
}

/// Implementation of `CGContextStrokePath` and similar for `CGBitmapContext`.
/// Joins and caps are round.
pub(super) fn stroke_path(env: &mut Environment, context: CGContextRef, path: &Path) {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    let color = host_obj.rgb_stroke_color;
    let t = host_obj.transform;
    let half_width = host_obj.line_width * (t.a * t.d - t.b * t.c).abs().sqrt() / 2.0;
    let Some(bounds) = path.bounds() else { return };
    let margin = half_width + 1.0;
    let area = CGRect {
        origin: CGPoint {
            x: bounds.origin.x - margin,
            y: bounds.origin.y - margin,
        },
        size: CGSize {
            width: bounds.size.width + margin * 2.0,
            height: bounds.size.height + margin * 2.0,
        },
    };
    let segments: Vec<_> = path.stroke_segments().collect();
    draw_area(env, context, Some(area), |center| {
        let distance = segments
            .iter()
            .map(|&(a, b)| cg_path::distance_to_segment(center, a, b))
            .fold(f32::INFINITY, f32::min);
        // Coverage falls off over one pixel at the edge of the line.
        let coverage = (half_width + 0.5 - distance).clamp(0.0, 1.0);
        (coverage > 0.0).then_some((coverage, color))
    });
}

fn intersect_device_rect(a: CGRect, b: CGRect) -> CGRect {
    let x0 = a.origin.x.max(b.origin.x);
    let y0 = a.origin.y.max(b.origin.y);
    let x1 = (a.origin.x + a.size.width).min(b.origin.x + b.size.width);
    let y1 = (a.origin.y + a.size.height).min(b.origin.y + b.size.height);
    CGRect {
        origin: CGPoint { x: x0, y: y0 },
        size: CGSize {
            width: (x1 - x0).max(0.0),
            height: (y1 - y0).max(0.0),
        },
    }
}

/// Implementation of `CGContextDrawImage` for `CGBitmapContext`.
pub(super) fn draw_image(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    image: CGImageRef,
) {
    let image = cg_image::borrow_image(&env.objc, image);

    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);

    //let _ = std::fs::write(
    //  format!(
    //      "image-{:?}-{:?}.data",
    //      (image as *const _ as *const ()),
    //      image.dimensions()
    //  ),
    //  image.pixels()
    //);

    //let _ = std::fs::write(
    //  format!(
    //      "bitmap-{:?}-{:?}-before.data",
    //      (image as *const _ as *const ()),
    //      (drawer.width(), drawer.height())
    //  ),
    //  &drawer.pixels
    //);

    let (image_width, image_height) = image.dimensions();

    // TODO: non-nearest-neighbour filtering? (what does CG actually do?)

    for ((x, y), (texel_x, texel_y)) in drawer.iter_transformed_pixels(rect) {
        let texel_x = (image_width as f32 * texel_x) as i32;
        // Image is in top-to-bottom order, but the bitmap is bottom-to-top
        let texel_y = (image_height as f32 * (1.0 - texel_y)) as i32;
        // FIXME: might need alpha format conversion here
        if let Some(color) = image.get_pixel((texel_x, texel_y)) {
            drawer.put_pixel((x, y), color, /* blend: */ true)
        }
    }

    //let _ = std::fs::write(
    //  format!(
    //      "bitmap-{:?}-{:?}-after.data",
    //      (image as *const _ as *const ()),
    //      (drawer.width(), drawer.height())
    //  ),
    //  &drawer.pixels
    //);
}

/// Used by Core Animation for layer backing stores. See
/// [CGBitmapContextData::y_down].
pub fn set_y_down(objc: &mut ObjC, context: CGContextRef) {
    let host_obj = objc.borrow_mut::<CGContextHostObject>(context);
    let CGContextSubclass::CGBitmapContext(ref mut bitmap_data) = host_obj.subclass;
    bitmap_data.y_down = true;
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGBitmapContextCreate(_, _, _, _, _, _, _)),
    export_c_func!(CGBitmapContextCreateImage(_)),
    export_c_func!(CGBitmapContextGetData(_)),
    export_c_func!(CGBitmapContextGetWidth(_)),
    export_c_func!(CGBitmapContextGetHeight(_)),
    export_c_func!(CGBitmapContextGetBytesPerRow(_)),
];
