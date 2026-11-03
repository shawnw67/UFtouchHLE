/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIBezierPath`, backed by Core Graphics path geometry.

use crate::frameworks::core_graphics::cg_context::{
    CGContextAddPath, CGContextClip, CGContextEOClip, CGContextEOFillPath, CGContextFillPath,
    CGContextRestoreGState, CGContextSaveGState, CGContextSetAlpha, CGContextSetBlendMode,
    CGContextSetLineWidth, CGContextStrokePath,
};
use crate::frameworks::core_graphics::cg_path::{self, Path};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect};
use crate::frameworks::uikit::ui_graphics::UIGraphicsGetCurrentContext;
use crate::objc::{
    autorelease, id, msg, objc_classes, release, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

struct UIBezierPathHostObject {
    path: Path,
    cached_path: Option<id>,
    line_width: CGFloat,
    even_odd: bool,
}
impl Default for UIBezierPathHostObject {
    fn default() -> Self {
        Self {
            path: Path::default(),
            cached_path: None,
            line_width: 1.0,
            even_odd: false,
        }
    }
}
impl HostObject for UIBezierPathHostObject {}

fn update_path(env: &mut Environment, this: id, change: impl FnOnce(&mut Path)) {
    let cached = {
        let object = env.objc.borrow_mut::<UIBezierPathHostObject>(this);
        change(&mut object.path);
        object.cached_path.take()
    };
    if let Some(cached) = cached {
        release(env, cached);
    }
}
fn add_rect(path: &mut Path, rect: CGRect) {
    let (x, y, w, h) = (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    );
    path.move_to(CGPoint { x, y });
    path.line_to(CGPoint { x: x + w, y });
    path.line_to(CGPoint { x: x + w, y: y + h });
    path.line_to(CGPoint { x, y: y + h });
    path.close();
}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation UIBezierPath: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<UIBezierPathHostObject>::default(), &mut env.mem)
}
+ (id)bezierPath {
    let path: id = msg![env; this new];
    autorelease(env, path)
}
+ (id)bezierPathWithRect:(CGRect)rect {
    let path: id = msg![env; this new];
    update_path(env, path, |value| add_rect(value, rect));
    autorelease(env, path)
}
+ (id)bezierPathWithOvalInRect:(CGRect)rect {
    let path: id = msg![env; this new];
    update_path(env, path, |value| {
        let (cx, cy) = (rect.origin.x + rect.size.width/2.0, rect.origin.y + rect.size.height/2.0);
        for i in 0..64 {
            let angle = std::f32::consts::TAU * i as CGFloat / 64.0;
            let point = CGPoint { x: cx + angle.cos()*rect.size.width/2.0, y: cy + angle.sin()*rect.size.height/2.0 };
            if i == 0 { value.move_to(point); } else { value.line_to(point); }
        }
        value.close();
    });
    autorelease(env, path)
}
+ (id)bezierPathWithCGPath:(id)cg_path {
    let path: id = msg![env; this new];
    if !cg_path.is_null() {
        let geometry = cg_path::copy_path(env, cg_path);
        update_path(env, path, |value| *value = geometry);
    }
    autorelease(env, path)
}
+ (id)bezierPathWithRoundedRect:(CGRect)rect cornerRadius:(CGFloat)radius {
    let path: id = msg![env; this new];
    update_path(env, path, |value| {
        let (x,y,w,h) = (rect.origin.x, rect.origin.y, rect.size.width, rect.size.height);
        let radius = radius.max(0.0).min(w.abs()/2.0).min(h.abs()/2.0);
        if radius == 0.0 { add_rect(value, rect); return; }
        value.move_to(CGPoint { x: x + radius, y });
        for (corner, next) in [
            (CGPoint {x:x+w,y}, CGPoint {x:x+w,y:y+h}),
            (CGPoint {x:x+w,y:y+h}, CGPoint {x,y:y+h}),
            (CGPoint {x,y:y+h}, CGPoint {x,y}),
            (CGPoint {x,y}, CGPoint {x:x+w,y}),
        ] {
            let start = value.current_point().unwrap();
            for point in cg_path::arc_to_point(start, corner, next, radius) { value.line_to(point); }
        }
        value.close();
    });
    autorelease(env, path)
}
- (())dealloc {
    let cached = env.objc.borrow::<UIBezierPathHostObject>(this).cached_path;
    if let Some(cached) = cached { release(env, cached); }
    env.objc.dealloc_object(this, &mut env.mem)
}
- (id)copyWithZone:(NSZonePtr)_zone {
    let object = env.objc.borrow::<UIBezierPathHostObject>(this);
    let path = object.path.clone();
    let width = object.line_width;
    let even_odd = object.even_odd;
    let class: id = msg![env; this class];
    let copy: id = msg![env; class new];
    let object = env.objc.borrow_mut::<UIBezierPathHostObject>(copy);
    object.path = path;
    object.line_width = width;
    object.even_odd = even_odd;
    copy
}
- (())moveToPoint:(CGPoint)point { update_path(env, this, |path| path.move_to(point)); }
- (())addLineToPoint:(CGPoint)point { update_path(env, this, |path| { cg_path::resume_closed_subpath(path); path.line_to(point); }); }
- (())addQuadCurveToPoint:(CGPoint)end controlPoint:(CGPoint)control {
    update_path(env, this, |path| {
        cg_path::resume_closed_subpath(path);
        let Some(start) = path.current_point() else { path.move_to(end); return; };
        for i in 1..=32 {
            let t = i as CGFloat / 32.0; let s = 1.0 - t;
            path.line_to(CGPoint { x:s*s*start.x + 2.0*s*t*control.x + t*t*end.x, y:s*s*start.y + 2.0*s*t*control.y + t*t*end.y });
        }
    });
}
- (())addCurveToPoint:(CGPoint)end controlPoint1:(CGPoint)a controlPoint2:(CGPoint)b {
    update_path(env, this, |path| {
        cg_path::resume_closed_subpath(path);
        let Some(start) = path.current_point() else { path.move_to(end); return; };
        for i in 1..=32 {
            let t = i as CGFloat / 32.0; let s = 1.0 - t;
            path.line_to(CGPoint { x:s*s*s*start.x + 3.0*s*s*t*a.x + 3.0*s*t*t*b.x + t*t*t*end.x, y:s*s*s*start.y + 3.0*s*s*t*a.y + 3.0*s*t*t*b.y + t*t*t*end.y });
        }
    });
}
- (())closePath { update_path(env, this, Path::close); }
- (())removeAllPoints { update_path(env, this, |path| *path = Path::default()); }
- (())appendPath:(id)other {
    let copy = env.objc.borrow::<UIBezierPathHostObject>(other).path.clone();
    update_path(env, this, |path| path.subpaths.extend(copy.subpaths));
}
- (())setLineWidth:(CGFloat)width { env.objc.borrow_mut::<UIBezierPathHostObject>(this).line_width = width; }
- (CGFloat)lineWidth { env.objc.borrow::<UIBezierPathHostObject>(this).line_width }
- (())setUsesEvenOddFillRule:(bool)value { env.objc.borrow_mut::<UIBezierPathHostObject>(this).even_odd = value; }
- (bool)usesEvenOddFillRule { env.objc.borrow::<UIBezierPathHostObject>(this).even_odd }
- (bool)isEmpty { env.objc.borrow::<UIBezierPathHostObject>(this).path.subpaths.is_empty() }
- (CGPoint)currentPoint {
    let object = env.objc.borrow::<UIBezierPathHostObject>(this);
    object.path.subpaths.last().and_then(|s| if s.closed { s.points.first().copied() } else { s.points.last().copied() }).unwrap_or_default()
}
- (bool)containsPoint:(CGPoint)point { let object = env.objc.borrow::<UIBezierPathHostObject>(this); object.path.contains(point, object.even_odd) }
- (CGRect)bounds {
    env.objc.borrow::<UIBezierPathHostObject>(this).path.bounds().unwrap_or_default()
}
- (id)CGPath {
    if let Some(cached) = env.objc.borrow::<UIBezierPathHostObject>(this).cached_path { return cached; }
    let path = env.objc.borrow::<UIBezierPathHostObject>(this).path.clone();
    let path = cg_path::alloc_path(env, path);
    env.objc.borrow_mut::<UIBezierPathHostObject>(this).cached_path = Some(path);
    path
}
- (())fill {
    let context = UIGraphicsGetCurrentContext(env);
    if context.is_null() { return; }
    let path: id = msg![env; this CGPath];
    let even_odd = env.objc.borrow::<UIBezierPathHostObject>(this).even_odd;
    CGContextAddPath(env, context, path);
    if even_odd { CGContextEOFillPath(env, context); } else { CGContextFillPath(env, context); }
}
- (())stroke {
    let context = UIGraphicsGetCurrentContext(env);
    if context.is_null() { return; }
    let width = env.objc.borrow::<UIBezierPathHostObject>(this).line_width;
    let path: id = msg![env; this CGPath];
    CGContextSaveGState(env, context);
    CGContextSetLineWidth(env, context, width);
    CGContextAddPath(env, context, path);
    CGContextStrokePath(env, context);
    CGContextRestoreGState(env, context);
}
- (())fillWithBlendMode:(i32)mode alpha:(CGFloat)alpha {
    let context = UIGraphicsGetCurrentContext(env);
    if context.is_null() { return; }
    CGContextSaveGState(env, context);
    CGContextSetBlendMode(env, context, mode);
    CGContextSetAlpha(env, context, alpha);
    let _: () = msg![env; this fill];
    CGContextRestoreGState(env, context);
}
- (())strokeWithBlendMode:(i32)mode alpha:(CGFloat)alpha {
    let context = UIGraphicsGetCurrentContext(env);
    if context.is_null() { return; }
    CGContextSaveGState(env, context);
    CGContextSetBlendMode(env, context, mode);
    CGContextSetAlpha(env, context, alpha);
    let _: () = msg![env; this stroke];
    CGContextRestoreGState(env, context);
}
- (())addClip {
    let context = UIGraphicsGetCurrentContext(env);
    if context.is_null() { return; }
    let path: id = msg![env; this CGPath];
    CGContextAddPath(env, context, path);
    if env.objc.borrow::<UIBezierPathHostObject>(this).even_odd {
        CGContextEOClip(env, context);
    } else {
        CGContextClip(env, context);
    }
}
@end
};
