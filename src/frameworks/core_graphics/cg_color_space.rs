/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CGColorSpace.h`

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant};
use crate::frameworks::core_foundation::cf_string::CFStringRef;
use crate::frameworks::core_foundation::{CFRelease, CFRetain, CFTypeRef};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSUInteger;
use crate::objc::nil;
use crate::objc::{msg, objc_classes, ClassExports, HostObject};
use crate::Environment;

fn alloc_color_space(env: &mut Environment, name: &'static str) -> CGColorSpaceRef {
    let isa = env
        .objc
        .get_known_class("_Podroid_CGColorSpace", &mut env.mem);
    env.objc
        .alloc_object(isa, Box::new(CGColorSpaceHostObject { name }), &mut env.mem)
}

pub const kCGColorSpaceGenericCMYK: &str = "kCGColorSpaceGenericCMYK";

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// CGColorSpace seems to be a CFType-based type, but in our implementation
// those are just Objective-C types, so we need a class for it, but its name is
// not visible anywhere.
@implementation _Podroid_CGColorSpace: NSObject
@end

};

pub type CGColorSpaceModel = i32;
#[allow(dead_code)]
pub const kCGColorSpaceModelUnknown: CGColorSpaceModel = -1;
pub const kCGColorSpaceModelMonochrome: CGColorSpaceModel = 0;
pub const kCGColorSpaceModelRGB: CGColorSpaceModel = 1;
#[allow(dead_code)]
pub const kCGColorSpaceModelCMYK: CGColorSpaceModel = 2;
#[allow(dead_code)]
pub const kCGColorSpaceModelLab: CGColorSpaceModel = 3;
#[allow(dead_code)]
pub const kCGColorSpaceModelDeviceN: CGColorSpaceModel = 4;
#[allow(dead_code)]
pub const kCGColorSpaceModelIndexed: CGColorSpaceModel = 5;
#[allow(dead_code)]
pub const kCGColorSpaceModelPattern: CGColorSpaceModel = 6;

pub(super) struct CGColorSpaceHostObject {
    pub(super) name: &'static str,
}
impl HostObject for CGColorSpaceHostObject {}

pub type CGColorSpaceRef = CFTypeRef;

pub fn CGColorSpaceCreateWithName(env: &mut Environment, name: CFStringRef) -> CGColorSpaceRef {
    let generic_rgb = ns_string::get_static_str(env, kCGColorSpaceGenericRGB);
    // TODO: support more color spaces
    assert!(msg![env; name isEqualToString:generic_rgb]);

    let isa = env
        .objc
        .get_known_class("_Podroid_CGColorSpace", &mut env.mem);
    env.objc.alloc_object(
        isa,
        Box::new(CGColorSpaceHostObject {
            name: kCGColorSpaceGenericRGB,
        }),
        &mut env.mem,
    )
}

pub fn CGColorSpaceCreateDeviceRGB(env: &mut Environment) -> CGColorSpaceRef {
    // TODO: figure out what characteristics kCGColorSpaceDeviceRGB actually has
    //       on an iPhone
    let isa = env
        .objc
        .get_known_class("_Podroid_CGColorSpace", &mut env.mem);
    env.objc.alloc_object(
        isa,
        Box::new(CGColorSpaceHostObject {
            name: kCGColorSpaceGenericRGB,
        }),
        &mut env.mem,
    )
}

fn CGColorSpaceCreateDeviceGray(env: &mut Environment) -> CGColorSpaceRef {
    let isa = env
        .objc
        .get_known_class("_Podroid_CGColorSpace", &mut env.mem);
    env.objc.alloc_object(
        isa,
        Box::new(CGColorSpaceHostObject {
            name: kCGColorSpaceGenericGray,
        }),
        &mut env.mem,
    )
}

pub fn CGColorSpaceRelease(env: &mut Environment, cs: CGColorSpaceRef) {
    if !cs.is_null() {
        CFRelease(env, cs);
    }
}
pub fn CGColorSpaceRetain(env: &mut Environment, cs: CGColorSpaceRef) -> CGColorSpaceRef {
    if !cs.is_null() {
        CFRetain(env, cs)
    } else {
        cs
    }
}

pub fn CGColorSpaceGetModel(env: &mut Environment, cs: CGColorSpaceRef) -> CGColorSpaceModel {
    let host_object = env.objc.borrow::<CGColorSpaceHostObject>(cs);
    match host_object.name {
        kCGColorSpaceGenericGray => kCGColorSpaceModelMonochrome,
        kCGColorSpaceGenericRGB => kCGColorSpaceModelRGB,
        _ => unimplemented!(),
    }
}

pub const kCGColorSpaceGenericRGB: &str = "kCGColorSpaceGenericRGB";
pub const kCGColorSpaceGenericGray: &str = "kCGColorSpaceGenericGray";

pub const CONSTANTS: ConstantExports = &[
    (
        "_kCGColorSpaceGenericRGB",
        HostConstant::NSString(kCGColorSpaceGenericRGB),
    ),
    (
        "_kCGColorSpaceGenericGray",
        HostConstant::NSString(kCGColorSpaceGenericGray),
    ),
];

fn CGColorSpaceCopyName(env: &mut Environment, cs: CGColorSpaceRef) -> CFStringRef {
    if cs.is_null() {
        return nil;
    }
    let name = env.objc.borrow::<CGColorSpaceHostObject>(cs).name;
    let ns = ns_string::from_rust_string(env, name.to_string());
    // CFStringRef is toll-free bridged with NSString; autorelease so caller
    // gets a +0 reference (matching real CG behaviour of "Copy" returning +1,
    // but apps usually don't release it — autorelease is the safe middle
    // ground).
    crate::objc::autorelease(env, ns)
}

fn CGColorSpaceCreateDeviceCMYK(env: &mut Environment) -> CGColorSpaceRef {
    alloc_color_space(env, kCGColorSpaceGenericCMYK)
}

fn CGColorSpaceCreateICCBased(
    env: &mut Environment,
    n_components: NSUInteger,
    _range: crate::mem::ConstPtr<crate::frameworks::core_graphics::CGFloat>,
    _profile: CFTypeRef, // CGDataProviderRef
    _alternate: CGColorSpaceRef,
) -> CGColorSpaceRef {
    log!(
        "Warning: CGColorSpaceCreateICCBased: ICC profiles not supported \
         (nComponents={}), falling back",
        n_components
    );
    match n_components {
        1 => alloc_color_space(env, kCGColorSpaceGenericGray),
        4 => alloc_color_space(env, kCGColorSpaceGenericCMYK),
        _ => alloc_color_space(env, kCGColorSpaceGenericRGB),
    }
}

fn CGColorSpaceCreateIndexed(
    env: &mut Environment,
    _base_space: CGColorSpaceRef,
    _last_index: NSUInteger,
    _color_table: crate::mem::ConstPtr<u8>,
) -> CGColorSpaceRef {
    log!(
        "Warning: CGColorSpaceCreateIndexed: indexed color spaces not supported, \
          falling back to GenericRGB"
    );
    alloc_color_space(env, kCGColorSpaceGenericRGB)
}

fn CGColorSpaceCreatePattern(
    env: &mut Environment,
    _base_space: CGColorSpaceRef,
) -> CGColorSpaceRef {
    log!(
        "Warning: CGColorSpaceCreatePattern: pattern color spaces not supported, \
          falling back to GenericRGB"
    );
    alloc_color_space(env, kCGColorSpaceGenericRGB)
}

fn CGColorSpaceCreateWithICCProfile(
    env: &mut Environment,
    _data: crate::objc::id, // CFDataRef
) -> CGColorSpaceRef {
    log!(
        "Warning: CGColorSpaceCreateWithICCProfile: ICC profiles not supported, \
          falling back to GenericRGB"
    );
    alloc_color_space(env, kCGColorSpaceGenericRGB)
}

/// Returns the number of colour components *excluding* alpha.
pub fn CGColorSpaceGetNumberOfComponents(env: &mut Environment, cs: CGColorSpaceRef) -> NSUInteger {
    if cs.is_null() {
        return 0;
    }
    match env.objc.borrow::<CGColorSpaceHostObject>(cs).name {
        kCGColorSpaceGenericGray => 1,
        kCGColorSpaceGenericRGB => 3,
        kCGColorSpaceGenericCMYK => 4,
        _ => 3,
    }
}

fn CGColorSpaceIsWideGamutRGB(_env: &mut Environment, _cs: CGColorSpaceRef) -> bool {
    // touchHLE only models sRGB-equivalent spaces — never wide gamut.
    false
}

fn CGColorSpaceSupportsOutput(_env: &mut Environment, cs: CGColorSpaceRef) -> bool {
    !cs.is_null()
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CGColorSpaceCreateWithName(_)),
    export_c_func!(CGColorSpaceCreateDeviceRGB()),
    export_c_func!(CGColorSpaceCreateDeviceGray()),
    export_c_func!(CGColorSpaceRetain(_)),
    export_c_func!(CGColorSpaceRelease(_)),
    export_c_func!(CGColorSpaceGetModel(_)),
    export_c_func!(CGColorSpaceCopyName(_)),
    export_c_func!(CGColorSpaceCreateDeviceCMYK()),
    export_c_func!(CGColorSpaceCreateICCBased(_, _, _, _)),
    export_c_func!(CGColorSpaceCreateIndexed(_, _, _)),
    export_c_func!(CGColorSpaceCreatePattern(_)),
    export_c_func!(CGColorSpaceCreateWithICCProfile(_)),
    export_c_func!(CGColorSpaceGetNumberOfComponents(_)),
    export_c_func!(CGColorSpaceIsWideGamutRGB(_)),
    export_c_func!(CGColorSpaceSupportsOutput(_)),
];
