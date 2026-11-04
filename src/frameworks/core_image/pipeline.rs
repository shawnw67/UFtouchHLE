/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Finite-image software Core Image pipeline. Pixels are linear-light,
//! premultiplied RGBA. Unknown filters fail instead of passing input through.
use std::collections::HashMap;
use std::sync::Arc;

use crate::frameworks::core_graphics::{cg_image, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_string::to_rust_string;
use crate::frameworks::foundation::NSUInteger;
use crate::image::{gamma_decode, gamma_encode, Image};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use crate::Environment;

#[derive(Clone, Default)]
struct Raster {
    width: usize,
    height: usize,
    x: i32,
    y: i32,
    // Bottom-up rows, matching Core Image's coordinate system.
    pixels: Vec<[f32; 4]>,
}
fn pixel_count(w: usize, h: usize) -> Option<usize> {
    w.checked_mul(h).filter(|&n| n <= 16 * 1024 * 1024)
}
impl Raster {
    fn extent(&self) -> CGRect {
        CGRect {
            origin: CGPoint {
                x: self.x as f32,
                y: self.y as f32,
            },
            size: CGSize {
                width: self.width as f32,
                height: self.height as f32,
            },
        }
    }
    fn sample(&self, x: i64, y: i64) -> [f32; 4] {
        let x = x - i64::from(self.x);
        let y = y - i64::from(self.y);
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return [0.0; 4];
        }
        self.pixels[y as usize * self.width + x as usize]
    }
    fn from_image(image: &Image) -> Option<Self> {
        let (w, h) = image.dimensions();
        let (w, h) = (w as usize, h as usize);
        let mut pixels = vec![[0.0; 4]; pixel_count(w, h)?];
        for y in 0..h {
            for x in 0..w {
                let i = ((h - 1 - y) * w + x) * 4;
                let bytes = &image.pixels()[i..i + 4];
                let a = bytes[3] as f32 / 255.0;
                let mut p = [0.0, 0.0, 0.0, a];
                if a > 0.0 {
                    for c in 0..3 {
                        p[c] = gamma_decode((bytes[c] as f32 / 255.0 / a).clamp(0.0, 1.0)) * a;
                    }
                }
                pixels[y * w + x] = p;
            }
        }
        Some(Self {
            width: w,
            height: h,
            pixels,
            x: 0,
            y: 0,
        })
    }
    fn crop_image(&self, rect: CGRect) -> Option<Image> {
        if ![
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height,
        ]
        .iter()
        .all(|v| v.is_finite())
            || rect.size.width <= 0.0
            || rect.size.height <= 0.0
        {
            return None;
        }
        let x0 = rect.origin.x.floor() as i64;
        let y0 = rect.origin.y.floor() as i64;
        let x1 = (rect.origin.x as f64 + rect.size.width as f64).ceil() as i64;
        let y1 = (rect.origin.y as f64 + rect.size.height as f64).ceil() as i64;
        let w = usize::try_from(x1.checked_sub(x0)?).ok()?;
        let h = usize::try_from(y1.checked_sub(y0)?).ok()?;
        let n = pixel_count(w, h)?;
        let mut bytes = vec![0; n * 4];
        for y in 0..h {
            for x in 0..w {
                let p = self.sample(x0 + x as i64, y0 + y as i64);
                let i = ((h - 1 - y) * w + x) * 4;
                let a = p[3].clamp(0.0, 1.0);
                if a > 0.0 {
                    for c in 0..3 {
                        bytes[i + c] =
                            (gamma_encode((p[c] / a).clamp(0.0, 1.0)) * a * 255.0).round() as u8;
                    }
                }
                bytes[i + 3] = (a * 255.0).round() as u8;
            }
        }
        Some(Image::from_pixel_vec(
            bytes,
            (w.try_into().ok()?, h.try_into().ok()?),
        ))
    }
}

#[derive(Default)]
struct CIImageObject {
    raster: Arc<Raster>,
}
impl HostObject for CIImageObject {}
#[derive(Default)]
struct CIContextObject {
    options: id,
}
impl HostObject for CIContextObject {}
#[derive(Default)]
struct CIFilterObject {
    name: String,
    inputs: HashMap<String, id>,
}
impl HostObject for CIFilterObject {}

fn image_object(env: &mut Environment, raster: Raster) -> id {
    let class = env.objc.get_known_class("CIImage", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(CIImageObject {
            raster: Arc::new(raster),
        }),
        &mut env.mem,
    )
}
fn supported(name: &str) -> bool {
    matches!(
        name,
        "CIColorControls"
            | "CIColorInvert"
            | "CIColorMonochrome"
            | "CISepiaTone"
            | "CIGaussianBlur"
    )
}
fn make_filter(env: &mut Environment, name: id) -> id {
    if name == nil {
        return nil;
    }
    let name = to_rust_string(env, name).into_owned();
    if !supported(&name) {
        log!(
            "Core Image: unsupported filter {:?}; returning nil (not an identity filter)",
            name
        );
        return nil;
    }
    let class = env.objc.get_known_class("CIFilter", &mut env.mem);
    let filter = env.objc.alloc_object(
        class,
        Box::new(CIFilterObject {
            name,
            inputs: HashMap::new(),
        }),
        &mut env.mem,
    );
    autorelease(env, filter)
}
fn number(env: &mut Environment, inputs: &HashMap<String, id>, key: &str, default: f32) -> f32 {
    match inputs.get(key) {
        Some(&value) => msg![env; value floatValue],
        None => default,
    }
}
fn clear_inputs(env: &mut Environment, this: id) {
    let inputs = std::mem::take(&mut env.objc.borrow_mut::<CIFilterObject>(this).inputs);
    for (_, value) in inputs {
        release(env, value);
    }
}

/// Colour operations act on straight linear RGB and preserve alpha.
fn color_filter(
    raster: &mut Raster,
    name: &str,
    saturation: f32,
    brightness: f32,
    contrast: f32,
    intensity: f32,
    tint: [f32; 3],
) {
    for p in &mut raster.pixels {
        let a = p[3];
        if a <= 0.0 {
            continue;
        }
        let rgb = [p[0] / a, p[1] / a, p[2] / a];
        let luma = rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722;
        let sepia = [
            0.393 * rgb[0] + 0.769 * rgb[1] + 0.189 * rgb[2],
            0.349 * rgb[0] + 0.686 * rgb[1] + 0.168 * rgb[2],
            0.272 * rgb[0] + 0.534 * rgb[1] + 0.131 * rgb[2],
        ];
        for c in 0..3 {
            let result = match name {
                "CIColorInvert" => 1.0 - rgb[c],
                "CIColorControls" => {
                    ((luma + saturation * (rgb[c] - luma)) - 0.5) * contrast + 0.5 + brightness
                }
                "CISepiaTone" => rgb[c] + intensity * (sepia[c] - rgb[c]),
                "CIColorMonochrome" => rgb[c] + intensity * (luma * tint[c] - rgb[c]),
                _ => unreachable!(),
            };
            p[c] = result * a;
        }
    }
}

/// Separable Gaussian convolution, transparent pixels outside the input.
/// Expand extent by three standard deviations; do not crop blur to input size.
fn gaussian(input: &Raster, radius: f32) -> Option<Raster> {
    if !radius.is_finite() || radius < 0.0 {
        return None;
    }
    if radius == 0.0 {
        return Some(input.clone());
    }
    // Refuse unreasonable workloads explicitly rather than silently changing
    // radius.
    let padding = (radius * 3.0).ceil() as usize;
    if padding > 1024 {
        return None;
    }
    let w = input.width.checked_add(padding * 2)?;
    let h = input.height.checked_add(padding * 2)?;
    let n = pixel_count(w, h)?;
    if n.checked_mul(padding * 2 + 1)? > 200_000_000 {
        return None;
    }
    let x = input.x.checked_sub(padding as i32)?;
    let y = input.y.checked_sub(padding as i32)?;
    let mut weights: Vec<f32> = (0..=padding * 2)
        .map(|i| {
            let d = i as f64 - padding as f64;
            (-0.5 * (d / radius as f64).powi(2)).exp() as f32
        })
        .collect();
    let sum: f32 = weights.iter().sum();
    for v in &mut weights {
        *v /= sum;
    }
    let mut horizontal = vec![[0.0f32; 4]; n];
    for row in 0..h {
        for col in 0..w {
            let p = &mut horizontal[row * w + col];
            for (k, &weight) in weights.iter().enumerate() {
                let src = input.sample(
                    x as i64 + col as i64 + k as i64 - padding as i64,
                    y as i64 + row as i64,
                );
                for c in 0..4 {
                    p[c] += src[c] * weight;
                }
            }
        }
    }
    let mut pixels = vec![[0.0f32; 4]; n];
    for row in 0..h {
        for col in 0..w {
            for (k, &weight) in weights.iter().enumerate() {
                let sy = row as isize + k as isize - padding as isize;
                if sy < 0 || sy >= h as isize {
                    continue;
                }
                for c in 0..4 {
                    pixels[row * w + col][c] += horizontal[sy as usize * w + col][c] * weight;
                }
            }
        }
    }
    Some(Raster {
        width: w,
        height: h,
        x,
        y,
        pixels,
    })
}

fn output_image(env: &mut Environment, this: id) -> id {
    let host = env.objc.borrow::<CIFilterObject>(this);
    let name = host.name.clone();
    let inputs = host.inputs.clone();
    let Some(&image) = inputs.get("inputImage") else {
        return nil;
    };
    let source = env.objc.borrow::<CIImageObject>(image).raster.clone();
    let output = if name == "CIGaussianBlur" {
        let radius = number(env, &inputs, "inputRadius", 10.0);
        let Some(output) = gaussian(&source, radius) else {
            log!("Core Image: Gaussian radius/extent exceeds software renderer limits");
            return nil;
        };
        output
    } else {
        let saturation = number(env, &inputs, "inputSaturation", 1.0);
        let brightness = number(env, &inputs, "inputBrightness", 0.0);
        let contrast = number(env, &inputs, "inputContrast", 1.0);
        let intensity = number(env, &inputs, "inputIntensity", 1.0).clamp(0.0, 1.0);
        let tint = inputs
            .get("inputColor")
            .map(|&color| {
                let (r, g, b, _) = super::ci_color_components(env, color);
                [gamma_decode(r), gamma_decode(g), gamma_decode(b)]
            })
            .unwrap_or([gamma_decode(0.6), gamma_decode(0.45), gamma_decode(0.3)]);
        let mut output = (*source).clone();
        color_filter(
            &mut output,
            &name,
            saturation,
            brightness,
            contrast,
            intensity,
            tint,
        );
        output
    };
    let result = image_object(env, output);
    autorelease(env, result)
}

// This renderer has a fixed linear-RGB working space and premultiplied
// sRGB output. Reject requests we cannot honour rather than ignore them.
fn context_options_supported(env: &mut Environment, options: id) -> bool {
    if options == nil {
        return true;
    }
    let keys: id = msg![env; options allKeys];
    let count: NSUInteger = msg![env; keys count];
    for i in 0..count {
        let key: id = msg![env; keys objectAtIndex:i];
        let value: id = msg![env; options objectForKey:key];
        let name = to_rust_string(env, key).into_owned();
        match name.as_str() {
            // Software rendering is available even without a GPU context.
            "kCIContextUseSoftwareRenderer" => {}
            "kCIContextOutputPremultiplied" => {
                let enabled: bool = msg![env; value boolValue];
                if enabled {
                    continue;
                }
                log!("Core Image: unpremultiplied output is not supported");
                return false;
            }
            _ => {
                log!(
                    "Core Image: unsupported context option {:?}; returning nil",
                    name
                );
                return false;
            }
        }
    }
    true
}

pub const CLASSES: ClassExports = objc_classes! {
(env, this, _cmd);
@implementation CIImage: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<CIImageObject>::default(), &mut env.mem)
}
+ (id)imageWithCGImage:(cg_image::CGImageRef)image {
    if image == nil { return nil; }
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithCGImage:image];
    autorelease(env, new)
}
- (id)initWithCGImage:(cg_image::CGImageRef)image {
    if image == nil { release(env, this); return nil; }
    let Some(raster) = Raster::from_image(cg_image::borrow_image(&env.objc, image)) else {
        release(env, this); return nil;
    };
    env.objc.borrow_mut::<CIImageObject>(this).raster = Arc::new(raster);
    this
}
- (CGRect)extent { env.objc.borrow::<CIImageObject>(this).raster.extent() }
- (id)copyWithZone:(NSZonePtr)_zone { retain(env, this) }
- (())dealloc { env.objc.dealloc_object(this, &mut env.mem) }
@end
@implementation CIFilter: NSObject
+ (id)filterWithName:(id)name { make_filter(env, name) }
+ (id)filterWithName:(id)name keysAndValues:(id)first_key, ...args {
    let filter = make_filter(env, name);
    if filter == nil { return nil; }
    let mut key = first_key;
    let mut args = args.start();
    while key != nil {
        let value: id = args.next(env);
        () = msg![env; filter setValue:value forKey:key];
        key = args.next(env);
    }
    filter
}
- (())setValue:(id)value forKey:(id)key {
    if key == nil { return; }
    let key = to_rust_string(env, key).into_owned();
    // Own inputs: autorelease-pool draining must not invalidate the graph.
    retain(env, value);
    let inputs = &mut env.objc.borrow_mut::<CIFilterObject>(this).inputs;
    let old = if value == nil { inputs.remove(&key) } else { inputs.insert(key, value) };
    if let Some(old) = old { release(env, old); }
}
- (id)valueForKey:(id)key {
    let key = to_rust_string(env, key).into_owned();
    if key == "outputImage" { return output_image(env, this); }
    if let Some(&value) = env.objc.borrow::<CIFilterObject>(this).inputs.get(&key) { return value; }
    let default = match key.as_str() {
        "inputRadius" => 10.0f32,
        "inputSaturation" | "inputContrast" | "inputIntensity" => 1.0f32,
        "inputBrightness" => 0.0f32,
        _ => return nil,
    };
    msg_class![env; NSNumber numberWithFloat:default]
}
- (id)outputImage { output_image(env, this) }
- (())setDefaults { clear_inputs(env, this); }
- (())dealloc { clear_inputs(env, this); env.objc.dealloc_object(this, &mut env.mem) }
@end
@implementation CIContext: NSObject
+ (id)contextWithOptions:(id)options {
    if !context_options_supported(env, options) { return nil; }
    retain(env, options);
    let context = env.objc.alloc_object(this, Box::new(CIContextObject { options }), &mut env.mem);
    autorelease(env, context)
}
- (cg_image::CGImageRef)createCGImage:(id)image fromRect:(CGRect)rect {
    if image == nil { return nil; }
    let raster = env.objc.borrow::<CIImageObject>(image).raster.clone();
    let Some(image) = raster.crop_image(rect) else { return nil; };
    cg_image::from_image(env, image)
}
- (())dealloc {
    let options = env.objc.borrow::<CIContextObject>(this).options;
    release(env, options);
    env.objc.dealloc_object(this, &mut env.mem)
}
@end
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invert_preserves_alpha_and_is_not_passthrough() {
        let mut raster = Raster {
            width: 1,
            height: 1,
            pixels: vec![[0.1, 0.2, 0.3, 0.5]],
            ..Raster::default()
        };
        color_filter(&mut raster, "CIColorInvert", 1.0, 0.0, 1.0, 1.0, [1.0; 3]);
        for (a, b) in raster.pixels[0].iter().zip([0.4, 0.3, 0.2, 0.5]) {
            assert!((a - b).abs() < 1e-6);
        }
    }
    #[test]
    fn gaussian_spreads_impulse_and_expands_extent() {
        let raster = Raster {
            width: 1,
            height: 1,
            pixels: vec![[1.0; 4]],
            ..Raster::default()
        };
        let result = gaussian(&raster, 1.0).unwrap();
        assert_eq!(
            (result.width, result.height, result.x, result.y),
            (7, 7, -3, -3)
        );
        assert!(result.sample(0, 0)[3] < 1.0);
        assert!(result.sample(1, 0)[3] > 0.0);
        assert!((result.pixels.iter().map(|p| p[3]).sum::<f32>() - 1.0).abs() < 1e-5);
    }
    #[test]
    fn color_controls_desaturates() {
        let mut raster = Raster {
            width: 1,
            height: 1,
            pixels: vec![[1.0, 0.0, 0.0, 1.0]],
            ..Raster::default()
        };
        color_filter(&mut raster, "CIColorControls", 0.0, 0.0, 1.0, 1.0, [1.0; 3]);
        assert_eq!(raster.pixels[0][0], raster.pixels[0][1]);
        assert_eq!(raster.pixels[0][1], raster.pixels[0][2]);
        assert!(!supported("NotAFilter"));
    }
    #[test]
    fn image_round_trip_and_crop_orientation() {
        let image = Image::from_pixel_vec(vec![255, 0, 0, 255, 0, 255, 0, 255], (1, 2));
        let raster = Raster::from_image(&image).unwrap();
        assert_eq!(
            raster.crop_image(raster.extent()).unwrap().pixels(),
            image.pixels()
        );
        let crop = raster
            .crop_image(CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: 1.0,
                    height: 1.0,
                },
            })
            .unwrap();
        assert_eq!(crop.pixels(), &[0, 255, 0, 255]);
    }
}
