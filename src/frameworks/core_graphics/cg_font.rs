/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGFont`

use super::cg_data_provider;
use super::cg_data_provider::CGDataProviderRef;
use super::{CGFloat, CGPoint, CGRect, CGSize};
use crate::dyld::{export_c_func, FunctionExports};
use crate::font::Font;
use crate::frameworks::core_foundation::cf_data::CFDataRef;
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::core_graphics::cg_geometry::CGRectZero;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::unichar;
use crate::mem::MutVoidPtr;
use crate::mem::Ptr;
use crate::mem::{ConstPtr, GuestUSize, MutPtr};
use crate::objc::nil;
use crate::objc::retain;
use crate::objc::ObjC;
use crate::objc::{id, msg, msg_class, objc_classes, ClassExports, HostObject};
use crate::Environment;
use rusttype::GlyphId;

// Note: on iOS SDK side this type is defined as a pointer to an opaque struct
pub(crate) type CGFontRef = CFTypeRef;

type CGFontIndex = u16;
pub(crate) type CGGlyph = CGFontIndex;

pub struct CGFontHostObject {
    pub font: Font,
}
impl HostObject for CGFontHostObject {}

pub type CGFontPostScriptFormat = i32;

/// Create a CGFont (+1) backed by `font`, like [CGFontCreateWithDataProvider].
pub fn cg_font_with_font(env: &mut Environment, font: Font) -> CGFontRef {
    let host_obj = Box::new(CGFontHostObject { font });
    let class = env.objc.get_known_class("_Podroid_CGFont", &mut env.mem);
    env.objc.alloc_object(class, host_obj, &mut env.mem)
}

/// Extract a name record from a TrueType 'name' table.
/// `name_id`: 1=family, 2=subfamily, 4=full name, 6=PostScript name
fn extract_name_from_name_table(data: &[u8], name_id: u16) -> Option<String> {
    if data.len() < 6 {
        return None;
    }
    let count = u16::from_be_bytes([data[2], data[3]]) as usize;
    let string_offset = u16::from_be_bytes([data[4], data[5]]) as usize;

    for i in 0..count {
        let record_offset = 6 + i * 12;
        if data.len() < record_offset + 12 {
            break;
        }
        let platform_id = u16::from_be_bytes([data[record_offset], data[record_offset + 1]]);
        let encoding_id = u16::from_be_bytes([data[record_offset + 2], data[record_offset + 3]]);
        let _language_id = u16::from_be_bytes([data[record_offset + 4], data[record_offset + 5]]);
        let nid = u16::from_be_bytes([data[record_offset + 6], data[record_offset + 7]]);
        let length =
            u16::from_be_bytes([data[record_offset + 8], data[record_offset + 9]]) as usize;
        let offset =
            u16::from_be_bytes([data[record_offset + 10], data[record_offset + 11]]) as usize;

        if nid != name_id {
            continue;
        }

        let str_start = string_offset + offset;
        if str_start + length > data.len() {
            continue;
        }

        let raw = &data[str_start..str_start + length];

        // Platform 3 (Windows) encoding 1 (Unicode BMP) — UTF-16BE
        if platform_id == 3 && encoding_id == 1 {
            let chars: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            return String::from_utf16(&chars).ok();
        }
        // Platform 1 (Macintosh) encoding 0 (Roman) — ASCII/Latin-1
        if platform_id == 1 && encoding_id == 0 {
            return Some(String::from_utf8_lossy(raw).into_owned());
        }
        // Platform 0 (Unicode) — UTF-16BE
        if platform_id == 0 {
            let chars: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            return String::from_utf16(&chars).ok();
        }
    }
    None
}

fn font_from_name(env: &mut Environment, name: CFStringRef) -> id {
    if name == nil {
        return nil;
    }
    let name = ns_string::to_rust_string(env, name).into_owned();
    crate::frameworks::uikit::ui_font::new_uifont_with_name(env, &name, 12.0)
}

/// Returns `true` if `font` is a CGFont created via
/// [CGFontCreateWithDataProvider] (backed by [CGFontHostObject]).
pub fn is_data_provider_font(env: &mut Environment, font: CGFontRef) -> bool {
    if font.is_null() {
        return false;
    }
    let class = env.objc.get_known_class("_Podroid_CGFont", &mut env.mem);
    let obj_class = ObjC::read_isa(font, &env.mem);
    obj_class == class || env.objc.class_is_subclass_of(obj_class, class)
}

/// Special glyph index values
pub const kCGFontIndexInvalid: CGFontIndex = 0xFFFF;

pub const kCGFontPostScriptFormatType42: CGFontPostScriptFormat = 42;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _Podroid_CGFont: NSObject
@end

};

fn CGFontCreateWithDataProvider(env: &mut Environment, provider: CGDataProviderRef) -> CGFontRef {
    let bytes = cg_data_provider::borrow_bytes(env, provider);
    let font = Font::from_vec(bytes.to_vec());
    from_font(env, font)
}

/// Create a new (retained) `CGFont` wrapping a host font.
pub(crate) fn from_font(env: &mut Environment, font: Font) -> CGFontRef {
    let host_obj = Box::new(CGFontHostObject { font });
    let class = env.objc.get_known_class("_Podroid_CGFont", &mut env.mem);
    env.objc.alloc_object(class, host_obj, &mut env.mem)
}

pub fn CGFontRetain(env: &mut Environment, font: CGFontRef) -> CGFontRef {
    if !font.is_null() {
        CFRetain(env, font)
    } else {
        font
    }
}
pub fn CGFontRelease(env: &mut Environment, font: CGFontRef) {
    if !font.is_null() {
        CFRelease(env, font);
    }
}

// This is an undocumented API! But some apps still may call it
fn CGFontGetGlyphsForUnichars(
    env: &mut Environment,
    font: CGFontRef,
    chars: ConstPtr<unichar>,
    glyphs: MutPtr<CGGlyph>,
    length: GuestUSize,
) -> bool {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    for i in 0..length {
        let c: unichar = env.mem.read(chars + i);
        let x = font.glyph_id_for_char(c).0;
        env.mem.write(glyphs + i, x);
    }
    true
}

fn CGFontGetUnitsPerEm(env: &mut Environment, font: CGFontRef) -> i32 {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    font.units_per_em().into()
}

fn CGFontGetAscent(env: &mut Environment, font: CGFontRef) -> i32 {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    font.ascent_unscaled() as i32
}
fn CGFontGetDescent(env: &mut Environment, font: CGFontRef) -> i32 {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    font.descent_unscaled() as i32
}

fn CGFontGetLeading(env: &mut Environment, font: CGFontRef) -> i32 {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    font.line_gap_unscaled() as i32
}

fn CGFontGetFontBBox(env: &mut Environment, font: CGFontRef) -> CGRect {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    let (x_min, y_min, x_max, y_max) = font.global_bounding_box();
    assert!(x_min <= x_max && y_min <= y_max);
    CGRect {
        origin: CGPoint {
            x: x_min as CGFloat,
            y: y_min as CGFloat,
        },
        size: CGSize {
            width: (x_max - x_min) as CGFloat,
            height: (y_max - y_min) as CGFloat,
        },
    }
}

fn CGFontGetGlyphAdvances(
    env: &mut Environment,
    font: CGFontRef,
    glyphs: ConstPtr<CGGlyph>,
    count: GuestUSize,
    advances: MutPtr<i32>,
) -> bool {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    for i in 0..count {
        let glyph_id = env.mem.read(glyphs + i);
        let advance_width = font.glyph_hor_advance(glyph_id).unwrap().into();
        env.mem.write(advances + i, advance_width);
    }
    true
}

fn CGFontGetGlyphBBoxes(
    env: &mut Environment,
    font: CGFontRef,
    glyphs: ConstPtr<CGGlyph>,
    count: GuestUSize,
    boxes: MutPtr<CGRect>,
) -> bool {
    let mut res = true;
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    for i in 0..count {
        let glyph_id = env.mem.read(glyphs + i);
        let Some((x_min, y_min, x_max, y_max)) = font.glyph_bounding_box(glyph_id) else {
            res = false;
            // TODO: not sure what real device does here?
            env.mem.write(boxes + i, CGRectZero);
            continue;
        };
        assert!(x_min <= x_max && y_min <= y_max);
        // TODO: extract to a helper
        let rect = CGRect {
            origin: CGPoint {
                x: x_min as CGFloat,
                y: y_min as CGFloat,
            },
            size: CGSize {
                width: (x_max - x_min) as CGFloat,
                height: (y_max - y_min) as CGFloat,
            },
        };
        env.mem.write(boxes + i, rect);
    }
    res
}

fn CGFontGetItalicAngle(env: &mut Environment, font: CGFontRef) -> CGFloat {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    font.italic_angle().unwrap_or(0.0)
}

fn CGFontCopyTableForTag(env: &mut Environment, font: CGFontRef, tag: u32) -> CFDataRef {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    let table_data = font.table_data(tag).unwrap();

    let len = table_data.len() as GuestUSize;
    let guest_bytes = env.mem.alloc(len);
    env.mem
        .bytes_at_mut(guest_bytes.cast(), len)
        .copy_from_slice(table_data);
    let new: id = msg_class![env; NSData alloc];
    msg![env; new initWithBytesNoCopy:guest_bytes length:len]
}

fn CGFontGetNumberOfGlyphs(env: &mut Environment, font: CGFontRef) -> GuestUSize {
    let font = &env.objc.borrow::<CGFontHostObject>(font).font;
    font.number_of_glyphs().into()
}

/// `bool CGFontCanCreatePostScriptSubset(CGFontRef font,
///     CGFontPostScriptFormat format)`
fn CGFontCanCreatePostScriptSubset(
    env: &mut Environment,
    font: CGFontRef,
    format: CGFontPostScriptFormat,
) -> bool {
    if font.is_null() {
        return false;
    }
    // We can theoretically create Type42 subsets (which are just TrueType
    // wrapped in PostScript). Type1 and Type3 conversions are not supported.
    format == kCGFontPostScriptFormatType42 && is_data_provider_font(env, font)
}

/// `CFStringRef CGFontCopyFullName(CGFontRef font)`
fn CGFontCopyFullName(env: &mut Environment, font: CGFontRef) -> CFStringRef {
    if font.is_null() {
        return nil;
    }
    if is_data_provider_font(env, font) {
        // Full name is nameID=4 in the 'name' table.
        let name_data = env
            .objc
            .borrow::<CGFontHostObject>(font)
            .font
            .table_data(0x6E616D65 /* 'name' */);
        if let Some(data) = name_data {
            if let Some(full_name) = extract_name_from_name_table(data, 4) {
                return ns_string::from_rust_string(env, full_name);
            }
        }
        return nil;
    }
    CGFontCopyPostScriptName(env, font)
}

/// `CFStringRef CGFontCopyGlyphNameForGlyph(CGFontRef font, CGGlyph glyph)`
///
/// Returns the PostScript name for a glyph, or nil if not available.
fn CGFontCopyGlyphNameForGlyph(
    env: &mut Environment,
    font: CGFontRef,
    glyph: CGGlyph,
) -> CFStringRef {
    if font.is_null() {
        return nil;
    }
    if !is_data_provider_font(env, font) {
        return nil;
    }

    let name = env
        .objc
        .borrow::<CGFontHostObject>(font)
        .font
        .glyph_name(GlyphId(glyph));

    match name {
        Some(s) => ns_string::from_rust_string(env, s),
        None => nil,
    }
}

/// `CFStringRef CGFontCopyPostScriptName(CGFontRef font)`
fn CGFontCopyPostScriptName(env: &mut Environment, font: CGFontRef) -> CFStringRef {
    if font.is_null() {
        return nil;
    }
    if is_data_provider_font(env, font) {
        // Try to extract PostScript name from the 'name' table (nameID=6).
        let name_data = env
            .objc
            .borrow::<CGFontHostObject>(font)
            .font
            .table_data(0x6E616D65 /* 'name' */);
        if let Some(data) = name_data {
            if let Some(ps_name) = extract_name_from_name_table(data, 6) {
                return ns_string::from_rust_string(env, ps_name);
            }
        }
        return nil;
    }
    // UIFont-backed path
    let name: id = msg![env; font fontName];
    if name == nil {
        return nil;
    }
    msg![env; name copy]
}

/// `CFArrayRef CGFontCopyTableTags(CGFontRef font)`
///
/// Returns an array of all table tags present in the font file. Each tag is
/// a 4-byte value stored as a CFNumber (UInt32). We return it as a CFArray.
fn CGFontCopyTableTags(env: &mut Environment, font: CGFontRef) -> CFTypeRef {
    if font.is_null() {
        return nil;
    }
    if !is_data_provider_font(env, font) {
        return nil;
    }

    let tags = env.objc.borrow::<CGFontHostObject>(font).font.table_tags();

    if tags.is_empty() {
        return nil;
    }

    // Create an NSMutableArray and populate it with NSNumber objects
    let array: id = msg_class![env; NSMutableArray arrayWithCapacity:(tags.len() as u32)];
    for tag in &tags {
        let num: id = msg_class![env; NSNumber numberWithUnsignedInt:(*tag)];
        () = msg![env; array addObject:num];
    }
    // Return retained (the caller owns this reference)
    retain(env, array);
    array
}

/// `CFArrayRef CGFontCopyVariationAxes(CGFontRef font)`
///
/// Returns variation axes if the font is a variable font. We check for the
/// 'fvar' table; if not present, returns nil.
fn CGFontCopyVariationAxes(env: &mut Environment, font: CGFontRef) -> CFTypeRef {
    if font.is_null() {
        return nil;
    }
    if !is_data_provider_font(env, font) {
        return nil;
    }
    // Check if fvar table exists
    let fvar = env
        .objc
        .borrow::<CGFontHostObject>(font)
        .font
        .table_data(0x66766172 /* 'fvar' */);
    if fvar.is_none() {
        return nil;
    }
    // Font variations are not supported in this emulation layer.
    // Returning nil (empty) for now — the font will use its default instance.
    nil
}

/// `CFDictionaryRef CGFontCopyVariations(CGFontRef font)`
fn CGFontCopyVariations(_env: &mut Environment, font: CGFontRef) -> CFTypeRef {
    log_dbg!("CGFontCopyVariations({:?}) — returning nil", font);
    nil
}

/// `CGFontRef CGFontCreateCopyWithVariations(CGFontRef font,
///                                           CFDictionaryRef variations)`
fn CGFontCreateCopyWithVariations(
    env: &mut Environment,
    font: CGFontRef,
    _variations: CFTypeRef,
) -> CGFontRef {
    // Font variations (optical size, weight axes, etc.) are not supported in
    // this emulation layer. Return a retained copy of the original font.
    log_dbg!("CGFontCreateCopyWithVariations: variations ignored, returning copy");
    if font.is_null() {
        return Ptr::null();
    }
    retain(env, font);
    font
}

fn CGFontCreatePostScriptEncoding(
    _env: &mut Environment,
    font: CGFontRef,
    _encoding: ConstPtr<CGGlyph>,
) -> CFTypeRef {
    log_dbg!(
        "CGFontCreatePostScriptEncoding({:?}) — hardcoded to NULL",
        font
    );
    nil
}

fn CGFontCreatePostScriptSubset(
    _env: &mut Environment,
    font: CGFontRef,
    _subset_name: CFStringRef,
    _format: CGFontPostScriptFormat,
    _glyphs: ConstPtr<CGGlyph>,
    _count: u32,
    _encoding: ConstPtr<CGGlyph>,
) -> CFTypeRef {
    log_dbg!(
        "CGFontCreatePostScriptSubset({:?}) — hardcoded to NULL",
        font
    );
    nil
}

/// `CGFontRef CGFontCreateWithFontName(CFStringRef name)`
pub fn CGFontCreateWithFontName(env: &mut Environment, name: CFStringRef) -> CGFontRef {
    let ui_font = font_from_name(env, name);
    if ui_font == nil {
        log!(
            "CGFontCreateWithFontName: could not create font for {:?}",
            name
        );
        return Ptr::null();
    }
    // CGFont is not toll-free bridged to UIFont in touchHLE. Returning the
    // UIFont here makes CGFontCopyTableForTag and CGFontGetGlyphsForUnichars
    // treat a valid font as an unsupported foreign object, so apps that build
    // their own cmap from the 'cmap' table receive no mapping data. Wrap the
    // rasterizable font in the same host object used by CGFont data providers.
    let font = crate::frameworks::uikit::ui_font::font_from_uifont(env, ui_font)
        .unwrap_or_else(Font::sans_regular);
    let cg_font = cg_font_with_font(env, font);
    log_dbg!(
        "CGFontCreateWithFontName({}) => {:?}",
        if name != nil {
            ns_string::to_rust_string(env, name).into_owned()
        } else {
            "(null)".into()
        },
        cg_font
    );
    cg_font
}

/// `CGFontRef CGFontCreateWithPlatformFont(void *platformFontReference)`
/// This is a deprecated API but some old apps still call it.
fn CGFontCreateWithPlatformFont(
    _env: &mut Environment,
    _platform_font_ref: MutVoidPtr,
) -> CGFontRef {
    log!("CGFontCreateWithPlatformFont: deprecated API, returning null");
    Ptr::null()
}

/// `int CGFontGetCapHeight(CGFontRef font)`
///
/// Returns the cap height in design units. Read from the 'OS/2' table if
/// available, otherwise approximated as 70% of ascent.
fn CGFontGetCapHeight(env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() {
        return 0;
    }
    if is_data_provider_font(env, font) {
        // Try OS/2 table: sCapHeight is at offset 88 (version >= 2)
        let os2_data = env
            .objc
            .borrow::<CGFontHostObject>(font)
            .font
            .table_data(0x4F532F32 /* 'OS/2' */);
        if let Some(data) = os2_data {
            if data.len() >= 90 {
                let version = u16::from_be_bytes([data[0], data[1]]);
                if version >= 2 {
                    let cap_height = i16::from_be_bytes([data[88], data[89]]);
                    if cap_height > 0 {
                        return cap_height as i32;
                    }
                }
            }
        }
        // Fallback: 70% of ascent
        return (CGFontGetAscent(env, font) as f32 * 0.70).round() as i32;
    }
    (CGFontGetAscent(env, font) as f32 * 0.70).round() as i32
}

/// `CGGlyph CGFontGetGlyphWithGlyphName(CGFontRef font, CFStringRef name)`
///
/// Looks up a glyph by its PostScript name using the 'post' table.
fn CGFontGetGlyphWithGlyphName(
    env: &mut Environment,
    font: CGFontRef,
    glyph_name: CFStringRef,
) -> CGGlyph {
    if font.is_null() || glyph_name == nil {
        return kCGFontIndexInvalid;
    }
    if !is_data_provider_font(env, font) {
        log_dbg!("CGFontGetGlyphWithGlyphName: non-data-provider font, returning invalid");
        return kCGFontIndexInvalid;
    }

    let name_str = ns_string::to_rust_string(env, glyph_name);
    let result = env
        .objc
        .borrow::<CGFontHostObject>(font)
        .font
        .glyph_for_name(&name_str);

    match result {
        Some(gid) => gid.0,
        None => kCGFontIndexInvalid,
    }
}

/// `CGFloat CGFontGetStemV(CGFontRef font)`
///
/// Returns the dominant vertical stem width. Read from OS/2 table or
/// approximated.
fn CGFontGetStemV(env: &mut Environment, font: CGFontRef) -> f32 {
    if font.is_null() {
        return 0.0;
    }
    // Approximate as ~12% of units-per-em
    let upm = CGFontGetUnitsPerEm(env, font) as f32;
    upm * 0.12
}

/// `CFTypeID CGFontGetTypeID(void)`
///
/// Returns the Core Foundation type identifier for CGFont objects.
fn CGFontGetTypeID(env: &mut Environment) -> u32 {
    let class = env.objc.get_known_class("_Podroid_CGFont", &mut env.mem);
    class.to_bits()
}

/// `int CGFontGetXHeight(CGFontRef font)`
///
/// Returns the x-height in design units. Read from the 'OS/2' table if
/// available, otherwise approximated as 50% of ascent.
fn CGFontGetXHeight(env: &mut Environment, font: CGFontRef) -> i32 {
    if font.is_null() {
        return 0;
    }
    if is_data_provider_font(env, font) {
        // Try OS/2 table: sxHeight is at offset 86 (version >= 2)
        let os2_data = env
            .objc
            .borrow::<CGFontHostObject>(font)
            .font
            .table_data(0x4F532F32 /* 'OS/2' */);
        if let Some(data) = os2_data {
            if data.len() >= 88 {
                let version = u16::from_be_bytes([data[0], data[1]]);
                if version >= 2 {
                    let x_height = i16::from_be_bytes([data[86], data[87]]);
                    if x_height > 0 {
                        return x_height as i32;
                    }
                }
            }
        }
        return (CGFontGetAscent(env, font) as f32 * 0.50).round() as i32;
    }
    (CGFontGetAscent(env, font) as f32 * 0.50).round() as i32
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGFontCreateWithDataProvider(_)),
    export_c_func!(CGFontRetain(_)),
    export_c_func!(CGFontRelease(_)),
    export_c_func!(CGFontGetGlyphsForUnichars(_, _, _, _)),
    export_c_func!(CGFontGetUnitsPerEm(_)),
    export_c_func!(CGFontGetAscent(_)),
    export_c_func!(CGFontGetDescent(_)),
    export_c_func!(CGFontGetLeading(_)),
    export_c_func!(CGFontGetFontBBox(_)),
    export_c_func!(CGFontGetGlyphAdvances(_, _, _, _)),
    export_c_func!(CGFontGetGlyphBBoxes(_, _, _, _)),
    export_c_func!(CGFontGetItalicAngle(_)),
    export_c_func!(CGFontCopyTableForTag(_, _)),
    export_c_func!(CGFontGetNumberOfGlyphs(_)),
    export_c_func!(CGFontCanCreatePostScriptSubset(_, _)),
    export_c_func!(CGFontCopyFullName(_)),
    export_c_func!(CGFontCopyGlyphNameForGlyph(_, _)),
    export_c_func!(CGFontCopyPostScriptName(_)),
    export_c_func!(CGFontCopyTableTags(_)),
    export_c_func!(CGFontCopyVariationAxes(_)),
    export_c_func!(CGFontCopyVariations(_)),
    export_c_func!(CGFontCreateCopyWithVariations(_, _)),
    export_c_func!(CGFontCreatePostScriptEncoding(_, _)),
    export_c_func!(CGFontCreatePostScriptSubset(_, _, _, _, _, _)),
    export_c_func!(CGFontCreateWithFontName(_)),
    export_c_func!(CGFontCreateWithPlatformFont(_)),
    export_c_func!(CGFontGetCapHeight(_)),
    export_c_func!(CGFontGetGlyphWithGlyphName(_, _)),
    export_c_func!(CGFontGetStemV(_)),
    export_c_func!(CGFontGetTypeID()),
    export_c_func!(CGFontGetXHeight(_)),
];
