/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! SCNetworkReachability

use crate::abi::GuestFunction;
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::core_foundation::cf_allocator::{kCFAllocatorDefault, CFAllocatorRef};
use crate::frameworks::core_foundation::CFTypeRef;
use crate::libc::sys::socket::sockaddr;
use crate::mem::{ConstPtr, MutPtr, MutVoidPtr, Ptr};
use crate::objc::{msg, objc_classes, Class, ClassExports, HostObject};
use crate::Environment;
use std::net::SocketAddrV4;

type SCNetworkReachabilityFlags = u32;
const kSCNetworkReachabilityFlagsReachable: SCNetworkReachabilityFlags = 1 << 1;
const kSCNetworkReachabilityFlagsIsDirect: SCNetworkReachabilityFlags = 1 << 17;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// SCNetworkReachabilityRef is not explicitly stated to be CFType-based type,
// but the result of "Create" or "Copy" functions here is expected to be
// released with CFRelease().
@implementation _touchHLE_SCNetworkReachability: NSObject
@end

};

struct SCNetworkReachabilityHostObject {
    address: Option<SocketAddrV4>,
    /// Created from a host name rather than an address.
    named: bool,
}
impl HostObject for SCNetworkReachabilityHostObject {}

// See comment for `_touchHLE_SCNetworkReachability` class
type SCNetworkReachabilityRef = CFTypeRef;

fn SCNetworkReachabilityCreateWithName(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    name: ConstPtr<u8>,
) -> SCNetworkReachabilityRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    if env
        .bundle
        .bundle_identifier()
        .starts_with("com.chillingo.cuttherope")
        && env.mem.cstr_at_utf8(name).unwrap() == "chillingo-crystal.appspot.com"
    {
        log!("Applying game-specific hack for Cut the Rope: SCNetworkReachabilityCreateWithName(\"chillingo-crystal.appspot.com\") returns NULL");
        return Ptr::null();
    }
    let isa = env
        .objc
        .get_known_class("_touchHLE_SCNetworkReachability", &mut env.mem);
    if env.options.network_trace {
        let name = env.mem.cstr_at_utf8(name).ok().map(str::to_owned);
        crate::network_trace::record(
            env,
            "reachability_name",
            serde_json::json!({ "name": name }),
        );
    }
    let res = env.objc.alloc_object(
        isa,
        Box::new(SCNetworkReachabilityHostObject {
            address: None,
            named: true,
        }),
        &mut env.mem,
    );
    log!(
        "TODO: SCNetworkReachabilityCreateWithName({:?}, {:?} {:?}) -> {:?}",
        allocator,
        name,
        env.mem.cstr_at_utf8(name),
        res
    );
    res
}

fn SCNetworkReachabilityCreateWithAddress(
    env: &mut Environment,
    allocator: CFAllocatorRef,
    address: ConstPtr<sockaddr>,
) -> SCNetworkReachabilityRef {
    assert!(allocator == kCFAllocatorDefault || env.mem.read(allocator).is_system_default()); // unimplemented
    let isa = env
        .objc
        .get_known_class("_touchHLE_SCNetworkReachability", &mut env.mem);
    let address_val = env.mem.read(address);
    if env.options.network_trace {
        crate::network_trace::record(
            env,
            "reachability_address",
            serde_json::json!({
                "address":address_val.to_sockaddr_v4().to_string()
            }),
        );
    }
    let res = env.objc.alloc_object(
        isa,
        Box::new(SCNetworkReachabilityHostObject {
            address: Some(address_val.to_sockaddr_v4()),
            named: false,
        }),
        &mut env.mem,
    );
    log_dbg!(
        "SCNetworkReachabilityCreateWithAddress({:?}, {:?} ({})) -> {:?}",
        allocator,
        address,
        address_val.to_sockaddr_v4(),
        res
    );
    res
}

fn SCNetworkReachabilityGetFlags(
    env: &mut Environment,
    target: SCNetworkReachabilityRef,
    flags: MutPtr<SCNetworkReachabilityFlags>,
) -> bool {
    if !env.options.network_access {
        if env.options.network_trace {
            crate::network_trace::record(
                env,
                "reachability_result",
                serde_json::json!({
                    "target":target.to_bits(), "network_access":false,
                    "success":false, "flags_written":false
                }),
            );
        }
        log_dbg!(
            "Network access is disabled, SCNetworkReachabilityGetFlags({:?}, {:?}) -> false",
            target,
            flags
        );
        return false;
    }

    let target_class: Class = msg![env; target class];
    assert_eq!(
        target_class,
        env.objc
            .get_known_class("_touchHLE_SCNetworkReachability", &mut env.mem)
    );
    let host_object = env.objc.borrow::<SCNetworkReachabilityHostObject>(target);
    let local_default = host_object.named
        || host_object
            .address
            .is_some_and(|addr| addr.ip().is_unspecified());
    // MHP2G queries the default route before it calls GameKit, and MHFU does
    // the same with a host name (www.capcom.co.jp). A name can't be resolved
    // without sending packets, so it is answered like the default route. Keep
    // the old behavior unless the local backend is explicitly enabled, and
    // report an actual OS route snapshot rather than treating configuration as
    // success.
    if local_default {
        if let Some(reachable) = crate::frameworks::game_kit::local_route(env) {
            let out_flags = if reachable {
                kSCNetworkReachabilityFlagsReachable
            } else {
                0
            };
            env.mem.write(flags, out_flags);
            if env.options.network_trace {
                crate::network_trace::record(
                    env,
                    "reachability_result",
                    serde_json::json!({
                        "target":target.to_bits(),"success":true,"flags_written":true,
                        "flags":out_flags,"source":"local_gamekit_os_route_no_peer_probe"
                    }),
                );
            }
            return true;
        }
    }
    let host_object = env.objc.borrow::<SCNetworkReachabilityHostObject>(target);
    if let Some(addr) = host_object.address {
        if addr.ip().is_link_local() {
            log_dbg!(
                "SCNetworkReachabilityGetFlags({:?}, {:?}) -> true",
                target,
                flags
            );
            // Those corresponds to local WiFi connection on a real iOS device
            // TODO: actually check for the connectivity
            // (but do we _really_ need it?)
            let out_flags =
                kSCNetworkReachabilityFlagsReachable | kSCNetworkReachabilityFlagsIsDirect;
            env.mem.write(flags, out_flags);
            if env.options.network_trace {
                crate::network_trace::record(
                    env,
                    "reachability_result",
                    serde_json::json!({
                        "target":target.to_bits(), "network_access":true,
                        "success":true, "flags_written":true, "flags":out_flags,
                        "source":"existing_link_local_shortcut"
                    }),
                );
            }
            return true;
        }
    }
    log!(
        "TODO: SCNetworkReachabilityGetFlags({:?}, {:?}) -> false",
        target,
        flags
    );
    false
}

fn SCNetworkReachabilitySetCallback(
    env: &mut Environment,
    target: SCNetworkReachabilityRef,
    callout: GuestFunction, // SCNetworkReachabilityCallBack
    context: MutVoidPtr,    // SCNetworkReachabilityContext *
) -> bool {
    crate::network_trace::callback_not_scheduled(
        env,
        "SCNetworkReachabilitySetCallback",
        callout.addr_with_thumb_bit(),
    );
    let target_class: Class = msg![env; target class];
    assert_eq!(
        target_class,
        env.objc
            .get_known_class("_touchHLE_SCNetworkReachability", &mut env.mem)
    );
    log!(
        "TODO: SCNetworkReachabilitySetCallback({:?}, {:?}, {:?}) -> FALSE",
        target,
        callout,
        context
    );
    false
}

fn SCNetworkReachabilityScheduleWithRunLoop(
    _env: &mut Environment,
    target: SCNetworkReachabilityRef,
    run_loop: CFTypeRef,      // CFRunLoopRef
    run_loop_mode: CFTypeRef, // CFStringRef
) -> bool {
    // Callbacks aren't supported (see SCNetworkReachabilitySetCallback).
    log!(
        "TODO: SCNetworkReachabilityScheduleWithRunLoop({:?}, {:?}, {:?}) -> FALSE",
        target,
        run_loop,
        run_loop_mode
    );
    false
}

fn SCNetworkReachabilityUnscheduleFromRunLoop(
    _env: &mut Environment,
    target: SCNetworkReachabilityRef,
    run_loop: CFTypeRef,      // CFRunLoopRef
    run_loop_mode: CFTypeRef, // CFStringRef
) -> bool {
    log!(
        "TODO: SCNetworkReachabilityUnscheduleFromRunLoop({:?}, {:?}, {:?}) -> FALSE",
        target,
        run_loop,
        run_loop_mode
    );
    false
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(SCNetworkReachabilityCreateWithName(_, _)),
    export_c_func!(SCNetworkReachabilityCreateWithAddress(_, _)),
    export_c_func!(SCNetworkReachabilityGetFlags(_, _)),
    export_c_func!(SCNetworkReachabilitySetCallback(_, _, _)),
    export_c_func!(SCNetworkReachabilityScheduleWithRunLoop(_, _, _)),
    export_c_func!(SCNetworkReachabilityUnscheduleFromRunLoop(_, _, _)),
];
