/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSNetService` / `NSNetServiceBrowser` — the Cocoa (Foundation) face of
//! Bonjour. Most iOS-era LAN multiplayer games use these classes directly
//! (rather than the C-level CFNetService API) to advertise and discover
//! peers. Both are thin wrappers over the real mDNS/DNS-SD stack in
//! `core_foundation::cf_net_service`.

use crate::frameworks::core_foundation::cf_net_service::{
    browse_services, cf_service_from_parts, cf_service_get_txt_records,
    cf_service_register_with_options, cf_service_resolve_with_timeout,
    cf_service_set_txt_data_with_dict, CFNetServiceRef,
};
use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::mem::MutVoidPtr;
use crate::objc::{autorelease, id, msg, msg_class, nil, objc_classes, ClassExports, HostObject};
use crate::Environment;
use std::time::Duration;

// NSNetServiceError domain/codes (NSNetServices.h).
const NS_NET_SERVICES_ERROR_DOMAIN: &str = "NSNetServicesErrorDomain";
// kNSNetServicesUnknownError_ etc. — only -72003 (activity in progress) and
// -72004 (bad argument) matter for callers that check codes.
const NS_NET_SERVICES_ACTIVITY_IN_PROGRESS: i32 = -72003;

/// Resolved data for a service, exposed through `addresses` / `hostName`.
#[derive(Default)]
struct NSNetServiceHostObject {
    domain: Option<id>,
    type_: Option<id>,
    name: Option<id>,
    port: i32,
    /// Underlying CFNetService used for register/resolve.
    service: CFNetServiceRef,
    /// Set once resolution has produced host/port data.
    resolved: bool,
    resolved_host: String,
    resolved_port: i32,
    resolved_addr: Option<std::net::Ipv4Addr>,
    /// Delegate + retained bookkeeping.
    delegate: id,
    /// Published state (publish / stop).
    published: bool,
    /// Socket fd hint stored by the game via TXT (unused by the wrapper).
    txt_data: id,
}
impl HostObject for NSNetServiceHostObject {}

#[derive(Default)]
struct NSNetServiceBrowserHostObject {
    delegate: id,
    /// Search state: `didStartSearch` has been sent, `didStopSearch` not yet.
    searching: bool,
}
impl HostObject for NSNetServiceBrowserHostObject {}

fn make_net_error(env: &mut Environment, code: i32, desc: &str) -> id {
    let domain = from_rust_string(env, NS_NET_SERVICES_ERROR_DOMAIN.to_string());
    autorelease(env, domain);
    let desc_val = from_rust_string(env, desc.to_string());
    autorelease(env, desc_val);
    let user_info: id = msg_class![env; NSMutableDictionary new];
    autorelease(env, user_info);
    let key =
        crate::frameworks::foundation::ns_string::get_static_str(env, "NSLocalizedDescription");
    () = msg![env; user_info setObject:desc_val forKey:key];
    let error: id = msg_class![env; NSError alloc];
    let error: id = msg![env; error initWithDomain:domain code:code userInfo:user_info];
    autorelease(env, error);
    error
}

#[allow(clippy::if_same_then_else)]
fn parse_service_type(raw: &str) -> String {
    // NSNetService accepts "_type._tcp" (no domain) or "_type._tcp.local.".
    let t = raw.trim().trim_end_matches('.').to_string();
    if t.is_empty() {
        t
    } else if t.contains('.') {
        t
    } else {
        format!("{}._tcp", t)
    }
}

fn parse_domain(raw: &str) -> String {
    let d = raw.trim().trim_end_matches('.').to_string();
    if d.is_empty() || d.eq_ignore_ascii_case("local") {
        "local".to_string()
    } else {
        d
    }
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// MARK: - NSNetService

@implementation NSNetService: NSObject

- (id)initWithDomain:(id)domain // NSString*
                 type:(id)type_ // NSString*
                 name:(id)name // NSString*
                 port:(i32)port {
    let host = Box::new(NSNetServiceHostObject {
        domain: if domain == nil { None } else { Some(domain) },
        type_: if type_ == nil { None } else { Some(type_) },
        name: if name == nil { None } else { Some(name) },
        port,
        ..Default::default()
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

// Common convenience initializer used by games.
- (id)init {
    let host = Box::new(NSNetServiceHostObject::default());
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)name {
    env.objc.borrow::<NSNetServiceHostObject>(this)
        .name
        .unwrap_or(nil)
}

- (id)type {
    env.objc.borrow::<NSNetServiceHostObject>(this)
        .type_
        .unwrap_or(nil)
}

- (id)domain {
    env.objc.borrow::<NSNetServiceHostObject>(this)
        .domain
        .unwrap_or(nil)
}

- (i32)port {
    let host = env.objc.borrow::<NSNetServiceHostObject>(this);
    if host.resolved && host.resolved_port != 0 {
        host.resolved_port
    } else {
        host.port
    }
}

- (id)hostName {
    let host = env.objc.borrow::<NSNetServiceHostObject>(this);
    if host.resolved && !host.resolved_host.is_empty() {
        from_rust_string(env, host.resolved_host.clone())
    } else {
        nil
    }
}

- (id)addresses {
    // NSArray of NSData, each a BSD `struct sockaddr` (16 bytes) — the
    // documented representation. We only ever produce IPv4 entries.
    let host = env.objc.borrow::<NSNetServiceHostObject>(this);
    let Some(addr) = host.resolved_addr else {
        return msg_class![env; NSArray array];
    };
    let port = host.resolved_port as u16;

    // Build the guest sockaddr_in: sin_len, sin_family, sin_port (BE), addr.
    let mut sa = [0u8; 16];
    sa[0] = 16;
    sa[1] = 2; // AF_INET
    sa[2..4].copy_from_slice(&port.to_be_bytes());
    sa[4..8].copy_from_slice(&addr.octets());

    let ptr: MutVoidPtr = env.mem.alloc(16);
    env.mem.bytes_at_mut(ptr.cast(), 16).copy_from_slice(&sa);
    let bytes_ptr = ptr.cast_const().cast_void();
    let data: id = msg_class![env; NSData dataWithBytes:bytes_ptr length:16u32];
    env.mem.free(ptr);
    autorelease(env, data);

    let arr: id = msg_class![env; NSArray arrayWithObject:data];
    arr
}

- (id)delegate {
    env.objc.borrow::<NSNetServiceHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<NSNetServiceHostObject>(this).delegate = delegate;
}

- (())setTXTRecordData:(id)data { // NSData*
    let host = env.objc.borrow_mut::<NSNetServiceHostObject>(this);
    host.txt_data = data;
    let service = host.service;
    if service != nil {
        cf_service_set_txt_data_with_dict(env, service, data);
    }
}

- (id)TXTRecordData {
    let host = env.objc.borrow::<NSNetServiceHostObject>(this);
    if host.txt_data != nil {
        return host.txt_data;
    }
    if host.service != nil {
        return cf_service_get_txt_records(env, host.service);
    }
    nil
}

- (bool)publish {
    let (service, published) = {
        let host = env.objc.borrow_mut::<NSNetServiceHostObject>(this);
        if host.published {
            return true;
        }
        let domain_id = host.domain;
        let type_id = host.type_;
        let name_id = host.name;
        let port = host.port;
        let domain = domain_id
            .map(|d| {
                let s = to_rust_string(env, d).into_owned();
                parse_domain(&s)
            })
            .unwrap_or_else(|| "local".to_string());
        let type_ = type_id
            .map(|t| {
                let s = to_rust_string(env, t).into_owned();
                parse_service_type(&s)
            })
            .unwrap_or_default();
        let name = name_id
            .map(|n| to_rust_string(env, n).into_owned())
            .unwrap_or_default();
        if type_.is_empty() || name.is_empty() {
            return false;
        }
        let service =
            cf_service_from_parts(env, &domain, &type_, &name, port);
        if service == nil {
            return false;
        }
        // Push any TXT data the game set before publishing.
        {
            let host2 = env.objc.borrow_mut::<NSNetServiceHostObject>(this);
            let txt = host2.txt_data;
            if txt != nil {
                cf_service_set_txt_data_with_dict(env, service, txt);
            }
        }
        let ok = cf_service_register_with_options(env, service, 0, MutVoidPtr::from_bits(0).cast());
        let host = env.objc.borrow_mut::<NSNetServiceHostObject>(this);
        host.service = service;
        host.published = ok;
        (service, ok)
    };
    let _ = service;
    if published {
        log_dbg!("NSNetService: published '{}'", "service");
        let delegate = env.objc.borrow::<NSNetServiceHostObject>(this).delegate;
        if delegate != nil {
            () = msg![env; delegate netServiceWillPublish:this];
        }
    }
    published
}

- (())publishInDomain:(id)_domain {
    let _: bool = msg![env; this publish];
}

- (())stop {
    let host = env.objc.borrow_mut::<NSNetServiceHostObject>(this);
    let was = host.published;
    host.published = false;
    if was {
        let delegate = env.objc.borrow::<NSNetServiceHostObject>(this).delegate;
        if delegate != nil {
            () = msg![env; delegate netServiceDidStop:this];
        }
    }
}

- (())resolve {
    let _: bool = msg![env; this resolveWithTimeout:5.0f64];
}

- (bool)resolveWithTimeout:(f64)timeout {
    let (service, ok, delegate) = {
        let (domain_id, type_id, name_id, existing_service) = {
            let host = env.objc.borrow::<NSNetServiceHostObject>(this);
            (host.domain, host.type_, host.name, host.service)
        };
        let (domain, type_, name) = {
            let d = domain_id
                .map(|d| {
                    let s = to_rust_string(env, d).into_owned();
                    parse_domain(&s)
                })
                .unwrap_or_else(|| "local".to_string());
            let t = type_id
                .map(|t| {
                    let s = to_rust_string(env, t).into_owned();
                    parse_service_type(&s)
                })
                .unwrap_or_default();
            let n = name_id
                .map(|n| to_rust_string(env, n).into_owned())
                .unwrap_or_default();
            (d, t, n)
        };
        if type_.is_empty() || name.is_empty() {
            return false;
        }
        let service = if existing_service != nil {
            existing_service
        } else {
            cf_service_from_parts(env, &domain, &type_, &name, 0)
        };
        if service == nil {
            return false;
        }
        {
            let host = env.objc.borrow_mut::<NSNetServiceHostObject>(this);
            host.service = service;
        }
        let ok = cf_service_resolve_with_timeout(
            env,
            service,
            timeout,
            MutVoidPtr::from_bits(0).cast(),
        );
        if ok {
            let (r_host, r_port, r_addr) = {
                let cf_host = env
                    .objc
                    .borrow::<crate::frameworks::core_foundation::cf_net_service::CFNetServiceHostObject>(service);
                (
                    cf_host.resolved_host.clone().unwrap_or_default(),
                    cf_host.resolved_port,
                    cf_host.resolved_addr,
                )
            };
            let host = env.objc.borrow_mut::<NSNetServiceHostObject>(this);
            host.resolved = true;
            host.resolved_host = r_host;
            host.resolved_port = r_port as i32;
            host.resolved_addr = r_addr;
        }
        let delegate = env.objc.borrow::<NSNetServiceHostObject>(this).delegate;
        (service, ok, delegate)
    };
    let _ = service;
    if delegate != nil {
        if ok {
            () = msg![env; delegate netServiceDidResolveAddress:this];
        } else {
            let error = make_net_error(
                env,
                NS_NET_SERVICES_ACTIVITY_IN_PROGRESS,
                "The service could not be resolved.",
            );
            () = msg![env; delegate netService:this didNotResolve:error];
        }
    }
    ok
}

- (())removeFromRunLoop:(id)_rl forMode:(id)_mode {}
- (())scheduleInRunLoop:(id)_rl forMode:(id)_mode {}
- (())startMonitoring {}
- (())stopMonitoring {}

- (id)description {
    let host = env.objc.borrow::<NSNetServiceHostObject>(this);
    let name = host
        .name
        .map(|n| to_rust_string(env, n).into_owned())
        .unwrap_or_default();
    from_rust_string(env, format!("<NSNetService {}>", name))
}

@end

// MARK: - NSNetServiceBrowser

@implementation NSNetServiceBrowser: NSObject

- (id)init {
    let host = Box::new(NSNetServiceBrowserHostObject::default());
    env.objc.alloc_object(this, host, &mut env.mem)
}

- (id)delegate {
    env.objc.borrow::<NSNetServiceBrowserHostObject>(this).delegate
}

- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<NSNetServiceBrowserHostObject>(this).delegate = delegate;
}

- (())searchForServicesOfType:(id)type_ inDomain:(id)domain {
    let type_str = if type_ == nil {
        String::new()
    } else {
        parse_service_type(&to_rust_string(env, type_))
    };
    let domain_str = if domain == nil {
        "local".to_string()
    } else {
        parse_domain(&to_rust_string(env, domain))
    };
    let delegate = env.objc.borrow::<NSNetServiceBrowserHostObject>(this).delegate;
    if delegate != nil {
        () = msg![env; delegate netServiceBrowserWillSearch:this];
    }

    if type_str.is_empty() {
        if delegate != nil {
            let error = make_net_error(
                env,
                NS_NET_SERVICES_ACTIVITY_IN_PROGRESS,
                "The search could not be started.",
            );
            () = msg![env; delegate netServiceBrowser:this
                          didNotSearch:error];
        }
        return;
    }

    // Run a real mDNS browse (synchronous window) and report each instance.
    let results = browse_services(&type_str, &domain_str, Duration::from_millis(1200));
    let delegate = env.objc.borrow::<NSNetServiceBrowserHostObject>(this).delegate;
    if delegate != nil {
        let count = results.len();
        for (idx, svc) in results.iter().enumerate() {
            // Instance label: strip trailing `._type._tcp.local`.
            let instance = svc
                .full_name
                .rsplit_once(&format!(".{}", type_str))
                .map(|(i, _)| i.to_string())
                .unwrap_or_else(|| svc.full_name.clone());
            let instance = instance.replace("\\.", ".");
            let type_ns = from_rust_string(env, type_str.clone());
            autorelease(env, type_ns);
            let domain_ns = from_rust_string(env, domain_str.clone());
            autorelease(env, domain_ns);
            let name_ns = from_rust_string(env, instance);
            autorelease(env, name_ns);
            let service: id = msg_class![env; NSNetService alloc];
            let service: id = msg![env; service initWithDomain:domain_ns
                                                         type:type_ns
                                                         name:name_ns
                                                         port:0i32];
            autorelease(env, service);
            // Pre-resolve inline data if the browse already got it.
            if svc.port != 0 {
                let host = env.objc.borrow_mut::<NSNetServiceHostObject>(service);
                host.resolved = true;
                host.resolved_port = svc.port as i32;
                host.resolved_host = svc.host.clone();
                host.resolved_addr = svc.addr;
            }
            let more_coming = idx + 1 < count;
            () = msg![env; delegate netServiceBrowser:this
                                          didFindService:service
                                          moreComing:more_coming];
        }
        if count == 0 {
            // Report an empty result set: browsers expect either found
            // services or nothing — no error for a quiet network.
        }
    }
    env.objc
        .borrow_mut::<NSNetServiceBrowserHostObject>(this)
        .searching = true;
}

- (())stopSearch {
    let host = env.objc.borrow_mut::<NSNetServiceBrowserHostObject>(this);
    let was = host.searching;
    host.searching = false;
    if was {
        let delegate = env.objc.borrow::<NSNetServiceBrowserHostObject>(this).delegate;
        if delegate != nil {
            () = msg![env; delegate netServiceBrowserDidStopSearch:this];
        }
    }
}

- (())removeFromRunLoop:(id)_rl forMode:(id)_mode {}
- (())scheduleInRunLoop:(id)_rl forMode:(id)_mode {}

@end

};
