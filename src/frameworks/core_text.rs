/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Core Text framework.
//!
//! Only enough of `CTFont` to draw individual glyphs into a `CGContext`, which
//! is how some games render text into textures. Every font is substituted
//! with the bundled Noto Sans fonts: Japanese and Simplified Chinese (which
//! both cover Latin text). Glyphs missing from the first are taken from the
//! second; their glyph IDs are marked with [FALLBACK_GLYPH].

use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::font::Font;
use crate::frameworks::core_foundation::{CFIndex, CFRelease, CFTypeRef};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::cg_context::{show_glyphs_with_font, CGContextRef};
use crate::frameworks::core_graphics::cg_font::{from_font, CGFontHostObject, CGFontRef, CGGlyph};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_string, unichar};
use crate::mem::{ConstPtr, GuestUSize, MutPtr};
use crate::objc::{id, objc_classes, ClassExports, HostObject};
use crate::Environment;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreText.framework/CoreText",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

type CTFontRef = CFTypeRef;
type CTFontOrientation = u32;

struct CTFontHostObject {
    /// `CGFontRef`s, retained: the main font and the fallback font.
    cg_fonts: [CGFontRef; 2],
    size: CGFloat,
}

/// Bit set in glyph IDs from the fallback font. Both fonts have fewer than
/// 0x8000 glyphs.
const FALLBACK_GLYPH: CGGlyph = 0x8000;
impl HostObject for CTFontHostObject {}

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_CTFont: NSObject

- (())dealloc {
    let cg_fonts = env.objc.borrow::<CTFontHostObject>(this).cg_fonts;
    for cg_font in cg_fonts {
        CFRelease(env, cg_font);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

fn font_and_size(env: &Environment, font: CTFontRef) -> (&Font, CGFloat) {
    let &CTFontHostObject { cg_fonts, size } = env.objc.borrow(font);
    (&env.objc.borrow::<CGFontHostObject>(cg_fonts[0]).font, size)
}

/// The font to use for a glyph ID, and the glyph ID in that font.
fn font_for_glyph(env: &Environment, font: CTFontRef, glyph: CGGlyph) -> (CGFontRef, CGGlyph) {
    let cg_fonts = env.objc.borrow::<CTFontHostObject>(font).cg_fonts;
    if glyph & FALLBACK_GLYPH != 0 {
        (cg_fonts[1], glyph & !FALLBACK_GLYPH)
    } else {
        (cg_fonts[0], glyph)
    }
}

fn CTFontCreateWithName(
    env: &mut Environment,
    name: id, // CFStringRef
    size: CGFloat,
    matrix: ConstPtr<CGAffineTransform>,
) -> CTFontRef {
    let name = if name.is_null() {
        String::new()
    } else {
        ns_string::to_rust_string(env, name).to_string()
    };
    if !matrix.is_null() {
        log!("TODO: CTFontCreateWithName() with a matrix (ignored)");
    }
    // Size 0 means the default size.
    let size = if size == 0.0 { 12.0 } else { size };
    let bold = name.contains("Bold") || name.contains("W6") || name.contains("W8");
    log!(
        "CTFontCreateWithName({:?}, {}): using Noto Sans{}",
        name,
        size,
        if bold { " Bold" } else { "" }
    );
    let (ja, sc) = if bold {
        (Font::sans_bold_ja(), Font::sans_bold_sc())
    } else {
        (Font::sans_regular_ja(), Font::sans_regular_sc())
    };
    assert!(ja.number_of_glyphs() < FALLBACK_GLYPH && sc.number_of_glyphs() < FALLBACK_GLYPH);
    // Han characters look different in Chinese and Japanese, so prefer the
    // user's language.
    let chinese = crate::window::get_preferred_language_codes(env)
        .first()
        .is_some_and(|language| language.starts_with("zh"));
    let (main, fallback) = if chinese { (sc, ja) } else { (ja, sc) };
    let cg_fonts = [
        from_font(env, main.with_em_size()),
        from_font(env, fallback.with_em_size()),
    ];
    let host_object = Box::new(CTFontHostObject { cg_fonts, size });
    let class = env.objc.get_known_class("_touchHLE_CTFont", &mut env.mem);
    env.objc.alloc_object(class, host_object, &mut env.mem)
}

fn CTFontGetSize(env: &mut Environment, font: CTFontRef) -> CGFloat {
    font_and_size(env, font).1
}

/// Noto Sans CJK has much taller line metrics (ascent 1.16 em, descent
/// 0.288 em) than Hiragino Kaku Gothic ProN, the Japanese system font that
/// apps are designed around. Apps position text using these metrics, so
/// report values close to Hiragino's (found by comparing with the real
/// thing) to avoid shifting the text.
const ASCENT_EM: CGFloat = 0.88;
const DESCENT_EM: CGFloat = 0.12;

fn CTFontGetAscent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    let (_, size) = font_and_size(env, font);
    ASCENT_EM * size
}

fn CTFontGetDescent(env: &mut Environment, font: CTFontRef) -> CGFloat {
    let (_, size) = font_and_size(env, font);
    // Core Text's descent is positive.
    DESCENT_EM * size
}

fn CTFontGetGlyphsForCharacters(
    env: &mut Environment,
    font: CTFontRef,
    characters: ConstPtr<unichar>,
    glyphs: MutPtr<CGGlyph>,
    count: CFIndex,
) -> bool {
    let count: GuestUSize = count.try_into().unwrap();
    let mut all_found = true;
    let cg_fonts = env.objc.borrow::<CTFontHostObject>(font).cg_fonts;
    for i in 0..count {
        let c = env.mem.read(characters + i);
        let main = &env.objc.borrow::<CGFontHostObject>(cg_fonts[0]).font;
        let mut glyph = main.glyph_id_for_char(c).0;
        if glyph == 0 {
            let fallback = &env.objc.borrow::<CGFontHostObject>(cg_fonts[1]).font;
            let fallback_glyph = fallback.glyph_id_for_char(c).0;
            if fallback_glyph != 0 {
                glyph = fallback_glyph | FALLBACK_GLYPH;
            }
        }
        all_found &= glyph != 0;
        env.mem.write(glyphs + i, glyph);
    }
    all_found
}

/// Bounding box of a glyph, relative to its origin on the baseline (y up).
fn glyph_rect(font: &Font, size: CGFloat, glyph: CGGlyph) -> CGRect {
    let scale = size / font.units_per_em() as CGFloat;
    let Some((x_min, y_min, x_max, y_max)) = font.glyph_bounding_box(glyph) else {
        return CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: 0.0,
                height: 0.0,
            },
        };
    };
    CGRect {
        origin: CGPoint {
            x: x_min as CGFloat * scale,
            y: y_min as CGFloat * scale,
        },
        size: CGSize {
            width: (x_max - x_min) as CGFloat * scale,
            height: (y_max - y_min) as CGFloat * scale,
        },
    }
}

fn CTFontGetBoundingRectsForGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    _orientation: CTFontOrientation,
    glyphs: ConstPtr<CGGlyph>,
    bounding_rects: MutPtr<CGRect>,
    count: CFIndex,
) -> CGRect {
    let count: GuestUSize = count.try_into().unwrap();
    let mut union: Option<(CGFloat, CGFloat, CGFloat, CGFloat)> = None;
    for i in 0..count {
        let glyph = env.mem.read(glyphs + i);
        let (cg_font, glyph) = font_for_glyph(env, font, glyph);
        let size = font_and_size(env, font).1;
        let rect = glyph_rect(
            &env.objc.borrow::<CGFontHostObject>(cg_font).font,
            size,
            glyph,
        );
        if !bounding_rects.is_null() {
            env.mem.write(bounding_rects + i, rect);
        }
        if rect.size.width == 0.0 && rect.size.height == 0.0 {
            continue;
        }
        let (x0, y0) = (rect.origin.x, rect.origin.y);
        let (x1, y1) = (x0 + rect.size.width, y0 + rect.size.height);
        union = Some(match union {
            None => (x0, y0, x1, y1),
            Some((ux0, uy0, ux1, uy1)) => (ux0.min(x0), uy0.min(y0), ux1.max(x1), uy1.max(y1)),
        });
    }
    let (x0, y0, x1, y1) = union.unwrap_or((0.0, 0.0, 0.0, 0.0));
    CGRect {
        origin: CGPoint { x: x0, y: y0 },
        size: CGSize {
            width: x1 - x0,
            height: y1 - y0,
        },
    }
}

fn CTFontGetAdvancesForGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    _orientation: CTFontOrientation,
    glyphs: ConstPtr<CGGlyph>,
    advances: MutPtr<CGSize>,
    count: CFIndex,
) -> f64 {
    let count: GuestUSize = count.try_into().unwrap();
    let mut total = 0.0;
    for i in 0..count {
        let glyph = env.mem.read(glyphs + i);
        let (cg_font, glyph) = font_for_glyph(env, font, glyph);
        let size = font_and_size(env, font).1;
        let ct_font = &env.objc.borrow::<CGFontHostObject>(cg_font).font;
        let scale = size / ct_font.units_per_em() as CGFloat;
        let advance = ct_font.glyph_hor_advance(glyph).unwrap_or(0) as CGFloat * scale;
        total += advance as f64;
        if !advances.is_null() {
            env.mem.write(
                advances + i,
                CGSize {
                    width: advance,
                    height: 0.0,
                },
            );
        }
    }
    total
}

fn CTFontDrawGlyphs(
    env: &mut Environment,
    font: CTFontRef,
    glyphs: ConstPtr<CGGlyph>,
    positions: ConstPtr<CGPoint>,
    count: GuestUSize,
    context: CGContextRef,
) {
    let size = env.objc.borrow::<CTFontHostObject>(font).size;
    // Each glyph may be from a different font, so draw them one at a time.
    let glyph_ptr: MutPtr<CGGlyph> = env.mem.alloc_and_write(0);
    for i in 0..count {
        let glyph = env.mem.read(glyphs + i);
        let (cg_font, glyph) = font_for_glyph(env, font, glyph);
        env.mem.write(glyph_ptr, glyph);
        show_glyphs_with_font(
            env,
            context,
            cg_font,
            size,
            glyph_ptr.cast_const(),
            positions + i,
            1,
        );
    }
    env.mem.free(glyph_ptr.cast());
}

const FUNCTIONS: FunctionExports = &[
    export_c_func!(CTFontCreateWithName(_, _, _)),
    export_c_func!(CTFontGetSize(_)),
    export_c_func!(CTFontGetAscent(_)),
    export_c_func!(CTFontGetDescent(_)),
    export_c_func!(CTFontGetGlyphsForCharacters(_, _, _, _)),
    export_c_func!(CTFontGetBoundingRectsForGlyphs(_, _, _, _, _)),
    export_c_func!(CTFontGetAdvancesForGlyphs(_, _, _, _, _)),
    export_c_func!(CTFontDrawGlyphs(_, _, _, _, _)),
];
