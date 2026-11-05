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
use crate::font::WrapMode;
use crate::frameworks::core_foundation::cf_array::CFArrayRef;
use crate::frameworks::core_foundation::cf_attributed_string::CFAttributedStringRef;
use crate::frameworks::core_foundation::{CFIndex, CFRange, CFRelease, CFTypeRef};
use crate::frameworks::core_graphics::cg_affine_transform::CGAffineTransform;
use crate::frameworks::core_graphics::cg_context::{show_glyphs_with_font, CGContextRef};
use crate::frameworks::core_graphics::cg_font::{
    from_font, CGFontHostObject, CGFontRef, CGFontRetain, CGGlyph,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::from_rust_string;
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::ns_string::to_rust_string;
use crate::frameworks::foundation::{ns_array, ns_string, unichar, NSRange, NSUInteger};
use crate::frameworks::uikit::ui_font::font_for_text;
use crate::frameworks::uikit::ui_font::is_uifont;
use crate::mem::ConstVoidPtr;
use crate::mem::Ptr;
use crate::mem::{guest_size_of, ConstPtr, GuestUSize, MutPtr};
use crate::objc::autorelease;
use crate::objc::msg_class;
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
    /// How much to enlarge narrow glyphs, see [narrow_glyph_scale_for_app].
    narrow_glyph_scale: CGFloat,
    /// The `UIFont` this was created from, if any.
    font: id,
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

/// Opaque CoreText font descriptor reference.
pub type CTFontDescriptorRef = crate::objc::id;

#[derive(Clone, Default)]
struct CTFrameHostObject {
    /// The single CTLine we produce for the (whole) attributed string. Our
    /// layout engine doesn't break a framesetter's string across multiple
    /// lines/columns, so a frame holds exactly one line. This matches what
    /// the apps in the corpus need (they mostly measure / draw short labels).
    line: id,
    /// The string range covered by the frame, mirrored back from the
    /// attributed string for `CTFrameGetStringRange`/`GetVisibleStringRange`.
    range: CFRange,
}

impl HostObject for CTFrameHostObject {}

/// Opaque CoreText frame reference.
pub type CTFrameRef = crate::objc::id;

#[derive(Clone, Default)]
struct CTFramesetterHostObject {
    attr_string: id,
}

impl HostObject for CTFramesetterHostObject {}

/// Opaque CoreText framesetter reference.
pub type CTFramesetterRef = crate::objc::id;

/// Opaque CoreText line reference.
pub type CTLineRef = crate::objc::id;

/// Opaque paragraph style reference.
pub type CTParagraphStyleRef = crate::objc::id;

fn alloc_descriptor(env: &mut Environment, attrs: id) -> CTFontDescriptorRef {
    let class = env
        .objc
        .get_known_class("_Podroid_CTFontDescriptor", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFontDescriptorHostObject { attrs }),
        &mut env.mem,
    )
}

fn alloc_font(env: &mut Environment, font: id) -> CTFontRef {
    let name: id = msg![env; font fontName];
    let size: CGFloat = msg![env; font pointSize];
    let ct_font = CTFontCreateWithName(env, name, size, Ptr::null());
    env.objc.borrow_mut::<CTFontHostObject>(ct_font).font = font;
    ct_font
}

fn alloc_frame(env: &mut Environment, line: id, range: CFRange) -> CTFrameRef {
    let class = env.objc.get_known_class("_Podroid_CTFrame", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFrameHostObject { line, range }),
        &mut env.mem,
    )
}

fn alloc_framesetter(env: &mut Environment, attr_string: id) -> CTFramesetterRef {
    let class = env
        .objc
        .get_known_class("_Podroid_CTFramesetter", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CTFramesetterHostObject { attr_string }),
        &mut env.mem,
    )
}

fn alloc_line(env: &mut Environment, text: id, font: id, attributes: id) -> CTLineRef {
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

fn descriptor_attrs(env: &mut Environment, descriptor: CTFontDescriptorRef) -> id {
    env.objc
        .borrow::<CTFontDescriptorHostObject>(descriptor)
        .attrs
}

fn font_from_descriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    size: CGFloat,
) -> id {
    let attrs = descriptor_attrs(env, descriptor);
    if attrs == nil {
        return msg_class![env; UIFont systemFontOfSize:size];
    }
    let font_name_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontNameAttribute");
    let family_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontFamilyAttribute");
    let size_key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSFontSizeAttribute");
    let mut name: id = nil;
    let by_name: id = msg![env; attrs objectForKey:font_name_key];
    if by_name != nil {
        name = by_name;
    } else {
        let family: id = msg![env; attrs objectForKey:family_key];
        if family != nil {
            let family_str = to_rust_string(env, family).into_owned();
            name = match family_str.as_str() {
                "Courier New" => get_static_str(env, "CourierNewPSMT"),
                "Helvetica" => get_static_str(env, "Helvetica"),
                "Times New Roman" => get_static_str(env, "TimesNewRomanPSMT"),
                _ => nil,
            };
        }
    }
    let font_size: CGFloat = if size > 0.0 {
        size
    } else {
        let size_num: id = msg![env; attrs objectForKey:size_key];
        if size_num != nil {
            msg![env; size_num floatValue]
        } else {
            12.0
        }
    };
    if name == nil {
        msg_class![env; UIFont systemFontOfSize:font_size]
    } else {
        msg_class![env; UIFont fontWithName:name size:font_size]
    }
}

fn line_font(env: &mut Environment, line: CTLineRef) -> id {
    env.objc.borrow::<CTLineHostObject>(line).font
}

fn line_text(env: &mut Environment, line: CTLineRef) -> id {
    env.objc.borrow::<CTLineHostObject>(line).text
}

fn string_and_font_from_attr_string(
    env: &mut Environment,
    attr_string: CFAttributedStringRef,
) -> (id, id, id) {
    let (string, attrs) = attributed_string_parts(env, attr_string);
    let font = if attrs == nil {
        msg_class![env; UIFont systemFontOfSize:12.0]
    } else {
        let key = get_static_str(env, "NSFont");
        let maybe_font = msg![env; attrs objectForKey:key];
        let ui_font = if maybe_font == nil || is_uifont(env, maybe_font) {
            maybe_font
        } else {
            env.objc
                .get_host_object(maybe_font)
                .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
                .map(|host| host.font)
                .unwrap_or(nil)
        };
        if ui_font == nil {
            msg_class![env; UIFont systemFontOfSize:12.0]
        } else {
            ui_font
        }
    };
    (string, font, attrs)
}

/// The file in `/System/Library/Fonts/Cache` (see `fs.rs`) that stands in for
/// a font with this name or family and style. touchHLE ships the metric
/// compatible Liberation fonts, plus Arial and a bold Helvetica.
fn system_font_file(name: &str, style: &str) -> &'static str {
    let name = name.to_ascii_lowercase();
    let style = style.to_ascii_lowercase();
    let bold = name.contains("bold") || style.contains("bold");
    let italic = ["italic", "oblique"]
        .iter()
        .any(|word| name.contains(word) || style.contains(word));
    let family = if name.contains("courier") || name.contains("mono") {
        "LiberationMono"
    } else if ["times", "georgia", "serif"]
        .iter()
        .any(|word| name.contains(word))
        && !name.contains("sans")
    {
        "LiberationSerif"
    } else if name.contains("helvetica") && bold && !italic {
        return "helvetica-bold.ttf";
    } else if name.contains("arial") && !bold && !italic {
        return "Arial.ttf";
    } else {
        "LiberationSans"
    };
    match (family, bold, italic) {
        ("LiberationMono", false, false) => "LiberationMono-Regular.ttf",
        ("LiberationMono", true, false) => "LiberationMono-Bold.ttf",
        ("LiberationMono", false, true) => "LiberationMono-Italic.ttf",
        ("LiberationMono", true, true) => "LiberationMono-BoldItalic.ttf",
        ("LiberationSerif", false, false) => "LiberationSerif-Regular.ttf",
        ("LiberationSerif", true, false) => "LiberationSerif-Bold.ttf",
        ("LiberationSerif", false, true) => "LiberationSerif-Italic.ttf",
        ("LiberationSerif", true, true) => "LiberationSerif-BoldItalic.ttf",
        (_, false, false) => "LiberationSans-Regular.ttf",
        (_, true, false) => "LiberationSans-Bold.ttf",
        (_, false, true) => "LiberationSans-Italic.ttf",
        (_, true, true) => "LiberationSans-BoldItalic.ttf",
    }
}

/// Resolve a `CTFontRef` to the `UIFont` backing it. Also accepts a bare
/// `UIFont`, which some apps pass where a `CTFontRef` is expected.
fn ui_font_for_ct_font(env: &mut Environment, font: CTFontRef) -> id {
    if font == nil {
        return nil;
    }
    if is_uifont(env, font) {
        return font;
    }
    env.objc
        .get_host_object(font)
        .and_then(|host| host.as_any().downcast_ref::<CTFontHostObject>())
        .map(|host| host.font)
        .unwrap_or(nil)
}

#[derive(Clone, Default)]
struct CTFontDescriptorHostObject {
    attrs: id,
}

impl HostObject for CTFontDescriptorHostObject {}

fn attributed_string_parts(env: &mut Environment, attr_string: CFAttributedStringRef) -> (id, id) {
    if attr_string == nil {
        return (nil, nil);
    }
    let string: id = msg![env; attr_string string];
    if string == nil {
        return (nil, nil);
    }
    let length: NSUInteger = msg![env; string length];
    let attrs = if length == 0 {
        nil
    } else {
        let range_ptr: MutPtr<NSRange> = MutPtr::null();
        msg![env; attr_string attributesAtIndex:(0u32) effectiveRange:range_ptr]
    };
    (string, attrs)
}

pub const CLASSES: ClassExports = objc_classes! {

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

@implementation _Podroid_CTFontDescriptor: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _Podroid_CTFramesetter: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

@implementation _Podroid_CTFrame: NSObject
- (())dealloc {
    env.objc.dealloc_object(this, &mut env.mem)
}
@end

};

fn font_and_size(env: &Environment, font: CTFontRef) -> (&Font, CGFloat) {
    let &CTFontHostObject { cg_fonts, size, .. } = env.objc.borrow(font);
    (&env.objc.borrow::<CGFontHostObject>(cg_fonts[0]).font, size)
}

/// The font to use for a glyph ID, and the glyph ID in that font.
/// Bundle ID of Monster Hunter Freedom Unite, and the start of its function
/// that draws a glyph, to check that it's the expected version.
const MHFU_BUNDLE_ID: &str = "jp.co.capcom.mhp2gwd";
const MHFU_DRAW_GLYPH: (u32, u32) = (0x2aea48, 0xaf03b5f0);
/// This looks right on an S10e: letters fill much more of their cell without
/// touching their neighbours.
const MHFU_NARROW_GLYPH_SCALE: CGFloat = 1.3;
/// Glyphs narrower than this many ems count as narrow.
const NARROW_GLYPH_WIDTH: CGFloat = 0.75;

/// MHFU uses the engine of the Japanese P2G, which centres each character in a
/// cell made for Japanese text. English letters are only about half as wide, so
/// they come out small and far apart. Drawing them larger makes them readable.
fn narrow_glyph_scale_for_app(env: &Environment) -> CGFloat {
    let (address, instruction) = MHFU_DRAW_GLYPH;
    if env.window.is_some()
        && env.bundle.bundle_identifier() == MHFU_BUNDLE_ID
        && env.mem.read(ConstPtr::<u32>::from_bits(address)) == instruction
    {
        MHFU_NARROW_GLYPH_SCALE
    } else {
        1.0
    }
}

/// The size to draw a glyph at.
fn glyph_size(env: &Environment, font: CTFontRef, cg_font: CGFontRef, glyph: CGGlyph) -> CGFloat {
    let host = env.objc.borrow::<CTFontHostObject>(font);
    let size = host.size;
    if host.narrow_glyph_scale != 1.0 {
        let font = &env.objc.borrow::<CGFontHostObject>(cg_font).font;
        let rect = glyph_rect(font, size, glyph);
        if rect.size.width < size * NARROW_GLYPH_WIDTH {
            return size * host.narrow_glyph_scale;
        }
    }
    size
}

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
    let narrow_glyph_scale = narrow_glyph_scale_for_app(env);
    let host_object = Box::new(CTFontHostObject {
        cg_fonts,
        size,
        uses_fallback: true,
        narrow_glyph_scale,
        font: nil,
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
            narrow_glyph_scale: 1.0,
            font: nil,
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
        let size = glyph_size(env, font, cg_font, glyph);
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
    // Each glyph may be from a different font, so draw them one at a time.
    let glyph_ptr: MutPtr<CGGlyph> = env.mem.alloc_and_write(0);
    for i in 0..count {
        let glyph = env.mem.read(glyphs + i);
        let (cg_font, glyph) = font_for_glyph(env, font, glyph);
        let size = glyph_size(env, font, cg_font, glyph);
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

fn CTFontCopyPostScriptName(env: &mut Environment, font: CTFontRef) -> id {
    if font.is_null() {
        return nil;
    }
    let ui_font = env.objc.borrow::<CTFontHostObject>(font).font;
    if ui_font == nil {
        return nil;
    }
    let name: id = msg![env; ui_font fontName];
    if name == nil {
        nil
    } else {
        retain(env, name)
    }
}

/// `CTFontRef CTFontCreateCopyWithAttributes(CTFontRef font, CGFloat size,
///     const CGAffineTransform *matrix, CTFontDescriptorRef attributes)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509393-ctfontcreatecopywithattributes>
///
/// > Returns a new font with additional attributes based on the original
/// > font. [...] `size`: The point size for the font reference. If 0.0 is
/// > specified, the original font's size is preserved.
///
/// The original font's name is kept unless `attributes` carries a font name
/// or family of its own, in which case those win (descriptor attributes
/// override the original font's, as on iOS). Like the other CTFont
/// constructors here the transform matrix is not applied: touchHLE's text
/// renderer only supports untransformed fonts.
fn CTFontCreateCopyWithAttributes(
    env: &mut Environment,
    font: CTFontRef,
    size: CGFloat,
    _matrix: ConstPtr<CGAffineTransform>,
    attributes: CTFontDescriptorRef,
) -> CTFontRef {
    let original = ui_font_for_ct_font(env, font);
    if original == nil {
        return nil;
    }
    let size: CGFloat = if size > 0.0 {
        size
    } else {
        msg![env; original pointSize]
    };

    let overrides_face = attributes != nil && {
        let attrs = descriptor_attrs(env, attributes);
        attrs != nil && {
            let name_key = get_static_str(env, "NSFontNameAttribute");
            let family_key = get_static_str(env, "NSFontFamilyAttribute");
            let by_name: id = msg![env; attrs objectForKey:name_key];
            let by_family: id = msg![env; attrs objectForKey:family_key];
            by_name != nil || by_family != nil
        }
    };

    let new_font: id = if overrides_face {
        font_from_descriptor(env, attributes, size)
    } else {
        msg![env; original fontWithSize:size]
    };
    let new_font = if new_font == nil { original } else { new_font };
    retain(env, new_font);
    alloc_font(env, new_font)
}

fn CTFontCreateWithFontDescriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    size: CGFloat,
    _matrix: ConstVoidPtr,
) -> CTFontRef {
    let font = font_from_descriptor(env, descriptor, size);
    retain(env, font);
    alloc_font(env, font)
}

/// `CFTypeRef CTFontDescriptorCopyAttribute(CTFontDescriptorRef descriptor,
///     CFStringRef attribute)`. Attributes the descriptor was created with are
/// returned as is. The font file URL is filled in from the bundled fonts:
/// engines such as Unity load the font file themselves through it, and draw
/// no text at all without one.
fn CTFontDescriptorCopyAttribute(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    attribute: id,
) -> CFTypeRef {
    if descriptor == nil || attribute == nil {
        return nil;
    }
    let attrs = descriptor_attrs(env, descriptor);
    if attrs != nil {
        let value: id = msg![env; attrs objectForKey:attribute];
        if value != nil {
            return retain(env, value);
        }
    }
    let url_key = get_static_str(env, "NSCTFontFileURLAttribute");
    let is_url_key: bool = msg![env; attribute isEqualToString:url_key];
    if !is_url_key {
        return nil;
    }
    let mut attr_string = |key: &'static str| {
        if attrs == nil {
            return None;
        }
        let key = get_static_str(env, key);
        let value: id = msg![env; attrs objectForKey:key];
        (value != nil).then(|| to_rust_string(env, value).into_owned())
    };
    let name = attr_string("NSFontNameAttribute")
        .or_else(|| attr_string("NSFontFamilyAttribute"))
        .unwrap_or_default();
    let style = attr_string("NSFontFaceAttribute").unwrap_or_default();
    let path = format!(
        "/System/Library/Fonts/Cache/{}",
        system_font_file(&name, &style)
    );
    let path = from_rust_string(env, path);
    let url: id = msg_class![env; NSURL fileURLWithPath:path];
    release(env, path);
    retain(env, url)
}

/// `CTFontDescriptorCreateMatchingFontDescriptor` — the single best match
/// for a descriptor. Every name falls back to one of the bundled fonts (see
/// [CTFontDescriptorCopyAttribute]), so the descriptor matches itself.
fn CTFontDescriptorCreateMatchingFontDescriptor(
    env: &mut Environment,
    descriptor: CTFontDescriptorRef,
    _mandatory_attributes: CFTypeRef, // CFSetRef of attribute keys
) -> CTFontDescriptorRef {
    if descriptor == nil {
        return nil;
    }
    retain(env, descriptor)
}

/// `CTFontDescriptorCreateMatchingFontDescriptors` — returns descriptors
/// matching the given one under `attributes` (a set of mandatory keys).
/// touchHLE's font stack only exposes the built-in system fonts, so no
/// additional descriptors can ever match: return an empty (non-null)
/// CFArray, which is what a real system returns when nothing matches.
/// Returning NULL here makes some engines treat Core Text as broken and
/// abort font fallback entirely.
fn CTFontDescriptorCreateMatchingFontDescriptors(
    env: &mut Environment,
    _descriptor: CTFontDescriptorRef,
    _attributes: CFTypeRef, // CFSetRef of mandatory attribute keys
) -> CFArrayRef {
    let empty: CFArrayRef = msg_class![env; NSArray array];
    retain(env, empty);
    empty
}

fn CTFontDescriptorCreateWithAttributes(
    env: &mut Environment,
    attributes: id,
) -> CTFontDescriptorRef {
    let attrs = if attributes == nil {
        msg_class![env; NSDictionary dictionary]
    } else {
        attributes
    };
    retain(env, attrs);
    alloc_descriptor(env, attrs)
}

/// `bool CTFontManagerRegisterGraphicsFont(CGFontRef font, CFErrorRef *error)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1499468-ctfontmanagerregistergraphicsfon>
fn CTFontManagerRegisterGraphicsFont(
    env: &mut Environment,
    font: ConstVoidPtr,             // CGFontRef
    error: MutPtr<crate::objc::id>, // CFErrorRef*
) -> bool {
    if !error.is_null() {
        env.mem.write(error, crate::objc::nil);
    }
    if font.is_null() {
        return false;
    }
    true
}

/// `void CTFrameDraw(CTFrameRef frame, CGContextRef context)`
///
/// Draws the frame's lines into the context. We draw our single line at the
/// origin, matching `CTLineDraw`.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509601-ctframedraw>
fn CTFrameDraw(env: &mut Environment, frame: CTFrameRef, context: id) {
    if frame.is_null() || context.is_null() {
        return;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    if line != nil {
        CTLineDraw(env, line, context);
    }
}

/// `void CTFrameGetLineOrigins(CTFrameRef frame, CFRange range, CGPoint
/// origins[])`
///
/// Writes the origin of each line into `origins`. Our frame is a single line
/// whose origin sits at the top-left of the frame's coordinate space.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509559-ctframegetlineorigins>
fn CTFrameGetLineOrigins(
    env: &mut Environment,
    frame: CTFrameRef,
    range: CFRange,
    origins: MutPtr<CGPoint>,
) {
    if frame.is_null() || origins.is_null() {
        return;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    let line_count: i32 = if line != nil { 1 } else { 0 };
    let count = if range.length == 0 {
        line_count
    } else {
        range.length.min(line_count)
    };
    for i in 0..count as u32 {
        env.mem.write(origins + i, CGPoint { x: 0.0, y: 0.0 });
    }
}

/// `CFArrayRef CTFrameGetLines(CTFrameRef frame)`
///
/// Returns the array of `CTLine`s that make up the frame. touchHLE lays the
/// frame out as a single line, so this returns a one-element array.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509601-ctframegetlines>
fn CTFrameGetLines(env: &mut Environment, frame: CTFrameRef) -> id {
    if frame.is_null() {
        return nil;
    }
    let line = env.objc.borrow::<CTFrameHostObject>(frame).line;
    let arr: id = msg_class![env; NSMutableArray arrayWithCapacity:1u32];
    if line != nil {
        () = msg![env; arr addObject:line];
    }
    let immutable: id = msg![env; arr copy];
    autorelease(env, immutable)
}

/// `CFRange CTFrameGetStringRange(CTFrameRef frame)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509593-ctframegetstringrange>
fn CTFrameGetStringRange(env: &mut Environment, frame: CTFrameRef) -> CFRange {
    if frame.is_null() {
        return CFRange {
            location: 0,
            length: 0,
        };
    }
    env.objc.borrow::<CTFrameHostObject>(frame).range
}

/// `CFRange CTFrameGetVisibleStringRange(CTFrameRef frame)`
///
/// We lay the entire string into the (single-line) frame, so the visible
/// range equals the full string range.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509563-ctframegetvisiblestringrange>
fn CTFrameGetVisibleStringRange(env: &mut Environment, frame: CTFrameRef) -> CFRange {
    if frame.is_null() {
        return CFRange {
            location: 0,
            length: 0,
        };
    }
    env.objc.borrow::<CTFrameHostObject>(frame).range
}

/// `CTFrameRef CTFramesetterCreateFrame(CTFramesetterRef framesetter,
///     CFRange stringRange, CGPathRef path, CFDictionaryRef frameAttributes)`
///
/// Creates a frame laying the framesetter's attributed string into the
/// supplied path's bounding region. touchHLE represents the laid-out frame as
/// a single `CTLine` over the whole text; `CTFrameGetLines` then returns it.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509702-ctframesettercreateframe>
fn CTFramesetterCreateFrame(
    env: &mut Environment,
    framesetter: CTFramesetterRef,
    string_range: CFRange,
    _path: id,
    _frame_attributes: id,
) -> CTFrameRef {
    if framesetter.is_null() {
        return nil;
    }
    let attr_string = env
        .objc
        .borrow::<CTFramesetterHostObject>(framesetter)
        .attr_string;
    let (text, font, attributes) = string_and_font_from_attr_string(env, attr_string);
    retain(env, text);
    retain(env, font);
    retain(env, attributes);
    let line = alloc_line(env, text, font, attributes);
    // alloc_line took the retains above; the frame owns the line.
    let full_len = {
        let len: NSUInteger = msg![env; text length];
        len as i32
    };
    let range = CFRange {
        location: string_range.location,
        length: if string_range.length == 0 {
            (full_len - string_range.location).max(0)
        } else {
            string_range.length
        },
    };
    alloc_frame(env, line, range)
}

/// `CTFramesetterRef
/// CTFramesetterCreateWithAttributedString(CFAttributedStringRef string)`
///
/// Creates a framesetter that manages laying the attributed string into
/// frames. We retain the attributed string and wrap it in a host object so
/// frame creation and size suggestion can re-derive the text and font.
///
/// Reference: <https://developer.apple.com/documentation/coretext/1509611-ctframesettercreatewithattribute>
fn CTFramesetterCreateWithAttributedString(
    env: &mut Environment,
    string: CFAttributedStringRef,
) -> CTFramesetterRef {
    if string.is_null() {
        return nil;
    }
    retain(env, string);
    alloc_framesetter(env, string)
}

/// `CGSize CTFramesetterSuggestFrameSizeWithConstraints(CTFramesetterRef
/// framesetter, CFRange stringRange, CFDictionaryRef frameAttributes, CGSize
/// constraints,
///     CFRange *fitRange)`
///
/// Determines the frame size needed for a string range. We measure the text
/// with the backing font, wrapping at the constraint width when one is given
/// (a width <= 0 or non-finite means "unconstrained"), and report the fit
/// range as the whole string. This produces correct sizing for the
/// single-/multi-line labels the apps in the corpus lay out.
///
/// Reference: <https://developer.apple.com/documentation/coretext/ctframesettersuggestframesizewithconstraints(_:_:_:_:_:)>
fn CTFramesetterSuggestFrameSizeWithConstraints(
    env: &mut Environment,
    framesetter: CTFramesetterRef,
    string_range: CFRange,
    _frame_attributes: id,
    constraints: CGSize,
    fit_range: MutPtr<CFRange>,
) -> CGSize {
    if framesetter.is_null() {
        if !fit_range.is_null() {
            env.mem.write(
                fit_range,
                CFRange {
                    location: 0,
                    length: 0,
                },
            );
        }
        return CGSize {
            width: 0.0,
            height: 0.0,
        };
    }
    let attr_string = env
        .objc
        .borrow::<CTFramesetterHostObject>(framesetter)
        .attr_string;
    let (text, font, _) = string_and_font_from_attr_string(env, attr_string);
    let size: CGFloat = msg![env; font pointSize];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);

    let wrap = if constraints.width.is_finite() && constraints.width > 0.0 {
        Some((constraints.width, WrapMode::Word))
    } else {
        None
    };
    let (w, h) = font_obj.calculate_text_size(size, &text_str, wrap);

    let full_len = {
        let len: NSUInteger = msg![env; text length];
        len as i32
    };
    if !fit_range.is_null() {
        // Report the entire requested range as fitting. A length of 0 in the
        // requested range means "to the end of the string" per Apple's docs.
        let length = if string_range.length == 0 {
            (full_len - string_range.location).max(0)
        } else {
            string_range.length
        };
        env.mem.write(
            fit_range,
            CFRange {
                location: string_range.location,
                length,
            },
        );
    }
    CGSize {
        width: w,
        height: h,
    }
}

fn CTLineGetImageBounds(env: &mut Environment, line: CTLineRef, _context: id) -> CGRect {
    if line.is_null() {
        return CGRect::default();
    }
    let text = line_text(env, line);
    let font = line_font(env, line);
    let size: CGFloat = msg![env; font pointSize];
    let ascent_val: CGFloat = msg![env; font ascender];
    let text_str = to_rust_string(env, text).into_owned();
    let font_obj = font_for_text(env, font, &text_str).unwrap_or_else(Font::sans_regular);
    let (w, h) = font_obj.calculate_text_size(size, &text_str, None);
    CGRect {
        origin: CGPoint {
            x: 0.0,
            y: -ascent_val,
        },
        size: CGSize {
            width: w,
            height: h,
        },
    }
}

/// `CTParagraphStyleRef CTParagraphStyleCreate(
///     const CTParagraphStyleSetting *settings, size_t settingCount)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1524171-ctparagraphstylecreate>
fn CTParagraphStyleCreate(
    env: &mut Environment,
    _settings: ConstVoidPtr,
    _setting_count: u32,
) -> CTParagraphStyleRef {
    msg_class![env; NSObject new]
}

/// `CTParagraphStyleRef CTParagraphStyleCreateCopy(CTParagraphStyleRef)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1525098-ctparagraphstylecreatecopy>
fn CTParagraphStyleCreateCopy(
    env: &mut Environment,
    paragraph_style: CTParagraphStyleRef,
) -> CTParagraphStyleRef {
    if paragraph_style.is_null() {
        return nil;
    }
    retain(env, paragraph_style);
    paragraph_style
}

/// `bool CTParagraphStyleGetValueForSpecifier(CTParagraphStyleRef,
/// CTParagraphStyleSpecifier,
///     size_t valueBufferSize, void *valueBuffer)`
///
/// Reference: <https://developer.apple.com/documentation/coretext/1525353-ctparagraphstylegetvalueforspeci>
fn CTParagraphStyleGetValueForSpecifier(
    env: &mut Environment,
    _paragraph_style: CTParagraphStyleRef,
    _spec: u32,
    value_buffer_size: u32,
    value_buffer: MutPtr<u8>,
) -> bool {
    if !value_buffer.is_null() && value_buffer_size > 0 {
        let slice = env.mem.bytes_at_mut(value_buffer, value_buffer_size);
        slice.fill(0);
    }
    false
}

pub const FUNCTIONS: FunctionExports = &[
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
    export_c_func!(CTFontCopyPostScriptName(_)),
    export_c_func!(CTFontCreateCopyWithAttributes(_, _, _, _)),
    export_c_func!(CTFontCreateWithFontDescriptor(_, _, _)),
    export_c_func!(CTFontDescriptorCopyAttribute(_, _)),
    export_c_func!(CTFontDescriptorCreateMatchingFontDescriptor(_, _)),
    export_c_func!(CTFontDescriptorCreateMatchingFontDescriptors(_, _)),
    export_c_func!(CTFontDescriptorCreateWithAttributes(_)),
    export_c_func!(CTFontManagerRegisterGraphicsFont(_, _)),
    export_c_func!(CTFrameDraw(_, _)),
    export_c_func!(CTFrameGetLineOrigins(_, _, _)),
    export_c_func!(CTFrameGetLines(_)),
    export_c_func!(CTFrameGetStringRange(_)),
    export_c_func!(CTFrameGetVisibleStringRange(_)),
    export_c_func!(CTFramesetterCreateFrame(_, _, _, _)),
    export_c_func!(CTFramesetterCreateWithAttributedString(_)),
    export_c_func!(CTFramesetterSuggestFrameSizeWithConstraints(_, _, _, _, _)),
    export_c_func!(CTLineGetImageBounds(_, _)),
    export_c_func!(CTParagraphStyleCreate(_, _)),
    export_c_func!(CTParagraphStyleCreateCopy(_)),
    export_c_func!(CTParagraphStyleGetValueForSpecifier(_, _, _, _)),
];
