/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGContext.h`

use super::cg_affine_transform::{CGAffineTransform, CGAffineTransformIdentity};
use super::cg_bitmap_context::{
    CGBitmapContextDrawer, CGBitmapContextGetHeight, CGBitmapContextGetWidth,
};
use super::cg_color::CGColorRef;
use super::cg_color_space::{
    kCGColorSpaceModelMonochrome, kCGColorSpaceModelRGB, CGColorSpaceGetModel, CGColorSpaceRef,
};
use super::cg_font::{
    self as cg_font, CGFontHostObject, CGFontRef, CGFontRelease, CGFontRetain, CGGlyph,
};
use super::cg_geometry::CGPointZero;
use super::cg_gradient::{CGGradientDrawingOptions, CGGradientRef};
use super::cg_image::CGImageRef;
use super::cg_path::{self, Path};
use super::{cg_bitmap_context, cg_color, CGFloat, CGPoint, CGRect, CGSize};
use crate::dyld::{export_c_func, FunctionExports};
use crate::font::Font;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::uikit;
use crate::mem::{ConstPtr, GuestUSize};
use crate::objc::{objc_classes, ClassExports, HostObject};
use crate::Environment;

type CGInterpolationQuality = i32;

pub(super) type CGTextDrawingMode = i32;
pub(super) const kCGTextFill: CGTextDrawingMode = 0;
const kCGTextFillStroke: CGTextDrawingMode = 2;
const kCGTextInvisible: CGTextDrawingMode = 3;

pub type CGBlendMode = i32;
pub const kCGBlendModeNormal: CGBlendMode = 0;
pub const kCGBlendModeMultiply: CGBlendMode = 1;
pub const kCGBlendModeScreen: CGBlendMode = 2;
#[allow(unused)]
pub const kCGBlendModeOverlay: CGBlendMode = 3;
pub const kCGBlendModeDarken: CGBlendMode = 4;
pub const kCGBlendModeLighten: CGBlendMode = 5;
pub const kCGBlendModeCopy: CGBlendMode = 17;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGContext seems to be a CFType-based type, but in our implementation those
// are just Objective-C types, so we need a class for it, but its name is not
// visible anywhere.
@implementation _Podroid_CGContext: NSObject

- (())dealloc {
    let host_obj = env.objc.borrow::<CGContextHostObject>(this);
    let CGContextSubclass::CGBitmapContext(bitmap_data) = host_obj.subclass;
    if bitmap_data.data_is_owned {
        env.mem.free(bitmap_data.data);
    }
    CGFontRelease(env, host_obj.font);

    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

// TODO: keep more states saved once they are implemented
type ContextState = (
    (CGFloat, CGFloat, CGFloat, CGFloat), // RGB fill color
    CGAffineTransform,                    // transform
    CGFontRef,                            // font
    CGFloat,                              // font size
    CGBlendMode,                          // blend mode
    Vec<(Path, bool)>,                    // clip paths and even-odd rule
    (CGFloat, CGFloat, CGFloat, CGFloat), // RGB stroke color
    CGFloat,                              // line width
    CGFloat,                              // global alpha
    i32,                                  // fill color space model
    i32,                                  // stroke color space model
);

pub(super) struct CGContextHostObject {
    pub(super) subclass: CGContextSubclass,
    pub(super) rgb_fill_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub(super) font: CGFontRef,
    pub(super) font_size: CGFloat,
    /// Current transform.
    pub(super) transform: CGAffineTransform,
    pub(super) blend_mode: CGBlendMode,
    /// Text transform.
    pub(super) text_transform: Option<CGAffineTransform>,
    pub(super) text_drawing_mode: CGTextDrawingMode,
    pub(super) state_stack: Vec<ContextState>,
    /// The current path, in device space.
    pub(super) path: Path,
    /// Paths that clip drawing, in device space: only what's inside all of
    /// them is drawn.
    pub(super) clip: Vec<(Path, bool)>,
    pub(super) rgb_stroke_color: (CGFloat, CGFloat, CGFloat, CGFloat),
    pub(super) line_width: CGFloat,
    pub(super) alpha: CGFloat,
    pub(super) fill_color_model: i32,
    pub(super) stroke_color_model: i32,
}
impl HostObject for CGContextHostObject {}

pub(super) enum CGContextSubclass {
    CGBitmapContext(cg_bitmap_context::CGBitmapContextData),
}

pub type CGContextRef = CFTypeRef;

pub fn CGContextRelease(env: &mut Environment, c: CGContextRef) {
    if !c.is_null() {
        CFRelease(env, c);
    }
}
pub fn CGContextRetain(env: &mut Environment, c: CGContextRef) -> CGContextRef {
    if !c.is_null() {
        CFRetain(env, c)
    } else {
        c
    }
}

pub(crate) fn CGContextSetBlendMode(
    env: &mut Environment,
    context: CGContextRef,
    blend_mode: CGBlendMode,
) {
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .blend_mode = blend_mode;
}

pub(crate) fn CGContextSetAlpha(env: &mut Environment, context: CGContextRef, alpha: CGFloat) {
    env.objc.borrow_mut::<CGContextHostObject>(context).alpha = alpha.clamp(0.0, 1.0);
}

fn CGContextSetFillColorSpace(
    env: &mut Environment,
    context: CGContextRef,
    space: CGColorSpaceRef,
) {
    let color_model = CGColorSpaceGetModel(env, space);
    assert!(color_model == kCGColorSpaceModelMonochrome || color_model == kCGColorSpaceModelRGB);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .fill_color_model = color_model;
}

fn CGContextSetStrokeColorSpace(
    env: &mut Environment,
    context: CGContextRef,
    space: CGColorSpaceRef,
) {
    let model = CGColorSpaceGetModel(env, space);
    assert!(model == kCGColorSpaceModelMonochrome || model == kCGColorSpaceModelRGB);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .stroke_color_model = model;
}

fn components_to_rgba(
    env: &mut Environment,
    components: ConstPtr<CGFloat>,
    model: i32,
) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    let first = env.mem.read(components);
    if model == kCGColorSpaceModelMonochrome {
        (first, first, first, env.mem.read(components + 1))
    } else {
        (
            first,
            env.mem.read(components + 1),
            env.mem.read(components + 2),
            env.mem.read(components + 3),
        )
    }
}

fn CGContextSetFillColor(
    env: &mut Environment,
    context: CGContextRef,
    components: ConstPtr<CGFloat>,
) {
    let model = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .fill_color_model;
    let color = components_to_rgba(env, components, model);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_fill_color = color;
}

fn CGContextSetStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    components: ConstPtr<CGFloat>,
) {
    let model = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .stroke_color_model;
    let color = components_to_rgba(env, components, model);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .rgb_stroke_color = color;
}

fn CGContextSetFillColorWithColor(env: &mut Environment, context: CGContextRef, color: CGColorRef) {
    let (r, g, b, a) = cg_color::to_rgba(&env.objc, color);
    CGContextSetRGBFillColor(env, context, r, g, b, a)
}

fn CGContextSetStrokeColorWithColor(
    env: &mut Environment,
    context: CGContextRef,
    color: CGColorRef,
) {
    let (r, g, b, a) = cg_color::to_rgba(&env.objc, color);
    CGContextSetRGBStrokeColor(env, context, r, g, b, a);
}

pub fn CGContextSetRGBFillColor(
    env: &mut Environment,
    context: CGContextRef,
    red: CGFloat,
    green: CGFloat,
    blue: CGFloat,
    alpha: CGFloat,
) {
    let color = (red, green, blue, alpha);
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.rgb_fill_color = color;
    host.fill_color_model = kCGColorSpaceModelRGB;
}

fn CGContextSetGrayFillColor(
    env: &mut Environment,
    context: CGContextRef,
    gray: CGFloat,
    alpha: CGFloat,
) {
    let color = (gray, gray, gray, alpha);
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.rgb_fill_color = color;
    host.fill_color_model = kCGColorSpaceModelMonochrome;
}

fn CGContextSetGrayStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    gray: CGFloat,
    alpha: CGFloat,
) {
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.rgb_stroke_color = (gray, gray, gray, alpha);
    host.stroke_color_model = kCGColorSpaceModelMonochrome;
}
fn CGContextSetRGBStrokeColor(
    env: &mut Environment,
    context: CGContextRef,
    r: CGFloat,
    g: CGFloat,
    b: CGFloat,
    a: CGFloat,
) {
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    host.rgb_stroke_color = (r, g, b, a);
    host.stroke_color_model = kCGColorSpaceModelRGB;
}

fn CGContextSetShadowWithColor(
    _env: &mut Environment,
    context: CGContextRef,
    offset: CGSize,
    blur: CGFloat,
    color: CGColorRef,
) {
    log!(
        "TODO: CGContextSetShadowWithColor({:?}, {}, {}, {:?})",
        context,
        offset,
        blur,
        color
    );
}

pub fn CGContextFillRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    cg_bitmap_context::fill_rect(env, context, rect, /* clear: */ false);
}

fn CGContextFillEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    cg_bitmap_context::fill_ellipse(env, context, rect);
}

pub fn CGContextClearRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    cg_bitmap_context::fill_rect(env, context, rect, /* clear: */ true);
}

fn CGContextClipToRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    if rect.origin == CGPointZero
        && rect.size.height == CGBitmapContextGetHeight(env, context) as f32
        && rect.size.width == CGBitmapContextGetWidth(env, context) as f32
    {
        assert!(env
            .objc
            .borrow_mut::<CGContextHostObject>(context)
            .transform
            .is_identity());
        // All good, clipping is not needed!
        return;
    }
    todo!();
}

pub fn CGContextConcatCTM(
    env: &mut Environment,
    context: CGContextRef,
    transform: CGAffineTransform,
) {
    log_dbg!("CGContextConcatCTM({:?})", transform);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = transform.concat(host_obj.transform);
}
pub fn CGContextGetCTM(env: &mut Environment, context: CGContextRef) -> CGAffineTransform {
    let res = env.objc.borrow::<CGContextHostObject>(context).transform;
    log_dbg!("CGContextGetCTM() => {:?}", res);
    res
}
pub fn CGContextRotateCTM(env: &mut Environment, context: CGContextRef, angle: CGFloat) {
    log_dbg!("CGContextRotateCTM({:?})", angle);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.rotate(angle);
}
pub fn CGContextScaleCTM(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    log_dbg!("CGContextScaleCTM({:?})", (x, y));
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.scale(x, y);
}
pub fn CGContextTranslateCTM(
    env: &mut Environment,
    context: CGContextRef,
    tx: CGFloat,
    ty: CGFloat,
) {
    log_dbg!("CGContextTranslateCTM({:?})", (tx, ty));
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.transform = host_obj.transform.translate(tx, ty);
}

pub fn CGContextDrawImage(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    image: CGImageRef,
) {
    cg_bitmap_context::draw_image(env, context, rect, image);
}

// HyperHLE tiles the image from rect.origin across the current clip.
fn CGContextDrawTiledImage(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    image: CGImageRef,
) {
    let (width, height) = (rect.size.width, rect.size.height);
    if context.is_null()
        || image.is_null()
        || !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || !rect.origin.x.is_finite()
        || !rect.origin.y.is_finite()
    {
        return;
    }
    let clip = CGContextGetClipBoundingBox(env, context);
    let start_x = rect.origin.x + ((clip.origin.x - rect.origin.x) / width).floor() * width;
    let start_y = rect.origin.y + ((clip.origin.y - rect.origin.y) / height).floor() * height;
    let columns = ((clip.origin.x + clip.size.width - start_x) / width).ceil();
    let rows = ((clip.origin.y + clip.size.height - start_y) / height).ceil();
    if !columns.is_finite() || !rows.is_finite() || columns <= 0.0 || rows <= 0.0 {
        return;
    }
    let (columns, rows) = (columns as u64, rows as u64);
    const MAX_TILES: u64 = 1_000_000;
    for index in 0..columns.saturating_mul(rows).min(MAX_TILES) {
        let tile = CGRect {
            origin: CGPoint {
                x: start_x + (index % columns) as f32 * width,
                y: start_y + (index / columns) as f32 * height,
            },
            size: CGSize { width, height },
        };
        cg_bitmap_context::draw_image(env, context, tile, image);
    }
}

pub(crate) fn CGContextSaveGState(env: &mut Environment, context: CGContextRef) {
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.state_stack.push((
        host_obj.rgb_fill_color,
        host_obj.transform,
        host_obj.font,
        host_obj.font_size,
        host_obj.blend_mode,
        host_obj.clip.clone(),
        host_obj.rgb_stroke_color,
        host_obj.line_width,
        host_obj.alpha,
        host_obj.fill_color_model,
        host_obj.stroke_color_model,
    ));
    CGFontRetain(env, env.objc.borrow::<CGContextHostObject>(context).font);
}

pub(crate) fn CGContextRestoreGState(env: &mut Environment, context: CGContextRef) {
    // We need to release _old_ font, there are 2 cases:
    // - font hasn't been set between save/restore -> this release corresponds
    // the font retain from save
    // - font has been set between save/restore -> we need to release old font
    // retained on the set
    CGFontRelease(env, env.objc.borrow::<CGContextHostObject>(context).font);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    let state = host_obj.state_stack.pop().unwrap();
    host_obj.rgb_fill_color = state.0;
    host_obj.transform = state.1;
    host_obj.font = state.2;
    host_obj.font_size = state.3;
    host_obj.blend_mode = state.4;
    host_obj.clip = state.5;
    host_obj.rgb_stroke_color = state.6;
    host_obj.line_width = state.7;
    host_obj.alpha = state.8;
    host_obj.fill_color_model = state.9;
    host_obj.stroke_color_model = state.10;
}

fn CGContextSetInterpolationQuality(
    _env: &mut Environment,
    context: CGContextRef,
    quality: CGInterpolationQuality,
) {
    log!(
        "TODO: CGContextSetInterpolationQuality({:?}, {:?})",
        context,
        quality
    );
}
fn CGContextSetAllowsAntialiasing(_env: &mut Environment, context: CGContextRef, allow: bool) {
    log!(
        "TODO: CGContextSetAllowsAntialiasing({:?}, {})",
        context,
        allow
    );
}

fn CGContextSetShouldSmoothFonts(_env: &mut Environment, context: CGContextRef, should: bool) {
    log!(
        "TODO: CGContextSetShouldSmoothFonts({:?}, {})",
        context,
        should
    );
}

fn CGContextSetFont(env: &mut Environment, context: CGContextRef, font: CGFontRef) {
    CGFontRetain(env, font);
    let old_font = env.objc.borrow_mut::<CGContextHostObject>(context).font;
    CGFontRelease(env, old_font);
    env.objc.borrow_mut::<CGContextHostObject>(context).font = font;
}

fn CGContextSetFontSize(env: &mut Environment, context: CGContextRef, size: CGFloat) {
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .font_size = size;
}

fn CGContextSetTextDrawingMode(
    env: &mut Environment,
    context: CGContextRef,
    mode: CGTextDrawingMode,
) {
    // TODO: support other modes
    assert!(mode == kCGTextFill || mode == kCGTextFillStroke || mode == kCGTextInvisible);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .text_drawing_mode = mode;
}

type CGTextEncoding = i32;

/// Deprecated in iOS 7, but used by some older apps.
fn CGContextSelectFont(
    env: &mut Environment,
    context: CGContextRef,
    name: ConstPtr<u8>,
    size: CGFloat,
    _text_encoding: CGTextEncoding,
) {
    // There's no font lookup by name here yet, so pick the closest bundled
    // font based on the name.
    let name = env
        .mem
        .cstr_at_utf8(name)
        .unwrap_or("")
        .to_ascii_lowercase();
    let bold = name.contains("bold");
    let family: u8 = if name.contains("courier") || name.contains("mono") {
        0
    } else if name.contains("times") || name.contains("georgia") {
        1
    } else {
        2
    };
    // Loading a font is slow (it's read from disk and parsed), and apps may
    // select a font every time they draw text, so cache them.
    let font = if let Some(&font) = env
        .framework_state
        .core_graphics
        .selected_fonts
        .get(&(family, bold))
    {
        font
    } else {
        let font = match (family, bold) {
            (0, true) => Font::mono_bold(),
            (0, false) => Font::mono_regular(),
            (1, true) => Font::serif_bold(),
            (1, false) => Font::serif_regular(),
            (_, true) => Font::sans_bold(),
            (_, false) => Font::sans_regular(),
        };
        // The cache keeps this reference forever.
        let font = cg_font::from_font(env, font);
        env.framework_state
            .core_graphics
            .selected_fonts
            .insert((family, bold), font);
        font
    };
    CGContextSetFont(env, context, font);
    CGContextSetFontSize(env, context, size);
}

/// Deprecated in iOS 7, but used by some older apps.
fn CGContextSetCharacterSpacing(_env: &mut Environment, _context: CGContextRef, spacing: CGFloat) {
    if spacing != 0.0 {
        log!("TODO: CGContextSetCharacterSpacing({}) (ignored)", spacing);
    }
}

/// Deprecated in iOS 7, but used by some older apps.
fn CGContextShowTextAtPoint(
    env: &mut Environment,
    context: CGContextRef,
    x: CGFloat,
    y: CGFloat,
    string: ConstPtr<u8>,
    length: GuestUSize,
) {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    if host_obj.text_drawing_mode == kCGTextInvisible || host_obj.font.is_null() {
        return;
    }
    // The text encoding is usually kCGEncodingMacRoman, which is the same as
    // Latin-1 for ASCII. Other bytes are mapped as Latin-1 too.
    // TODO: proper MacRoman support
    let bytes = env.mem.bytes_at(string, length).to_vec();
    let font = &env.objc.borrow::<CGFontHostObject>(host_obj.font).font;
    let glyph_ids = bytes
        .iter()
        .map(|&b| font.glyph_id_for_char(b as u16))
        .collect();
    show_glyphs_at_point(env, context, x, y, glyph_ids);
}

fn CGContextSetTextMatrix(
    env: &mut Environment,
    context: CGContextRef,
    transform: CGAffineTransform,
) {
    log_dbg!("CGContextSetTextMatrix({:?})", transform);
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .text_transform = Some(transform);
}

fn CGContextGetTextMatrix(env: &mut Environment, context: CGContextRef) -> CGAffineTransform {
    env.objc
        .borrow::<CGContextHostObject>(context)
        .text_transform
        .unwrap_or(CGAffineTransformIdentity)
}

fn CGContextSetTextPosition(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    let host = env.objc.borrow_mut::<CGContextHostObject>(context);
    let matrix = host.text_transform.get_or_insert(CGAffineTransformIdentity);
    matrix.tx = x;
    matrix.ty = y;
}

fn CGContextGetTextPosition(env: &mut Environment, context: CGContextRef) -> CGPoint {
    let matrix = CGContextGetTextMatrix(env, context);
    CGPoint {
        x: matrix.tx,
        y: matrix.ty,
    }
}

fn CGContextShowGlyphsAtPoint(
    env: &mut Environment,
    context: CGContextRef,
    x: CGFloat,
    y: CGFloat,
    glyphs: ConstPtr<CGGlyph>,
    count: GuestUSize,
) {
    let mut glyph_ids = Vec::new();
    for i in 0..count {
        let glyph_id = env.mem.read(glyphs + i);
        glyph_ids.push(rusttype::GlyphId(glyph_id));
    }
    show_glyphs_at_point(env, context, x, y, glyph_ids);
}

fn show_glyphs_at_point(
    env: &mut Environment,
    context: CGContextRef,
    x: CGFloat,
    y: CGFloat,
    glyph_ids: Vec<rusttype::GlyphId>,
) {
    let font = env.objc.borrow::<CGContextHostObject>(context).font;
    let font_size = env.objc.borrow::<CGContextHostObject>(context).font_size;
    let text_transform = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .text_transform
        .unwrap_or(CGAffineTransformIdentity);

    let font = &env.objc.borrow::<CGFontHostObject>(font).font;

    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();

    font.draw_glyphs(
        font_size,
        glyph_ids,
        (x, y),
        text_transform,
        |raster_glyph| {
            uikit::ui_font::draw_font_glyph(
                &mut drawer,
                raster_glyph,
                fill_color,
                /* clip_x: */ None,
                /* clip_y: */ None,
            )
        },
    );
}

/// Draw glyphs at positions with a font and size other than the context's
/// (used by Core Text's `CTFontDrawGlyphs`).
pub(crate) fn show_glyphs_with_font(
    env: &mut Environment,
    context: CGContextRef,
    font: CGFontRef,
    font_size: CGFloat,
    glyphs: ConstPtr<CGGlyph>,
    positions: ConstPtr<CGPoint>,
    count: GuestUSize,
) {
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    let old_font = std::mem::replace(&mut host_obj.font, font);
    let old_font_size = std::mem::replace(&mut host_obj.font_size, font_size);
    CGContextShowGlyphsAtPositions(env, context, glyphs, positions, count);
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    host_obj.font = old_font;
    host_obj.font_size = old_font_size;
}

fn CGContextShowGlyphsAtPositions(
    env: &mut Environment,
    context: CGContextRef,
    glyphs: ConstPtr<CGGlyph>,
    positions: ConstPtr<CGPoint>,
    count: GuestUSize,
) {
    let text_transform = env
        .objc
        .borrow::<CGContextHostObject>(context)
        .text_transform
        .unwrap_or(CGAffineTransformIdentity);
    for i in 0..count {
        let glyph_ptr = glyphs + i;
        let point = env.mem.read(positions + i);
        let transformed_point = text_transform.apply_to_point(point);
        // The origin already includes translation. Apply only the linear
        // part of the matrix to each glyph's shape.
        let mut glyph_transform = text_transform;
        glyph_transform.tx = 0.0;
        glyph_transform.ty = 0.0;
        env.objc
            .borrow_mut::<CGContextHostObject>(context)
            .text_transform = Some(glyph_transform);
        CGContextShowGlyphsAtPoint(
            env,
            context,
            transformed_point.x,
            transformed_point.y,
            glyph_ptr,
            1,
        );
        env.objc
            .borrow_mut::<CGContextHostObject>(context)
            .text_transform = Some(text_transform);
    }
}

type CGPathDrawingMode = i32;

const kCGPathFill: CGPathDrawingMode = 0;
const kCGPathEOFill: CGPathDrawingMode = 1;
const kCGPathStroke: CGPathDrawingMode = 2;
const kCGPathFillStroke: CGPathDrawingMode = 3;
const kCGPathEOFillStroke: CGPathDrawingMode = 4;

fn current_point(env: &mut Environment, context: CGContextRef) -> Option<CGPoint> {
    env.objc
        .borrow::<CGContextHostObject>(context)
        .path
        .current_point()
}

fn device_point(env: &mut Environment, context: CGContextRef, point: CGPoint) -> CGPoint {
    let host_obj = env.objc.borrow::<CGContextHostObject>(context);
    host_obj.transform.apply_to_point(point)
}

fn CGContextBeginPath(env: &mut Environment, context: CGContextRef) {
    env.objc.borrow_mut::<CGContextHostObject>(context).path = Path::default();
}

fn CGContextMoveToPoint(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    let point = device_point(env, context, CGPoint { x, y });
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .path
        .move_to(point);
}

fn CGContextAddLineToPoint(env: &mut Environment, context: CGContextRef, x: CGFloat, y: CGFloat) {
    let point = device_point(env, context, CGPoint { x, y });
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .path
        .line_to(point);
}

fn CGContextAddQuadCurveToPoint(
    env: &mut Environment,
    context: CGContextRef,
    cx: CGFloat,
    cy: CGFloat,
    x: CGFloat,
    y: CGFloat,
) {
    let Some(start) = current_point(env, context) else {
        CGContextMoveToPoint(env, context, x, y);
        return;
    };
    let control = device_point(env, context, CGPoint { x: cx, y: cy });
    let end = device_point(env, context, CGPoint { x, y });
    let path = &mut env.objc.borrow_mut::<CGContextHostObject>(context).path;
    for i in 1..=32 {
        let t = i as CGFloat / 32.0;
        let s = 1.0 - t;
        path.line_to(CGPoint {
            x: s * s * start.x + 2.0 * s * t * control.x + t * t * end.x,
            y: s * s * start.y + 2.0 * s * t * control.y + t * t * end.y,
        });
    }
}

fn CGContextAddCurveToPoint(
    env: &mut Environment,
    context: CGContextRef,
    c1x: CGFloat,
    c1y: CGFloat,
    c2x: CGFloat,
    c2y: CGFloat,
    x: CGFloat,
    y: CGFloat,
) {
    let Some(start) = current_point(env, context) else {
        CGContextMoveToPoint(env, context, x, y);
        return;
    };
    let c1 = device_point(env, context, CGPoint { x: c1x, y: c1y });
    let c2 = device_point(env, context, CGPoint { x: c2x, y: c2y });
    let end = device_point(env, context, CGPoint { x, y });
    let path = &mut env.objc.borrow_mut::<CGContextHostObject>(context).path;
    for i in 1..=32 {
        let t = i as CGFloat / 32.0;
        let s = 1.0 - t;
        path.line_to(CGPoint {
            x: s * s * s * start.x
                + 3.0 * s * s * t * c1.x
                + 3.0 * s * t * t * c2.x
                + t * t * t * end.x,
            y: s * s * s * start.y
                + 3.0 * s * s * t * c1.y
                + 3.0 * s * t * t * c2.y
                + t * t * t * end.y,
        });
    }
}

fn CGContextClosePath(env: &mut Environment, context: CGContextRef) {
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .path
        .close();
}

fn CGContextAddArcToPoint(
    env: &mut Environment,
    context: CGContextRef,
    x1: CGFloat,
    y1: CGFloat,
    x2: CGFloat,
    y2: CGFloat,
    radius: CGFloat,
) {
    let p1 = device_point(env, context, CGPoint { x: x1, y: y1 });
    let p2 = device_point(env, context, CGPoint { x: x2, y: y2 });
    let Some(p0) = current_point(env, context) else {
        CGContextMoveToPoint(env, context, x1, y1);
        return;
    };
    // The radius is scaled like the points. This assumes the transform
    // scales both axes equally.
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    let t = host_obj.transform;
    let scale = (t.a * t.d - t.b * t.c).abs().sqrt();
    for point in cg_path::arc_to_point(p0, p1, p2, radius * scale) {
        host_obj.path.line_to(point);
    }
}

fn CGContextAddArc(
    env: &mut Environment,
    context: CGContextRef,
    x: CGFloat,
    y: CGFloat,
    radius: CGFloat,
    start: CGFloat,
    end: CGFloat,
    clockwise: i32,
) {
    if radius < 0.0 || ![x, y, radius, start, end].iter().all(|v| v.is_finite()) {
        return;
    }
    let tau = std::f32::consts::TAU;
    let raw = end - start;
    let sweep = if raw.abs() >= tau {
        if clockwise != 0 {
            -tau
        } else {
            tau
        }
    } else if clockwise != 0 {
        -(-raw).rem_euclid(tau)
    } else {
        raw.rem_euclid(tau)
    };
    let start_x = x + radius * start.cos();
    let start_y = y + radius * start.sin();
    if current_point(env, context).is_none() {
        CGContextMoveToPoint(env, context, start_x, start_y);
    } else {
        CGContextAddLineToPoint(env, context, start_x, start_y);
    }
    let steps = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as u32;
    for i in 0..steps {
        let a = start + sweep * i as CGFloat / steps as CGFloat;
        let b = start + sweep * (i + 1) as CGFloat / steps as CGFloat;
        let k = 4.0 / 3.0 * ((b - a) / 4.0).tan();
        CGContextAddCurveToPoint(
            env,
            context,
            x + radius * (a.cos() - k * a.sin()),
            y + radius * (a.sin() + k * a.cos()),
            x + radius * (b.cos() + k * b.sin()),
            y + radius * (b.sin() - k * b.cos()),
            x + radius * b.cos(),
            y + radius * b.sin(),
        );
    }
}

fn CGContextFlush(_env: &mut Environment, _context: CGContextRef) {
    // Bitmap drawing is synchronous.
}

fn CGContextSynchronize(env: &mut Environment, context: CGContextRef) {
    CGContextFlush(env, context);
}

fn CGContextAddRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    let (x0, y0) = (rect.origin.x, rect.origin.y);
    let (x1, y1) = (x0 + rect.size.width, y0 + rect.size.height);
    CGContextMoveToPoint(env, context, x0, y0);
    CGContextAddLineToPoint(env, context, x1, y0);
    CGContextAddLineToPoint(env, context, x1, y1);
    CGContextAddLineToPoint(env, context, x0, y1);
    CGContextClosePath(env, context);
}

fn CGContextAddRects(
    env: &mut Environment,
    context: CGContextRef,
    rects: ConstPtr<CGRect>,
    count: GuestUSize,
) {
    for i in 0..count {
        let rect = env.mem.read(rects + i);
        CGContextAddRect(env, context, rect);
    }
}

fn CGContextAddLines(
    env: &mut Environment,
    context: CGContextRef,
    points: ConstPtr<CGPoint>,
    count: GuestUSize,
) {
    if count == 0 {
        return;
    }
    let first = env.mem.read(points);
    CGContextMoveToPoint(env, context, first.x, first.y);
    for i in 1..count {
        let point = env.mem.read(points + i);
        CGContextAddLineToPoint(env, context, point.x, point.y);
    }
}

fn CGContextAddEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    let center_x = rect.origin.x + rect.size.width / 2.0;
    let center_y = rect.origin.y + rect.size.height / 2.0;
    for i in 0..64 {
        let angle = std::f32::consts::TAU * i as f32 / 64.0;
        let x = center_x + angle.cos() * rect.size.width / 2.0;
        let y = center_y + angle.sin() * rect.size.height / 2.0;
        if i == 0 {
            CGContextMoveToPoint(env, context, x, y);
        } else {
            CGContextAddLineToPoint(env, context, x, y);
        }
    }
    CGContextClosePath(env, context);
}

pub(crate) fn CGContextAddPath(env: &mut Environment, context: CGContextRef, path: CFTypeRef) {
    if path.is_null() {
        return;
    }
    let mut path = cg_path::copy_path(env, path);
    for subpath in &mut path.subpaths {
        for point in &mut subpath.points {
            *point = device_point(env, context, *point);
        }
    }
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .path
        .subpaths
        .extend(path.subpaths);
}

pub(crate) fn CGContextClip(env: &mut Environment, context: CGContextRef) {
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    // An empty path clips everything away, which an empty clip path does.
    let path = std::mem::take(&mut host_obj.path);
    host_obj.clip.push((path, false));
}

pub(crate) fn CGContextEOClip(env: &mut Environment, context: CGContextRef) {
    let host_obj = env.objc.borrow_mut::<CGContextHostObject>(context);
    let path = std::mem::take(&mut host_obj.path);
    host_obj.clip.push((path, true));
}

fn CGContextResetClip(env: &mut Environment, context: CGContextRef) {
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .clip
        .clear();
}

fn CGContextGetClipBoundingBox(env: &mut Environment, context: CGContextRef) -> CGRect {
    let (mut x0, mut y0) = (0.0f32, 0.0f32);
    let (mut x1, mut y1) = (
        CGBitmapContextGetWidth(env, context) as f32,
        CGBitmapContextGetHeight(env, context) as f32,
    );
    let host = env.objc.borrow::<CGContextHostObject>(context);
    for (path, _) in &host.clip {
        let Some(bounds) = path.bounds() else {
            return CGRect::default();
        };
        x0 = x0.max(bounds.origin.x);
        y0 = y0.max(bounds.origin.y);
        x1 = x1.min(bounds.origin.x + bounds.size.width);
        y1 = y1.min(bounds.origin.y + bounds.size.height);
    }
    if x1 <= x0 || y1 <= y0 {
        return CGRect::default();
    }
    host.transform.invert().apply_to_rect(CGRect {
        origin: CGPoint { x: x0, y: y0 },
        size: CGSize {
            width: x1 - x0,
            height: y1 - y0,
        },
    })
}

fn CGContextDrawPath(env: &mut Environment, context: CGContextRef, mode: CGPathDrawingMode) {
    let path = std::mem::take(&mut env.objc.borrow_mut::<CGContextHostObject>(context).path);
    let (fill, even_odd, stroke) = match mode {
        kCGPathFill => (true, false, false),
        kCGPathEOFill => (true, true, false),
        kCGPathStroke => (false, false, true),
        kCGPathFillStroke => (true, false, true),
        kCGPathEOFillStroke => (true, true, true),
        _ => unimplemented!("CGContextDrawPath mode {}", mode),
    };
    if fill {
        cg_bitmap_context::fill_path(env, context, &path, even_odd);
    }
    if stroke {
        cg_bitmap_context::stroke_path(env, context, &path);
    }
}

pub(crate) fn CGContextFillPath(env: &mut Environment, context: CGContextRef) {
    CGContextDrawPath(env, context, kCGPathFill);
}

pub(crate) fn CGContextEOFillPath(env: &mut Environment, context: CGContextRef) {
    CGContextDrawPath(env, context, kCGPathEOFill);
}

fn CGContextStrokeRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    let path = std::mem::take(&mut env.objc.borrow_mut::<CGContextHostObject>(context).path);
    CGContextAddRect(env, context, rect);
    CGContextStrokePath(env, context);
    env.objc.borrow_mut::<CGContextHostObject>(context).path = path;
}

fn CGContextStrokeRectWithWidth(
    env: &mut Environment,
    context: CGContextRef,
    rect: CGRect,
    width: CGFloat,
) {
    let original = env.objc.borrow::<CGContextHostObject>(context).line_width;
    CGContextSetLineWidth(env, context, width);
    CGContextStrokeRect(env, context, rect);
    CGContextSetLineWidth(env, context, original);
}

fn CGContextStrokeEllipseInRect(env: &mut Environment, context: CGContextRef, rect: CGRect) {
    let path = std::mem::take(&mut env.objc.borrow_mut::<CGContextHostObject>(context).path);
    CGContextAddEllipseInRect(env, context, rect);
    CGContextStrokePath(env, context);
    env.objc.borrow_mut::<CGContextHostObject>(context).path = path;
}

pub(crate) fn CGContextStrokePath(env: &mut Environment, context: CGContextRef) {
    CGContextDrawPath(env, context, kCGPathStroke);
}

pub(crate) fn CGContextSetLineWidth(env: &mut Environment, context: CGContextRef, width: CGFloat) {
    env.objc
        .borrow_mut::<CGContextHostObject>(context)
        .line_width = width;
}

fn CGContextSetShouldAntialias(_env: &mut Environment, _context: CGContextRef, _antialias: bool) {
    // Nothing is anti-aliased yet.
}

fn CGContextDrawLinearGradient(
    env: &mut Environment,
    context: CGContextRef,
    gradient: CGGradientRef,
    start: CGPoint,
    end: CGPoint,
    options: CGGradientDrawingOptions,
) {
    let (dx, dy) = (end.x - start.x, end.y - start.y);
    let length_squared = dx * dx + dy * dy;
    cg_bitmap_context::draw_gradient(env, context, gradient, options, |p| {
        if length_squared == 0.0 {
            return None;
        }
        Some(((p.x - start.x) * dx + (p.y - start.y) * dy) / length_squared)
    });
}

fn CGContextDrawRadialGradient(
    env: &mut Environment,
    context: CGContextRef,
    gradient: CGGradientRef,
    start_center: CGPoint,
    start_radius: CGFloat,
    end_center: CGPoint,
    end_radius: CGFloat,
    options: CGGradientDrawingOptions,
) {
    cg_bitmap_context::draw_gradient(env, context, gradient, options, |p| {
        radial_gradient_t(p, start_center, start_radius, end_center, end_radius)
    });
}

/// The largest `t` for which `p` is on the circle interpolated between the
/// start and end circles with a non-negative radius, if any.
fn radial_gradient_t(
    p: CGPoint,
    c0: CGPoint,
    r0: CGFloat,
    c1: CGPoint,
    r1: CGFloat,
) -> Option<CGFloat> {
    let (dcx, dcy, dr) = (c1.x - c0.x, c1.y - c0.y, r1 - r0);
    let (pdx, pdy) = (p.x - c0.x, p.y - c0.y);
    // |pd - t*dc|^2 = (r0 + t*dr)^2, as a*t^2 + b*t + c = 0.
    let a = dcx * dcx + dcy * dcy - dr * dr;
    let b = -2.0 * (pdx * dcx + pdy * dcy + r0 * dr);
    let c = pdx * pdx + pdy * pdy - r0 * r0;
    let valid = |t: CGFloat| r0 + t * dr >= 0.0;
    if a.abs() < 1e-6 {
        if b == 0.0 {
            return None;
        }
        let t = -c / b;
        return valid(t).then_some(t);
    }
    let discriminant = b * b - 4.0 * a * c;
    if discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let (t0, t1) = ((-b + root) / (2.0 * a), (-b - root) / (2.0 * a));
    let (larger, smaller) = (t0.max(t1), t0.min(t1));
    if valid(larger) {
        Some(larger)
    } else if valid(smaller) {
        Some(smaller)
    } else {
        None
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGContextRetain(_)),
    export_c_func!(CGContextRelease(_)),
    export_c_func!(CGContextSetBlendMode(_, _)),
    export_c_func!(CGContextSetAlpha(_, _)),
    export_c_func!(CGContextSetFillColorSpace(_, _)),
    export_c_func!(CGContextSetStrokeColorSpace(_, _)),
    export_c_func!(CGContextSetFillColor(_, _)),
    export_c_func!(CGContextSetStrokeColor(_, _)),
    export_c_func!(CGContextSetFillColorWithColor(_, _)),
    export_c_func!(CGContextSetStrokeColorWithColor(_, _)),
    export_c_func!(CGContextSetRGBFillColor(_, _, _, _, _)),
    export_c_func!(CGContextSetGrayFillColor(_, _, _)),
    export_c_func!(CGContextSetGrayStrokeColor(_, _, _)),
    export_c_func!(CGContextSetRGBStrokeColor(_, _, _, _, _)),
    export_c_func!(CGContextSetShadowWithColor(_, _, _, _)),
    export_c_func!(CGContextFillRect(_, _)),
    export_c_func!(CGContextClearRect(_, _)),
    export_c_func!(CGContextClipToRect(_, _)),
    export_c_func!(CGContextConcatCTM(_, _)),
    export_c_func!(CGContextGetCTM(_)),
    export_c_func!(CGContextRotateCTM(_, _)),
    export_c_func!(CGContextScaleCTM(_, _, _)),
    export_c_func!(CGContextTranslateCTM(_, _, _)),
    export_c_func!(CGContextDrawImage(_, _, _)),
    export_c_func!(CGContextDrawTiledImage(_, _, _)),
    export_c_func!(CGContextSaveGState(_)),
    export_c_func!(CGContextBeginPath(_)),
    export_c_func!(CGContextMoveToPoint(_, _, _)),
    export_c_func!(CGContextAddLineToPoint(_, _, _)),
    export_c_func!(CGContextAddQuadCurveToPoint(_, _, _, _, _)),
    export_c_func!(CGContextAddCurveToPoint(_, _, _, _, _, _, _)),
    export_c_func!(CGContextClosePath(_)),
    export_c_func!(CGContextAddRect(_, _)),
    export_c_func!(CGContextAddRects(_, _, _)),
    export_c_func!(CGContextAddLines(_, _, _)),
    export_c_func!(CGContextAddEllipseInRect(_, _)),
    export_c_func!(CGContextFillPath(_)),
    export_c_func!(CGContextEOFillPath(_)),
    export_c_func!(CGContextStrokePath(_)),
    export_c_func!(CGContextStrokeRect(_, _)),
    export_c_func!(CGContextStrokeRectWithWidth(_, _, _)),
    export_c_func!(CGContextStrokeEllipseInRect(_, _)),
    export_c_func!(CGContextAddArcToPoint(_, _, _, _, _, _)),
    export_c_func!(CGContextAddArc(_, _, _, _, _, _, _)),
    export_c_func!(CGContextFlush(_)),
    export_c_func!(CGContextSynchronize(_)),
    export_c_func!(CGContextAddPath(_, _)),
    export_c_func!(CGContextClip(_)),
    export_c_func!(CGContextEOClip(_)),
    export_c_func!(CGContextResetClip(_)),
    export_c_func!(CGContextGetClipBoundingBox(_)),
    export_c_func!(CGContextDrawPath(_, _)),
    export_c_func!(CGContextSetLineWidth(_, _)),
    export_c_func!(CGContextSetShouldAntialias(_, _)),
    export_c_func!(CGContextDrawLinearGradient(_, _, _, _, _)),
    export_c_func!(CGContextDrawRadialGradient(_, _, _, _, _, _, _)),
    export_c_func!(CGContextRestoreGState(_)),
    export_c_func!(CGContextSetInterpolationQuality(_, _)),
    export_c_func!(CGContextSetAllowsAntialiasing(_, _)),
    export_c_func!(CGContextSetShouldSmoothFonts(_, _)),
    export_c_func!(CGContextSetFont(_, _)),
    export_c_func!(CGContextSetFontSize(_, _)),
    export_c_func!(CGContextSetTextDrawingMode(_, _)),
    export_c_func!(CGContextSelectFont(_, _, _, _)),
    export_c_func!(CGContextSetCharacterSpacing(_, _)),
    export_c_func!(CGContextShowTextAtPoint(_, _, _, _, _)),
    export_c_func!(CGContextFillEllipseInRect(_, _)),
    export_c_func!(CGContextSetTextMatrix(_, _)),
    export_c_func!(CGContextGetTextMatrix(_)),
    export_c_func!(CGContextSetTextPosition(_, _, _)),
    export_c_func!(CGContextGetTextPosition(_)),
    export_c_func!(CGContextShowGlyphsAtPoint(_, _, _, _, _)),
    export_c_func!(CGContextShowGlyphsAtPositions(_, _, _, _)),
];
