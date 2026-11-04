/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Core Telephony framework.

use crate::dyld::HostDylib;
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::objc::release;
use crate::objc::retain;
use crate::objc::HostObject;
use crate::objc::NSZonePtr;
use crate::objc::{id, nil, objc_classes, ClassExports};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreTelephony.framework/CoreTelephony",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CTTelephonyNetworkInfo: NSObject

- (id)init {
    // TODO
    nil
}

@end

@implementation CTCarrier: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(CTCarrierHostObject {
        carrier_name: nil,
        iso_country_code: nil,
        mobile_country_code: nil,
        mobile_network_code: nil,
        allows_voip: DEFAULT_ALLOWS_VOIP,
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)init {
    // Install the simulator-equivalent default values. Apps that build a
    // carrier through `[[CTCarrier alloc] init]` directly get the same
    // "Carrier" / no SIM environment as the one we attach to
    // `CTTelephonyNetworkInfo`.
    let name = get_static_str(env, DEFAULT_CARRIER_NAME);
    retain(env, name);
    env.objc.borrow_mut::<CTCarrierHostObject>(this).carrier_name = name;
    this
}

- (())dealloc {
    let host = env.objc.borrow::<CTCarrierHostObject>(this);
    let n = host.carrier_name;
    let iso = host.iso_country_code;
    let mcc = host.mobile_country_code;
    let mnc = host.mobile_network_code;
    release(env, n);
    release(env, iso);
    release(env, mcc);
    release(env, mnc);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)carrierName {
    env.objc.borrow::<CTCarrierHostObject>(this).carrier_name
}

- (id)mobileCountryCode {
    env.objc.borrow::<CTCarrierHostObject>(this).mobile_country_code
}

- (id)mobileNetworkCode {
    env.objc.borrow::<CTCarrierHostObject>(this).mobile_network_code
}

- (id)isoCountryCode {
    env.objc.borrow::<CTCarrierHostObject>(this).iso_country_code
}

- (bool)allowsVOIP {
    env.objc.borrow::<CTCarrierHostObject>(this).allows_voip
}

@end

};
#[derive(Default)]
struct CTCarrierHostObject {
    /// `NSString*` — never `nil` on Apple. We retain whatever string was
    /// installed in the carrier; a default instance gets a static string.
    carrier_name: id,
    /// `NSString*` — ISO 3166-1 alpha-2 country code, `nil` if unavailable.
    iso_country_code: id,
    /// `NSString*` — three-digit Mobile Country Code, `nil` if unavailable.
    mobile_country_code: id,
    /// `NSString*` — two- or three-digit Mobile Network Code, `nil` if
    /// unavailable.
    mobile_network_code: id,
    allows_voip: bool,
}

impl HostObject for CTCarrierHostObject {}

const DEFAULT_ALLOWS_VOIP: bool = true;

const DEFAULT_CARRIER_NAME: &str = "Carrier";

impl HostObject for CTTelephonyNetworkInfoHostObject {}

#[derive(Default)]
#[allow(dead_code)]
struct CTTelephonyNetworkInfoHostObject {
    /// `CTCarrier*` — owned by this info object. Lazily created the first
    /// time `subscriberCellularProvider` is queried so that we don't allocate
    /// a guest object unless an app actually asks for one.
    carrier: id,
    /// Block passed to `subscriberCellularProviderDidUpdateNotifier` —
    /// retained so apps can install a block and have the info object hang on
    /// to it. We never actually fire it (there's no carrier change to
    /// observe in touchHLE), but the property must round-trip via KVC.
    update_notifier_block: id,
}
