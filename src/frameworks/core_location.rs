/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Core Location framework.
//!
//! Proper implementation of this framework _could_ make sense on Android,
//! but it seems like early iOS games were using it exclusively to
//! ~~spy on~~ track users without actual location-based gameplay.
//!
//! Some apps (e.g. maps) would _require_ location support to work properly,
//! but it is not the current focus of the touchHLE. The current focus is,
//! you know, **GAMES**.

use crate::abi::GuestArg;
use crate::dyld::{ConstantExports, HostConstant, HostDylib};
use crate::mem::SafeRead;
use crate::objc::msg_class;
use crate::objc::nil;
use crate::objc::release;
use crate::objc::retain;
use crate::objc::HostObject;
use crate::objc::NSZonePtr;
use crate::objc::{id, objc_classes, ClassExports};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreLocation.framework/CoreLocation",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};

type CLLocationAccuracy = f64;
type CLLocationDegrees = f64;
type CLLocationDistance = f64;

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CLLocationManager: NSObject

+ (bool)headingAvailable {
    false
}

- (())setDelegate:(id)_delegate {
    // TODO
}

- (bool)headingAvailable {
    false
}

- (())startUpdatingHeading {
    // TODO
}
- (())stopUpdatingHeading {
    // TODO
}

- (())startUpdatingLocation {
    // TODO
}
- (())stopUpdatingLocation {
    // TODO
}

- (())setDesiredAccuracy:(CLLocationAccuracy)_acc {
    // TODO
}
- (())setHeadingFilter:(CLLocationDegrees)_filter {
    // TODO
}
- (())setDistanceFilter:(CLLocationDistance)_filter {
    // TODO
}

@end

@implementation CLHeading: NSObject
// TODO
@end

@implementation CLLocation: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(CLLocationHostObject {
        latitude:  0.0,
        longitude: 0.0,
        altitude:  0.0,
        horizontal_accuracy: -1.0,
        vertical_accuracy:   -1.0,
        speed: -1.0,
        course: -1.0,
        timestamp: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithLatitude:(CLLocationDegrees)lat
             longitude:(CLLocationDegrees)lon {
    {
        let host = env.objc.borrow_mut::<CLLocationHostObject>(this);
        host.latitude  = lat;
        host.longitude = lon;
        host.horizontal_accuracy = 0.0;
    }
    let ts: id = msg_class![env; NSDate date];
    env.objc.borrow_mut::<CLLocationHostObject>(this).timestamp = ts;
    this
}

// Replace the 6-parameter init with two separate inits that are within
// the HostIMP parameter limit.
- (id)initWithCoordinate:(CLLocationDegrees)lat
              longitude:(CLLocationDegrees)lon
               altitude:(CLLocationDistance)alt
     horizontalAccuracy:(CLLocationAccuracy)h_acc
       verticalAccuracy:(CLLocationAccuracy)v_acc {
    {
        let host = env.objc.borrow_mut::<CLLocationHostObject>(this);
        host.latitude  = lat;
        host.longitude = lon;
        host.altitude  = alt;
        host.horizontal_accuracy = h_acc;
        host.vertical_accuracy   = v_acc;
    }
    let ts: id = msg_class![env; NSDate date];
    env.objc.borrow_mut::<CLLocationHostObject>(this).timestamp = ts;
    this
}

// Apple's full initializer:
// -
// Our CLLocation initializer, used to build a location from raw fields.
// where coordinate is a CLLocationCoordinate2D struct.
- (id)initWithCoordinate:(CLLocationCoordinate2D)coord
               altitude:(CLLocationDistance)alt
     horizontalAccuracy:(CLLocationAccuracy)h_acc
       verticalAccuracy:(CLLocationAccuracy)v_acc
              timestamp:(id)ts {
    {
        let host = env.objc.borrow_mut::<CLLocationHostObject>(this);
        host.latitude  = coord.latitude;
        host.longitude = coord.longitude;
        host.altitude  = alt;
        host.horizontal_accuracy = h_acc;
        host.vertical_accuracy   = v_acc;
    }
    retain(env, ts);
    env.objc.borrow_mut::<CLLocationHostObject>(this).timestamp = ts;
    this
}

- (())dealloc {
    let ts = env.objc.borrow::<CLLocationHostObject>(this).timestamp;
    release(env, ts);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (CLLocationDegrees)latitude  { env.objc.borrow::<CLLocationHostObject>(this).latitude }
- (CLLocationDegrees)longitude { env.objc.borrow::<CLLocationHostObject>(this).longitude }
- (CLLocationDistance)altitude { env.objc.borrow::<CLLocationHostObject>(this).altitude }
- (CLLocationAccuracy)horizontalAccuracy { env.objc.borrow::<CLLocationHostObject>(this).horizontal_accuracy }
- (CLLocationAccuracy)verticalAccuracy   { env.objc.borrow::<CLLocationHostObject>(this).vertical_accuracy }
- (CLLocationSpeed)speed       { env.objc.borrow::<CLLocationHostObject>(this).speed }
- (CLLocationDirection)course  { env.objc.borrow::<CLLocationHostObject>(this).course }
- (id)timestamp                { env.objc.borrow::<CLLocationHostObject>(this).timestamp }

// In the initializer above, use NSDate for the timestamp automatically.
// Then expose a separate timestamp setter for callers that need it:

- (())_setTimestamp:(id)ts { // NSDate* — private helper
    let old = env.objc.borrow::<CLLocationHostObject>(this).timestamp;
    release(env, old);
    retain(env, ts);
    env.objc.borrow_mut::<CLLocationHostObject>(this).timestamp = ts;
}

- (CLLocationDistance)distanceFromLocation:(id)other { // CLLocation*
    // Simple equirectangular approximation — sufficient for games.
    let (lat1, lon1) = {
        let h = env.objc.borrow::<CLLocationHostObject>(this);
        (h.latitude, h.longitude)
    };
    let (lat2, lon2) = {
        let h = env.objc.borrow::<CLLocationHostObject>(other);
        (h.latitude, h.longitude)
    };
    let earth_r = 6_371_000.0f64; // metres
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    earth_r * c
}

- (id)description { // NSString*
    let (lat, lon, alt, h_acc, v_acc) = {
        let h = env.objc.borrow::<CLLocationHostObject>(this);
        (h.latitude, h.longitude, h.altitude, h.horizontal_accuracy, h.vertical_accuracy)
    };
    let s = format!(
        "<+{:.6},{:.6}> +/- {:.0}m (speed {:.2} mps / course {:.2}) @ alt {:.0}m +/- {:.0}m",
        lat, lon, h_acc, -1.0f64, -1.0f64, alt, v_acc
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};

const CONSTANTS: ConstantExports = &[
    (
        "_kCLHeadingFilterNone",
        HostConstant::Custom(|env| env.mem.alloc_and_write(-1f64).cast().cast_const()),
    ),
    (
        "_kCLLocationAccuracyBest",
        HostConstant::Custom(|env| env.mem.alloc_and_write(-1f64).cast().cast_const()),
    ),
    (
        "_kCLLocationAccuracyKilometer",
        HostConstant::Custom(|env| env.mem.alloc_and_write(1000f64).cast().cast_const()),
    ),
];
/// A geographical coordinate (latitude/longitude pair), matching Apple's
/// `CLLocationCoordinate2D` struct layout on 32-bit ARM.
#[derive(Copy, Clone, Debug, Default)]
#[repr(C, packed)]
pub struct CLLocationCoordinate2D {
    pub latitude: CLLocationDegrees,
    pub longitude: CLLocationDegrees,
}

unsafe impl SafeRead for CLLocationCoordinate2D {}

impl GuestArg for CLLocationCoordinate2D {
    const REG_COUNT: usize = 4; // 2 x f64, each f64 = 2 u32 registers
    fn from_regs(regs: &[u32]) -> Self {
        CLLocationCoordinate2D {
            latitude: f64::from_regs(&regs[0..2]),
            longitude: f64::from_regs(&regs[2..4]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        self.latitude.to_regs(&mut regs[0..2]);
        self.longitude.to_regs(&mut regs[2..4]);
    }
}

type CLLocationDirection = f64;

#[derive(Default)]
struct CLLocationHostObject {
    latitude: CLLocationDegrees,
    longitude: CLLocationDegrees,
    altitude: CLLocationDistance,
    horizontal_accuracy: CLLocationAccuracy,
    vertical_accuracy: CLLocationAccuracy,
    speed: CLLocationSpeed,
    course: CLLocationDirection,
    // NSDate* timestamp
    timestamp: id,
}

impl HostObject for CLLocationHostObject {}

type CLLocationSpeed = f64;

impl HostObject for CLLocationManagerHostObject {}

impl HostObject for CLHeadingHostObject {}

#[derive(Default)]
#[allow(dead_code)]
struct CLHeadingHostObject {
    magnetic_heading: CLHeadingComponentValue,
    true_heading: CLHeadingComponentValue,
    heading_accuracy: CLHeadingComponentValue,
    x: CLHeadingComponentValue,
    y: CLHeadingComponentValue,
    z: CLHeadingComponentValue,
    // NSDate* timestamp
    timestamp: id,
}

#[derive(Default)]
#[allow(dead_code)]
struct CLLocationManagerHostObject {
    delegate: id,
    desired_accuracy: CLLocationAccuracy,
    distance_filter: CLLocationDistance,
    heading_filter: CLLocationDirection,
    heading_orientation: CLDeviceOrientation,
    activity_type: CLActivityType,
    pauses_location_updates_automatically: bool,
    allows_background_location_updates: bool,
    shows_background_location_indicator: bool,
    updates_location: bool,
    updates_heading: bool,
}

#[allow(dead_code)]
type CLActivityType = i32;

#[allow(dead_code)]
type CLDeviceOrientation = i32;

#[allow(dead_code)]
type CLHeadingComponentValue = f64;
