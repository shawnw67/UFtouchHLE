/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIGraphics.h`

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_graphics::cg_bitmap_context::CGBitmapContextCreate;
use crate::frameworks::core_graphics::cg_bitmap_context::CGBitmapContextCreateImage;
use crate::frameworks::core_graphics::cg_context::{
    CGContextFillRect, CGContextRef, CGContextRelease, CGContextRetain,
};
use crate::frameworks::core_graphics::CGFloat;
use crate::frameworks::core_graphics::CGRect;
use crate::frameworks::core_graphics::CGSize;
use crate::mem::MutVoidPtr;
use crate::objc::id;
use crate::objc::msg_class;
use crate::objc::nil;
use crate::Environment;

#[derive(Default)]
pub(super) struct State {
    pub(super) context_stack: Vec<CGContextRef>,
}

pub fn UIGraphicsPushContext(env: &mut Environment, context: CGContextRef) {
    CGContextRetain(env, context);
    env.framework_state
        .uikit
        .ui_graphics
        .context_stack
        .push(context);
}
pub fn UIGraphicsPopContext(env: &mut Environment) {
    let context = env.framework_state.uikit.ui_graphics.context_stack.pop();
    CGContextRelease(env, context.unwrap());
}
pub fn UIGraphicsGetCurrentContext(env: &mut Environment) -> CGContextRef {
    env.framework_state
        .uikit
        .ui_graphics
        .context_stack
        .last()
        .copied()
        .unwrap_or(nil)
}

/// Fill a rectangle in the current context with its fill color.
fn UIRectFill(env: &mut Environment, rect: CGRect) {
    let context = UIGraphicsGetCurrentContext(env);
    if context != nil {
        CGContextFillRect(env, context, rect);
    }
}

fn UIGraphicsBeginImageContext(env: &mut Environment, size: CGSize) {
    UIGraphicsBeginImageContextWithOptions(env, size, false, 1.0);
}

/// Begin an offscreen bitmap context sized `size` at `scale`.
/// Pass `scale` = 0.0 to use the main screen scale (we treat that as 1.0).
/// `opaque` = true omits the alpha channel (we always use RGBA for simplicity).
fn UIGraphicsBeginImageContextWithOptions(
    env: &mut Environment,
    size: CGSize,
    _opaque: bool,
    scale: CGFloat,
) {
    let effective_scale = if scale == 0.0 { 1.0 } else { scale };
    let width = (size.width * effective_scale).ceil() as u32;
    let height = (size.height * effective_scale).ceil() as u32;

    if width == 0 || height == 0 {
        log!(
            "Warning: UIGraphicsBeginImageContextWithOptions: zero size ({}, {}), skipping",
            width,
            height
        );
        return;
    }

    let bytes_per_row = width * 4;
    let data: MutVoidPtr = env.mem.alloc(bytes_per_row * height).cast();

    // kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big = 0x00020008
    let context = CGBitmapContextCreate(
        env,
        data,
        width as _,
        height as _,
        8,
        bytes_per_row as _,
        nil, // device RGB
        0x0002_0008,
    );

    if context.is_null() {
        log!("Warning: UIGraphicsBeginImageContextWithOptions: failed to create context");
        return;
    }

    UIGraphicsPushContext(env, context);
    // CGBitmapContextCreate returns a +1 ref; we transferred ownership to the
    // stack via push (which also retains), so release the creation ref.
    CGContextRelease(env, context);
}

fn UIGraphicsBeginPDFContextToData(
    _env: &mut Environment,
    _data: id, // NSMutableData*
    _bounds: CGRect,
    _document_info: id, // NSDictionary*
) {
    log!("UIGraphicsBeginPDFContextToData: stubbed");
}

fn UIGraphicsBeginPDFContextToFile(
    _env: &mut Environment,
    _path: id, // NSString*
    _bounds: CGRect,
    _document_info: id, // NSDictionary*
) -> bool {
    log!("UIGraphicsBeginPDFContextToFile: stubbed, returning false");
    false
}

fn UIGraphicsBeginPDFPage(_env: &mut Environment) {
    log!("UIGraphicsBeginPDFPage: stubbed");
}

fn UIGraphicsBeginPDFPageWithInfo(
    _env: &mut Environment,
    _bounds: CGRect,
    _page_info: id, // NSDictionary*
) {
    log!("UIGraphicsBeginPDFPageWithInfo: stubbed");
}

fn UIGraphicsEndImageContext(env: &mut Environment) {
    if env
        .framework_state
        .uikit
        .ui_graphics
        .context_stack
        .is_empty()
    {
        log!("Warning: UIGraphicsEndImageContext called on empty stack");
        return;
    }
    UIGraphicsPopContext(env);
}

fn UIGraphicsEndPDFContext(_env: &mut Environment) {
    log!("UIGraphicsEndPDFContext: stubbed");
}

/// Snapshot the current image context as a UIImage, then pop it.
fn UIGraphicsGetImageFromCurrentImageContext(env: &mut Environment) -> id {
    let context = UIGraphicsGetCurrentContext(env);
    if context.is_null() {
        log!("Warning: UIGraphicsGetImageFromCurrentImageContext: no current context");
        return nil;
    }
    let cg_image = CGBitmapContextCreateImage(env, context);
    if cg_image.is_null() {
        log!("Warning: UIGraphicsGetImageFromCurrentImageContext: failed to create CGImage");
        return nil;
    }
    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    crate::frameworks::core_graphics::cg_image::CGImageRelease(env, cg_image);
    ui_image
}

fn UIGraphicsGetPDFContextBounds(_env: &mut Environment) -> CGRect {
    CGRect {
        origin: crate::frameworks::core_graphics::CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    }
}

fn UIRectClip(env: &mut Environment, rect: CGRect) {
    let ctx = UIGraphicsGetCurrentContext(env);
    if ctx.is_null() {
        return;
    }
    crate::frameworks::core_graphics::cg_context::CGContextClipToRect(env, ctx, rect);
}

fn UIRectFillUsingBlendMode(
    env: &mut Environment,
    rect: CGRect,
    _blend_mode: i32, // CGBlendMode
) {
    // Ignore blend mode — just fill.
    UIRectFill(env, rect);
}

fn UIRectFrame(env: &mut Environment, rect: CGRect) {
    let ctx = UIGraphicsGetCurrentContext(env);
    if ctx.is_null() {
        return;
    }
    crate::frameworks::core_graphics::cg_context::CGContextClearRect(env, ctx, rect);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(UIRectFill(_)),
    export_c_func!(UIGraphicsPushContext(_)),
    export_c_func!(UIGraphicsPopContext()),
    export_c_func!(UIGraphicsGetCurrentContext()),
    export_c_func!(UIGraphicsBeginImageContext(_)),
    export_c_func!(UIGraphicsBeginImageContextWithOptions(_, _, _)),
    export_c_func!(UIGraphicsBeginPDFContextToData(_, _, _)),
    export_c_func!(UIGraphicsBeginPDFContextToFile(_, _, _)),
    export_c_func!(UIGraphicsBeginPDFPage()),
    export_c_func!(UIGraphicsBeginPDFPageWithInfo(_, _)),
    export_c_func!(UIGraphicsEndImageContext()),
    export_c_func!(UIGraphicsEndPDFContext()),
    export_c_func!(UIGraphicsGetImageFromCurrentImageContext()),
    export_c_func!(UIGraphicsGetPDFContextBounds()),
    export_c_func!(UIRectClip(_)),
    export_c_func!(UIRectFillUsingBlendMode(_, _)),
    export_c_func!(UIRectFrame(_)),
];
