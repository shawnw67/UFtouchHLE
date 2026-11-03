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

use crate::dyld::{export_c_func, FunctionExports, HostConstant, HostDylib};
use crate::font::Font;
use crate::frameworks::core_foundation::{CFIndex, CFRange, CFRelease, CFTypeRef};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::cg_context::{show_glyphs_with_font, CGContextRef};
use crate::frameworks::core_graphics::cg_font::{
    from_font, CGFontHostObject, CGFontRef, CGFontRetain, CGGlyph,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::{ns_array, ns_string, unichar, NSRange, NSUInteger};
use crate::mem::{guest_size_of, ConstPtr, GuestUSize, MutPtr};
use crate::objc::{id, msg, nil, objc_classes, release, retain, ClassExports, HostObject};
use crate::Environment;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreText.framework/CoreText",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[&[("_kCTFontAttributeName", HostConstant::NSString("NSFont"))]],
    function_exports: &[FUNCTIONS],
};

type CTFontRef = CFTypeRef;
type CTFontOrientation = u32;

struct CTFontHostObject {
    /// `CGFontRef`s, retained: the main font and the fallback font.
    cg_fonts: [CGFontRef; 2],
    size: CGFloat,
    uses_fallback: bool,
}

struct CTLineHostObject {
    text: id,
    font: CTFontRef,
    attributes: id,
    runs: id,
}
impl HostObject for CTLineHostObject {}

struct CTRunHostObject {
    attributes: id,
    glyphs: Vec<CGGlyph>,
    positions: Vec<CGPoint>,
    glyphs_ptr: MutPtr<CGGlyph>,
    positions_ptr: MutPtr<CGPoint>,
}
impl HostObject for CTRunHostObject {}

struct CTTypesetterHostObject {
    attributed: id,
}
impl HostObject for CTTypesetterHostObject {}

/// Bit set in glyph IDs from the fallback font. Both fonts have fewer than
/// 0x8000 glyphs.
const FALLBACK_GLYPH: CGGlyph = 0x8000;
impl HostObject for CTFontHostObject {}

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _Podroid_CTFont: NSObject

- (())dealloc {
    let cg_fonts = env.objc.borrow::<CTFontHostObject>(this).cg_fonts;
    for cg_font in cg_fonts {
        CFRelease(env, cg_font);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation _Podroid_CTLine: NSObject
- (())dealloc {
    let host = env.objc.borrow::<CTLineHostObject>(this);
    let (text, font, attributes, runs) = (host.text, host.font, host.attributes, host.runs);
    release(env, text);
    release(env, font);
    release(env, attributes);
    release(env, runs);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _Podroid_CTRun: NSObject
- (())dealloc {
    let host = env.objc.borrow::<CTRunHostObject>(this);
    let (attributes, glyphs, positions) = (host.attributes, host.glyphs_ptr, host.positions_ptr);
    release(env, attributes);
    if !glyphs.is_null() { env.mem.free(glyphs.cast()); }
    if !positions.is_null() { env.mem.free(positions.cast()); }
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _Podroid_CTTypesetter: NSObject
- (())dealloc {
    let attributed = env.objc.borrow::<CTTypesetterHostObject>(this).attributed;
    release(env, attributed);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

};

fn font_and_size(env: &Environment, font: CTFontRef) -> (&Font, CGFloat) {
    let &CTFontHostObject { cg_fonts, size, .. } = env.objc.borrow(font);
    (&env.objc.borrow::<CGFontHostObject>(cg_fonts[0]).font, size)
}

/// The font to use for a glyph ID, and the glyph ID in that font.
fn font_for_glyph(env: &Environment, font: CTFontRef, glyph: CGGlyph) -> (CGFontRef, CGGlyph) {
    let host = env.objc.borrow::<CTFontHostObject>(font);
    let cg_fonts = host.cg_fonts;
    if host.uses_fallback && glyph & FALLBACK_GLYPH != 0 {
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
    let chinese = env.window.is_some()
        && crate::window::get_preferred_language_codes(env)
            .first()
            .is_some_and(|language| language.starts_with("zh"));
    let (main, fallback) = if chinese { (sc, ja) } else { (ja, sc) };
    let cg_fonts = [
        from_font(env, main.with_em_size()),
        from_font(env, fallback.with_em_size()),
    ];
    let host_object = Box::new(CTFontHostObject {
        cg_fonts,
        size,
        uses_fallback: true,
    });
    let class = env.objc.get_known_class("_Podroid_CTFont", &mut env.mem);
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

fn CTFontGetLeading(env: &mut Environment, font: CTFontRef) -> CGFloat {
    let (face, size) = font_and_size(env, font);
    face.line_gap(size)
}

fn CTLineCreateWithAttributedString(env: &mut Environment, attributed: id) -> id {
    if attributed.is_null() {
        return nil;
    }
    let source: id = msg![env; attributed string];
    let text: id = msg![env; source copy];
    let length: NSUInteger = msg![env; source length];
    let mut font = nil;
    let mut attributes = nil;
    if length != 0 {
        let range: MutPtr<NSRange> = MutPtr::null();
        let attrs: id =
            msg![env; attributed attributesAtIndex:(0 as NSUInteger) effectiveRange:range];
        attributes = retain(env, attrs);
        let key = ns_string::get_static_str(env, "NSFont");
        let candidate: id = msg![env; attrs objectForKey:key];
        if !candidate.is_null() {
            let class = env.objc.get_known_class("_Podroid_CTFont", &mut env.mem);
            let is_ct_font: bool = msg![env; candidate isKindOfClass:class];
            if is_ct_font {
                font = retain(env, candidate);
            }
        }
    }
    if font.is_null() {
        font = CTFontCreateWithName(env, nil, 12.0, ConstPtr::null());
    }
    let class = env.objc.get_known_class("_Podroid_CTLine", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTLineHostObject {
            text,
            font,
            attributes,
            runs: nil,
        }),
        &mut env.mem,
    )
}

fn CTTypesetterCreateWithAttributedString(env: &mut Environment, attributed: id) -> id {
    if attributed.is_null() {
        return nil;
    }
    retain(env, attributed);
    let class = env
        .objc
        .get_known_class("_Podroid_CTTypesetter", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTTypesetterHostObject { attributed }),
        &mut env.mem,
    )
}

fn CTTypesetterCreateLine(env: &mut Environment, typesetter: id, mut range: CFRange) -> id {
    if typesetter.is_null() {
        return nil;
    }
    let attributed = env
        .objc
        .borrow::<CTTypesetterHostObject>(typesetter)
        .attributed;
    let length =
        crate::frameworks::core_foundation::cf_attributed_string::CFAttributedStringGetLength(
            env, attributed,
        );
    if range.location < 0 || range.location > length || range.length < 0 {
        return nil;
    }
    if range.length == 0 {
        range.length = length - range.location;
    }
    let substring = crate::frameworks::core_foundation::cf_attributed_string::CFAttributedStringCreateWithSubstring(
        env, crate::frameworks::core_foundation::cf_allocator::kCFAllocatorDefault, attributed, range,
    );
    if substring.is_null() {
        return nil;
    }
    let line = CTLineCreateWithAttributedString(env, substring);
    release(env, substring);
    line
}

fn CTLineGetGlyphCount(env: &mut Environment, line: id) -> CFIndex {
    let text = env.objc.borrow::<CTLineHostObject>(line).text;
    let length: NSUInteger = msg![env; text length];
    length.try_into().unwrap_or(CFIndex::MAX)
}

fn CTLineGetGlyphRuns(env: &mut Environment, line: id) -> id {
    let host = env.objc.borrow::<CTLineHostObject>(line);
    if !host.runs.is_null() {
        return host.runs;
    }
    let (text, font, attributes) = (host.text, host.font, host.attributes);
    let mut units = Vec::new();
    ns_string::for_each_code_unit(env, text, |_, unit| units.push(unit));
    let host = env.objc.borrow::<CTFontHostObject>(font);
    let (fonts, size, uses_fallback) = (host.cg_fonts, host.size, host.uses_fallback);
    let mut glyphs = Vec::with_capacity(units.len());
    let mut positions = Vec::with_capacity(units.len());
    let mut x = 0.0;
    for unit in units {
        let mut glyph = if (0xd800..=0xdfff).contains(&unit) {
            0
        } else {
            env.objc
                .borrow::<CGFontHostObject>(fonts[0])
                .font
                .glyph_id_for_char(unit)
                .0
        };
        let mut selected = fonts[0];
        if glyph == 0 && uses_fallback && !(0xd800..=0xdfff).contains(&unit) {
            let fallback = &env.objc.borrow::<CGFontHostObject>(fonts[1]).font;
            let id = fallback.glyph_id_for_char(unit).0;
            if id != 0 {
                glyph = id | FALLBACK_GLYPH;
                selected = fonts[1];
            }
        }
        let face = &env.objc.borrow::<CGFontHostObject>(selected).font;
        let actual = if selected == fonts[1] && uses_fallback {
            glyph & !FALLBACK_GLYPH
        } else {
            glyph
        };
        positions.push(CGPoint { x, y: 0.0 });
        glyphs.push(glyph);
        x += face.glyph_hor_advance(actual).unwrap_or(0) as CGFloat * size
            / face.units_per_em() as CGFloat;
    }
    let mut entries = Vec::new();
    if !glyphs.is_empty() {
        retain(env, attributes);
        let class = env.objc.get_known_class("_Podroid_CTRun", &mut env.mem);
        let run = env.objc.alloc_object(
            class,
            Box::new(CTRunHostObject {
                attributes,
                glyphs,
                positions,
                glyphs_ptr: MutPtr::null(),
                positions_ptr: MutPtr::null(),
            }),
            &mut env.mem,
        );
        entries.push(run);
    }
    let runs = ns_array::from_vec(env, entries);
    env.objc.borrow_mut::<CTLineHostObject>(line).runs = runs;
    runs
}

fn CTRunGetGlyphCount(env: &mut Environment, run: id) -> CFIndex {
    env.objc
        .borrow::<CTRunHostObject>(run)
        .glyphs
        .len()
        .try_into()
        .unwrap_or(CFIndex::MAX)
}

fn CTRunGetAttributes(env: &mut Environment, run: id) -> id {
    env.objc.borrow::<CTRunHostObject>(run).attributes
}

fn run_range(range: CFRange, length: usize) -> Option<std::ops::Range<usize>> {
    let start = usize::try_from(range.location).ok()?;
    if start > length {
        return None;
    }
    let count = usize::try_from(range.length).ok()?;
    let end = if count == 0 {
        length
    } else {
        start.checked_add(count)?
    };
    (end <= length).then_some(start..end)
}

fn CTRunGetGlyphs(env: &mut Environment, run: id, range: CFRange, output: MutPtr<CGGlyph>) {
    let host = env.objc.borrow::<CTRunHostObject>(run);
    let Some(range) = run_range(range, host.glyphs.len()) else {
        return;
    };
    let glyphs = host.glyphs[range].to_vec();
    for (index, glyph) in glyphs.into_iter().enumerate() {
        env.mem.write(output + index as u32, glyph);
    }
}

fn CTRunGetPositions(env: &mut Environment, run: id, range: CFRange, output: MutPtr<CGPoint>) {
    let host = env.objc.borrow::<CTRunHostObject>(run);
    let Some(range) = run_range(range, host.positions.len()) else {
        return;
    };
    let positions = host.positions[range].to_vec();
    for (index, point) in positions.into_iter().enumerate() {
        env.mem.write(output + index as u32, point);
    }
}

fn CTRunGetGlyphsPtr(env: &mut Environment, run: id) -> ConstPtr<CGGlyph> {
    let host = env.objc.borrow::<CTRunHostObject>(run);
    if !host.glyphs_ptr.is_null() {
        return host.glyphs_ptr.cast_const();
    }
    if host.glyphs.is_empty() {
        return ConstPtr::null();
    }
    let glyphs = host.glyphs.clone();
    let bytes = (glyphs.len() as u32)
        .checked_mul(guest_size_of::<CGGlyph>())
        .unwrap();
    let ptr: MutPtr<CGGlyph> = env.mem.alloc(bytes).cast();
    for (index, glyph) in glyphs.into_iter().enumerate() {
        env.mem.write(ptr + index as u32, glyph);
    }
    env.objc.borrow_mut::<CTRunHostObject>(run).glyphs_ptr = ptr;
    ptr.cast_const()
}

fn CTRunGetPositionsPtr(env: &mut Environment, run: id) -> ConstPtr<CGPoint> {
    let host = env.objc.borrow::<CTRunHostObject>(run);
    if !host.positions_ptr.is_null() {
        return host.positions_ptr.cast_const();
    }
    if host.positions.is_empty() {
        return ConstPtr::null();
    }
    let positions = host.positions.clone();
    let bytes = (positions.len() as u32)
        .checked_mul(guest_size_of::<CGPoint>())
        .unwrap();
    let ptr: MutPtr<CGPoint> = env.mem.alloc(bytes).cast();
    for (index, point) in positions.into_iter().enumerate() {
        env.mem.write(ptr + index as u32, point);
    }
    env.objc.borrow_mut::<CTRunHostObject>(run).positions_ptr = ptr;
    ptr.cast_const()
}

fn CTLineGetTypographicBounds(
    env: &mut Environment,
    line: id,
    ascent: MutPtr<CGFloat>,
    descent: MutPtr<CGFloat>,
    leading: MutPtr<CGFloat>,
) -> f64 {
    let host = env.objc.borrow::<CTLineHostObject>(line);
    let (text, font) = (host.text, host.font);
    let (ascent_value, descent_value, leading_value) = (
        CTFontGetAscent(env, font),
        CTFontGetDescent(env, font),
        CTFontGetLeading(env, font),
    );
    if !ascent.is_null() {
        env.mem.write(ascent, ascent_value);
    }
    if !descent.is_null() {
        env.mem.write(descent, descent_value);
    }
    if !leading.is_null() {
        env.mem.write(leading, leading_value);
    }
    let mut units = Vec::new();
    ns_string::for_each_code_unit(env, text, |_, unit| units.push(unit));
    let size = env.objc.borrow::<CTFontHostObject>(font).size;
    let fonts = env.objc.borrow::<CTFontHostObject>(font).cg_fonts;
    let mut total = 0.0f64;
    for unit in units {
        if (0xd800..=0xdfff).contains(&unit) {
            continue;
        }
        let first = &env.objc.borrow::<CGFontHostObject>(fonts[0]).font;
        let mut glyph = first.glyph_id_for_char(unit).0;
        let mut selected = fonts[0];
        if glyph == 0 {
            let fallback = &env.objc.borrow::<CGFontHostObject>(fonts[1]).font;
            glyph = fallback.glyph_id_for_char(unit).0;
            selected = fonts[1];
        }
        let face = &env.objc.borrow::<CGFontHostObject>(selected).font;
        total += face.glyph_hor_advance(glyph).unwrap_or(0) as f64 * size as f64
            / face.units_per_em() as f64;
    }
    total
}

fn CTLineDraw(env: &mut Environment, line: id, context: CGContextRef) {
    if line.is_null() || context.is_null() {
        return;
    }
    let host = env.objc.borrow::<CTLineHostObject>(line);
    let (text, font) = (host.text, host.font);
    let font_host = env.objc.borrow::<CTFontHostObject>(font);
    let (fonts, size) = (font_host.cg_fonts, font_host.size);
    let mut units = Vec::new();
    ns_string::for_each_code_unit(env, text, |_, unit| units.push(unit));
    let glyph_ptr: MutPtr<CGGlyph> = env.mem.alloc_and_write(0);
    let position: MutPtr<CGPoint> = env.mem.alloc_and_write(CGPoint { x: 0.0, y: 0.0 });
    let mut x = 0.0;
    for unit in units {
        if (0xd800..=0xdfff).contains(&unit) {
            continue;
        }
        let first = &env.objc.borrow::<CGFontHostObject>(fonts[0]).font;
        let mut glyph = first.glyph_id_for_char(unit).0;
        let mut selected = fonts[0];
        if glyph == 0 {
            let fallback = &env.objc.borrow::<CGFontHostObject>(fonts[1]).font;
            glyph = fallback.glyph_id_for_char(unit).0;
            selected = fonts[1];
        }
        let face = &env.objc.borrow::<CGFontHostObject>(selected).font;
        let advance = face.glyph_hor_advance(glyph).unwrap_or(0) as CGFloat * size
            / face.units_per_em() as CGFloat;
        env.mem.write(glyph_ptr, glyph);
        env.mem.write(position, CGPoint { x, y: 0.0 });
        show_glyphs_with_font(
            env,
            context,
            selected,
            size,
            glyph_ptr.cast_const(),
            position.cast_const(),
            1,
        );
        x += advance;
    }
    env.mem.free(glyph_ptr.cast());
    env.mem.free(position.cast());
}

fn CTFontCopyGraphicsFont(
    env: &mut Environment,
    font: CTFontRef,
    attributes: MutPtr<id>,
) -> CGFontRef {
    if !attributes.is_null() {
        env.mem.write(attributes, id::null());
    }
    if font.is_null() {
        return id::null();
    }
    let main = env.objc.borrow::<CTFontHostObject>(font).cg_fonts[0];
    CGFontRetain(env, main)
}

fn CTFontCreateWithGraphicsFont(
    env: &mut Environment,
    graphics_font: CGFontRef,
    size: CGFloat,
    matrix: ConstPtr<CGAffineTransform>,
    attributes: id,
) -> CTFontRef {
    if graphics_font.is_null() {
        return id::null();
    }
    if !matrix.is_null() {
        log!("TODO: CTFontCreateWithGraphicsFont() with a matrix (ignored)");
    }
    if !attributes.is_null() {
        log!("TODO: CTFontCreateWithGraphicsFont() attributes (ignored)");
    }
    let size = if size <= 0.0 { 12.0 } else { size };
    // Core Text sizes refer to the em square, unlike rusttype's default.
    let face = env
        .objc
        .borrow::<CGFontHostObject>(graphics_font)
        .font
        .clone()
        .with_em_size();
    let main = from_font(env, face);
    let cg_fonts = [main, CGFontRetain(env, main)];
    let class = env.objc.get_known_class("_Podroid_CTFont", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFontHostObject {
            cg_fonts,
            size,
            uses_fallback: false,
        }),
        &mut env.mem,
    )
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
    export_c_func!(CTFontGetLeading(_)),
    export_c_func!(CTLineCreateWithAttributedString(_)),
    export_c_func!(CTTypesetterCreateWithAttributedString(_)),
    export_c_func!(CTTypesetterCreateLine(_, _)),
    export_c_func!(CTLineGetGlyphCount(_)),
    export_c_func!(CTLineGetGlyphRuns(_)),
    export_c_func!(CTRunGetGlyphCount(_)),
    export_c_func!(CTRunGetAttributes(_)),
    export_c_func!(CTRunGetGlyphs(_, _, _)),
    export_c_func!(CTRunGetPositions(_, _, _)),
    export_c_func!(CTRunGetGlyphsPtr(_)),
    export_c_func!(CTRunGetPositionsPtr(_)),
    export_c_func!(CTLineGetTypographicBounds(_, _, _, _)),
    export_c_func!(CTLineDraw(_, _)),
    export_c_func!(CTFontCopyGraphicsFont(_, _)),
    export_c_func!(CTFontCreateWithGraphicsFont(_, _, _, _)),
    export_c_func!(CTFontGetGlyphsForCharacters(_, _, _, _)),
    export_c_func!(CTFontGetBoundingRectsForGlyphs(_, _, _, _, _)),
    export_c_func!(CTFontGetAdvancesForGlyphs(_, _, _, _, _)),
    export_c_func!(CTFontDrawGlyphs(_, _, _, _, _)),
];
