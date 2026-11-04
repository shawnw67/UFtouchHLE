//! Contacts framework (`CNPostalAddress` string constants, `CNContact`).
//!
//! Apps (e.g. Alto's Adventure, PassKit integrations) reference the
//! `CNPostalAddress*Key` NSString constants at link time. They are plain
//! key names for `CNMutablePostalAddress` key-value coding.

use crate::objc::{id, objc_classes, ClassExports, NSZonePtr, TrivialHostObject};

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CNPostalAddress: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_static_object(this, Box::new(TrivialHostObject), &mut env.mem)
}
- (id)init {
    this
}
@end

@implementation CNMutablePostalAddress: CNPostalAddress
@end

};

pub const CONSTANTS: crate::dyld::ConstantExports = &[
    (
        "_CNPostalAddressCityKey",
        crate::dyld::HostConstant::NSString("city"),
    ),
    (
        "_CNPostalAddressCountryKey",
        crate::dyld::HostConstant::NSString("country"),
    ),
    (
        "_CNPostalAddressISOCountryCodeKey",
        crate::dyld::HostConstant::NSString("ISOCountryCode"),
    ),
    (
        "_CNPostalAddressPostalCodeKey",
        crate::dyld::HostConstant::NSString("postalCode"),
    ),
    (
        "_CNPostalAddressStateKey",
        crate::dyld::HostConstant::NSString("state"),
    ),
    (
        "_CNPostalAddressStreetKey",
        crate::dyld::HostConstant::NSString("street"),
    ),
    (
        "_CNPostalAddressSubAdministrativeAreaKey",
        crate::dyld::HostConstant::NSString("subAdministrativeArea"),
    ),
    (
        "_CNPostalAddressSubLocalityKey",
        crate::dyld::HostConstant::NSString("subLocality"),
    ),
];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/Contacts.framework/Contacts",
    aliases: &["/System/Library/Frameworks/Contacts.framework/Versions/A/Contacts"],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};
