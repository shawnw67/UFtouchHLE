/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Core Motion framework.

use crate::abi::impl_GuestRet_for_large_struct;
use crate::abi::GuestArg;
use crate::dyld::HostDylib;
use crate::objc::autorelease;
use crate::objc::id;
use crate::objc::msg_class;
use crate::objc::HostObject;
use crate::objc::NSZonePtr;
use crate::objc::{objc_classes, ClassExports};
use std::time::Instant;

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/CoreMotion.framework/CoreMotion",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CMMotionManager: NSObject

- (bool)isGyroAvailable {
    // It could make sense to implement gyroscope support at least for Android.
    log!("TODO: [(CMMotionManager *){:?} isGyroAvailable] -> false", this);
    false
}
- (bool)isDeviceMotionAvailable {
    log!("TODO: [(CMMotionManager *){:?} isDeviceMotionAvailable] -> false", this);
    // According to docs, this is functionally equivalent to `isGyroAvailable`
    // method. (All devices have accelerometer, but only some do have gyro).
    false
}
- (bool)isAccelerometerAvailable {
    // According to https://developer.apple.com/documentation/coremotion/getting-raw-accelerometer-events?language=objc,
    // every iOS device has an accelerometer, but on real hardware this method
    // can still return false if the device isn't ready to produce data yet.
    // Here we always return true since we don't model that readiness state.
    true
}

@end

@implementation CMAccelerometerData: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(CMAccelerometerDataHostObject {
        acceleration: CMAcceleration { x: 0.0, y: 0.0, z: -1.0 },
        timestamp: 0.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (CMAcceleration)acceleration {
    env.objc.borrow::<CMAccelerometerDataHostObject>(this).acceleration
}

- (f64)timestamp {
    env.objc.borrow::<CMAccelerometerDataHostObject>(this).timestamp
}

- (f64)_accelerationX {
    env.objc.borrow::<CMAccelerometerDataHostObject>(this).acceleration.x
}
- (f64)_accelerationY {
    env.objc.borrow::<CMAccelerometerDataHostObject>(this).acceleration.y
}
- (f64)_accelerationZ {
    env.objc.borrow::<CMAccelerometerDataHostObject>(this).acceleration.z
}

- (id)description {
    let host = env.objc.borrow::<CMAccelerometerDataHostObject>(this);
    let acceleration = host.acceleration;
    let (x, y, z) = (acceleration.x, acceleration.y, acceleration.z);
    let s = format!(
        "<CMAccelerometerData: timestamp={:.4} x={:.4} y={:.4} z={:.4}>",
        host.timestamp, x, y, z
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

@implementation CMAttitude: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(CMAttitudeHostObject::default());
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (f64)roll {
    env.objc.borrow::<CMAttitudeHostObject>(this).attitude.roll
}

- (f64)pitch {
    env.objc.borrow::<CMAttitudeHostObject>(this).attitude.pitch
}

- (f64)yaw {
    env.objc.borrow::<CMAttitudeHostObject>(this).attitude.yaw
}

- (CMQuaternion)quaternion {
    env.objc.borrow::<CMAttitudeHostObject>(this).quaternion
}

- (CMRotationMatrix)rotationMatrix {
    let q = env.objc.borrow::<CMAttitudeHostObject>(this).quaternion;
    quat_to_rotation_matrix((q.x, q.y, q.z, q.w))
}

// Per Apple docs, this is how apps turn the absolute world-frame attitude
// into one relative to a stored reference attitude (i.e. "calibrate" the
// neutral pose when a camera mode starts). Games' 3D tilt cameras rely on
// this — without it they fall back to the absolute attitude, which in the
// upright gaming hold sits near its extremes and makes a tiny tilt swing
// the camera wildly. In-place per Apple's signature.
- (())multiplyByInverseOfAttitude:(id)other {
    let q_ref = {
        let CMQuaternion { x, y, z, w } =
            env.objc.borrow::<CMAttitudeHostObject>(other).quaternion;
        (x, y, z, w)
    };
    let q_self = {
        let CMQuaternion { x, y, z, w } =
            env.objc.borrow::<CMAttitudeHostObject>(this).quaternion;
        (x, y, z, w)
    };
    // q_out = q_self * inverse(q_ref), and the inverse of a unit quaternion
    // is its conjugate.
    let q_ref_inv = (-q_ref.0, -q_ref.1, -q_ref.2, q_ref.3);
    let q_out = quat_normalized(quat_mul(q_self, q_ref_inv));
    let (pitch, roll, yaw) = quat_to_apple_angles(q_out);
    let host = env.objc.borrow_mut::<CMAttitudeHostObject>(this);
    host.quaternion = CMQuaternion {
        x: q_out.0,
        y: q_out.1,
        z: q_out.2,
        w: q_out.3,
    };
    host.attitude = CMAttitude { roll, pitch, yaw };
}

@end

@implementation CMDeviceMotion: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(CMDeviceMotionHostObject {
        gravity: CMAcceleration { x: 0.0, y: 0.0, z: -1.0 },
        user_acceleration: CMAcceleration { x: 0.0, y: 0.0, z: 0.0 },
        rotation_rate: CMRotationRate { x: 0.0, y: 0.0, z: 0.0 },
        timestamp: 0.0,
        attitude: CMAttitude::default(),
        quaternion: CMQuaternion { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)attitude {
    let (attitude_angles, quaternion) = {
        let host = env.objc.borrow::<CMDeviceMotionHostObject>(this);
        (host.attitude, host.quaternion)
    };
    let attitude: id = msg_class![env; CMAttitude new];
    {
        let attitude_host = env.objc.borrow_mut::<CMAttitudeHostObject>(attitude);
        attitude_host.attitude = attitude_angles;
        attitude_host.quaternion = quaternion;
    }
    autorelease(env, attitude)
}

- (CMAcceleration)gravity {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).gravity
}

- (CMAcceleration)userAcceleration {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).user_acceleration
}

- (CMRotationRate)rotationRate {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).rotation_rate
}

- (f64)timestamp {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).timestamp
}

- (f64)_gravityX {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).gravity.x
}
- (f64)_gravityY {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).gravity.y
}
- (f64)_gravityZ {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).gravity.z
}

- (f64)_userAccelerationX {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).user_acceleration.x
}
- (f64)_userAccelerationY {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).user_acceleration.y
}
- (f64)_userAccelerationZ {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).user_acceleration.z
}

- (f64)_rotationRateX {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).rotation_rate.x
}
- (f64)_rotationRateY {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).rotation_rate.y
}
- (f64)_rotationRateZ {
    env.objc.borrow::<CMDeviceMotionHostObject>(this).rotation_rate.z
}

@end

@implementation CMGyroData: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(CMGyroDataHostObject {
        rotation_rate: CMRotationRate { x: 0.0, y: 0.0, z: 0.0 },
        timestamp: 0.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (CMRotationRate)rotationRate {
    env.objc.borrow::<CMGyroDataHostObject>(this).rotation_rate
}

- (f64)timestamp {
    env.objc.borrow::<CMGyroDataHostObject>(this).timestamp
}

- (f64)_rotationRateX {
    env.objc.borrow::<CMGyroDataHostObject>(this).rotation_rate.x
}
- (f64)_rotationRateY {
    env.objc.borrow::<CMGyroDataHostObject>(this).rotation_rate.y
}
- (f64)_rotationRateZ {
    env.objc.borrow::<CMGyroDataHostObject>(this).rotation_rate.z
}

@end

};
/// Per Apple docs: CMAcceleration is a structure with x, y, z fields
/// representing acceleration in G-force units along each axis.
/// iPhone coordinate system:
///   x: lateral (positive = right)
///   y: longitudinal (positive = up toward top of device)
///   z: perpendicular to screen (positive = toward user)
/// When device is flat on table face-up: x=0, y=0, z=-1 (gravity pulling down)
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C, packed)]
struct CMAcceleration {
    x: f64,
    y: f64,
    z: f64,
}

unsafe impl crate::mem::SafeRead for CMAcceleration {}

impl GuestArg for CMAcceleration {
    const REG_COUNT: usize = 6;
    fn from_regs(regs: &[u32]) -> Self {
        CMAcceleration {
            x: f64::from_regs(&regs[0..2]),
            y: f64::from_regs(&regs[2..4]),
            z: f64::from_regs(&regs[4..6]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        let (x, y, z) = (self.x, self.y, self.z);
        x.to_regs(&mut regs[0..2]);
        y.to_regs(&mut regs[2..4]);
        z.to_regs(&mut regs[4..6]);
    }
}

impl_GuestRet_for_large_struct!(CMAcceleration);

#[derive(Default)]
struct CMAccelerometerDataHostObject {
    acceleration: CMAcceleration,
    timestamp: f64,
}

impl HostObject for CMAccelerometerDataHostObject {}

#[derive(Clone, Copy, Debug, Default)]
struct CMAttitude {
    roll: f64,
    pitch: f64,
    yaw: f64,
}

#[derive(Default)]
struct CMAttitudeHostObject {
    attitude: CMAttitude,
    quaternion: CMQuaternion,
}

impl HostObject for CMAttitudeHostObject {}

#[derive(Default)]
struct CMDeviceMotionHostObject {
    /// Gravity component of acceleration
    gravity: CMAcceleration,
    /// User-generated acceleration (total minus gravity)
    user_acceleration: CMAcceleration,
    /// Rotation rate
    rotation_rate: CMRotationRate,
    timestamp: f64,
    /// Attitude from the sensor-fusion filter (pitch/roll/yaw + quaternion).
    attitude: CMAttitude,
    quaternion: CMQuaternion,
}

impl HostObject for CMDeviceMotionHostObject {}

#[derive(Default)]
struct CMGyroDataHostObject {
    rotation_rate: CMRotationRate,
    timestamp: f64,
}

impl HostObject for CMGyroDataHostObject {}

/// Per Apple docs: CMQuaternion is (x, y, z, w) as IEEE doubles. Rotation of
/// angle theta about the unit axis (x, y, z) is represented as
/// (x*sin(theta/2), y*sin(theta/2), z*sin(theta/2), cos(theta/2)).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C, packed)]
struct CMQuaternion {
    x: f64,
    y: f64,
    z: f64,
    w: f64,
}

unsafe impl crate::mem::SafeRead for CMQuaternion {}

impl GuestArg for CMQuaternion {
    const REG_COUNT: usize = 8;
    fn from_regs(regs: &[u32]) -> Self {
        CMQuaternion {
            x: f64::from_regs(&regs[0..2]),
            y: f64::from_regs(&regs[2..4]),
            z: f64::from_regs(&regs[4..6]),
            w: f64::from_regs(&regs[6..8]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        x.to_regs(&mut regs[0..2]);
        y.to_regs(&mut regs[2..4]);
        z.to_regs(&mut regs[4..6]);
        w.to_regs(&mut regs[6..8]);
    }
}

impl_GuestRet_for_large_struct!(CMQuaternion);

/// Per Apple docs: CMRotationMatrix is the device attitude expressed as a 3x3
/// rotation matrix of IEEE doubles (row-major m11..m33).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C, packed)]
struct CMRotationMatrix {
    m11: f64,
    m12: f64,
    m13: f64,
    m21: f64,
    m22: f64,
    m23: f64,
    m31: f64,
    m32: f64,
    m33: f64,
}

unsafe impl crate::mem::SafeRead for CMRotationMatrix {}

impl GuestArg for CMRotationMatrix {
    const REG_COUNT: usize = 18;
    fn from_regs(regs: &[u32]) -> Self {
        CMRotationMatrix {
            m11: f64::from_regs(&regs[0..2]),
            m12: f64::from_regs(&regs[2..4]),
            m13: f64::from_regs(&regs[4..6]),
            m21: f64::from_regs(&regs[6..8]),
            m22: f64::from_regs(&regs[8..10]),
            m23: f64::from_regs(&regs[10..12]),
            m31: f64::from_regs(&regs[12..14]),
            m32: f64::from_regs(&regs[14..16]),
            m33: f64::from_regs(&regs[16..18]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        self.m11.to_regs(&mut regs[0..2]);
        self.m12.to_regs(&mut regs[2..4]);
        self.m13.to_regs(&mut regs[4..6]);
        self.m21.to_regs(&mut regs[6..8]);
        self.m22.to_regs(&mut regs[8..10]);
        self.m23.to_regs(&mut regs[10..12]);
        self.m31.to_regs(&mut regs[12..14]);
        self.m32.to_regs(&mut regs[14..16]);
        self.m33.to_regs(&mut regs[16..18]);
    }
}

impl_GuestRet_for_large_struct!(CMRotationMatrix);

/// Per Apple docs: CMRotationRate in radians per second around each axis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[repr(C, packed)]
struct CMRotationRate {
    x: f64,
    y: f64,
    z: f64,
}

unsafe impl crate::mem::SafeRead for CMRotationRate {}

impl GuestArg for CMRotationRate {
    const REG_COUNT: usize = 6;
    fn from_regs(regs: &[u32]) -> Self {
        CMRotationRate {
            x: f64::from_regs(&regs[0..2]),
            y: f64::from_regs(&regs[2..4]),
            z: f64::from_regs(&regs[4..6]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        let (x, y, z) = (self.x, self.y, self.z);
        x.to_regs(&mut regs[0..2]);
        y.to_regs(&mut regs[2..4]);
        z.to_regs(&mut regs[4..6]);
    }
}

impl_GuestRet_for_large_struct!(CMRotationRate);

fn quat_mul(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    let (ax, ay, az, aw) = a;
    let (bx, by, bz, bw) = b;
    (
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    )
}

fn quat_normalized(q: (f64, f64, f64, f64)) -> (f64, f64, f64, f64) {
    let norm = (q.0 * q.0 + q.1 * q.1 + q.2 * q.2 + q.3 * q.3).sqrt();
    if norm < ATTITUDE_EPSILON {
        (0.0, 0.0, 0.0, 1.0)
    } else {
        (q.0 / norm, q.1 / norm, q.2 / norm, q.3 / norm)
    }
}

/// Tait-Bryan Z-Y-X decomposition with Apple's axis mapping (pitch about the
/// device x-axis, roll about the y-axis, yaw about the z-axis).
fn quat_to_apple_angles(q: (f64, f64, f64, f64)) -> (f64, f64, f64) {
    let (x, y, z, w) = q;
    let pitch = (2.0 * (w * x + y * z)).atan2(1.0 - 2.0 * (x * x + y * y));
    let roll = (2.0 * (w * y - z * x)).clamp(-1.0, 1.0).asin();
    let yaw = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z));
    (pitch, roll, yaw)
}

/// Standard quaternion -> rotation matrix conversion.
fn quat_to_rotation_matrix(q: (f64, f64, f64, f64)) -> CMRotationMatrix {
    let (x, y, z, w) = q;
    CMRotationMatrix {
        m11: 1.0 - 2.0 * (y * y + z * z),
        m12: 2.0 * (x * y - w * z),
        m13: 2.0 * (x * z + w * y),
        m21: 2.0 * (x * y + w * z),
        m22: 1.0 - 2.0 * (x * x + z * z),
        m23: 2.0 * (y * z - w * x),
        m31: 2.0 * (x * z - w * y),
        m32: 2.0 * (y * z + w * x),
        m33: 1.0 - 2.0 * (x * x + y * y),
    }
}

/// Smallest angular step worth applying, to avoid normalising ~zero vectors.
const ATTITUDE_EPSILON: f64 = 1.0e-9;

impl HostObject for CMMotionManagerHostObject {}

#[derive(Default)]
#[allow(dead_code)]
struct CMMotionManagerHostObject {
    accelerometer_update_interval: f64,
    gyro_update_interval: f64,
    device_motion_update_interval: f64,
    accelerometer_active: bool,
    gyro_active: bool,
    device_motion_active: bool,
    /// Cached last accelerometer reading
    last_acceleration: CMAcceleration,
    /// Timestamp of last accelerometer update
    last_accel_timestamp: f64,
    /// Reference time for computing timestamps. `None` only for the phantom
    /// fallback created via `Default::default()`; real motion managers
    /// allocated through `+alloc` always populate this with `Instant::now()`.
    start_time: Option<Instant>,
    /// Complementary attitude filter state (quaternion, learned gyro bias,
    /// low-passed gravity input). `None` until the first device-motion read.
    attitude_state: Option<MotionFilter>,
}

/// Complementary gyro+accelerometer attitude filter with long-lived state:
/// low-passed gravity input and a learned gyroscope bias.
#[allow(dead_code)]
struct MotionFilter {
    /// Attitude quaternion (x, y, z, w) expressing device orientation.
    q: (f64, f64, f64, f64),
    /// Timestamp of the previous step.
    last: Instant,
    /// Low-pass filtered accelerometer input (gravity reference).
    smoothed_accel: (f64, f64, f64),
    /// Gyroscope bias learned while the device is stationary.
    gyro_bias: (f64, f64, f64),
}

impl MotionFilter {
    /// Initialise from the first accelerometer reading so the implied
    /// gravity matches it exactly (yaw arbitrary = 0, matching the
    /// XArbitraryZVertical reference Apple uses without a magnetometer).
    #[allow(dead_code)]
    fn new(accel: (f64, f64, f64)) -> MotionFilter {
        let g = attitude_normalize_vec(accel);
        MotionFilter {
            q: quat_shortest_arc(g, (0.0, 0.0, -1.0)),
            last: Instant::now(),
            smoothed_accel: accel,
            gyro_bias: (0.0, 0.0, 0.0),
        }
    }

    /// One filter step; `now` is the current instant, `allow_gyro` disables
    /// integration for the TOUCHHLE_NO_MOTION_GYRO diagnostic mode.
    /// Returns (gravity estimate, user acceleration), both device frame.
    #[allow(dead_code)]
    fn step(
        &mut self,
        gyro: (f64, f64, f64),
        accel: (f64, f64, f64),
        now: Instant,
        allow_gyro: bool,
    ) -> ((f64, f64, f64), (f64, f64, f64)) {
        let dt = now
            .duration_since(self.last)
            .min(std::time::Duration::from_secs(1))
            .as_secs_f64();
        self.last = now;

        // 0. Low-pass the accelerometer input (limits ~[0, 5Hz] content for
        //    the gravity anchor; kills sensor wobble in a resting hand).
        let lerp_t = (1.0 - (-dt / ATTITUDE_ACCEL_LPF_TAU).exp()).clamp(0.0, 1.0);
        self.smoothed_accel = vec_lerp(self.smoothed_accel, accel, lerp_t);

        // 0b. Learn the gyroscope bias while the device is evidently
        //     stationary: raw magnitude near 1g *and* no recent change
        //     between the raw sample and its own low-pass. Learning is
        //     deliberately one-sided (only from accel evidence), so even a
        //     huge constant gyro offset eventually converges instead of
        //     spinning the estimate forever.
        let raw_len = vec_len(accel);
        let recent_change = vec_len(vec_sub(accel, self.smoothed_accel));
        let gyro_is_bias_evidence = (raw_len - 1.0).abs() < 0.05
            && recent_change < 0.01
            && vec_len(gyro) <= ATTITUDE_BIAS_MAX_RATE;
        if gyro_is_bias_evidence && dt > 0.0 {
            let bias_t = (dt / ATTITUDE_BIAS_TAU).clamp(0.0, 1.0);
            self.gyro_bias = vec_lerp(self.gyro_bias, gyro, bias_t);
        }
        let gyro_clean = vec_sub(gyro, self.gyro_bias);

        // 1. Integrate the (debiased) gyroscope: body-frame rate => right
        //    multiply. This is the smooth, low-latency part of the motion.
        if allow_gyro && dt > 0.0 && dt <= ATTITUDE_MAX_GYRO_DT {
            let angular_speed = vec_len(gyro_clean);
            if angular_speed > ATTITUDE_EPSILON {
                let half_angle = angular_speed * dt * 0.5;
                let vector_scale = half_angle.sin() / angular_speed;
                let dq = (
                    gyro_clean.0 * vector_scale,
                    gyro_clean.1 * vector_scale,
                    gyro_clean.2 * vector_scale,
                    half_angle.cos(),
                );
                self.q = quat_normalized(quat_mul(self.q, dq));
            }
        }

        // 2. Gravity correction, from the *smoothed* samples; trust is
        //    decided on the *raw* magnitude so shaking disables it.
        let sm_len = vec_len(self.smoothed_accel);
        let trust_weight = if raw_len > 1.0e-3 {
            (1.0 - (raw_len - 1.0).abs() / ATTITUDE_ACCEL_TRUST_BAND).clamp(0.0, 1.0)
        } else {
            0.0
        };
        if sm_len > 1.0e-3 && trust_weight > 0.0 {
            let measured_g = (
                self.smoothed_accel.0 / sm_len,
                self.smoothed_accel.1 / sm_len,
                self.smoothed_accel.2 / sm_len,
            );
            let predicted_g = quat_world_to_device(self.q, (0.0, 0.0, -1.0));
            // NOTE the arc direction: the implied gravity is
            // quat_world_to_device(q, ¦-z¦) = R(q)^-1(-z), and extending
            // q <- q*r rotates the implied vector by R(r)^-1. To move the
            // predicted vector towards the measured one the correction arc
            // must map *measured -> predicted*; its inverse then maps
            // predicted -> measured.
            let correction = quat_shortest_arc(measured_g, predicted_g);
            let gain = (dt * ATTITUDE_CORRECTION_PER_SEC * trust_weight).clamp(0.0, 1.0);
            let correction = quat_scale_angle(correction, gain);
            self.q = quat_normalized(quat_mul(self.q, correction));
        }

        // 3. Derived outputs.
        let gravity = quat_world_to_device(self.q, (0.0, 0.0, -1.0));
        let user = vec_sub(accel, gravity);
        (gravity, user)
    }
}

/// Time constant of the low-pass filter applied to the accelerometer before
/// it may push the attitude around. Smartphones' accelerometers wobble by
/// +-0.02g even in a resting hand (and much more during normal handling);
/// feeding that straight into a strong gravity correction shows up as
/// visible camera jitter. The gravity anchor only needs the slow content.
#[allow(dead_code)]
const ATTITUDE_ACCEL_LPF_TAU: f64 = 0.15;

/// How far the measured acceleration magnitude may deviate from 1g before
/// the gravity correction is distrusted completely. When the device is
/// being shaken/accelerated, the accelerometer's reading is no longer
/// "gravity" — weighting it then would inject the user's linear shaking
/// into the orientation. Tested against the RAW magnitude.
#[allow(dead_code)]
const ATTITUDE_ACCEL_TRUST_BAND: f64 = 0.2;

/// Bias learning is limited to near-zero angular rates so a deliberate slow
/// turn is not mistaken for sensor drift.
#[allow(dead_code)]
const ATTITUDE_BIAS_MAX_RATE: f64 = 0.025;

/// Time constant for learning the gyroscope bias while the device is
/// evidently stationary. Host gyroscopes delivered through SDL can carry a
/// constant offset large enough to spin a naive complementary filter away.
/// Bias learning still requires a steady accelerometer reading, and the
/// near-zero-rate guard prevents a deliberate slow turn from being learned
/// as sensor drift.
#[allow(dead_code)]
const ATTITUDE_BIAS_TAU: f64 = 2.0;

/// Gain [0..1] of the gravity correction applied per second of elapsed time
/// while the (smoothed) accelerometer reading looks trustworthy. The anchor
/// must stay assertive — when it was weakened in testing, gyroscope bias on
/// real devices made the estimate wander — while the low-passed input keeps
/// it from twitching.
#[allow(dead_code)]
const ATTITUDE_CORRECTION_PER_SEC: f64 = 6.0;

/// Gyroscope integration is skipped for steps larger than this: beyond it a
/// stale rate sample would integrate garbage (e.g. after a suspended frame).
#[allow(dead_code)]
const ATTITUDE_MAX_GYRO_DT: f64 = 0.25;

/// Normalise a 3-vector, returning the unit vector (or (0, 0, -1) for
/// degenerate input).
#[allow(dead_code)]
fn attitude_normalize_vec(v: (f64, f64, f64)) -> (f64, f64, f64) {
    let len = (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt();
    if len < ATTITUDE_EPSILON {
        (0.0, 0.0, -1.0)
    } else {
        (v.0 / len, v.1 / len, v.2 / len)
    }
}

/// Slerp-style attenuation of a corrective quaternion: scale its rotation
/// angle by `gain` in [0, 1].
#[allow(dead_code)]
fn quat_scale_angle(q: (f64, f64, f64, f64), gain: f64) -> (f64, f64, f64, f64) {
    let (qx, qy, qz, qw) = q;
    let half_angle = qw.clamp(-1.0, 1.0).acos();
    let scaled = half_angle * gain;
    let sin_scaled = scaled.sin();
    let sin_half = half_angle.sin();
    if sin_half.abs() < ATTITUDE_EPSILON {
        return (0.0, 0.0, 0.0, 1.0);
    }
    let factor = sin_scaled / sin_half;
    (qx * factor, qy * factor, qz * factor, scaled.cos())
}

/// Compute the shortest-arc quaternion rotating vector `from` onto `to`
/// (both need not be normalised). Used for gravity-vector corrections.
#[allow(dead_code)]
fn quat_shortest_arc(from: (f64, f64, f64), to: (f64, f64, f64)) -> (f64, f64, f64, f64) {
    let (fx, fy, fz) = from;
    let (tx, ty, tz) = to;
    let cross = (fy * tz - fz * ty, fz * tx - fx * tz, fx * ty - fy * tx);
    let dot = fx * tx + fy * ty + fz * tz;
    let w = ((fx * fx + fy * fy + fz * fz) * (tx * tx + ty * ty + tz * tz)).sqrt() + dot;
    let mut q = (cross.0, cross.1, cross.2, w);
    let norm = (q.0 * q.0 + q.1 * q.1 + q.2 * q.2 + q.3 * q.3).sqrt();
    if norm < ATTITUDE_EPSILON {
        // Vectors are opposite; pick any perpendicular axis.
        return (1.0, 0.0, 0.0, 0.0);
    }
    q.0 /= norm;
    q.1 /= norm;
    q.2 /= norm;
    q.3 /= norm;
    q
}

/// Rotate world-frame vector `v` into the device frame expressed by
/// attitude quaternion `q` (q maps device orientation -> world; applying the
/// conjugate gives world->device).
#[allow(dead_code)]
fn quat_world_to_device(q: (f64, f64, f64, f64), v: (f64, f64, f64)) -> (f64, f64, f64) {
    let (qx, qy, qz, qw) = q;
    // v' = q^-1 * v * q, expanded without building intermediate quaternions.
    let (vx, vy, vz) = v;
    // t = 2 * q_vec x v
    let tx = 2.0 * (qy * vz - qz * vy);
    let ty = 2.0 * (qz * vx - qx * vz);
    let tz = 2.0 * (qx * vy - qy * vx);
    // v' = v - qw * t + q_vec x t ... (conjugate form of q v q*)
    (
        vx - qw * tx + (qy * tz - qz * ty),
        vy - qw * ty + (qz * tx - qx * tz),
        vz - qw * tz + (qx * ty - qy * tx),
    )
}

#[inline]
#[allow(dead_code)]
fn vec_len(v: (f64, f64, f64)) -> f64 {
    (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt()
}

#[inline]
#[allow(dead_code)]
fn vec_lerp(a: (f64, f64, f64), b: (f64, f64, f64), t: f64) -> (f64, f64, f64) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
    )
}

#[inline]
#[allow(dead_code)]
fn vec_sub(a: (f64, f64, f64), b: (f64, f64, f64)) -> (f64, f64, f64) {
    (a.0 - b.0, a.1 - b.1, a.2 - b.2)
}
