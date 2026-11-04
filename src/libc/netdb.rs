/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `netdb.h`

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::libc::sys::socket::AF_INET6;
use crate::libc::sys::socket::{sockaddr, AF_INET, SOCK_DGRAM, SOCK_STREAM};
use crate::mem::{guest_size_of, ConstPtr, MutPtr, Ptr, SafeRead};
use crate::Environment;
use std::net::IpAddr;
use std::net::Ipv4Addr;
use std::net::Ipv6Addr;
use std::net::ToSocketAddrs;

const AI_PASSIVE: i32 = 0x1;

pub const IPPROTO_TCP: i32 = 6;
pub const IPPROTO_UDP: i32 = 17;

const EAI_FAIL: i32 = 4;

#[allow(non_camel_case_types)]
pub type socklen_t = u32;

// TODO: struct definition
#[allow(non_camel_case_types)]
struct hostent {}

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
#[allow(non_camel_case_types)]
pub struct addrinfo {
    ai_flags: i32,
    ai_family: i32,
    ai_socktype: i32,
    ai_protocol: i32,
    ai_addrlen: socklen_t,
    ai_canonname: MutPtr<u8>,
    ai_addr: MutPtr<sockaddr>,
    ai_next: MutPtr<addrinfo>,
}
unsafe impl SafeRead for addrinfo {}

fn getaddrinfo(
    env: &mut Environment,
    node_name: MutPtr<u8>,
    serv_name: MutPtr<u8>,
    hints: ConstPtr<addrinfo>,
    res: MutPtr<MutPtr<addrinfo>>,
) -> i32 {
    if !env.options.network_access {
        log_dbg!(
            "Network access is disabled, getaddrinfo({:?}, {:?}, {:?}, {:?}) -> EAI_FAIL",
            node_name,
            serv_name,
            hints,
            res
        );
        return EAI_FAIL;
    }

    assert!(node_name.is_null()); // TODO

    let hint = env.mem.read(hints);
    let ai_flags = hint.ai_flags;
    assert_eq!(ai_flags, AI_PASSIVE);
    let ai_family = hint.ai_family;
    assert_eq!(ai_family, AF_INET);
    assert!(hint.ai_socktype == SOCK_STREAM || hint.ai_socktype == SOCK_DGRAM);
    assert!(
        hint.ai_protocol == IPPROTO_TCP || hint.ai_protocol == IPPROTO_UDP || hint.ai_protocol == 0
    );
    let ai_addrlen = hint.ai_addrlen;
    assert_eq!(ai_addrlen, 0);
    assert!(hint.ai_canonname.is_null());
    assert!(hint.ai_addr.is_null());
    assert!(hint.ai_next.is_null());

    let mut addr_info = hint;
    let port: u16 = env.mem.cstr_at_utf8(serv_name).unwrap().parse().unwrap();
    log_dbg!("getaddrinfo: port {}", port);
    let addr = sockaddr::from_ipv4_parts([0; 4], port);

    let tmp_addr = env.mem.alloc_and_write(addr);
    addr_info.ai_addr = tmp_addr;
    addr_info.ai_addrlen = guest_size_of::<sockaddr>();

    let tmp_addr_info = env.mem.alloc_and_write(addr_info);
    env.mem.write(res, tmp_addr_info);

    0 // Success
}

fn freeaddrinfo(env: &mut Environment, addrinfo: MutPtr<addrinfo>) {
    let addrinfo_val = env.mem.read(addrinfo);
    assert!(addrinfo_val.ai_next.is_null()); // TODO
    let ai_addrlen = addrinfo_val.ai_addrlen;
    assert_eq!(ai_addrlen, guest_size_of::<sockaddr>());
    env.mem.free(addrinfo_val.ai_addr.cast());
    env.mem.free(addrinfo.cast());
}

fn gethostbyname(env: &mut Environment, name: ConstPtr<u8>) -> MutPtr<hostent> {
    log!(
        "TODO: gethostbyname({:?} \"{}\") => NULL",
        name,
        env.mem.cstr_at_utf8(name).unwrap()
    );
    // TODO: set h_errno
    Ptr::null()
}

fn gethostent(_env: &mut Environment) -> MutPtr<hostent> {
    log!("TODO: gethostent() => NULL");
    // TODO: set h_errno
    Ptr::null()
}

fn __h_errno_location(env: &mut Environment) -> MutPtr<i32> {
    // Apple's `netdb.h` (`__h_errno_location` is the thread-local accessor
    // used by the macro `#define h_errno (*__h_errno_location())`) must
    // return a stable pointer to the *single* `h_errno` cell so that
    // `&h_errno` is well-defined and the value survives across calls.
    // Allocating a fresh cell each call — as a previous revision did —
    // breaks `errno`-style code (`*__h_errno_location() = 0;`) because
    // the next read sees a stale address. Funnel everyone through
    // `h_errno_ptr` which allocates once and caches in `State`.
    h_errno_ptr(env)
}

fn freehostent(env: &mut Environment, hostent: MutPtr<u8>) {
    if hostent.is_null() {
        return;
    }

    if hostent.to_bits() == env.libc_state.netdb.dummy_hostent_ptr {
        env.libc_state.netdb.dummy_hostent_ptr = 0;
    }
    env.mem.free(hostent.cast());
}

fn gai_strerror(env: &mut Environment, ecode: i32) -> ConstPtr<u8> {
    let msg: &[u8] = match ecode {
        0 => b"Success\0",
        EAI_AGAIN => b"Temporary failure in name resolution\0",
        EAI_FAIL => b"Non-recoverable failure in name resolution\0",
        EAI_FAMILY => b"ai_family not supported\0",
        EAI_SERVICE => b"Servname not supported for ai_socktype\0",
        EAI_SOCKTYPE => b"ai_socktype not supported\0",
        EAI_PROTOCOL => b"ai_protocol not supported\0",
        EAI_MEMORY => b"Memory allocation failure\0",
        EAI_OVERFLOW => b"Argument buffer overflow\0",
        EAI_SYSTEM => b"System error\0",
        _ => b"Unknown error\0",
    };
    env.mem
        .alloc_and_write_cstr(&msg[..msg.len() - 1])
        .cast_const()
}

fn gethostbyaddr(
    env: &mut Environment,
    addr: ConstPtr<u8>,
    len: socklen_t,
    type_: i32,
) -> MutPtr<u8> {
    set_h_errno(env, H_ERRNO_SUCCESS);
    let address_len = match type_ {
        AF_INET => 4,
        AF_INET6 => 16,
        _ => {
            log!("gethostbyaddr: unsupported family {}", type_);
            set_h_errno(env, H_ERRNO_NO_RECOVERY);
            return MutPtr::null();
        }
    };
    if addr.is_null() || len < address_len {
        log!(
            "gethostbyaddr: address length {} is shorter than {}",
            len,
            address_len
        );
        set_h_errno(env, H_ERRNO_NO_RECOVERY);
        return MutPtr::null();
    }

    let octets = env.mem.bytes_at(addr, address_len).to_vec();
    let address = match type_ {
        AF_INET => IpAddr::V4(Ipv4Addr::new(octets[0], octets[1], octets[2], octets[3])),
        AF_INET6 => IpAddr::V6(Ipv6Addr::from(
            <[u8; 16]>::try_from(octets.as_slice()).unwrap(),
        )),
        _ => unreachable!(),
    };
    let name = address.to_string();
    log_dbg!("gethostbyaddr({}) -> returning numeric form", name);
    if env.libc_state.netdb.dummy_hostent_ptr != 0 {
        let old: MutPtr<u8> = MutPtr::from_bits(env.libc_state.netdb.dummy_hostent_ptr);
        env.mem.free(old.cast());
    }

    let ptr = alloc_hostent(env, &octets, type_, &name);
    env.libc_state.netdb.dummy_hostent_ptr = ptr;
    MutPtr::from_bits(ptr)
}

fn gethostbyname2(env: &mut Environment, name: ConstPtr<u8>, af: i32) -> MutPtr<u8> {
    if af == AF_INET {
        return gethostbyname(env, name).cast();
    }
    set_h_errno(env, H_ERRNO_SUCCESS);
    if af != AF_INET6 {
        log!("gethostbyname2: unsupported address family {}", af);
        set_h_errno(env, H_ERRNO_NO_RECOVERY);
        return MutPtr::null();
    }

    let hostname = if name.is_null() {
        "localhost".to_owned()
    } else {
        env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned()
    };
    let addresses =
        resolve_host_addresses(&hostname, env.options.network_access).unwrap_or_default();
    let Some(IpAddr::V6(address)) = addresses_for_family(&addresses, AF_INET6, 0)
        .into_iter()
        .next()
    else {
        log!("gethostbyname2(\"{}\"): no IPv6 address", hostname);
        set_h_errno(env, H_ERRNO_NO_DATA);
        return MutPtr::null();
    };

    if env.libc_state.netdb.dummy_hostent_ptr != 0 {
        let old: MutPtr<u8> = MutPtr::from_bits(env.libc_state.netdb.dummy_hostent_ptr);
        env.mem.free(old.cast());
    }
    let octets = address.octets();
    let ptr = alloc_hostent(env, &octets, AF_INET6, &hostname);
    env.libc_state.netdb.dummy_hostent_ptr = ptr;
    log_dbg!("gethostbyname2(\"{}\") -> {}", hostname, address);
    MutPtr::from_bits(ptr)
}

fn getipnodebyname(
    env: &mut Environment,
    name: ConstPtr<u8>,
    af: i32,
    flags: i32,
    error_num: MutPtr<i32>,
) -> MutPtr<u8> {
    set_h_errno(env, H_ERRNO_SUCCESS);
    let host = if name.is_null() {
        "localhost".to_owned()
    } else {
        env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned()
    };
    let allowed_flags = AI_V4MAPPED | AI_ALL | AI_ADDRCONFIG;

    if !error_num.is_null() {
        env.mem.write(error_num, 0);
    }
    if flags & !allowed_flags != 0 {
        set_h_errno(env, H_ERRNO_NO_RECOVERY);
        if !error_num.is_null() {
            env.mem.write(error_num, NO_RECOVERY);
        }
        log!(
            "getipnodebyname(\"{}\"): unsupported flags 0x{:x} -> NO_RECOVERY",
            host,
            flags
        );
        return MutPtr::null();
    }
    if af != AF_INET && af != AF_INET6 {
        set_h_errno(env, H_ERRNO_NO_RECOVERY);
        if !error_num.is_null() {
            env.mem.write(error_num, NO_RECOVERY);
        }
        log!(
            "getipnodebyname(\"{}\"): unsupported family {} -> NO_RECOVERY",
            host,
            af
        );
        return MutPtr::null();
    }
    if af == AF_INET {
        let result = gethostbyname(env, name).cast();
        if result.is_null() && !error_num.is_null() {
            env.mem.write(error_num, H_ERRNO_HOST_NOT_FOUND);
        }
        return result;
    }

    let Some(addresses) = resolve_host_addresses(&host, env.options.network_access) else {
        set_h_errno(env, H_ERRNO_HOST_NOT_FOUND);
        if !error_num.is_null() {
            env.mem.write(error_num, H_ERRNO_HOST_NOT_FOUND);
        }
        return MutPtr::null();
    };
    let Some(IpAddr::V6(address)) = addresses_for_family(&addresses, AF_INET6, flags)
        .into_iter()
        .next()
    else {
        set_h_errno(env, H_ERRNO_NO_DATA);
        if !error_num.is_null() {
            env.mem.write(error_num, H_ERRNO_NO_DATA);
        }
        return MutPtr::null();
    };

    if env.libc_state.netdb.dummy_hostent_ptr != 0 {
        let old: MutPtr<u8> = MutPtr::from_bits(env.libc_state.netdb.dummy_hostent_ptr);
        env.mem.free(old.cast());
    }
    let octets = address.octets();
    let ptr = alloc_hostent(env, &octets, AF_INET6, &host);
    env.libc_state.netdb.dummy_hostent_ptr = ptr;
    if !error_num.is_null() {
        env.mem.write(error_num, 0);
    }
    log_dbg!("getipnodebyname(\"{}\") -> {}", host, address);
    MutPtr::from_bits(ptr)
}

#[allow(clippy::too_many_arguments)]
fn getnameinfo(
    env: &mut Environment,
    sa: ConstPtr<sockaddr>,
    salen: socklen_t,
    host: MutPtr<u8>,
    hostlen: u32,
    serv: MutPtr<u8>,
    servlen: u32,
    flags: i32,
) -> i32 {
    let address = match crate::libc::sys::socket::read_socket_addr(env, sa, salen) {
        Ok(address) => address,
        Err(crate::libc::errno::EAFNOSUPPORT) => return EAI_FAMILY,
        Err(_) => return EAI_FAIL,
    };
    let port = address.port();

    if !host.is_null() && hostlen > 0 {
        let host_string = format!("{}\0", address.ip());
        let bytes = host_string.as_bytes();
        let copy_len = bytes.len().min(hostlen as usize);
        for (i, &byte) in bytes[..copy_len].iter().enumerate() {
            env.mem.write(host + i as u32, byte);
        }
        env.mem.write(host + (copy_len - 1) as u32, 0u8);
    }

    if !serv.is_null() && servlen > 0 {
        let service = if flags & NI_NUMERICSERV != 0 {
            format!("{}\0", port)
        } else {
            match port_service(port) {
                Some(name) => format!("{}\0", name),
                None => format!("{}\0", port),
            }
        };
        let bytes = service.as_bytes();
        let copy_len = bytes.len().min(servlen as usize);
        for (i, &byte) in bytes[..copy_len].iter().enumerate() {
            env.mem.write(serv + i as u32, byte);
        }
        env.mem.write(serv + (copy_len - 1) as u32, 0u8);
    }

    0
}

fn getservbyname(env: &mut Environment, name: ConstPtr<u8>, proto: ConstPtr<u8>) -> MutPtr<u8> {
    let name_str = env.mem.cstr_at_utf8(name).unwrap_or_default().to_owned();
    let port = match service_port(&name_str) {
        Some(p) => p,
        None => {
            log!("getservbyname(\"{}\"): unknown service", name_str);
            return MutPtr::null();
        }
    };

    let proto_str = if proto.is_null() {
        "tcp".to_string()
    } else {
        env.mem.cstr_at_utf8(proto).unwrap_or_default().to_owned()
    };
    // port in network byte order (big-endian).
    let port_nbo = port.to_be() as i32;
    alloc_servent(env, &name_str, port_nbo, &proto_str)
}

fn getservbyport(env: &mut Environment, port_nbo: i32, proto: ConstPtr<u8>) -> MutPtr<u8> {
    let port_hbo = u16::from_be((port_nbo as u16).to_be());
    let name = match port_service(port_hbo) {
        Some(n) => n,
        None => {
            log!("getservbyport({}): unknown port", port_hbo);
            return MutPtr::null();
        }
    };

    let proto_str = if proto.is_null() {
        "tcp".to_string()
    } else {
        env.mem.cstr_at_utf8(proto).unwrap_or_default().to_owned()
    };
    alloc_servent(env, name, port_nbo, &proto_str)
}

const AI_ADDRCONFIG: i32 = 0x400;

const AI_ALL: i32 = 0x100;

const AI_V4MAPPED: i32 = 0x800;

const EAI_FAMILY: i32 = 5;

pub const H_ERRNO_HOST_NOT_FOUND: i32 = HOST_NOT_FOUND;

pub const H_ERRNO_NO_DATA: i32 = NO_DATA;

pub const H_ERRNO_NO_RECOVERY: i32 = NO_RECOVERY;

// h_errno values stored in libc state.
pub const H_ERRNO_SUCCESS: i32 = 0;

const NI_NUMERICSERV: i32 = 0x08;

const NO_RECOVERY: i32 = 3;

fn addresses_for_family(addresses: &[IpAddr], family: i32, flags: i32) -> Vec<IpAddr> {
    match family {
        AF_INET => addresses.iter().copied().filter(IpAddr::is_ipv4).collect(),
        AF_INET6 => {
            let mut ipv6: Vec<IpAddr> = addresses.iter().copied().filter(IpAddr::is_ipv6).collect();
            let include_ipv4 = flags & AI_V4MAPPED != 0 && (flags & AI_ALL != 0 || ipv6.is_empty());
            if include_ipv4 {
                ipv6.extend(addresses.iter().filter_map(|address| match address {
                    IpAddr::V4(ip) => Some(IpAddr::V6(ip.to_ipv6_mapped())),
                    IpAddr::V6(_) => None,
                }));
            }
            ipv6
        }
        0 => addresses.to_vec(),
        _ => Vec::new(),
    }
}

fn alloc_hostent(
    env: &mut Environment,
    ip_bytes: &[u8],
    address_family: i32,
    canonical_name: &str,
) -> u32 {
    let name_bytes = canonical_name.as_bytes();
    let name_len = name_bytes.len() as u32 + 1;
    let address_len = ip_bytes.len() as u32;
    let total = guest_size_of::<hostent_guest>() + name_len + address_len + 12;
    let block: MutPtr<u8> = env.mem.alloc(total).cast();
    let base = block.to_bits();
    let name_off = guest_size_of::<hostent_guest>();
    let address_off = name_off + name_len;
    let addrlist_off = address_off + address_len;
    let aliases_off = addrlist_off + 8;
    let name_ptr: MutPtr<u8> = MutPtr::from_bits(base + name_off);
    env.mem
        .bytes_at_mut(name_ptr, name_bytes.len() as u32)
        .copy_from_slice(name_bytes);
    env.mem.write(name_ptr + name_bytes.len() as u32, 0u8);
    let address_ptr: MutPtr<u8> = MutPtr::from_bits(base + address_off);
    env.mem
        .bytes_at_mut(address_ptr, address_len)
        .copy_from_slice(ip_bytes);
    let addrlist_ptr: MutPtr<MutPtr<u8>> = MutPtr::from_bits(base + addrlist_off);
    env.mem.write(addrlist_ptr, address_ptr);
    env.mem.write(addrlist_ptr + 1u32, MutPtr::null());
    let aliases_ptr: MutPtr<MutPtr<u8>> = MutPtr::from_bits(base + aliases_off);
    env.mem.write(aliases_ptr, MutPtr::null());
    let hostent = hostent_guest {
        h_name: name_ptr,
        h_aliases: aliases_ptr,
        h_addrtype: address_family,
        h_length: address_len as i32,
        h_addr_list: addrlist_ptr,
    };
    env.mem
        .write(MutPtr::<hostent_guest>::from_bits(base), hostent);
    base
}

fn alloc_servent(env: &mut Environment, name: &str, port_nbo: i32, proto: &str) -> MutPtr<u8> {
    let name_bytes = name.as_bytes();
    let proto_bytes = proto.as_bytes();
    let total = guest_size_of::<servent_guest>()
        + name_bytes.len() as u32
        + 1
        + proto_bytes.len() as u32
        + 1
        + 4; // aliases[0] = NULL

    let block: MutPtr<u8> = env.mem.alloc(total).cast();
    let base = block.to_bits();
    let name_off = guest_size_of::<servent_guest>();
    let proto_off = name_off + name_bytes.len() as u32 + 1;
    let aliases_off = proto_off + proto_bytes.len() as u32 + 1;

    let name_ptr: MutPtr<u8> = MutPtr::from_bits(base + name_off);
    let proto_ptr: MutPtr<u8> = MutPtr::from_bits(base + proto_off);
    let aliases_ptr: MutPtr<MutPtr<u8>> = MutPtr::from_bits(base + aliases_off);
    for (i, &b) in name_bytes.iter().enumerate() {
        env.mem.write(name_ptr + i as u32, b);
    }
    env.mem.write(name_ptr + name_bytes.len() as u32, 0u8);
    for (i, &b) in proto_bytes.iter().enumerate() {
        env.mem.write(proto_ptr + i as u32, b);
    }
    env.mem.write(proto_ptr + proto_bytes.len() as u32, 0u8);
    env.mem.write(aliases_ptr, MutPtr::<u8>::null());
    let sv = servent_guest {
        s_name: name_ptr,
        s_aliases: aliases_ptr,
        s_port: port_nbo,
        s_proto: proto_ptr,
    };
    env.mem.write(MutPtr::<servent_guest>::from_bits(base), sv);
    block
}

/// Returns the guest-memory pointer to the persistent `h_errno` cell,
/// allocating it on first use. Apple's libsystem exposes `h_errno` as
/// a real global; this helper guarantees we hand out the *same* address
/// every time, which is required by code that compares the pointer
/// returned from `__h_errno_location()` across calls (and by anything
/// that resolves the non-lazy `_h_errno` symbol).
pub fn h_errno_ptr(env: &mut Environment) -> MutPtr<i32> {
    if let Some(ptr) = env.libc_state.netdb.h_errno_cell {
        return ptr;
    }
    let ptr: MutPtr<i32> = env.mem.alloc(guest_size_of::<i32>()).cast();
    env.mem.write(ptr, env.libc_state.netdb.h_errno);
    env.libc_state.netdb.h_errno_cell = Some(ptr);
    ptr
}

/// Port (host byte order) → well-known service name.
fn port_service(port: u16) -> Option<&'static str> {
    match port {
        20 => Some("ftp-data"),
        21 => Some("ftp"),
        22 => Some("ssh"),
        23 => Some("telnet"),
        25 => Some("smtp"),
        53 => Some("dns"),
        80 => Some("http"),
        110 => Some("pop3"),
        119 => Some("nntp"),
        123 => Some("ntp"),
        143 => Some("imap"),
        161 => Some("snmp"),
        443 => Some("https"),
        465 => Some("smtps"),
        993 => Some("imaps"),
        995 => Some("pop3s"),
        _ => None,
    }
}

fn resolve_host_addresses(hostname: &str, network_access: bool) -> Option<Vec<IpAddr>> {
    if let Ok(address) = hostname.parse::<IpAddr>() {
        return Some(vec![address]);
    }

    let normalized = hostname.to_ascii_lowercase();
    match normalized.as_str() {
        "localhost" | "loopback" | "touchhle" => Some(vec![
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ]),
        "broadcasthost" => Some(vec![IpAddr::V4(Ipv4Addr::BROADCAST)]),
        _ if normalized.ends_with(".local") => {
            let address = crate::libc::ifaddrs::primary_lan_ipv4()
                .and_then(|address| address.parse::<Ipv4Addr>().ok())
                .unwrap_or(Ipv4Addr::LOCALHOST);
            Some(vec![IpAddr::V4(address)])
        }
        _ if network_access => resolve_hostname_ips(hostname),
        _ => None,
    }
}

/// Well-known service name → port (host byte order).
fn service_port(name: &str) -> Option<u16> {
    match name {
        "http" => Some(80),
        "https" => Some(443),
        "ftp" => Some(21),
        "ftp-data" => Some(20),
        "ssh" => Some(22),
        "telnet" => Some(23),
        "smtp" => Some(25),
        "dns" => Some(53),
        "pop3" => Some(110),
        "nntp" => Some(119),
        "imap" => Some(143),
        "imap2" => Some(143),
        "ldap" => Some(389),
        // Убрано дублирующееся значение "https" =>
        // Some(443),
        "smtps" => Some(465),
        "imaps" => Some(993),
        "pop3s" => Some(995),
        "ntp" => Some(123),
        "snmp" => Some(161),
        _ => None,
    }
}

/// Sets `h_errno` per Apple's `netdb.h` semantics: updates both the
/// host-side mirror and the persistent guest cell so that guest code
/// reading the cell directly (e.g. via `_h_errno`) sees the new value.
pub fn set_h_errno(env: &mut Environment, value: i32) {
    env.libc_state.netdb.h_errno = value;
    if let Some(ptr) = env.libc_state.netdb.h_errno_cell {
        env.mem.write(ptr, value);
    }
}

const HOST_NOT_FOUND: i32 = 1;

const NO_DATA: i32 = 4;

/// Real in-memory layout of `struct hostent` on 32-bit iOS/macOS.
/// All pointer fields are 4-byte guest pointers.
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
struct hostent_guest {
    h_name: MutPtr<u8>,              // canonical name
    h_aliases: MutPtr<MutPtr<u8>>,   // NULL-terminated alias list
    h_addrtype: i32,                 // AF_INET
    h_length: i32,                   // 4 for IPv4
    h_addr_list: MutPtr<MutPtr<u8>>, // NULL-terminated address list
}

unsafe impl SafeRead for hostent_guest {}

fn resolve_hostname_ips(hostname: &str) -> Option<Vec<IpAddr>> {
    let mut addresses = Vec::new();
    for address in (hostname, 0u16).to_socket_addrs().ok()? {
        let ip = address.ip();
        if !addresses.contains(&ip) {
            addresses.push(ip);
        }
    }
    (!addresses.is_empty()).then_some(addresses)
}

/// `struct servent` layout.
#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
struct servent_guest {
    s_name: MutPtr<u8>,
    s_aliases: MutPtr<MutPtr<u8>>,
    s_port: i32, // port in network byte order
    s_proto: MutPtr<u8>,
}

unsafe impl SafeRead for servent_guest {}

#[derive(Default)]
pub struct State {
    /// Raw guest address of the persistent `hostent` block.
    pub dummy_hostent_ptr: u32,
    /// Cached host-side mirror of the guest `h_errno` cell. Kept in sync
    /// with `h_errno_cell` so that Rust code can read the most-recently-set
    /// value without going through `env.mem`.
    pub h_errno: i32,
    /// Address (in guest memory) of the persistent `int h_errno;` cell
    /// exported by libSystem. Apple's `netdb.h` declares `h_errno` as
    /// `extern int h_errno;` and ships a real storage slot in libsystem;
    /// `__h_errno_location()` returns the same pointer on every call.
    /// We lazily allocate one 4-byte cell on first access and re-use it
    /// for both the non-lazy `_h_errno` symbol and `__h_errno_location`.
    pub h_errno_cell: Option<MutPtr<i32>>,
}

const EAI_AGAIN: i32 = 2;

const EAI_MEMORY: i32 = 6;

const EAI_OVERFLOW: i32 = 14;

const EAI_PROTOCOL: i32 = 13;

const EAI_SERVICE: i32 = 9;

const EAI_SOCKTYPE: i32 = 10;

const EAI_SYSTEM: i32 = 11;

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getaddrinfo(_, _, _, _)),
    export_c_func!(freeaddrinfo(_)),
    export_c_func!(gethostbyname(_)),
    export_c_func!(gethostent()),
    export_c_func!(__h_errno_location()),
    export_c_func!(freehostent(_)),
    export_c_func!(gai_strerror(_)),
    export_c_func!(gethostbyaddr(_, _, _)),
    export_c_func!(gethostbyname2(_, _)),
    export_c_func!(getipnodebyname(_, _, _, _)),
    export_c_func!(getnameinfo(_, _, _, _, _, _, _)),
    export_c_func!(getservbyname(_, _)),
    export_c_func!(getservbyport(_, _)),
];
