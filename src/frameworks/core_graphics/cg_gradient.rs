/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGGradient.h`

use super::cg_color;
use super::cg_color_space::{
    kCGColorSpaceModelMonochrome, kCGColorSpaceModelRGB, CGColorSpaceGetModel, CGColorSpaceRef,
};
use super::CGFloat;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_array::CFArrayRef;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::NSUInteger;
use crate::mem::{ConstPtr, GuestUSize};
use crate::objc::{id, msg, objc_classes, ClassExports, HostObject};
use crate::Environment;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGGradient seems to be a CFType-based type, but in our implementation those
// are just Objective-C types, so we need a class for it, but its name is not
// visible anywhere.
@implementation _Podroid_CGGradient: NSObject
@end

};

pub type CGGradientRef = CFTypeRef;

pub type CGGradientDrawingOptions = u32;
pub const kCGGradientDrawsBeforeStartLocation: CGGradientDrawingOptions = 1 << 0;
pub const kCGGradientDrawsAfterEndLocation: CGGradientDrawingOptions = 1 << 1;

type Rgba = (CGFloat, CGFloat, CGFloat, CGFloat);

#[derive(Clone)]
pub(super) struct CGGradientHostObject {
    /// Color stops as (location, non-premultiplied sRGB color), sorted by
    /// location.
    stops: Vec<(CGFloat, Rgba)>,
}
impl HostObject for CGGradientHostObject {}

impl CGGradientHostObject {
    /// The color at `t`, where 0 is the start and 1 is the end. Colors before
    /// the first stop and after the last are those stops' colors.
    pub(super) fn color_at(&self, t: CGFloat) -> Rgba {
        let Some(&(first_location, first_color)) = self.stops.first() else {
            return (0.0, 0.0, 0.0, 0.0);
        };
        if t <= first_location {
            return first_color;
        }
        for pair in self.stops.windows(2) {
            let [(l0, c0), (l1, c1)] = [pair[0], pair[1]];
            if t <= l1 {
                let f = if l1 > l0 { (t - l0) / (l1 - l0) } else { 1.0 };
                let mix = |a: CGFloat, b: CGFloat| a + (b - a) * f;
                return (
                    mix(c0.0, c1.0),
                    mix(c0.1, c1.1),
                    mix(c0.2, c1.2),
                    mix(c0.3, c1.3),
                );
            }
        }
        self.stops.last().unwrap().1
    }
}

fn create(env: &mut Environment, colors: Vec<Rgba>, locations: ConstPtr<CGFloat>) -> CGGradientRef {
    let count = colors.len();
    let mut stops: Vec<(CGFloat, Rgba)> = colors
        .into_iter()
        .enumerate()
        .map(|(i, color)| {
            let location = if locations.is_null() {
                // Without locations, the colors are spread evenly.
                if count > 1 {
                    i as CGFloat / (count - 1) as CGFloat
                } else {
                    0.0
                }
            } else {
                env.mem.read(locations + i as GuestUSize)
            };
            (location, color)
        })
        .collect();
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    let isa = env
        .objc
        .get_known_class("_Podroid_CGGradient", &mut env.mem);
    env.objc
        .alloc_object(isa, Box::new(CGGradientHostObject { stops }), &mut env.mem)
}

fn CGGradientCreateWithColorComponents(
    env: &mut Environment,
    space: CGColorSpaceRef,
    components: ConstPtr<CGFloat>,
    locations: ConstPtr<CGFloat>,
    count: GuestUSize,
) -> CGGradientRef {
    let model = CGColorSpaceGetModel(env, space);
    let components_per_color = match model {
        kCGColorSpaceModelMonochrome => 2,
        kCGColorSpaceModelRGB => 4,
        _ => unimplemented!("CGGradient with color space model {}", model),
    };
    let colors = (0..count)
        .map(|i| {
            let c = |j: GuestUSize| env.mem.read(components + i * components_per_color + j);
            if components_per_color == 2 {
                (c(0), c(0), c(0), c(1))
            } else {
                (c(0), c(1), c(2), c(3))
            }
        })
        .collect();
    create(env, colors, locations)
}

fn CGGradientCreateWithColors(
    env: &mut Environment,
    _space: CGColorSpaceRef,
    colors: CFArrayRef,
    locations: ConstPtr<CGFloat>,
) -> CGGradientRef {
    let count: NSUInteger = msg![env; colors count];
    let colors = (0..count)
        .map(|i| {
            let color: id = msg![env; colors objectAtIndex:i];
            cg_color::to_rgba(&env.objc, color)
        })
        .collect();
    create(env, colors, locations)
}

fn CGGradientRetain(env: &mut Environment, gradient: CGGradientRef) -> CGGradientRef {
    if !gradient.is_null() {
        CFRetain(env, gradient);
    }
    gradient
}

fn CGGradientRelease(env: &mut Environment, gradient: CGGradientRef) {
    if !gradient.is_null() {
        CFRelease(env, gradient);
    }
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGGradientCreateWithColorComponents(_, _, _, _)),
    export_c_func!(CGGradientCreateWithColors(_, _, _)),
    export_c_func!(CGGradientRetain(_)),
    export_c_func!(CGGradientRelease(_)),
];
