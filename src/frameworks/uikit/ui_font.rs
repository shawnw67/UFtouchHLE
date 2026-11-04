/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIFont`.

use super::ui_graphics::UIGraphicsGetCurrentContext;
use crate::font::{Font, TextAlignment, WrapMode};
use crate::frameworks::core_graphics::cg_bitmap_context::CGBitmapContextDrawer;
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::to_rust_string;
use crate::frameworks::foundation::NSInteger;
use crate::objc::autorelease;
use crate::objc::{id, msg, objc_classes, ClassExports, HostObject};
use crate::Environment;
use std::collections::HashMap;
use std::ops::Range;

#[derive(Default)]
pub(super) struct State {
    fonts: HashMap<FontKind, Font>,
    sans_regular_ja: Option<Font>,
    sans_bold_ja: Option<Font>,
    sans_regular_sc: Option<Font>,
    sans_bold_sc: Option<Font>,
    /// `UIFont` objects are cached and never freed, as on iOS. Some apps rely
    /// on this and don't retain the fonts they get from convenience methods.
    instances: HashMap<(FontKind, u32), id>,
}
impl State {
    fn get_font_by_kind(&mut self, font_kind: FontKind) -> &Font {
        self.fonts
            .entry(font_kind)
            .or_insert_with(|| match font_kind {
                FontKind::MonoRegular => Font::mono_regular(),
                FontKind::MonoBold => Font::mono_bold(),
                FontKind::MonoBoldItalic => Font::mono_bold_italic(),
                FontKind::MonoItalic => Font::mono_italic(),
                FontKind::SansRegular => Font::sans_regular(),
                FontKind::SansBold => Font::sans_bold(),
                FontKind::SansBoldItalic => Font::sans_bold_italic(),
                FontKind::SansItalic => Font::sans_italic(),
                FontKind::SerifRegular => Font::serif_regular(),
                FontKind::SerifBold => Font::serif_bold(),
                FontKind::SerifBoldItalic => Font::serif_bold_italic(),
                FontKind::SerifItalic => Font::serif_italic(),
            })
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
enum FontKind {
    MonoRegular,
    MonoBold,
    MonoBoldItalic,
    MonoItalic,
    SansRegular,
    SansBold,
    SansBoldItalic,
    SansItalic,
    SerifRegular,
    SerifBold,
    SerifBoldItalic,
    SerifItalic,
}

struct UIFontHostObject {
    size: CGFloat,
    kind: FontKind,
    /// A font face that `kind` can't represent, such as a Core Graphics font.
    custom: Option<Font>,
}
impl HostObject for UIFontHostObject {}

/// Get the shared `UIFont` instance for a font kind and size. The caller does
/// not own the returned object (like any convenience constructor), but it is
/// never freed.
fn get_font_instance(env: &mut Environment, kind: FontKind, size: CGFloat) -> id {
    let key = (kind, size.to_bits());
    if let Some(&font) = env.framework_state.uikit.ui_font.instances.get(&key) {
        return font;
    }
    let class = env.objc.get_known_class("UIFont", &mut env.mem);
    let host_object = UIFontHostObject {
        size,
        kind,
        custom: None,
    };
    let new = env
        .objc
        .alloc_object(class, Box::new(host_object), &mut env.mem);
    env.framework_state.uikit.ui_font.instances.insert(key, new);
    new
}

/// Line break mode.
///
/// This is put here for convenience since it's font-related.
/// Apple puts it in its own header, also in UIKit.
pub type UILineBreakMode = NSInteger;
pub const UILineBreakModeWordWrap: UILineBreakMode = 0;
pub const UILineBreakModeCharacterWrap: UILineBreakMode = 1;
#[allow(dead_code)]
pub const UILineBreakModeClip: UILineBreakMode = 2;
#[allow(dead_code)]
pub const UILineBreakModeHeadTruncation: UILineBreakMode = 3;
pub const UILineBreakModeTailTruncation: UILineBreakMode = 4;
#[allow(dead_code)]
pub const UILineBreakModeMiddleTruncation: UILineBreakMode = 5;

/// Text alignment.
///
/// This is put here for convenience since it's font-related.
/// Apple puts it in its own header, also in UIKit.
pub type UITextAlignment = NSInteger;
pub const UITextAlignmentLeft: UITextAlignment = 0;
pub const UITextAlignmentCenter: UITextAlignment = 1;
pub const UITextAlignmentRight: UITextAlignment = 2;

/// Returns `true` if `obj` is a UIFont instance (backed by [UIFontHostObject]).
pub fn is_uifont(env: &mut Environment, obj: crate::objc::id) -> bool {
    if obj.is_null() {
        return false;
    }
    let class = env.objc.get_known_class("UIFont", &mut env.mem);
    let obj_class = crate::objc::ObjC::read_isa(obj, &env.mem);
    obj_class == class || env.objc.class_is_subclass_of(obj_class, class)
}

/// Select the font used for a particular string, applying touchHLE's built-in
/// Unicode fallbacks when the requested font cannot map the text.
pub fn font_for_text(env: &mut Environment, obj: id, text: &str) -> Option<Font> {
    if !is_uifont(env, obj) {
        return None;
    }
    let (kind, custom) = {
        let host = env.objc.borrow::<UIFontHostObject>(obj);
        (host.kind, host.custom.clone())
    };
    if let Some(font) = custom {
        if font_maps_text(&font, text) {
            return Some(font);
        }
    }
    Some(get_font(&mut env.framework_state.uikit.ui_font, kind, text).clone())
}

/// Create a UIFont using the shared resolver used by the Core Text and
/// Core Graphics font-name entry points.
pub fn new_uifont_with_name(env: &mut Environment, name: &str, size: CGFloat) -> id {
    let (kind, custom) = resolve_font_name(env, name);
    let host_object = UIFontHostObject { size, kind, custom };
    let class = env.objc.get_known_class("UIFont", &mut env.mem);
    let new = env
        .objc
        .alloc_object(class, Box::new(host_object), &mut env.mem);
    autorelease(env, new)
}

/// Returns the [Font] backing a UIFont object.
/// Returns `None` if `obj` is not a UIFont.
pub fn font_from_uifont(env: &mut Environment, obj: crate::objc::id) -> Option<Font> {
    if !is_uifont(env, obj) {
        return None;
    }
    Some(host_font(env, obj).0)
}

fn font_maps_text(font: &Font, text: &str) -> bool {
    text.chars()
        .all(|c| c.is_control() || font.glyph_id_for_unicode(c).0 != 0)
}

/// The font a UIFont draws with and its point size: the app's own font if it
/// has one, otherwise the built-in font of its kind.
fn host_font(env: &mut Environment, this: id) -> (Font, CGFloat) {
    let host_object = env.objc.borrow::<UIFontHostObject>(this);
    let font = match &host_object.custom {
        Some(font) => font.clone(),
        None => env
            .framework_state
            .uikit
            .ui_font
            .get_font_by_kind(host_object.kind)
            .clone(),
    };
    (font, host_object.size)
}

fn resolve_font_name(env: &mut Environment, name: &str) -> (FontKind, Option<Font>) {
    match resolve_legacy_chinese_font_face(name) {
        FontFace::BuiltIn(kind) => (kind, None),
        FontFace::SimplifiedChinese => {
            let font = env
                .framework_state
                .uikit
                .ui_font
                .sans_regular_sc
                .get_or_insert_with(Font::sans_regular_zh)
                .clone();
            (FontKind::SansRegular, Some(font))
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum FontFace {
    BuiltIn(FontKind),
    SimplifiedChinese,
}

/// Resolve legacy iOS font names used by the Chinese-text compatibility fix.
/// These CJK/Unicode aliases are routed to touchHLE's Simplified Chinese
/// fallback; other names keep the existing built-in font substitutions. This
/// shared mapping keeps UIKit, Core Text, and Core Graphics on the same font
/// data and Unicode cmap.
fn resolve_legacy_chinese_font_face(name: &str) -> FontFace {
    match name {
        "ArialUnicodeMS" | "Arial Unicode MS" | "STHeitiSC-Light" | "STHeitiSC-Medium"
        | "STHeitiSC-Regular" | "STHeitiTC-Light" | "STHeitiTC-Medium" | "STHeiti-Light"
        | "STHeiti-Medium" => FontFace::SimplifiedChinese,
        _ => FontFace::BuiltIn(get_equivalent_font(name).unwrap_or(FontKind::SansRegular)),
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIFont: NSObject

// Values are checked against iPhone 3GS, iOS 4.0.1
+ (CGFloat)labelFontSize {
    17.0
}
+ (CGFloat)buttonFontSize {
    18.0
}
+ (CGFloat)smallSystemFontSize {
    12.0
}
+ (CGFloat)systemFontSize {
    14.0
}

+ (id)systemFontOfSize:(CGFloat)size {
    get_font_instance(env, FontKind::SansRegular, size)
}
+ (id)boldSystemFontOfSize:(CGFloat)size {
    get_font_instance(env, FontKind::SansBold, size)
}
+ (id)italicSystemFontOfSize:(CGFloat)size {
    get_font_instance(env, FontKind::SansItalic, size)
}
+ (id)fontWithName:(id)fontName // NSString*
            size:(CGFloat)fontSize {
    let font_name = to_rust_string(env, fontName).to_string();
    let kind = get_equivalent_font(&font_name).unwrap_or_else(|| {
        log!("No replacement found for font {}. Using system font instead.", font_name);
        FontKind::SansRegular
    });
    get_font_instance(env, kind, fontSize)
}

- (id)fontWithSize:(CGFloat)size {
    let kind = env.objc.borrow::<UIFontHostObject>(this).kind;
    get_font_instance(env, kind, size)
}

- (CGFloat)ascender {
    let host_object = env.objc.borrow::<UIFontHostObject>(this);
    let font = env.framework_state.uikit.ui_font.get_font_by_kind(host_object.kind);
    font.ascent(host_object.size)
}
- (CGFloat)descender {
    let host_object = env.objc.borrow::<UIFontHostObject>(this);
    let font = env.framework_state.uikit.ui_font.get_font_by_kind(host_object.kind);
    font.descent(host_object.size)
}
- (CGFloat)leading {
    let host_object = env.objc.borrow::<UIFontHostObject>(this);
    let font = env.framework_state.uikit.ui_font.get_font_by_kind(host_object.kind);
    font.line_gap(host_object.size)
}

- (CGFloat)lineHeight {
    // This is calculated based on the documentation:
    // https://developer.apple.com/library/archive/documentation/TextFonts/Conceptual/CocoaTextArchitecture/FontHandling/FontHandling.html
    let ascender: CGFloat = msg![env; this ascender];
    let descender: CGFloat = msg![env; this descender];
    let leading: CGFloat = msg![env; this leading];
    assert!(descender <= 0.0);
    ascender + leading - descender
}

@end

};

fn convert_line_break_mode(ui_mode: UILineBreakMode) -> WrapMode {
    match ui_mode {
        UILineBreakModeWordWrap => WrapMode::Word,
        UILineBreakModeCharacterWrap => WrapMode::Char,
        // TODO: support this properly; fake support is so that UILabel works,
        // which has this as its default line break mode
        UILineBreakModeTailTruncation => WrapMode::Word,
        _ => unimplemented!("TODO: line break mode {}", ui_mode),
    }
}

#[rustfmt::skip]
fn get_font<'a>(state: &'a mut State, kind: FontKind, text: &str) -> &'a Font {
    // The default fonts (see font.rs) are the Liberation family, which are a
    // good substitute for Helvetica, the iPhone OS system font. Unfortunately,
    // there is no CJK support in these fonts. To support Super Monkey Ball in
    // Japanese, let's fall back to Noto Sans JP when necessary, and to Noto
    // Sans SC for (simplified) Chinese characters the Japanese font lacks.
    // FIXME: This heuristic is incomplete and a proper font fallback system
    // should be used instead.
    let is_cjk = |c: char| {
        let c = c as u32;
        (0x3000..=0x30FF).contains(&c) || // JA punctuation, kana
        (0xFF00..=0xFFEF).contains(&c) || // full-width/half-width chars
        (0x4e00..=0x9FA0).contains(&c) || // various kanji
        (0x3400..=0x4DBF).contains(&c) // more kanji
    };
    if !text.chars().any(is_cjk) {
        return state.get_font_by_kind(kind);
    }
    // CJK has no italic equivalent
    let bold = matches!(
        kind,
        FontKind::MonoBold
            | FontKind::MonoBoldItalic
            | FontKind::SansBold
            | FontKind::SansBoldItalic
            | FontKind::SerifBold
            | FontKind::SerifBoldItalic
    );
    let ja = if bold {
        state.sans_bold_ja.get_or_insert_with(Font::sans_bold_ja)
    } else {
        state.sans_regular_ja.get_or_insert_with(Font::sans_regular_ja)
    };
    if text.chars().filter(|&c| is_cjk(c)).all(|c| ja.has_glyph(c)) {
        return if bold {
            state.sans_bold_ja.as_ref().unwrap()
        } else {
            state.sans_regular_ja.as_ref().unwrap()
        };
    }
    if bold {
        state.sans_bold_sc.get_or_insert_with(Font::sans_bold_sc)
    } else {
        state.sans_regular_sc.get_or_insert_with(Font::sans_regular_sc)
    }
}

/// Called by the `sizeWithFont:` method family on `NSString`.
pub fn size_with_font(
    env: &mut Environment,
    font: id,
    text: &str,
    constrained: Option<(CGSize, UILineBreakMode)>,
) -> CGSize {
    let host_object = env.objc.borrow::<UIFontHostObject>(font);

    let font = get_font(
        &mut env.framework_state.uikit.ui_font,
        host_object.kind,
        text,
    );

    let wrap = constrained.map(|(size, ui_mode)| (size.width, convert_line_break_mode(ui_mode)));

    let (width, height) = font.calculate_text_size(host_object.size, text, wrap);

    CGSize { width, height }
}

/// Determine how the text lines will be rendered given a constraint
pub fn break_lines_with_font<'a>(
    env: &mut Environment,
    font: id,
    text: &'a str,
    constrained: Option<(CGSize, UILineBreakMode)>,
) -> Vec<(f32, &'a str)> {
    let host_object = env.objc.borrow::<UIFontHostObject>(font);

    let font = get_font(
        &mut env.framework_state.uikit.ui_font,
        host_object.kind,
        text,
    );

    let wrap = constrained.map(|(size, ui_mode)| (size.width, convert_line_break_mode(ui_mode)));

    font.break_lines(host_object.size, text, wrap)
}

#[inline(always)]
pub fn draw_font_glyph(
    drawer: &mut CGBitmapContextDrawer,
    raster_glyph: crate::font::RasterGlyph,
    fill_color: (f32, f32, f32, f32),
    clip_x: Option<Range<f32>>,
    clip_y: Option<Range<f32>>,
) {
    let mut glyph_rect = {
        let (x, y) = raster_glyph.origin();
        let (width, height) = raster_glyph.dimensions();
        CGRect {
            origin: CGPoint { x, y },
            size: CGSize {
                width: width as f32,
                height: height as f32,
            },
        }
    };
    // The code in font.rs won't and can't clip glyphs hanging over the right
    // and bottom sides of the rect, so it has to be done here. Bear in mind
    // that this must not incorrectly affect the texture co-ordinates, otherwise
    // the glyphs become squashed instead.
    // Note that there isn't clipping for the other sides currently because it
    // doesn't seem to be needed.
    if let Some(clip_x) = clip_x {
        if glyph_rect.origin.x >= clip_x.end {
            return;
        }
        if glyph_rect.origin.x + glyph_rect.size.width > clip_x.end {
            glyph_rect.size.width = clip_x.end - glyph_rect.origin.x;
        }
    }
    if let Some(clip_y) = clip_y {
        if glyph_rect.origin.y >= clip_y.end {
            return;
        }
        if glyph_rect.origin.y + glyph_rect.size.height > clip_y.end {
            glyph_rect.size.height = clip_y.end - glyph_rect.origin.y;
        }
    }

    for ((x, y), (tex_x, tex_y)) in drawer.iter_transformed_pixels(glyph_rect) {
        // TODO: bilinear sampling
        let coverage = raster_glyph.pixel_at((
            (tex_x * glyph_rect.size.width - 0.5).round() as i32,
            (tex_y * glyph_rect.size.height - 0.5).round() as i32,
        ));
        let (r, g, b, a) = fill_color;
        let (r, g, b, a) = (r * coverage, g * coverage, b * coverage, a * coverage);
        drawer.put_pixel((x, y), (r, g, b, a), /* blend: */ true);
    }
}

/// Called by the `drawAtPoint:` method family on `NSString`.
pub fn draw_at_point(
    env: &mut Environment,
    font: id,
    text: &str,
    point: CGPoint,
    width_and_line_break_mode: Option<(CGFloat, UILineBreakMode)>,
) -> CGSize {
    let context = UIGraphicsGetCurrentContext(env);

    let host_object = env.objc.borrow::<UIFontHostObject>(font);

    let font = get_font(
        &mut env.framework_state.uikit.ui_font,
        host_object.kind,
        text,
    );

    let width_and_line_break_mode =
        width_and_line_break_mode.map(|(width, ui_mode)| (width, convert_line_break_mode(ui_mode)));
    let clip_x = width_and_line_break_mode.map(|(width, _)| point.x..(point.x + width));
    let (width, height) =
        font.calculate_text_size(host_object.size, text, width_and_line_break_mode);

    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();

    font.draw(
        host_object.size,
        text,
        (point.x, point.y),
        width_and_line_break_mode,
        TextAlignment::Left,
        |raster_glyph| {
            draw_font_glyph(
                &mut drawer,
                raster_glyph,
                fill_color,
                clip_x.clone(),
                /* clip_y: */ None,
            )
        },
    );

    CGSize { width, height }
}

/// Called by the `drawInRect:` method family on `NSString`.
pub fn draw_in_rect(
    env: &mut Environment,
    font: id,
    text: &str,
    rect: CGRect,
    line_break_mode: UILineBreakMode,
    alignment: UITextAlignment,
) -> CGSize {
    let context = UIGraphicsGetCurrentContext(env);

    let text_size = size_with_font(env, font, text, Some((rect.size, line_break_mode)));

    let host_object = env.objc.borrow::<UIFontHostObject>(font);

    let font = get_font(
        &mut env.framework_state.uikit.ui_font,
        host_object.kind,
        text,
    );

    let mut drawer = CGBitmapContextDrawer::new(&env.objc, &mut env.mem, context);
    let fill_color = drawer.rgb_fill_color();

    let (origin_x_offset, alignment) = match alignment {
        UITextAlignmentLeft => (0.0, TextAlignment::Left),
        UITextAlignmentCenter => (rect.size.width / 2.0, TextAlignment::Center),
        UITextAlignmentRight => (rect.size.width, TextAlignment::Right),
        _ => unimplemented!(),
    };

    font.draw(
        host_object.size,
        text,
        (rect.origin.x + origin_x_offset, rect.origin.y),
        Some((rect.size.width, convert_line_break_mode(line_break_mode))),
        alignment,
        |raster_glyph| {
            draw_font_glyph(
                &mut drawer,
                raster_glyph,
                fill_color,
                /* clip_x: */ Some(rect.origin.x..(rect.origin.x + rect.size.width)),
                /* clip_y: */ Some(rect.origin.y..(rect.origin.y + rect.size.height)),
            )
        },
    );

    text_size
}

fn get_equivalent_font(system_font: &str) -> Option<FontKind> {
    // Maps every font found in every font family in an iOS 2 Simulator
    match system_font {
        // Font Family: Courier
        "Courier" => None,
        "Courier-BoldOblique" => None,
        "Courier-Oblique" => None,
        "Courier-Bold" => None,
        // Font Family: AppleGothic
        "AppleGothic" => None,
        // Font Family: Arial
        "ArialMT" => Some(FontKind::SansRegular),
        "Arial-BoldMT" => Some(FontKind::SansBold),
        "Arial-BoldItalicMT" => Some(FontKind::SansBoldItalic),
        "Arial-ItalicMT" => Some(FontKind::SansItalic),
        // Font Family: STHeiti TC
        "STHeitiTC-Light" => None,
        "STHeitiTC-Medium" => None,
        // Font Family: Hiragino Kaku Gothic ProN
        "HiraKakuProN-W6" => None,
        "HiraKakuProN-W3" => None,
        // Font Family: Courier New
        "CourierNewPS-BoldMT" => Some(FontKind::MonoRegular),
        "CourierNewPS-ItalicMT" => Some(FontKind::MonoBold),
        "CourierNewPS-BoldItalicMT" => Some(FontKind::MonoBoldItalic),
        "CourierNewPSMT" => Some(FontKind::MonoItalic),
        // Font Family: Zapfino
        "Zapfino" => None,
        // Font Family: Arial Unicode MS
        "ArialUnicodeMS" => None,
        // Font Family: STHeiti SC
        "STHeitiSC-Medium" => None,
        "STHeitiSC-Light" => None,
        // Font Family: American Typewriter
        "AmericanTypewriter" => Some(FontKind::MonoRegular),
        "AmericanTypewriter-Bold" => Some(FontKind::MonoBold),
        // Font Family: Helvetica
        "Helvetica-Oblique" => None,
        "Helvetica-BoldOblique" => None,
        "Helvetica" => None,
        "Helvetica-Bold" => None,
        // Font Family: Marker Felt
        "MarkerFelt-Thin" => None,
        // Font Family: Helvetica Neue
        "HelveticaNeue" => None,
        "HelveticaNeue-Bold" => None,
        // Font Family: DB LCD Temp
        "DBLCDTempBlack" => None,
        // Font Family: Verdana
        "Verdana-Bold" => None,
        "Verdana-BoldItalic" => None,
        "Verdana" => None,
        "Verdana-Italic" => None,
        // Font Family: Times New Roman
        "TimesNewRomanPSMT" => Some(FontKind::SerifRegular),
        "TimesNewRomanPS-BoldMT" => Some(FontKind::SerifBold),
        "TimesNewRomanPS-BoldItalicMT" => Some(FontKind::SerifBoldItalic),
        "TimesNewRomanPS-ItalicMT" => Some(FontKind::SerifItalic),
        // Font Family: Georgia
        "Georgia-Bold" => None,
        "Georgia" => None,
        "Georgia-BoldItalic" => None,
        "Georgia-Italic" => None,
        _ => None,
    }
}
