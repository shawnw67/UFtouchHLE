/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Single-image ImageIO sources backed by the existing image decoder.

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant, HostDylib};
use crate::frameworks::core_foundation::cf_data::{CFDataGetBytePtr, CFDataGetLength};
use crate::frameworks::core_foundation::CFTypeRef;
use crate::frameworks::core_graphics::cg_image::{self, CGImageRef};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::image::Image;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject};
use crate::Environment;

struct ImageSource {
    image: Image,
}
impl HostObject for ImageSource {}
pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);
    @implementation _Podroid_CGImageSource: NSObject
    - (())dealloc { env.objc.dealloc_object(this, &mut env.mem) }
    @end
};

fn CGImageSourceCreateWithData(
    env: &mut Environment,
    data: CFTypeRef,
    _options: CFTypeRef,
) -> CFTypeRef {
    if data.is_null() {
        return nil;
    }
    let length = CFDataGetLength(env, data);
    if length <= 0 {
        return nil;
    }
    let ptr = CFDataGetBytePtr(env, data);
    if ptr.is_null() {
        return nil;
    }
    let Ok(image) = Image::from_bytes(env.mem.bytes_at(ptr, length as u32)) else {
        return nil;
    };
    let class = env
        .objc
        .get_known_class("_Podroid_CGImageSource", &mut env.mem);
    env.objc
        .alloc_object(class, Box::new(ImageSource { image }), &mut env.mem)
}
fn CGImageSourceCreateWithURL(
    env: &mut Environment,
    url: CFTypeRef,
    options: CFTypeRef,
) -> CFTypeRef {
    if url.is_null() {
        return nil;
    }
    let data: CFTypeRef = msg_class![env; NSData dataWithContentsOfURL:url];
    if data.is_null() {
        return nil;
    }
    CGImageSourceCreateWithData(env, data, options)
}
fn CGImageSourceGetCount(_env: &mut Environment, source: CFTypeRef) -> u32 {
    u32::from(!source.is_null())
}
fn CGImageSourceGetStatus(_env: &mut Environment, source: CFTypeRef) -> i32 {
    if source.is_null() {
        -4
    } else {
        0
    }
}
fn CGImageSourceGetStatusAtIndex(env: &mut Environment, source: CFTypeRef, index: u32) -> i32 {
    if index != 0 {
        -4
    } else {
        CGImageSourceGetStatus(env, source)
    }
}
fn CGImageSourceCreateImageAtIndex(
    env: &mut Environment,
    source: CFTypeRef,
    index: u32,
    _options: CFTypeRef,
) -> CGImageRef {
    if source.is_null() || index != 0 {
        return nil;
    }
    let image = env.objc.borrow::<ImageSource>(source).image.clone();
    cg_image::from_image(env, image)
}

fn CGImageSourceCopyPropertiesAtIndex(
    env: &mut Environment,
    source: CFTypeRef,
    index: u32,
    _options: CFTypeRef,
) -> CFTypeRef {
    if source.is_null() || index != 0 {
        return nil;
    }
    let (width, height) = env.objc.borrow::<ImageSource>(source).image.dimensions();
    let dictionary: id = msg_class![env; NSMutableDictionary new];
    let width_number: id = msg_class![env; NSNumber numberWithUnsignedInt:width];
    let height_number: id = msg_class![env; NSNumber numberWithUnsignedInt:height];
    let width_key = get_static_str(env, "PixelWidth");
    let height_key = get_static_str(env, "PixelHeight");
    let _: () = msg![env; dictionary setObject:width_number forKey:width_key];
    let _: () = msg![env; dictionary setObject:height_number forKey:height_key];
    dictionary
}

fn CGImageSourceCreateThumbnailAtIndex(
    env: &mut Environment,
    source: CFTypeRef,
    index: u32,
    options: CFTypeRef,
) -> CGImageRef {
    if source.is_null() || index != 0 || options.is_null() {
        return nil;
    }
    let always_key = get_static_str(env, "kCGImageSourceCreateThumbnailFromImageAlways");
    let absent_key = get_static_str(env, "kCGImageSourceCreateThumbnailFromImageIfAbsent");
    let max_key = get_static_str(env, "kCGImageSourceThumbnailMaxPixelSize");
    let always: id = msg![env; options objectForKey:always_key];
    let absent: id = msg![env; options objectForKey:absent_key];
    let allow_always: bool = msg![env; always boolValue];
    let allow_absent: bool = msg![env; absent boolValue];
    if !allow_always && !allow_absent {
        return nil;
    }
    let maximum: id = msg![env; options objectForKey:max_key];
    let maximum: i32 = if maximum.is_null() {
        0
    } else {
        msg![env; maximum intValue]
    };
    let (width, height) = env.objc.borrow::<ImageSource>(source).image.dimensions();
    let image = CGImageSourceCreateImageAtIndex(env, source, index, nil);
    if maximum <= 0 || width.max(height) <= maximum as u32 {
        return image;
    }
    let scale = maximum as f64 / width.max(height) as f64;
    let target_width = ((width as f64 * scale).round() as u32).max(1);
    let target_height = ((height as f64 * scale).round() as u32).max(1);
    let scaled = cg_image::CGImageCreateScaled(env, image, target_width, target_height);
    release(env, image);
    scaled
}

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCGImagePropertyPixelWidth",
        HostConstant::NSString("PixelWidth"),
    ),
    (
        "_kCGImagePropertyPixelHeight",
        HostConstant::NSString("PixelHeight"),
    ),
    (
        "_kCGImageSourceCreateThumbnailFromImageAlways",
        HostConstant::NSString("kCGImageSourceCreateThumbnailFromImageAlways"),
    ),
    (
        "_kCGImageSourceCreateThumbnailFromImageIfAbsent",
        HostConstant::NSString("kCGImageSourceCreateThumbnailFromImageIfAbsent"),
    ),
    (
        "_kCGImageSourceThumbnailMaxPixelSize",
        HostConstant::NSString("kCGImageSourceThumbnailMaxPixelSize"),
    ),
];
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGImageSourceCreateWithData(_, _)),
    export_c_func!(CGImageSourceCreateWithURL(_, _)),
    export_c_func!(CGImageSourceGetCount(_)),
    export_c_func!(CGImageSourceGetStatus(_)),
    export_c_func!(CGImageSourceGetStatusAtIndex(_, _)),
    export_c_func!(CGImageSourceCreateImageAtIndex(_, _, _)),
    export_c_func!(CGImageSourceCopyPropertiesAtIndex(_, _, _)),
    export_c_func!(CGImageSourceCreateThumbnailAtIndex(_, _, _)),
];
pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/ImageIO.framework/ImageIO",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};
