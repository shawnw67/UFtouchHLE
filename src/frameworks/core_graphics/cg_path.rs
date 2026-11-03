/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Path geometry for `CGContext`'s current path and clipping.
//!
//! Paths are kept as polylines in device space: curves are flattened when
//! they're added, using the transform at that time, like Core Graphics does.

use super::{CGFloat, CGPoint, CGRect, CGSize};

/// A path as a list of subpaths, each a polyline. Subpaths are implicitly
/// closed when filling or clipping.
#[derive(Clone, Default, Debug, PartialEq)]
pub struct Path {
    pub subpaths: Vec<Subpath>,
}

#[derive(Clone, Default, Debug, PartialEq)]
pub struct Subpath {
    pub points: Vec<CGPoint>,
    pub closed: bool,
}

impl Path {
    pub fn current_point(&self) -> Option<CGPoint> {
        self.subpaths.last()?.points.last().copied()
    }

    pub fn move_to(&mut self, point: CGPoint) {
        self.subpaths.push(Subpath {
            points: vec![point],
            closed: false,
        });
    }

    pub fn line_to(&mut self, point: CGPoint) {
        match self.subpaths.last_mut() {
            Some(subpath) if !subpath.closed => subpath.points.push(point),
            _ => self.move_to(point),
        }
    }

    pub fn close(&mut self) {
        if let Some(subpath) = self.subpaths.last_mut() {
            subpath.closed = true;
        }
    }

    /// Bounding box of all points, if there are any.
    pub fn bounds(&self) -> Option<CGRect> {
        let mut points = self.subpaths.iter().flat_map(|s| s.points.iter());
        let first = points.next()?;
        let (mut x0, mut y0, mut x1, mut y1) = (first.x, first.y, first.x, first.y);
        for p in points {
            x0 = x0.min(p.x);
            y0 = y0.min(p.y);
            x1 = x1.max(p.x);
            y1 = y1.max(p.y);
        }
        Some(CGRect {
            origin: CGPoint { x: x0, y: y0 },
            size: CGSize {
                width: x1 - x0,
                height: y1 - y0,
            },
        })
    }

    /// Whether `p` is inside the path by the non-zero winding rule (or the
    /// even-odd rule if `even_odd`). Every subpath counts as closed.
    pub fn contains(&self, p: CGPoint, even_odd: bool) -> bool {
        let mut winding = 0i32;
        for subpath in &self.subpaths {
            let points = &subpath.points;
            for i in 0..points.len() {
                let a = points[i];
                let b = points[(i + 1) % points.len()];
                if a.y <= p.y {
                    if b.y > p.y && cross(a, b, p) > 0.0 {
                        winding += 1;
                    }
                } else if b.y <= p.y && cross(a, b, p) < 0.0 {
                    winding -= 1;
                }
            }
        }
        if even_odd {
            winding % 2 != 0
        } else {
            winding != 0
        }
    }

    /// The segments that stroking draws.
    pub fn stroke_segments(&self) -> impl Iterator<Item = (CGPoint, CGPoint)> + '_ {
        self.subpaths.iter().flat_map(|subpath| {
            let points = &subpath.points;
            let closing =
                (subpath.closed && points.len() > 2).then(|| (*points.last().unwrap(), points[0]));
            points
                .windows(2)
                .map(|pair| (pair[0], pair[1]))
                .chain(closing)
        })
    }
}

/// Which side of the line from `a` to `b` the point `p` is on.
fn cross(a: CGPoint, b: CGPoint, p: CGPoint) -> CGFloat {
    (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y)
}

/// Distance from `p` to the segment from `a` to `b`.
pub fn distance_to_segment(p: CGPoint, a: CGPoint, b: CGPoint) -> CGFloat {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.x + t * dx, a.y + t * dy);
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

/// The points `CGContextAddArcToPoint` adds after the current point `p0`: a
/// line to where the arc of `radius` tangent to `p0`-`p1` and `p1`-`p2`
/// starts, then the arc, flattened. All points are in the same space.
pub fn arc_to_point(p0: CGPoint, p1: CGPoint, p2: CGPoint, radius: CGFloat) -> Vec<CGPoint> {
    let (ux, uy) = (p0.x - p1.x, p0.y - p1.y);
    let (vx, vy) = (p2.x - p1.x, p2.y - p1.y);
    let (u_len, v_len) = ((ux * ux + uy * uy).sqrt(), (vx * vx + vy * vy).sqrt());
    if radius <= 0.0 || u_len == 0.0 || v_len == 0.0 {
        return vec![p1];
    }
    let (ux, uy, vx, vy) = (ux / u_len, uy / u_len, vx / v_len, vy / v_len);
    let cos_angle = (ux * vx + uy * vy).clamp(-1.0, 1.0);
    let angle = cos_angle.acos();
    if angle.abs() < 1e-4 || (std::f32::consts::PI - angle).abs() < 1e-4 {
        // Straight line: there's no arc.
        return vec![p1];
    }
    // Distance from the corner to the tangent points.
    let tangent_distance = radius / (angle / 2.0).tan();
    let t1 = CGPoint {
        x: p1.x + ux * tangent_distance,
        y: p1.y + uy * tangent_distance,
    };
    let t2 = CGPoint {
        x: p1.x + vx * tangent_distance,
        y: p1.y + vy * tangent_distance,
    };
    // The center is along the angle bisector.
    let (bx, by) = (ux + vx, uy + vy);
    let b_len = (bx * bx + by * by).sqrt();
    let center_distance = radius / (angle / 2.0).sin();
    let center = CGPoint {
        x: p1.x + bx / b_len * center_distance,
        y: p1.y + by / b_len * center_distance,
    };
    let start = (t1.y - center.y).atan2(t1.x - center.x);
    let mut sweep = (t2.y - center.y).atan2(t2.x - center.x) - start;
    // The arc is the short way round (less than half a turn).
    if sweep > std::f32::consts::PI {
        sweep -= 2.0 * std::f32::consts::PI;
    } else if sweep < -std::f32::consts::PI {
        sweep += 2.0 * std::f32::consts::PI;
    }
    let steps = ((sweep.abs() * radius.sqrt() * 2.0).ceil() as usize).clamp(2, 64);
    let mut points = vec![t1];
    for i in 1..=steps {
        let a = start + sweep * i as CGFloat / steps as CGFloat;
        points.push(CGPoint {
            x: center.x + radius * a.cos(),
            y: center.y + radius * a.sin(),
        });
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(x: CGFloat, y: CGFloat) -> CGPoint {
        CGPoint { x, y }
    }

    /// The rounded rectangle path hxGradationButton (MHP2G) draws.
    fn rounded_rect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> Path {
        let mut path = Path::default();
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        path.move_to(pt(x0, my));
        for (c, n) in [
            (pt(x0, y0), pt(mx, y0)),
            (pt(x1, y0), pt(x1, my)),
            (pt(x1, y1), pt(mx, y1)),
            (pt(x0, y1), pt(x0, my)),
        ] {
            let current = path.current_point().unwrap();
            for p in arc_to_point(current, c, n, r) {
                path.line_to(p);
            }
        }
        path
    }

    #[test]
    fn test_rounded_rect() {
        let path = rounded_rect(0.0, 0.0, 100.0, 40.0, 10.0);
        let bounds = path.bounds().unwrap();
        assert!((bounds.origin.x - 0.0).abs() < 0.01 && (bounds.size.width - 100.0).abs() < 0.01);
        assert!((bounds.origin.y - 0.0).abs() < 0.01 && (bounds.size.height - 40.0).abs() < 0.01);
        assert!(path.contains(pt(50.0, 20.0), false));
        assert!(path.contains(pt(5.0, 20.0), false));
        assert!(path.contains(pt(50.0, 1.0), false));
        // The corners are cut off.
        assert!(!path.contains(pt(0.5, 0.5), false));
        assert!(!path.contains(pt(99.5, 39.5), false));
        assert!(!path.contains(pt(150.0, 20.0), false));
    }

    #[test]
    fn test_distance_to_segment() {
        assert_eq!(
            distance_to_segment(pt(5.0, 3.0), pt(0.0, 0.0), pt(10.0, 0.0)),
            3.0
        );
        assert_eq!(
            distance_to_segment(pt(13.0, 4.0), pt(0.0, 0.0), pt(10.0, 0.0)),
            5.0
        );
    }
}

// Guest-visible CGPath objects use the same flattened geometry as CGContext.
use super::cg_affine_transform::CGAffineTransform;
use super::cg_geometry::{CGPointZero, CGRectNull};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::mem::ConstPtr;
use crate::objc::{objc_classes, ClassExports, HostObject};
use crate::Environment;

struct CGPathHostObject {
    path: Path,
}
impl HostObject for CGPathHostObject {}
pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);
    @implementation _Podroid_CGPath: NSObject
    - (())dealloc { env.objc.dealloc_object(this, &mut env.mem) }
    @end
};

fn alloc_path(env: &mut Environment, path: Path) -> CFTypeRef {
    let class = env.objc.get_known_class("_Podroid_CGPath", &mut env.mem);
    env.objc
        .alloc_object(class, Box::new(CGPathHostObject { path }), &mut env.mem)
}
pub(crate) fn copy_path(env: &mut Environment, path: CFTypeRef) -> Path {
    env.objc.borrow::<CGPathHostObject>(path).path.clone()
}
fn transform_point(
    env: &Environment,
    transform: ConstPtr<CGAffineTransform>,
    p: CGPoint,
) -> CGPoint {
    if transform.is_null() {
        return p;
    }
    let t = env.mem.read(transform);
    CGPoint {
        x: t.a * p.x + t.c * p.y + t.tx,
        y: t.b * p.x + t.d * p.y + t.ty,
    }
}
fn CGPathCreateMutable(env: &mut Environment) -> CFTypeRef {
    alloc_path(env, Path::default())
}
fn CGPathCreateCopy(env: &mut Environment, path: CFTypeRef) -> CFTypeRef {
    if path.is_null() {
        return path;
    }
    let value = copy_path(env, path);
    alloc_path(env, value)
}
fn CGPathCreateMutableCopy(env: &mut Environment, path: CFTypeRef) -> CFTypeRef {
    CGPathCreateCopy(env, path)
}
fn CGPathRetain(env: &mut Environment, path: CFTypeRef) -> CFTypeRef {
    if path.is_null() {
        path
    } else {
        CFRetain(env, path)
    }
}
fn CGPathRelease(env: &mut Environment, path: CFTypeRef) {
    if !path.is_null() {
        CFRelease(env, path);
    }
}
fn CGPathMoveToPoint(
    env: &mut Environment,
    path: CFTypeRef,
    transform: ConstPtr<CGAffineTransform>,
    x: CGFloat,
    y: CGFloat,
) {
    let point = transform_point(env, transform, CGPoint { x, y });
    env.objc
        .borrow_mut::<CGPathHostObject>(path)
        .path
        .move_to(point);
}
fn CGPathAddLineToPoint(
    env: &mut Environment,
    path: CFTypeRef,
    transform: ConstPtr<CGAffineTransform>,
    x: CGFloat,
    y: CGFloat,
) {
    let point = transform_point(env, transform, CGPoint { x, y });
    let path = &mut env.objc.borrow_mut::<CGPathHostObject>(path).path;
    resume_closed_subpath(path);
    path.line_to(point);
}

fn resume_closed_subpath(path: &mut Path) {
    if let Some(start) = path
        .subpaths
        .last()
        .filter(|s| s.closed)
        .and_then(|s| s.points.first())
        .copied()
    {
        path.move_to(start);
    }
}
fn CGPathCloseSubpath(env: &mut Environment, path: CFTypeRef) {
    env.objc.borrow_mut::<CGPathHostObject>(path).path.close();
}
fn CGPathAddRect(
    env: &mut Environment,
    path: CFTypeRef,
    transform: ConstPtr<CGAffineTransform>,
    rect: CGRect,
) {
    let (x, y, w, h) = (
        rect.origin.x,
        rect.origin.y,
        rect.size.width,
        rect.size.height,
    );
    CGPathMoveToPoint(env, path, transform, x, y);
    for (px, py) in [(x + w, y), (x + w, y + h), (x, y + h)] {
        CGPathAddLineToPoint(env, path, transform, px, py);
    }
    CGPathCloseSubpath(env, path);
}
fn CGPathCreateWithRect(
    env: &mut Environment,
    rect: CGRect,
    transform: ConstPtr<CGAffineTransform>,
) -> CFTypeRef {
    let path = CGPathCreateMutable(env);
    CGPathAddRect(env, path, transform, rect);
    path
}
fn CGPathIsEmpty(env: &mut Environment, path: CFTypeRef) -> bool {
    env.objc
        .borrow::<CGPathHostObject>(path)
        .path
        .subpaths
        .is_empty()
}
fn CGPathGetCurrentPoint(env: &mut Environment, path: CFTypeRef) -> CGPoint {
    let path = &env.objc.borrow::<CGPathHostObject>(path).path;
    path.subpaths
        .last()
        .and_then(|s| {
            if s.closed {
                s.points.first().copied()
            } else {
                s.points.last().copied()
            }
        })
        .unwrap_or(CGPointZero)
}
fn CGPathGetBoundingBox(env: &mut Environment, path: CFTypeRef) -> CGRect {
    env.objc
        .borrow::<CGPathHostObject>(path)
        .path
        .bounds()
        .unwrap_or(CGRectNull)
}
fn CGPathContainsPoint(
    env: &mut Environment,
    path: CFTypeRef,
    transform: ConstPtr<CGAffineTransform>,
    point: CGPoint,
    even_odd: bool,
) -> bool {
    let mut path = copy_path(env, path);
    for subpath in &mut path.subpaths {
        for p in &mut subpath.points {
            *p = transform_point(env, transform, *p);
        }
    }
    path.contains(point, even_odd)
}
fn CGPathAddQuadCurveToPoint(
    env: &mut Environment,
    path: CFTypeRef,
    transform: ConstPtr<CGAffineTransform>,
    cx: CGFloat,
    cy: CGFloat,
    x: CGFloat,
    y: CGFloat,
) {
    let control = transform_point(env, transform, CGPoint { x: cx, y: cy });
    let end = transform_point(env, transform, CGPoint { x, y });
    let value = &mut env.objc.borrow_mut::<CGPathHostObject>(path).path;
    resume_closed_subpath(value);
    let Some(start) = value.current_point() else {
        value.move_to(end);
        return;
    };
    for i in 1..=32 {
        let t = i as CGFloat / 32.0;
        let s = 1.0 - t;
        value.line_to(CGPoint {
            x: s * s * start.x + 2.0 * s * t * control.x + t * t * end.x,
            y: s * s * start.y + 2.0 * s * t * control.y + t * t * end.y,
        });
    }
}
#[allow(clippy::too_many_arguments)]
fn CGPathAddCurveToPoint(
    env: &mut Environment,
    path: CFTypeRef,
    transform: ConstPtr<CGAffineTransform>,
    x1: CGFloat,
    y1: CGFloat,
    x2: CGFloat,
    y2: CGFloat,
    x: CGFloat,
    y: CGFloat,
) {
    let a = transform_point(env, transform, CGPoint { x: x1, y: y1 });
    let b = transform_point(env, transform, CGPoint { x: x2, y: y2 });
    let end = transform_point(env, transform, CGPoint { x, y });
    let value = &mut env.objc.borrow_mut::<CGPathHostObject>(path).path;
    resume_closed_subpath(value);
    let Some(start) = value.current_point() else {
        value.move_to(end);
        return;
    };
    for i in 1..=32 {
        let t = i as CGFloat / 32.0;
        let s = 1.0 - t;
        value.line_to(CGPoint {
            x: s * s * s * start.x
                + 3.0 * s * s * t * a.x
                + 3.0 * s * t * t * b.x
                + t * t * t * end.x,
            y: s * s * s * start.y
                + 3.0 * s * s * t * a.y
                + 3.0 * s * t * t * b.y
                + t * t * t * end.y,
        });
    }
}
fn CGPathCreateWithEllipseInRect(
    env: &mut Environment,
    rect: CGRect,
    transform: ConstPtr<CGAffineTransform>,
) -> CFTypeRef {
    let path = CGPathCreateMutable(env);
    let (cx, cy) = (
        rect.origin.x + rect.size.width / 2.0,
        rect.origin.y + rect.size.height / 2.0,
    );
    for i in 0..64 {
        let angle = std::f32::consts::TAU * i as CGFloat / 64.0;
        let (x, y) = (
            cx + angle.cos() * rect.size.width / 2.0,
            cy + angle.sin() * rect.size.height / 2.0,
        );
        if i == 0 {
            CGPathMoveToPoint(env, path, transform, x, y);
        } else {
            CGPathAddLineToPoint(env, path, transform, x, y);
        }
    }
    CGPathCloseSubpath(env, path);
    path
}
fn CGPathAddPath(
    env: &mut Environment,
    destination: CFTypeRef,
    transform: ConstPtr<CGAffineTransform>,
    source: CFTypeRef,
) {
    let mut path = copy_path(env, source);
    for subpath in &mut path.subpaths {
        for point in &mut subpath.points {
            *point = transform_point(env, transform, *point);
        }
    }
    env.objc
        .borrow_mut::<CGPathHostObject>(destination)
        .path
        .subpaths
        .extend(path.subpaths);
}
fn CGPathEqualToPath(env: &mut Environment, first: CFTypeRef, second: CFTypeRef) -> bool {
    if first == second {
        return true;
    }
    if first.is_null() || second.is_null() {
        return false;
    }
    copy_path(env, first) == copy_path(env, second)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGPathAddQuadCurveToPoint(_, _, _, _, _, _)),
    export_c_func!(CGPathAddCurveToPoint(_, _, _, _, _, _, _, _)),
    export_c_func!(CGPathCreateWithEllipseInRect(_, _)),
    export_c_func!(CGPathAddPath(_, _, _)),
    export_c_func!(CGPathEqualToPath(_, _)),
    export_c_func!(CGPathCreateMutable()),
    export_c_func!(CGPathCreateCopy(_)),
    export_c_func!(CGPathCreateMutableCopy(_)),
    export_c_func!(CGPathRetain(_)),
    export_c_func!(CGPathRelease(_)),
    export_c_func!(CGPathMoveToPoint(_, _, _, _)),
    export_c_func!(CGPathAddLineToPoint(_, _, _, _)),
    export_c_func!(CGPathCloseSubpath(_)),
    export_c_func!(CGPathAddRect(_, _, _)),
    export_c_func!(CGPathCreateWithRect(_, _)),
    export_c_func!(CGPathIsEmpty(_)),
    export_c_func!(CGPathGetCurrentPoint(_)),
    export_c_func!(CGPathGetBoundingBox(_)),
    export_c_func!(CGPathContainsPoint(_, _, _, _)),
];
