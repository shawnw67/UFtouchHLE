/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Device-space tessellation and point coverage for CGContext paths.
use super::cg_path::PathElement;
use super::CGPoint;

#[derive(Clone, Default)]
#[allow(dead_code)]
pub(super) struct Contour {
    pub points: Vec<CGPoint>,
    pub closed: bool,
}
#[allow(dead_code)]
fn mid(a: CGPoint, b: CGPoint) -> CGPoint {
    CGPoint {
        x: (a.x + b.x) * 0.5,
        y: (a.y + b.y) * 0.5,
    }
}
#[allow(dead_code)]
fn distance2(p: CGPoint, a: CGPoint, b: CGPoint) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
    };
    (p.x - a.x - t * dx).powi(2) + (p.y - a.y - t * dy).powi(2)
}
#[allow(dead_code)]
fn cubic(out: &mut Vec<CGPoint>, a: CGPoint, b: CGPoint, c: CGPoint, d: CGPoint, depth: u8) {
    if depth == 12 || (distance2(b, a, d).max(distance2(c, a, d)) <= 0.0625) {
        out.push(d);
        return;
    }
    let ab = mid(a, b);
    let bc = mid(b, c);
    let cd = mid(c, d);
    let abc = mid(ab, bc);
    let bcd = mid(bc, cd);
    let m = mid(abc, bcd);
    cubic(out, a, ab, abc, m, depth + 1);
    cubic(out, m, bcd, cd, d, depth + 1);
}
#[allow(dead_code)]
pub(super) fn flatten(elements: &[PathElement]) -> Vec<Contour> {
    let mut contours = Vec::new();
    let mut current = Contour::default();
    let mut point = CGPoint { x: 0.0, y: 0.0 };
    for element in elements {
        match *element {
            PathElement::MoveTo(p) => {
                if !current.points.is_empty() {
                    contours.push(std::mem::take(&mut current));
                }
                point = p;
                current.points.push(p);
            }
            PathElement::LineTo(p) => {
                if current.points.is_empty() {
                    current.points.push(point);
                }
                current.points.push(p);
                point = p;
            }
            PathElement::QuadCurveTo { control, to } => {
                if current.points.is_empty() {
                    current.points.push(point);
                }
                let b = CGPoint {
                    x: point.x + (control.x - point.x) * 2.0 / 3.0,
                    y: point.y + (control.y - point.y) * 2.0 / 3.0,
                };
                let c = CGPoint {
                    x: to.x + (control.x - to.x) * 2.0 / 3.0,
                    y: to.y + (control.y - to.y) * 2.0 / 3.0,
                };
                cubic(&mut current.points, point, b, c, to, 0);
                point = to;
            }
            PathElement::CurveTo { c1, c2, to } => {
                if current.points.is_empty() {
                    current.points.push(point);
                }
                cubic(&mut current.points, point, c1, c2, to, 0);
                point = to;
            }
            PathElement::Close => {
                if let Some(&first) = current.points.first() {
                    point = first;
                    current.closed = true;
                    contours.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if !current.points.is_empty() {
        contours.push(current);
    }
    contours
}
#[allow(dead_code)]
pub(super) fn contains(contours: &[Contour], p: CGPoint, even_odd: bool) -> bool {
    let mut winding = 0i32;
    for contour in contours {
        // Fill closes every subpath implicitly; stroke does not.
        let points = &contour.points;
        if points.len() < 3 {
            continue;
        }
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            let cross = (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y);
            if a.y <= p.y && b.y > p.y && cross > 0.0 {
                winding += 1;
            }
            if a.y > p.y && b.y <= p.y && cross < 0.0 {
                winding -= 1;
            }
        }
    }
    if even_odd {
        winding & 1 != 0
    } else {
        winding != 0
    }
}
#[allow(dead_code)]
fn triangle(p: CGPoint, a: CGPoint, b: CGPoint, c: CGPoint) -> bool {
    let cross = |a: CGPoint, b: CGPoint| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
    let (x, y, z) = (cross(a, b), cross(b, c), cross(c, a));
    (x >= 0.0 && y >= 0.0 && z >= 0.0) || (x <= 0.0 && y <= 0.0 && z <= 0.0)
}
#[allow(dead_code)]
pub(super) fn on_stroke(
    contours: &[Contour],
    p: CGPoint,
    width: f32,
    cap: i32,
    join: i32,
    miter_limit: f32,
) -> bool {
    let radius = width * 0.5;
    for contour in contours {
        let n = contour.points.len();
        if n < 2 {
            continue;
        }
        let segments = if contour.closed { n } else { n - 1 };
        for i in 0..segments {
            let a = contour.points[i];
            let b = contour.points[(i + 1) % n];
            let dx = b.x - a.x;
            let dy = b.y - a.y;
            let length = (dx * dx + dy * dy).sqrt();
            if length == 0.0 {
                continue;
            }
            let projection = ((p.x - a.x) * dx + (p.y - a.y) * dy) / length;
            let cross = ((p.x - a.x) * dy - (p.y - a.y) * dx).abs() / length;
            let square_start = !contour.closed && i == 0 && cap == 2;
            let square_end = !contour.closed && i + 1 == segments && cap == 2;
            if projection >= if square_start { -radius } else { 0.0 }
                && projection <= length + if square_end { radius } else { 0.0 }
                && cross <= radius
            {
                return true;
            }
            if cap == 1
                && !contour.closed
                && (i == 0 || i + 1 == segments)
                && distance2(p, a, b) <= radius * radius
            {
                return true;
            }
        }
        let start = if contour.closed { 0 } else { 1 };
        let end = if contour.closed { n } else { n - 1 };
        for i in start..end {
            let v = contour.points[i];
            let a = contour.points[(i + n - 1) % n];
            let b = contour.points[(i + 1) % n];
            let (ux, uy) = (v.x - a.x, v.y - a.y);
            let (vx, vy) = (b.x - v.x, b.y - v.y);
            let (ul, vl) = ((ux * ux + uy * uy).sqrt(), (vx * vx + vy * vy).sqrt());
            if ul == 0.0 || vl == 0.0 {
                continue;
            }
            let (ux, uy, vx, vy) = (ux / ul, uy / ul, vx / vl, vy / vl);
            let cross = ux * vy - uy * vx;
            if cross.abs() < 1e-6 {
                continue;
            }
            if join == 1 && (p.x - v.x).powi(2) + (p.y - v.y).powi(2) <= radius * radius {
                return true;
            }
            let sign = cross.signum();
            let a = CGPoint {
                x: v.x + uy * radius * sign,
                y: v.y - ux * radius * sign,
            };
            let b = CGPoint {
                x: v.x + vy * radius * sign,
                y: v.y - vx * radius * sign,
            };
            if triangle(p, v, a, b) {
                return true;
            }
            if join == 0 {
                let denom = 1.0 + ux * vx + uy * vy;
                if denom <= 0.0 {
                    continue;
                }
                let m = CGPoint {
                    x: v.x + (uy + vy) * radius * sign / denom,
                    y: v.y - (ux + vx) * radius * sign / denom,
                };
                if (m.x - v.x).powi(2) + (m.y - v.y).powi(2)
                    <= radius * radius * miter_limit * miter_limit
                    && triangle(p, a, m, b)
                {
                    return true;
                }
            }
        }
    }
    false
}
#[cfg(test)]
mod tests {
    use super::*;
    fn p(x: f32, y: f32) -> CGPoint {
        CGPoint { x, y }
    }
    #[test]
    fn triangle_is_not_its_bounding_box() {
        let c = flatten(&[
            PathElement::MoveTo(p(0.0, 0.0)),
            PathElement::LineTo(p(10.0, 0.0)),
            PathElement::LineTo(p(0.0, 10.0)),
            PathElement::Close,
        ]);
        assert!(contains(&c, p(1.0, 1.0), false));
        assert!(!contains(&c, p(9.0, 9.0), false));
    }
    #[test]
    fn subpaths_and_even_odd_holes() {
        let mut e = Vec::new();
        for (lo, hi) in [(0.0, 10.0), (2.0, 8.0)] {
            e.extend([
                PathElement::MoveTo(p(lo, lo)),
                PathElement::LineTo(p(hi, lo)),
                PathElement::LineTo(p(hi, hi)),
                PathElement::LineTo(p(lo, hi)),
                PathElement::Close,
            ]);
        }
        let c = flatten(&e);
        assert_eq!(c.len(), 2);
        assert!(contains(&c, p(5.0, 5.0), false));
        assert!(!contains(&c, p(5.0, 5.0), true));
    }
    #[test]
    fn curves_are_tessellated() {
        let c = flatten(&[
            PathElement::MoveTo(p(0.0, 0.0)),
            PathElement::QuadCurveTo {
                control: p(5.0, 10.0),
                to: p(10.0, 0.0),
            },
        ]);
        assert!(c[0].points.len() > 3);
        assert!(c[0].points.iter().any(|p| p.y > 4.0));
    }
    #[test]
    fn stroke_caps_and_miter_limit() {
        let line = flatten(&[
            PathElement::MoveTo(p(0.0, 0.0)),
            PathElement::LineTo(p(10.0, 0.0)),
        ]);
        assert!(!on_stroke(&line, p(-0.4, 0.0), 1.0, 0, 0, 10.0));
        assert!(on_stroke(&line, p(-0.4, 0.0), 1.0, 1, 0, 10.0));
        assert!(!on_stroke(&line, p(-0.4, 0.4), 1.0, 1, 0, 10.0));
        assert!(on_stroke(&line, p(-0.4, 0.4), 1.0, 2, 0, 10.0));
        let corner = flatten(&[
            PathElement::MoveTo(p(0.0, 0.0)),
            PathElement::LineTo(p(10.0, 0.0)),
            PathElement::LineTo(p(10.0, 10.0)),
        ]);
        assert!(on_stroke(&corner, p(10.8, -0.8), 2.0, 0, 0, 10.0));
        assert!(!on_stroke(&corner, p(10.8, -0.8), 2.0, 0, 0, 1.0));
        assert!(!on_stroke(&corner, p(10.8, -0.8), 2.0, 0, 1, 10.0));
        assert!(!on_stroke(&corner, p(10.8, -0.8), 2.0, 0, 2, 10.0));
    }
}
