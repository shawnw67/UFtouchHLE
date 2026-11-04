/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIImage`.

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_graphics::cg_context::CGContextDrawImage;
use crate::frameworks::core_graphics::cg_image::{
    self, CGImageGetHeight, CGImageGetWidth, CGImageRef, CGImageRelease, CGImageRetain,
};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::{ns_data, ns_string, NSInteger};
use crate::frameworks::uikit::ui_graphics::UIGraphicsGetCurrentContext;
use crate::fs::GuestPath;
use crate::image::Image;
use crate::mem::MutVoidPtr;
use crate::objc::{
    autorelease, id, msg, msg_class, msg_send, nil, objc_classes, release, retain, ClassExports,
    HostObject, NSZonePtr, SEL,
};
use crate::Environment;
use std::collections::HashMap;

const CACHE_SIZE: usize = 60;

#[derive(Default)]
pub struct State {
    /// Cache of images for `[UIImage imageNamed:]` method.
    /// Images are explicitly retained.
    cached_images: HashMap<String, id>,
}
impl State {
    fn get(env: &Environment) -> &Self {
        &env.framework_state.uikit.ui_image
    }
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.framework_state.uikit.ui_image
    }
}

struct UIImageHostObject {
    cg_image: CGImageRef,
}
impl HostObject for UIImageHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIImage: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(UIImageHostObject { cg_image: nil });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)imageWithCGImage:(CGImageRef)cg_image {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCGImage:cg_image];
    autorelease(env, new)
}

+ (id)imageNamed:(id)name { // NSString*
    // TODO: figure out whether this is actually correct in all cases
    let bundle: id = msg_class![env; NSBundle mainBundle];
    let path: id = msg![env; bundle pathForResource:name ofType:nil];
    let name_str = ns_string::to_rust_string(env, name).to_string();
    if path == nil {
        log!("Warning: [UIImage imageNamed:{:?}] => nil", name_str);
        return nil;
    }
    // TODO: find a better eviction policy
    if State::get(env).cached_images.len() > CACHE_SIZE {
        let cache = std::mem::take(&mut State::get_mut(env).cached_images);
        log_dbg!("Evicting {} images from UIImage cache.", cache.len());
        for (_, img) in cache {
            release(env, img);
        }
    }
    if !State::get(env).cached_images.contains_key(&name_str) {
        let img = msg![env; this imageWithContentsOfFile:path];
        retain(env, img);
        State::get_mut(env).cached_images.insert(name_str.clone(), img);
    }
    *State::get(env).cached_images.get(&name_str).unwrap()
}

+ (id)imageWithContentsOfFile:(id)path { // NSString*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithContentsOfFile:path];
    autorelease(env, new)
}

+ (id)imageWithData:(id)data { // NSData*
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithData:data];
    autorelease(env, new)
}

- (())dealloc {
    let &UIImageHostObject { cg_image } = env.objc.borrow(this);
    CGImageRelease(env, cg_image);

    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)initWithCGImage:(CGImageRef)cg_image {
    CGImageRetain(env, cg_image);
    env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = cg_image;
    this
}

- (id)initWithContentsOfFile:(id)path { // NSString*
    if path == nil {
        return nil;
    }
    let path = ns_string::to_rust_string(env, path); // TODO: avoid copy
    let Ok(bytes) = env.fs.read(GuestPath::new(&path)) else {
        log!("Warning: couldn't read image file at {:?}, returning nil", path);
        release(env, this);
        return nil;
    };
    // TODO: Real error handling. For now, most errors are likely to be caused
    //       by a functionality gap in touchHLE, not the app actually trying to
    //       load a broken file, so panicking is most useful.
    let image = Image::from_bytes(&bytes).unwrap();
    let cg_image = cg_image::from_image(env, image);
    env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = cg_image;
    this
}

- (id)initWithData:(id)data { // NSData*
    let slice = ns_data::to_rust_slice(env, data);
    // TODO: refactor common parts
    let image = Image::from_bytes(slice).unwrap();
    let cg_image = cg_image::from_image(env, image);
    env.objc.borrow_mut::<UIImageHostObject>(this).cg_image = cg_image;
    this
}

- (id)stretchableImageWithLeftCapWidth:(NSInteger)_leftCapWidth
                          topCapHeight:(NSInteger)_topCapHeight {
    log!("TODO: properly support stretchableImageWithLeftCapWidth:topCapHeight:");
    retain(env, this)
}

// TODO: more init methods
// TODO: more accessors

- (CGImageRef)CGImage {
    env.objc.borrow::<UIImageHostObject>(this).cg_image
}

// TODO: should have UIImageOrientation type
- (NSInteger)imageOrientation {
    // FIXME: load image orientation info from file?
    0 // UIImageOrientationUp
}

- (CGSize)size {
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    let (width, height) = cg_image::borrow_image(&env.objc, image).dimensions();
    CGSize {
        width: width as _,
        height: height as _,
    }
}

- (CGFloat)scale {
    // TODO: support other scales, such as @2x
    1.0
}

- (())drawInRect:(CGRect)rect {
    let context = UIGraphicsGetCurrentContext(env);
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    CGContextDrawImage(env, context, rect, image);
}

- (())drawAtPoint:(CGPoint)point {
    let context = UIGraphicsGetCurrentContext(env);
    if context == nil {
        log!("Warning: [(UIImage*){:?} drawAtPoint:{:?}] is called with nil context, ignoring.", this, point);
        return;
    }
    let image = env.objc.borrow::<UIImageHostObject>(this).cg_image;
    let rect = CGRect {
        origin: point,
        size: CGSize {
            width: CGImageGetWidth(env, image) as CGFloat,
            height: CGImageGetHeight(env, image) as CGFloat,
        }
    };
    CGContextDrawImage(env, context, rect, image);
}

@end

// Undocumented class used in NIBs
// TODO: It's not clear _why_ placeholder is needed?
@implementation UIImageNibPlaceholder: UIImage

// NSCoding implementation
- (id)initWithCoder:(id)coder {
    release(env, this);

    // TODO: decode other attributes
    let key_ns_string = get_static_str(env, "UIResourceName");
    let resource_name: id = msg![env; coder decodeObjectForKey:key_ns_string];

    let res = msg_class![env; UIImage imageNamed:resource_name];
    // TODO: It is not clear if we need to additionally retain here?
    retain(env, res)
}

@end

};

/// `void UIImageWriteToSavedPhotosAlbum(UIImage *image, id completionTarget,
///     SEL completionSelector, void *contextInfo);`
///
/// Apple documentation:
/// <https://developer.apple.com/documentation/uikit/1619125-uiimagewritetosavedphotosalbum>
///
/// Adds the specified image to the user's Camera Roll album. In the emulator
/// we persist the PNG data into the app's sandboxed Documents directory (since
/// there is no real photo library). After saving, the optional completion
/// callback is invoked with a nil error to signal success.
///
/// The completion selector signature expected by Apple is:
/// `- (void)image:(UIImage *)image didFinishSavingWithError:(NSError *)error
///               contextInfo:(void *)contextInfo;`
fn UIImageWriteToSavedPhotosAlbum(
    env: &mut Environment,
    image: id,
    completion_target: id,
    completion_selector: SEL,
    context_info: MutVoidPtr,
) {
    if image == nil {
        log!("UIImageWriteToSavedPhotosAlbum: nil image, ignoring.");
        // Still invoke callback if requested, with nil error
        invoke_save_completion(
            env,
            completion_target,
            completion_selector,
            image,
            nil,
            context_info,
        );
        return;
    }

    // Get the CGImage backing the UIImage
    let cg_image_ref: CGImageRef = msg![env; image CGImage];

    let save_success = if !cg_image_ref.is_null() {
        let img = cg_image::borrow_image(&env.objc, cg_image_ref);
        let (w, h) = img.dimensions();
        let rgba = img.pixels();
        let stride = w as usize * 4;

        let mut png_data: Vec<u8> = Vec::new();
        let ctx_ptr: *mut std::ffi::c_void = (&mut png_data as *mut Vec<u8>).cast();
        let ok = img.write_png_image(ctx_ptr, w as i32, h as i32, rgba, stride as i32);

        if ok != 0 && !png_data.is_empty() {
            // Generate a unique filename based on timestamp
            let filename = format!(
                "SavedPhoto_{}.png",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0)
            );

            // Save to the app's Documents directory
            let docs_path = format!("/Documents/{}", filename);
            let guest_path = GuestPath::new(&docs_path);
            match env.fs.write(guest_path, &png_data) {
                Ok(()) => {
                    log!(
                        "UIImageWriteToSavedPhotosAlbum: saved {}x{} image to {}",
                        w,
                        h,
                        docs_path
                    );
                    true
                }
                Err(e) => {
                    log!(
                        "UIImageWriteToSavedPhotosAlbum: failed to write {}: {:?}",
                        docs_path,
                        e
                    );
                    false
                }
            }
        } else {
            log!("UIImageWriteToSavedPhotosAlbum: PNG encoding failed.");
            false
        }
    } else {
        log!("UIImageWriteToSavedPhotosAlbum: image has nil CGImage.");
        false
    };

    // Invoke the completion callback if provided.
    // We pass nil for the error on success and a generic NSError on failure
    // (most apps only check whether the error is nil).
    let error: id = if save_success {
        nil
    } else {
        let domain = ns_string::get_static_str(env, "UIImageWriteToSavedPhotosAlbumErrorDomain");
        let error: id = msg_class![env; NSError errorWithDomain:domain
                                                           code:(-1 as NSInteger)
                                                       userInfo:nil];
        error
    };
    invoke_save_completion(
        env,
        completion_target,
        completion_selector,
        image,
        error,
        context_info,
    );
}

fn UIImagePNGRepresentation(env: &mut Environment, image: id) -> id {
    if image == nil {
        return nil;
    }
    let cg_image: CGImageRef = msg![env; image CGImage];
    if cg_image.is_null() {
        return nil;
    }

    let img = cg_image::borrow_image(&env.objc, cg_image);
    let (w, h) = img.dimensions();
    let rgba = img.pixels();
    let stride = w as usize * 4;

    let mut png_data: Vec<u8> = Vec::new();
    let ctx_ptr: *mut std::ffi::c_void = (&mut png_data as *mut Vec<u8>).cast();
    let ok = img.write_png_image(ctx_ptr, w as i32, h as i32, rgba, stride as i32);
    if ok == 0 {
        return nil;
    }

    let len = png_data.len() as crate::frameworks::foundation::NSUInteger;
    let buf: crate::mem::MutVoidPtr = env.mem.alloc(len as u32).cast();
    env.mem
        .bytes_at_mut(buf.cast(), len as u32)
        .copy_from_slice(&png_data);
    msg_class![env; NSData dataWithBytesNoCopy:buf length:len]
}

fn UIImageJPEGRepresentation(
    env: &mut Environment,
    image: id,
    _compression_quality: CGFloat,
) -> id {
    // В эмуляторе пока фоллбек на PNG, если нет
    // JPEG энкодера
    UIImagePNGRepresentation(env, image)
}

/// Invokes the `UIImageWriteToSavedPhotosAlbum` completion callback. Per
/// Apple's documentation the selector must have this signature:
/// `- (void)image:(UIImage *)image didFinishSavingWithError:(NSError *)error
///               contextInfo:(void *)contextInfo;`
fn invoke_save_completion(
    env: &mut Environment,
    target: id,
    selector: SEL,
    image: id,
    error: id,
    context_info: MutVoidPtr,
) {
    if target == nil || selector.is_null() {
        return;
    }
    let _: () = msg_send(env, (target, selector, image, error, context_info));
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(UIImageWriteToSavedPhotosAlbum(_, _, _, _)),
    export_c_func!(UIImagePNGRepresentation(_)),
    export_c_func!(UIImageJPEGRepresentation(_, _)),
];
