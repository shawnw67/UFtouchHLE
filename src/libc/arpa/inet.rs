/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `arpa/inet.h` (Internet address manipulation routines)

use crate::libc::netdb::socklen_t;
use crate::libc::sys::socket::AF_INET;
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, SafeRead};
use crate::{export_c_func, Environment};

use crate::dyld::FunctionExports;
use std::net::Ipv4Addr;

#[allow(non_camel_case_types)]
type in_addr_t = u32;

#[derive(Copy, Clone, Debug)]
#[repr(C, packed)]
#[allow(non_camel_case_types)]
struct in_addr {
    s_addr: in_addr_t,
}
unsafe impl SafeRead for in_addr {}

fn inet_addr(env: &mut Environment, str: ConstPtr<u8>) -> in_addr_t {
    let inet_addr_str = env.mem.cstr_at_utf8(str).unwrap();
    let address: Ipv4Addr = inet_addr_str.parse().unwrap();
    let res = u32::from_le_bytes(address.octets());
    log_dbg!("inet_addr({:?}) => {}", inet_addr_str, res);
    res
}

fn inet_ntop(
    env: &mut Environment,
    af: i32,
    src: ConstVoidPtr,
    dst: MutPtr<u8>,
    size: socklen_t,
) -> ConstPtr<u8> {
    assert_eq!(af, AF_INET);
    let addr_ptr: ConstPtr<in_addr> = src.cast();
    let addr = env.mem.read(addr_ptr);
    let ipv4_addr = Ipv4Addr::from_bits(u32::from_be(addr.s_addr));
    log_dbg!("inet_ntop: addr = {:?}", ipv4_addr);
    let binding = ipv4_addr.to_string();
    let addr_bytes = binding.as_bytes();
    let len: GuestUSize = addr_bytes.len().try_into().unwrap();
    assert!(len < size);
    env.mem.bytes_at_mut(dst, len).copy_from_slice(addr_bytes);
    env.mem.write(dst + len, b'\0');
    dst.cast_const()
}

fn inet_pton(env: &mut Environment, af: i32, src: ConstPtr<u8>, dst: MutVoidPtr) -> i32 {
    assert_eq!(af, AF_INET);
    let str = env.mem.cstr_at_utf8(src.cast()).unwrap();
    log_dbg!("inet_pton '{}'", str);
    let address: Ipv4Addr = str.parse().unwrap();
    let addr = in_addr {
        s_addr: u32::from_le_bytes(address.octets()),
    };
    let addr_ptr: MutPtr<in_addr> = dst.cast();
    env.mem.write(addr_ptr, addr);
    1 // address was valid, success
}

/// `htonl` — host to network byte order (32-bit).
fn htonl(_env: &mut Environment, hostlong: u32) -> u32 {
    hostlong.to_be()
}

/// `htons` — host to network byte order (16-bit).
fn htons(_env: &mut Environment, hostshort: u16) -> u16 {
    hostshort.to_be()
}

/// `inet_aton` — converts a dotted-decimal string into an `in_addr`.
/// Returns 1 on success, 0 on failure.
fn inet_aton(env: &mut Environment, str: ConstPtr<u8>, addr_out: MutPtr<in_addr>) -> i32 {
    let s = env.mem.cstr_at_utf8(str).unwrap_or_default().to_owned();
    match s.parse::<Ipv4Addr>() {
        Ok(addr) => {
            if !addr_out.is_null() {
                env.mem.write(
                    addr_out,
                    in_addr {
                        s_addr: u32::from_le_bytes(addr.octets()),
                    },
                );
            }
            log_dbg!("inet_aton({:?}) => 1", s);
            1
        }
        Err(_) => {
            log!("inet_aton({:?}): invalid address, returning 0", s);
            0
        }
    }
}

/// `inet_lnaof` — extract host portion of a Class A/B/C address.
/// Takes in_addr_t instead of in_addr because in_addr does not implement
/// GuestArg.
fn inet_lnaof(_env: &mut Environment, s_addr: in_addr_t) -> in_addr_t {
    let a = u32::from_be(s_addr);
    if a & 0x8000_0000 == 0 {
        a & 0x00FF_FFFF
    } else if a & 0x4000_0000 == 0 {
        a & 0x0000_FFFF
    } else {
        a & 0x0000_00FF
    }
}

/// `inet_makeaddr` — construct an in_addr from a network number and host
/// part. Returns in_addr_t instead of in_addr because in_addr does not
/// implement GuestRet.
fn inet_makeaddr(_env: &mut Environment, net: u32, host: u32) -> in_addr_t {
    let addr = if net < 128 {
        (net << 24) | (host & 0x00FF_FFFF)
    } else if net < 65536 {
        (net << 16) | (host & 0x0000_FFFF)
    } else {
        (net << 8) | (host & 0x0000_00FF)
    };
    addr.to_be()
}

/// `inet_netof` — extract network portion of a classful address.
fn inet_netof(_env: &mut Environment, s_addr: in_addr_t) -> in_addr_t {
    let a = u32::from_be(s_addr);
    if a & 0x8000_0000 == 0 {
        (a >> 24) & 0xFF
    } else if a & 0x4000_0000 == 0 {
        (a >> 16) & 0xFFFF
    } else {
        (a >> 8) & 0xFF_FFFF
    }
}

/// `inet_network` — parse a network address and return it in host byte order.
fn inet_network(env: &mut Environment, str: ConstPtr<u8>) -> in_addr_t {
    let s = env.mem.cstr_at_utf8(str).unwrap_or_default().to_owned();
    match s.parse::<Ipv4Addr>() {
        Ok(addr) => {
            let val = u32::from_be_bytes(addr.octets());
            log_dbg!("inet_network({:?}) => {:#010x}", s, val);
            val
        }
        Err(_) => {
            log!("inet_network({:?}): invalid, returning INADDR_NONE", s);
            INADDR_NONE
        }
    }
}

/// `inet_ntoa` — converts an in_addr (passed as raw u32) to a
/// dotted-decimal string. Returns a pointer to a per-call guest buffer.
/// Takes in_addr_t instead of in_addr because in_addr does not implement
/// GuestArg.
fn inet_ntoa(env: &mut Environment, s_addr: in_addr_t) -> ConstPtr<u8> {
    let octets = s_addr.to_le_bytes();
    let s = format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]);
    log_dbg!("inet_ntoa({:#010x}) => {:?}", s_addr, s);
    let bytes = s.as_bytes();
    let buf: MutPtr<u8> = env.mem.alloc(bytes.len() as u32 + 1).cast();
    env.mem
        .bytes_at_mut(buf, bytes.len() as u32)
        .copy_from_slice(bytes);
    env.mem.write(buf + bytes.len() as u32, b'\0');
    buf.cast_const()
}

/// `ntohl` — network to host byte order (32-bit).
fn ntohl(_env: &mut Environment, netlong: u32) -> u32 {
    u32::from_be(netlong)
}

/// `ntohs` — network to host byte order (16-bit).
fn ntohs(_env: &mut Environment, netshort: u16) -> u16 {
    u16::from_be(netshort)
}

const INADDR_NONE: in_addr_t = u32::MAX;

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(inet_addr(_)),
    export_c_func!(inet_ntop(_, _, _, _)),
    export_c_func!(inet_pton(_, _, _)),
    export_c_func!(htonl(_)),
    export_c_func!(htons(_)),
    export_c_func!(inet_aton(_, _)),
    export_c_func!(inet_lnaof(_)),
    export_c_func!(inet_makeaddr(_, _)),
    export_c_func!(inet_netof(_)),
    export_c_func!(inet_network(_)),
    export_c_func!(inet_ntoa(_)),
    export_c_func!(ntohl(_)),
    export_c_func!(ntohs(_)),
];
