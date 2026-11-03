/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGImage.h`

use super::cg_color_space::{
    kCGColorSpaceGenericRGB, kCGColorSpaceModelRGB, CGColorSpaceCreateWithName,
    CGColorSpaceGetModel, CGColorSpaceRef,
};
use super::cg_data_provider::{self, CGDataProviderRef};
use super::cg_geometry::{CGPointZero, CGRectIntegral, CGRectIntersection, CGRectNull};
use super::{CGFloat, CGRect, CGSize};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::ns_string;
use crate::image::Image;
use crate::mem::{ConstPtr, GuestUSize, MutPtr};
use crate::objc::{autorelease, nil, objc_classes, ClassExports, HostObject, ObjC};
use crate::Environment;

pub type CGImageAlphaInfo = u32;
pub const kCGImageAlphaNone: CGImageAlphaInfo = 0;
pub const kCGImageAlphaPremultipliedLast: CGImageAlphaInfo = 1;
pub const kCGImageAlphaPremultipliedFirst: CGImageAlphaInfo = 2;
pub const kCGImageAlphaLast: CGImageAlphaInfo = 3;
pub const kCGImageAlphaFirst: CGImageAlphaInfo = 4;
pub const kCGImageAlphaNoneSkipLast: CGImageAlphaInfo = 5;
pub const kCGImageAlphaNoneSkipFirst: CGImageAlphaInfo = 6;
pub const kCGImageAlphaOnly: CGImageAlphaInfo = 7;

pub type CGImageByteOrderInfo = u32;
pub const kCGImageByteOrderMask: CGImageByteOrderInfo = 0x7000;
pub const kCGImageByteOrderDefault: CGImageByteOrderInfo = 0 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder16Little: CGImageByteOrderInfo = 1 << 12;
pub const kCGImageByteOrder32Little: CGImageByteOrderInfo = 2 << 12;
#[allow(dead_code)]
pub const kCGImageByteOrder16Big: CGImageByteOrderInfo = 3 << 12;
pub const kCGImageByteOrder32Big: CGImageByteOrderInfo = 4 << 12;

pub type CGBitmapInfo = u32;
pub const kCGBitmapAlphaInfoMask: CGBitmapInfo = 0x1F; // huh, it's not 0x7?
pub const kCGBitmapByteOrderMask: CGBitmapInfo = kCGImageByteOrderMask;
// TODO: other stuff in this enum (for now, always assert the rest is 0)

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGImage seems to be a CFType-based type, but in our implementation those
// are just Objective-C types, so we need a class for it, but its name is not
// visible anywhere.
@implementation _Podroid_CGImage: NSObject
@end

};

struct CGImageHostObject {
    image: Image,
    should_interpolate: bool,
    rendering_intent: i32,
}
impl HostObject for CGImageHostObject {}

pub type CGImageRef = CFTypeRef;
pub fn CGImageRelease(env: &mut Environment, c: CGImageRef) {
    if !c.is_null() {
        CFRelease(env, c);
    }
}
pub fn CGImageRetain(env: &mut Environment, c: CGImageRef) -> CGImageRef {
    if !c.is_null() {
        CFRetain(env, c)
    } else {
        c
    }
}

/// Shortcut for use by `UIImage`: directly construct a `CGImage` instance from
/// an [Image] instance.
pub fn from_image(env: &mut Environment, image: Image) -> CGImageRef {
    from_image_with_options(env, image, false, 0)
}

fn from_image_with_options(
    env: &mut Environment,
    image: Image,
    should_interpolate: bool,
    rendering_intent: i32,
) -> CGImageRef {
    let host_obj = Box::new(CGImageHostObject {
        image,
        should_interpolate,
        rendering_intent,
    });
    let class = env.objc.get_known_class("_Podroid_CGImage", &mut env.mem);
    env.objc.alloc_object(class, host_obj, &mut env.mem)
}

/// Shortcut for use by `CGBitmapContext` etc: borrow the [Image] from a
/// `CGImage` instance.
pub fn borrow_image(objc: &ObjC, image: CGImageRef) -> &Image {
    &objc.borrow::<CGImageHostObject>(image).image
}

/// Shortcut used by the app picker, counterpart to [borrow_image].
/// FIXME: This should not exist!
pub fn borrow_image_mut(objc: &mut ObjC, image: CGImageRef) -> &mut Image {
    &mut objc.borrow_mut::<CGImageHostObject>(image).image
}

// TODO: More create methods.

fn CGImageCreate(
    env: &mut Environment,
    width: GuestUSize,
    height: GuestUSize,
    bits_per_component: GuestUSize,
    bits_per_pixel: GuestUSize,
    bytes_per_row: GuestUSize,
    colorspace: CGColorSpaceRef,
    bitmap_info: CGBitmapInfo,
    provider: CGDataProviderRef,
    decode: ConstPtr<CGFloat>,
    should_interpolate: bool,
    intent: i32,
) -> CGImageRef {
    log_dbg!(
        "CGImageCreate w {}, h {}, bpc {}, bpp {}, bpr {}, bi {}",
        width,
        height,
        bits_per_component,
        bits_per_pixel,
        bytes_per_row,
        bitmap_info
    );
    assert!(decode.is_null()); // TODO
    assert_eq!(CGColorSpaceGetModel(env, colorspace), kCGColorSpaceModelRGB);
    assert_eq!(bits_per_component, 8);
    assert_eq!(bits_per_pixel, 32);
    assert_eq!(width as u64 * 4, bytes_per_row as u64);

    let byte_order = bitmap_info & kCGBitmapByteOrderMask;
    let alpha_info = bitmap_info & kCGBitmapAlphaInfoMask;
    assert_eq!(alpha_info | byte_order, bitmap_info); // TODO
    if alpha_info == kCGImageAlphaNone {
        // This 32-bit RGB layout has no alpha or skip component. CoreGraphics
        // rejects it. P2G attempts it for a snapshot after resuming and already
        // handles a null result; do not abort before that failure path runs.
        log_once!("CGImageCreate: unsupported RGB32 layout without alpha; returning null.");
        return nil;
    }

    let mut pixels = cg_data_provider::borrow_bytes(env, provider).to_vec();
    assert_eq!(pixels.len() as u64, width as u64 * height as u64 * 4);
    match byte_order {
        kCGImageByteOrderDefault | kCGImageByteOrder32Big => {
            assert_eq!(alpha_info, kCGImageAlphaPremultipliedLast); // TODO
        }
        kCGImageByteOrder32Little => {
            // TODO: fix CGImageGetAlphaInfo()
            assert_eq!(alpha_info, kCGImageAlphaNoneSkipFirst); // TODO
            for chunk in pixels.as_chunks_mut::<4>().0 {
                // XRGB in 32 little endian -> RGBX in 32 big endian
                chunk.swap(0, 2);
                // Assume opaque, even though it is undefined
                chunk[3] = 0xFF;
            }
        }
        _ => unimplemented!("{byte_order}"),
    }

    let image = Image::from_pixel_vec(pixels, (width, height));
    from_image_with_options(env, image, should_interpolate, intent)
}

fn CGImageCreateCopyWithColorSpace(
    env: &mut Environment,
    image: CGImageRef,
    color_space: CGColorSpaceRef,
) -> CGImageRef {
    let image_color_space = CGImageGetColorSpace(env, image);
    assert_eq!(
        CGColorSpaceGetModel(env, image_color_space),
        CGColorSpaceGetModel(env, color_space)
    );
    // If color space matches, we could just create a copy.
    CGImageCreateCopy(env, image)
}

fn CGImageCreateWithPNGDataProvider(
    env: &mut Environment,
    source: CGDataProviderRef,
    decode: ConstPtr<CGFloat>,
    should_interpolate: bool,
    intent: i32,
) -> CGImageRef {
    assert!(decode.is_null()); // TODO

    let bytes = cg_data_provider::borrow_bytes(env, source);
    let Ok(image) = Image::from_bytes(bytes) else {
        // Docs don't say what happens on failure, but this would make sense.
        return nil;
    };

    from_image_with_options(env, image, should_interpolate, intent)
}

fn CGImageCreateWithJPEGDataProvider(
    env: &mut Environment,
    source: CGDataProviderRef,
    decode: ConstPtr<CGFloat>,
    should_interpolate: bool,
    intent: i32,
) -> CGImageRef {
    assert!(decode.is_null());

    let bytes = cg_data_provider::borrow_bytes(env, source);
    let Ok(image) = Image::from_bytes(bytes) else {
        // Docs don't say what happens on failure, but this would make sense.
        return nil;
    };

    from_image_with_options(env, image, should_interpolate, intent)
}

fn CGImageGetAlphaInfo(_env: &mut Environment, _image: CGImageRef) -> CGImageAlphaInfo {
    // our Image type always returns premultiplied RGBA
    // (the premultiplied part must match what the real UIImage does, but
    // considering CgBI's design, maybe the order doesn't?)
    kCGImageAlphaPremultipliedLast
}

fn CGImageGetColorSpace(env: &mut Environment, _image: CGImageRef) -> CGColorSpaceRef {
    // Caller must release
    // FIXME: what if a loaded image is not sRGB?

    let srgb_name = ns_string::get_static_str(env, kCGColorSpaceGenericRGB);
    CGColorSpaceCreateWithName(env, srgb_name)
}

pub fn CGImageGetWidth(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    let (width, _height) = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .dimensions();
    width
}
pub fn CGImageGetHeight(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    let (_width, height) = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .dimensions();
    height
}
fn CGImageGetBitsPerPixel(_env: &mut Environment, _image: CGImageRef) -> GuestUSize {
    32
}
fn CGImageGetBytesPerRow(env: &mut Environment, image: CGImageRef) -> GuestUSize {
    let (width, _height) = env
        .objc
        .borrow::<CGImageHostObject>(image)
        .image
        .dimensions();
    width * 4
}

fn CGImageGetDataProvider(env: &mut Environment, image: CGImageRef) -> CGDataProviderRef {
    // CGImageGetDataProvider() seems to be intended to return the underlying
    // data provider that is retained by the CGImage. That's not how CGImage is
    // implemented here though, so instead we make a data provider that
    // retains the CGImage: exactly the opposite approach!
    let cg_data_provider = cg_data_provider::from_cg_image(env, image);
    // CGImageGetDataProvider() isn't meant to return a new object, so the
    // caller won't free this. The CGImage can't retain the CGDataProvider
    // without causing a cycle, so let's autorelease it instead.
    autorelease(env, cg_data_provider)
}

fn CGImageGetBitsPerComponent(_: &mut Environment, _: CGImageRef) -> GuestUSize {
    8 // Fix this when we support anything else
}

fn CGImageCreateWithImageInRect(
    env: &mut Environment,
    image: CGImageRef,
    rect: CGRect,
) -> CGImageRef {
    let (img_w, img_h) = borrow_image(&env.objc, image).dimensions();
    log_dbg!(
        "CGImageCreateWithImageInRect: rect {:?}, img dim {:?}",
        rect,
        (img_w, img_h)
    );
    let rect = CGRectIntegral(env, rect);
    let intersect = CGRectIntersection(
        env,
        rect,
        CGRect {
            origin: CGPointZero,
            size: CGSize {
                width: img_w as CGFloat,
                height: img_h as CGFloat,
            },
        },
    );
    assert!(intersect != CGRectNull);
    let (x, y, w, h) = (
        intersect.origin.x as u32,
        intersect.origin.y as u32,
        intersect.size.width as u32,
        intersect.size.height as u32,
    );
    assert_eq!(32, CGImageGetBitsPerPixel(env, image));
    let mut new_pixels = Vec::with_capacity((w * h * 4) as usize);
    let old_pixels = borrow_image(&env.objc, image).pixels();
    for i in 0..h {
        new_pixels.extend_from_slice(
            &old_pixels[((y + i) * img_w * 4 + x * 4) as usize..][..(w * 4) as usize],
        );
    }
    // Note: instead of keeping reference to the orig image,
    // we're creating a copy here
    let new_img = Image::from_pixel_vec(new_pixels, (w, h));
    let source = env.objc.borrow::<CGImageHostObject>(image);
    let (interpolate, intent) = (source.should_interpolate, source.rendering_intent);
    from_image_with_options(env, new_img, interpolate, intent)
}

fn CGImageCreateCopy(env: &mut Environment, image: CGImageRef) -> CGImageRef {
    if image.is_null() {
        return nil;
    }
    let source = env.objc.borrow::<CGImageHostObject>(image);
    let (copy, interpolate, intent) = (
        source.image.clone(),
        source.should_interpolate,
        source.rendering_intent,
    );
    from_image_with_options(env, copy, interpolate, intent)
}
fn CGImageGetBitmapInfo(_env: &mut Environment, image: CGImageRef) -> CGBitmapInfo {
    if image.is_null() {
        0
    } else {
        kCGImageAlphaPremultipliedLast | kCGImageByteOrder32Big
    }
}
fn CGImageGetDecode(_env: &mut Environment, _image: CGImageRef) -> ConstPtr<CGFloat> {
    ConstPtr::null()
}
fn CGImageGetShouldInterpolate(env: &mut Environment, image: CGImageRef) -> bool {
    !image.is_null()
        && env
            .objc
            .borrow::<CGImageHostObject>(image)
            .should_interpolate
}
fn CGImageGetRenderingIntent(env: &mut Environment, image: CGImageRef) -> i32 {
    if image.is_null() {
        0
    } else {
        env.objc.borrow::<CGImageHostObject>(image).rendering_intent
    }
}
fn CGImageIsMask(_env: &mut Environment, _image: CGImageRef) -> bool {
    false
}
fn CGImageGetPixelFormatInfo(_env: &mut Environment, _image: CGImageRef) -> u32 {
    0
}

// HyperHLE helper exports used by some applications for raw image conversion.
fn CGImageReadPixels(env: &mut Environment, image: CGImageRef, output: MutPtr<u8>) {
    if image.is_null() || output.is_null() {
        return;
    }
    let pixels = borrow_image(&env.objc, image).pixels();
    let len = u32::try_from(pixels.len()).expect("image exceeds guest address space");
    env.mem.bytes_at_mut(output, len).copy_from_slice(pixels);
}

fn CGImageCreateFlippedVertically(env: &mut Environment, image: CGImageRef) -> CGImageRef {
    if image.is_null() {
        return nil;
    }
    let source = env.objc.borrow::<CGImageHostObject>(image);
    let (width, height) = source.image.dimensions();
    let stride = width as usize * 4;
    let mut pixels = Vec::with_capacity(source.image.pixels().len());
    for row in (0..height as usize).rev() {
        pixels.extend_from_slice(&source.image.pixels()[row * stride..(row + 1) * stride]);
    }
    let (interpolate, intent) = (source.should_interpolate, source.rendering_intent);
    from_image_with_options(
        env,
        Image::from_pixel_vec(pixels, (width, height)),
        interpolate,
        intent,
    )
}

fn CGImageCreateScaled(
    env: &mut Environment,
    image: CGImageRef,
    width: u32,
    height: u32,
) -> CGImageRef {
    if image.is_null() || width == 0 || height == 0 {
        return nil;
    }
    let Some(bytes) = width.checked_mul(height).and_then(|n| n.checked_mul(4)) else {
        return nil;
    };
    let source = env.objc.borrow::<CGImageHostObject>(image);
    let (src_width, src_height) = source.image.dimensions();
    if src_width == 0 || src_height == 0 {
        return nil;
    }
    let mut pixels = Vec::new();
    if pixels.try_reserve_exact(bytes as usize).is_err() {
        return nil;
    }
    for y in 0..height {
        let sy = u64::from(y) * u64::from(src_height) / u64::from(height);
        for x in 0..width {
            let sx = u64::from(x) * u64::from(src_width) / u64::from(width);
            let offset = ((sy * u64::from(src_width) + sx) * 4) as usize;
            pixels.extend_from_slice(&source.image.pixels()[offset..offset + 4]);
        }
    }
    let (interpolate, intent) = (source.should_interpolate, source.rendering_intent);
    from_image_with_options(
        env,
        Image::from_pixel_vec(pixels, (width, height)),
        interpolate,
        intent,
    )
}
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGImageReadPixels(_, _)),
    export_c_func!(CGImageCreateFlippedVertically(_)),
    export_c_func!(CGImageCreateScaled(_, _, _)),
    export_c_func!(CGImageCreateCopy(_)),
    export_c_func!(CGImageGetBitmapInfo(_)),
    export_c_func!(CGImageGetDecode(_)),
    export_c_func!(CGImageGetShouldInterpolate(_)),
    export_c_func!(CGImageGetRenderingIntent(_)),
    export_c_func!(CGImageIsMask(_)),
    export_c_func!(CGImageGetPixelFormatInfo(_)),
    export_c_func!(CGImageRelease(_)),
    export_c_func!(CGImageRetain(_)),
    export_c_func!(CGImageCreate(_, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(CGImageCreateCopyWithColorSpace(_, _)),
    export_c_func!(CGImageCreateWithPNGDataProvider(_, _, _, _)),
    export_c_func!(CGImageCreateWithJPEGDataProvider(_, _, _, _)),
    export_c_func!(CGImageGetAlphaInfo(_)),
    export_c_func!(CGImageGetColorSpace(_)),
    export_c_func!(CGImageGetWidth(_)),
    export_c_func!(CGImageGetHeight(_)),
    export_c_func!(CGImageGetBitsPerPixel(_)),
    export_c_func!(CGImageGetBytesPerRow(_)),
    export_c_func!(CGImageGetDataProvider(_)),
    export_c_func!(CGImageGetBitsPerComponent(_)),
    export_c_func!(CGImageCreateWithImageInRect(_, _)),
];
