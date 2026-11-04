/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `ifaddrs.h` and `net/if.h` (interface addresses and interface naming)

use crate::dyld::FunctionExports;
use crate::export_c_func;
use crate::libc::errno::{set_errno, ENOENT, ENXIO};
use crate::mem::{ConstPtr, GuestUSize, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;

#[allow(clippy::type_complexity)]
unsafe fn host_interfaces() -> Option<Vec<(String, Option<u32>, Option<u32>, Option<u32>, u32)>> {
    #[cfg(not(windows))]
    {
        let mut ifap_host: *mut ::libc::ifaddrs = std::ptr::null_mut();
        if ::libc::getifaddrs(&mut ifap_host) != 0 {
            return None;
        }
        let mut out = Vec::new();
        let mut cursor = ifap_host;
        let mut lan_index = 0u32;
        while !cursor.is_null() {
            let ia = &*cursor;
            let sa = ia.ifa_addr;
            let mut addr = None;
            let mut netmask = None;
            let mut broadcast = None;
            if !sa.is_null() && (*sa).sa_family as i32 == ::libc::AF_INET {
                let sin = sa as *const ::libc::sockaddr_in;
                let octets = u32::from_be((*sin).sin_addr.s_addr).to_be_bytes();
                addr = Some(u32::from_be_bytes(octets));
                if !ia.ifa_netmask.is_null() {
                    let nm = ia.ifa_netmask as *const ::libc::sockaddr_in;
                    netmask = Some(u32::from_be((*nm).sin_addr.s_addr));
                }
                // The dstaddr/broadaddr field: a plain `ifa_dstaddr` on
                // BSD/macOS, a `ifu` union on Linux (aliases broadaddr).
                #[cfg(any(target_os = "linux", target_os = "android"))]
                let dst = ia.ifa_ifu;
                #[cfg(not(any(target_os = "linux", target_os = "android")))]
                let dst = ia.ifa_dstaddr;
                if !dst.is_null() {
                    let bc = dst.cast::<::libc::sockaddr_in>();
                    if (*bc).sin_family as i32 == ::libc::AF_INET {
                        broadcast = Some(u32::from_be((*bc).sin_addr.s_addr));
                    }
                }
            }
            let flags = ia.ifa_flags as u32;
            let is_loopback = flags & (::libc::IFF_LOOPBACK as u32) != 0;
            // Only expose IPv4-enabled, up interfaces; skip IPv6-only entries.
            if addr.is_some() {
                let ios_name = if is_loopback {
                    "lo0".to_string()
                } else {
                    let name = format!("en{}", lan_index);
                    lan_index += 1;
                    name
                };
                out.push((ios_name, addr, netmask, broadcast, flags));
            }
            cursor = ia.ifa_next;
        }
        ::libc::freeifaddrs(ifap_host);
        Some(out)
    }
    #[cfg(windows)]
    {
        // libc's ifaddrs API isn't available on Windows. Discover the
        // LAN address with a connected UDP socket instead: the routing
        // table picks the interface that would reach the address, so
        // the socket's local endpoint is the address other LAN devices
        // can reach. No packets are sent (UDP connect is local-only).
        use std::net::UdpSocket;
        let sock = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(_) => return None,
        };
        if sock.connect("10.255.255.255:9").is_err() {
            return None;
        }
        let local = match sock.local_addr() {
            Ok(a) => a,
            Err(_) => return None,
        };
        if let std::net::IpAddr::V4(v4) = local.ip() {
            let addr = u32::from(v4);
            // A common Class C /24 netmask is a reasonable guess; LAN
            // games generally only compare the network prefix.
            let netmask = 0xffff_ff00u32.to_be();
            // Limited broadcast; GameKit-style discovery also accepts it.
            let broadcast = 0xffff_ffffu32.to_be();
            // IFF_UP (0x1) | IFF_BROADCAST (0x2) | IFF_MULTICAST (0x800);
            // values are stable across platforms, no libc dep needed.
            const IFF_UP: u32 = 0x1;
            const IFF_BROADCAST: u32 = 0x2;
            const IFF_MULTICAST: u32 = 0x800;
            return Some(vec![(
                "en0".to_string(),
                Some(addr),
                Some(netmask),
                Some(broadcast),
                IFF_UP | IFF_BROADCAST | IFF_MULTICAST,
            )]);
        }
        None
    }
}

/// Best-effort primary LAN IPv4 of the host, as a dotted-quad string.
/// `None` when no non-loopback IPv4 interface exists. Used by `netdb`
/// to map `.local` hostnames to the address peers can actually reach.
/// LAN (Wi-Fi/Ethernet) interfaces are preferred over cellular ones,
/// because `.local` peers are on the LAN, not on the mobile network.
pub fn primary_lan_ipv4() -> Option<String> {
    lan_interfaces()
        .first()
        .map(|iface| iface.ip.to_string())
        .or_else(|| {
            let list = unsafe { host_interfaces() }?;
            list.into_iter()
                .find(|(_, addr, _, _, flags)| addr.is_some() && *flags & IFF_LOOPBACK == 0)
                .and_then(|(_, addr, _, _, _)| addr)
                .map(|a| {
                    let b = a.to_be_bytes();
                    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
                })
        })
}

// MARK: - Host LAN interfaces (host-side helpers)

/// One real IPv4 interface of the host. Unlike the guest-facing mapping
/// (en0/en1/…), this keeps the host's real interface name, which lets us
/// tell Wi-Fi/Ethernet apart from cellular links.
#[derive(Clone, Debug)]
pub struct HostInterface {
    /// Real host interface name, e.g. "wlan0", "rmnet_data0".
    pub name: String,
    /// Interface IPv4 address.
    pub ip: std::net::Ipv4Addr,
    /// Interface netmask.
    #[allow(dead_code)]
    pub netmask: std::net::Ipv4Addr,
    /// Directed broadcast address, if known.
    #[allow(dead_code)]
    pub broadcast: Option<std::net::Ipv4Addr>,
}

/// Real IPv4 interfaces of the host (loopback included).
pub fn real_ipv4_interfaces() -> Vec<HostInterface> {
    #[cfg(not(windows))]
    {
        let mut ifap_host: *mut ::libc::ifaddrs = std::ptr::null_mut();
        if unsafe { ::libc::getifaddrs(&mut ifap_host) } != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cursor = ifap_host;
        while !cursor.is_null() {
            let ia = unsafe { &*cursor };
            let sa = ia.ifa_addr;
            if !sa.is_null() && unsafe { (*sa).sa_family as i32 } == ::libc::AF_INET {
                let sin = sa as *const ::libc::sockaddr_in;
                let ip = std::net::Ipv4Addr::from(
                    u32::from_be(unsafe { (*sin).sin_addr.s_addr }).to_be_bytes(),
                );
                let netmask = if !ia.ifa_netmask.is_null() {
                    let nm = ia.ifa_netmask as *const ::libc::sockaddr_in;
                    std::net::Ipv4Addr::from(
                        u32::from_be(unsafe { (*nm).sin_addr.s_addr }).to_be_bytes(),
                    )
                } else {
                    // Assume a /24 if the netmask is missing; typical for LANs.
                    std::net::Ipv4Addr::new(255, 255, 255, 0)
                };
                let broadcast = {
                    #[cfg(any(target_os = "linux", target_os = "android"))]
                    let dst = ia.ifa_ifu;
                    #[cfg(not(any(target_os = "linux", target_os = "android")))]
                    let dst = ia.ifa_dstaddr;
                    if !dst.is_null() && unsafe { (*dst).sa_family as i32 } == ::libc::AF_INET {
                        let bc = dst.cast::<::libc::sockaddr_in>();
                        let addr = std::net::Ipv4Addr::from(
                            u32::from_be(unsafe { (*bc).sin_addr.s_addr }).to_be_bytes(),
                        );
                        if !addr.is_broadcast() {
                            Some(addr)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                let name = if ia.ifa_name.is_null() {
                    String::new()
                } else {
                    unsafe { std::ffi::CStr::from_ptr(ia.ifa_name) }
                        .to_string_lossy()
                        .into_owned()
                };
                out.push(HostInterface {
                    name,
                    ip,
                    netmask,
                    broadcast,
                });
            }
            cursor = ia.ifa_next;
        }
        unsafe { ::libc::freeifaddrs(ifap_host) };
        out
    }
    #[cfg(windows)]
    {
        // libc's ifaddrs API isn't available on Windows. Use the
        // routing-table trick instead (no packets are sent for UDP).
        use std::net::UdpSocket;
        let mut out = Vec::new();
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect("10.255.255.255:9").is_ok() {
                if let Ok(local) = sock.local_addr() {
                    if let std::net::IpAddr::V4(v4) = local.ip() {
                        out.push(HostInterface {
                            name: "en0".to_string(),
                            ip: v4,
                            netmask: std::net::Ipv4Addr::new(255, 255, 255, 0),
                            broadcast: Some(std::net::Ipv4Addr::new(255, 255, 255, 255)),
                        });
                    }
                }
            }
        }
        out
    }
}

fn is_lan_interface_name(name: &str) -> bool {
    // Wi-Fi/Ethernet/AP/bridge names used by Linux, Android and iOS hosts.
    // Cellular links (rmnet*, clat*, ccmni*), USB and tunnels are excluded
    // because LAN peers are never reachable through them.
    let n = name.to_ascii_lowercase();
    ["wlan", "wifi", "eth", "swlan", "sw", "ap", "ra", "br", "en"]
        .iter()
        .any(|p| n.starts_with(p))
}

/// IPv4 interfaces usable for LAN play, best (LAN-class) interfaces first.
///
/// On phones with both Wi-Fi and mobile data, multicast and broadcast sent
/// without picking an interface egress the *default route* (cellular), so
/// peers on the same Wi-Fi never receive them. Anything that needs to reach
/// the LAN should send via these interfaces.
pub fn lan_interfaces() -> Vec<HostInterface> {
    let all: Vec<HostInterface> = real_ipv4_interfaces()
        .into_iter()
        .filter(|iface| !iface.ip.is_loopback() && !iface.ip.is_unspecified())
        .collect();
    let lan: Vec<HostInterface> = all
        .iter()
        .filter(|iface| is_lan_interface_name(&iface.name))
        .cloned()
        .collect();
    if !lan.is_empty() {
        return lan;
    }
    // No Wi-Fi-like name found (or unusual host): fall back to every
    // non-loopback interface, excluding typical cellular/tunnel names.
    let fallback: Vec<HostInterface> = all
        .iter()
        .filter(|iface| {
            let n = iface.name.to_ascii_lowercase();
            ![
                "rmnet", "clat", "ccmni", "usb", "tun", "tap", "dummy", "virbr", "sit", "can",
            ]
            .iter()
            .any(|p| n.starts_with(p))
        })
        .cloned()
        .collect();
    if !fallback.is_empty() {
        return fallback;
    }
    all
}

// BSD interface flags (net/if.h) as seen by 32-bit iOS guests.
const IFF_UP: u32 = 0x1;
const IFF_BROADCAST: u32 = 0x2;
const IFF_LOOPBACK: u32 = 0x8;
const IFF_RUNNING: u32 = 0x40;
const IFF_MULTICAST: u32 = 0x8000;

/// BSD `struct sockaddr` as laid out for 32-bit ARM guests (16 bytes).
#[repr(C, packed)]
struct guest_sockaddr_in {
    sa_len: u8,
    sa_family: u8,
    sa_data: [u8; 14],
}
unsafe impl SafeRead for guest_sockaddr_in {}

fn guest_sockaddr_from_ipv4(octets: [u8; 4], port: u16) -> guest_sockaddr_in {
    let mut sa = guest_sockaddr_in {
        sa_len: 16,
        sa_family: 2, // AF_INET
        sa_data: [0; 14],
    };
    sa.sa_data[0..2].copy_from_slice(&port.to_be_bytes());
    sa.sa_data[2..6].copy_from_slice(&octets);
    sa
}

// Mirrors the POSIX `struct ifaddrs` layout as seen by 32-bit ARM guests.
// All pointer fields are 4-byte guest pointers.
#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub struct ifaddrs {
    /// Next node in the linked list (NULL = end).
    pub ifa_next: MutPtr<ifaddrs>,
    /// NUL-terminated interface name, e.g. "en0".
    pub ifa_name: ConstPtr<u8>,
    /// Interface flags (IFF_UP, IFF_LOOPBACK, …).
    pub ifa_flags: u32,
    /// Primary address (may be NULL).
    pub ifa_addr: u32, // guest ptr to sockaddr – typed as u32 to avoid pulling in socket types
    /// Netmask (may be NULL).
    pub ifa_netmask: u32,
    /// Broadcast or point-to-point destination address (may be NULL).
    pub ifa_broadaddr: u32,
    /// Protocol-specific data (may be NULL).
    pub ifa_data: u32,
}
// SAFETY: the struct is plain data; every field is either a scalar or a guest
// pointer that touchHLE's pointer type already validates.
unsafe impl SafeRead for ifaddrs {}

// ---------------------------------------------------------------------------
// getifaddrs / freeifaddrs
// ---------------------------------------------------------------------------

/// `int getifaddrs(struct ifaddrs **ifap)`
///
/// Enumerates the HOST device's real IPv4 network interfaces and mirrors
/// them into a guest-allocated linked list with iOS-style interface names:
/// the loopback interface is always named `lo0` and real LAN interfaces are
/// mapped in order to `en0`, `en1`, … (matching an iPhone where `en0` is
/// Wi-Fi). Each node carries the interface's actual IPv4 address, netmask
/// and broadcast address, so games that discover their own LAN IP for
/// local multiplayer (Gameloft, ngmoco, etc.) see the same address that
/// other devices on the network can reach via the guest sockets.
fn getifaddrs(env: &mut Environment, ifap: MutPtr<MutPtr<ifaddrs>>) -> i32 {
    if ifap.is_null() {
        set_errno(env, ENOENT);
        return -1;
    }

    let interfaces = match unsafe { host_interfaces() } {
        Some(list) if !list.is_empty() => list,
        _ => {
            // Fall back to the loopback-only view so apps that treat an
            // empty list as "no network at all" still behave sanely.
            vec![(
                "lo0".to_string(),
                Some(u32::from_be_bytes([127, 0, 0, 1])),
                Some(u32::from_be_bytes([255, 0, 0, 0])),
                None,
                IFF_UP | IFF_LOOPBACK | IFF_RUNNING,
            )]
        }
    };

    // Compute the total allocation size: one ifaddrs node + one sockaddr per
    // address family slot + one name string (with NUL) per interface.
    const SOCKADDR_SIZE: GuestUSize = 16;
    let mut total: GuestUSize = 0;
    for (name, addr, netmask, broadcast, _) in &interfaces {
        let _ = (addr, netmask, broadcast);
        total += std::mem::size_of::<ifaddrs>() as GuestUSize;
        total += name.len() as GuestUSize + 1;
        total += SOCKADDR_SIZE; // addr
        total += SOCKADDR_SIZE; // netmask
        if broadcast.is_some() {
            total += SOCKADDR_SIZE;
        }
    }
    // Terminator node (NULL next pointer).
    total += std::mem::size_of::<ifaddrs>() as GuestUSize;

    let base: MutPtr<u8> = env.mem.alloc(total).cast();
    if base.is_null() {
        set_errno(env, ENOENT);
        env.mem.write(ifap, MutPtr::null());
        return -1;
    }

    let mut cursor_bits = base.to_bits();
    let mut first_ptr: MutPtr<ifaddrs> = MutPtr::null();
    let mut prev_ptr: MutPtr<ifaddrs> = MutPtr::null();

    for (name, addr, netmask, broadcast, flags) in interfaces.iter().cloned() {
        let node_ptr: MutPtr<ifaddrs> = MutPtr::from_bits(cursor_bits);
        cursor_bits += std::mem::size_of::<ifaddrs>() as GuestUSize;

        // Name string.
        let name_ptr: MutPtr<u8> = MutPtr::from_bits(cursor_bits);
        for (i, byte) in name.bytes().chain(std::iter::once(0)).enumerate() {
            env.mem.write(name_ptr + i as u32, byte);
        }
        cursor_bits += name.len() as GuestUSize + 1;

        let mut write_sockaddr = |bits: &mut GuestUSize, octets: [u8; 4]| -> MutVoidPtr {
            let sa_ptr: MutPtr<guest_sockaddr_in> = MutPtr::from_bits(*bits);
            *bits += SOCKADDR_SIZE;
            env.mem.write(sa_ptr, guest_sockaddr_from_ipv4(octets, 0));
            sa_ptr.cast()
        };

        let addr_ptr = addr.map(|a| write_sockaddr(&mut cursor_bits, a.to_be_bytes()));
        let netmask_ptr = netmask.map(|a| write_sockaddr(&mut cursor_bits, a.to_be_bytes()));
        let broadcast_ptr = broadcast.map(|a| write_sockaddr(&mut cursor_bits, a.to_be_bytes()));

        let mut ifa_flags = flags;
        ifa_flags |= IFF_UP | IFF_RUNNING | IFF_MULTICAST;
        if broadcast.is_some() {
            ifa_flags |= IFF_BROADCAST;
        }
        if name == "lo0" {
            ifa_flags |= IFF_LOOPBACK;
            ifa_flags &= !IFF_BROADCAST;
        }

        env.mem.write(
            node_ptr,
            ifaddrs {
                ifa_next: MutPtr::null(),
                ifa_name: name_ptr.cast_const(),
                ifa_flags,
                ifa_addr: addr_ptr.map(|p| p.to_bits()).unwrap_or(0),
                ifa_netmask: netmask_ptr.map(|p| p.to_bits()).unwrap_or(0),
                ifa_broadaddr: broadcast_ptr.map(|p| p.to_bits()).unwrap_or(0),
                ifa_data: 0,
            },
        );

        if !prev_ptr.is_null() {
            let mut prev = env.mem.read(prev_ptr);
            prev.ifa_next = node_ptr;
            env.mem.write(prev_ptr, prev);
        } else {
            first_ptr = node_ptr;
        }
        prev_ptr = node_ptr;
    }

    env.mem.write(ifap, first_ptr);
    log_dbg!(
        "getifaddrs() => {} interface(s): {:?}",
        interfaces.len(),
        interfaces
            .iter()
            .map(|(n, a, ..)| (n.clone(), a.unwrap_or(0)))
            .collect::<Vec<_>>()
    );
    0 // success
}

/// `void freeifaddrs(struct ifaddrs *ifa)`
///
/// Frees the single guest allocation backing the linked list returned by
/// [getifaddrs].
fn freeifaddrs(env: &mut Environment, ifa: MutPtr<ifaddrs>) {
    if !ifa.is_null() {
        let base: MutVoidPtr = ifa.cast();
        env.mem.free(base);
    }
}

// ---------------------------------------------------------------------------
// net/if.h – interface index / name mapping
// (commonly used together with ifaddrs by network-aware apps)
// ---------------------------------------------------------------------------

/// Maximum length of an interface name including the NUL terminator.
const IF_NAMESIZE: usize = 16;

// ---------------------------------------------------------------------------
// Fake interface table
// ---------------------------------------------------------------------------
//
// touchHLE doesn't expose host networking to the guest (`getifaddrs()`
// returns an empty list), but a completely empty interface table breaks
// some apps: e.g. Turbo Dismount's prime31 SocialNetworking plugin calls
// `if_nametoindex()` on a Wi-Fi/3G probe, treats a 0 return as a hard
// failure and crashes with an unhandled Mono NullReferenceException.
//
// On a real iPhone OS device, `lo0` and `en0` (Wi-Fi) always exist with
// well-known indices, so we present a minimal virtual table that matches
// that expectation. Interface *data* is still absent (no addresses, no
// routes), so apps that actually open sockets get the usual
// "network not supported" error path instead of a crash.

/// Fake loopback interface (`lo0`), index 1 — always exists on BSD/iOS.
const FAKE_LOOPBACK_INDEX: u32 = 1;
const FAKE_LOOPBACK_NAME: &[u8] = b"lo0";
/// Fake Wi-Fi interface (`en0`), index 2 — always exists on iOS devices.
const FAKE_WIFI_INDEX: u32 = 2;
const FAKE_WIFI_NAME: &[u8] = b"en0";
/// Index assigned to any other probed interface name (pdp_ip0, utun0, …).
/// Returning a valid index for unknown names is deliberately permissive:
/// the guest is told the interface exists, which keeps network-availability
/// probes happy even when they look for a name we don't model.
const FAKE_OTHER_INDEX: u32 = 3;

/// `unsigned int if_nametoindex(const char *ifname)`
///
/// Returns the index for the named interface, or 0 on error (per POSIX,
/// which also documents `errno` getting set to `ENXIO`).
///
/// Unlike real kernels, unknown-but-plausible names map to a stable fake
/// index ([FAKE_OTHER_INDEX]) so availability probes succeed. A NULL or
/// empty name still returns 0, as on real OSes.
fn if_nametoindex(env: &mut Environment, ifname: ConstPtr<u8>) -> u32 {
    let name = env.mem.cstr_at_utf8(ifname).unwrap_or("");
    let index = if name.is_empty() {
        0
    } else if name.eq_ignore_ascii_case("lo0") {
        FAKE_LOOPBACK_INDEX
    } else if name.eq_ignore_ascii_case("en0") {
        FAKE_WIFI_INDEX
    } else {
        FAKE_OTHER_INDEX
    };
    if index == 0 {
        set_errno(env, ENXIO);
    } else {
        log_dbg!(
            "if_nametoindex(\"{}\") => {} (fake virtual interface)",
            name,
            index
        );
    }
    index
}

/// `char *if_indextoname(unsigned int ifindex, char *ifname)`
///
/// Writes the name of interface `ifindex` into `ifname` (at least
/// `IF_NAMESIZE` bytes) and returns `ifname`, or NULL on error.
/// Inverse of [if_nametoindex]: only the modeled fake interfaces resolve.
fn if_indextoname(env: &mut Environment, ifindex: u32, ifname: MutPtr<u8>) -> MutPtr<u8> {
    let name: &[u8] = match ifindex {
        FAKE_LOOPBACK_INDEX => FAKE_LOOPBACK_NAME,
        FAKE_WIFI_INDEX => FAKE_WIFI_NAME,
        FAKE_OTHER_INDEX => b"en1",
        _ => {
            set_errno(env, ENXIO);
            return MutPtr::null();
        }
    };
    if ifname.is_null() {
        set_errno(env, ENXIO);
        return MutPtr::null();
    }
    if name.len() + 1 > IF_NAMESIZE {
        set_errno(env, ENXIO);
        return MutPtr::null();
    }
    for (i, &byte) in name.iter().chain(std::iter::once(&0)).enumerate() {
        env.mem.write(ifname + i as u32, byte);
    }
    log_dbg!(
        "if_indextoname({}) => \"{}\"",
        ifindex,
        std::str::from_utf8(name).unwrap_or("?")
    );
    ifname
}

// `struct if_nameindex` used by if_nameindex() / if_freenameindex().
#[allow(non_camel_case_types)]
#[repr(C, packed)]
pub struct if_nameindex {
    pub if_index: u32,
    pub if_name: ConstPtr<u8>,
}
unsafe impl SafeRead for if_nameindex {}

// Layout of the guest-allocated if_nameindex() result:
// [lo0 entry][en0 entry][other entry][terminator][lo0 name][en0 name][en1 name]
const IF_NAMEINDEX_TABLE_BYTES: GuestUSize = (std::mem::size_of::<if_nameindex>() as GuestUSize)
    * 4
    + (FAKE_LOOPBACK_NAME.len() as GuestUSize + 1)
    + (FAKE_WIFI_NAME.len() as GuestUSize + 1)
    + (3 + 1); // "en1"

/// `struct if_nameindex *if_nameindex(void)`
///
/// Returns a guest-allocated array of all interface name/index pairs
/// terminated by an entry with `if_index == 0` and `if_name == NULL`.
/// The array (and the referenced names) live in a single guest allocation
/// and are freed by [if_freenameindex].
fn if_nameindex(env: &mut Environment) -> MutPtr<if_nameindex> {
    let base: MutPtr<u8> = env.mem.alloc(IF_NAMEINDEX_TABLE_BYTES).cast();
    if base.is_null() {
        set_errno(env, ENOENT);
        return MutPtr::null();
    }

    let entry_size = std::mem::size_of::<if_nameindex>() as u32;
    let name_area: MutPtr<u8> = MutPtr::from_bits(base.to_bits() + entry_size * 4);

    let mut write_entry = |slot: u32, index: u32, name: &[u8], name_off: u32| {
        let entry: MutPtr<if_nameindex> = MutPtr::from_bits(base.to_bits() + slot * entry_size);
        let name_ptr: MutPtr<u8> = MutPtr::from_bits(name_area.to_bits() + name_off);
        for (i, &byte) in name.iter().chain(std::iter::once(&0)).enumerate() {
            env.mem.write(
                MutPtr::from_bits(name_area.to_bits() + name_off + i as u32),
                byte,
            );
        }
        env.mem.write(
            entry,
            if_nameindex {
                if_index: index,
                if_name: name_ptr.cast_const(),
            },
        );
    };

    let mut name_off: u32 = 0;
    let names: [(u32, &[u8]); 3] = [
        (FAKE_LOOPBACK_INDEX, FAKE_LOOPBACK_NAME),
        (FAKE_WIFI_INDEX, FAKE_WIFI_NAME),
        (FAKE_OTHER_INDEX, b"en1"),
    ];
    for (slot, (index, name)) in names.iter().enumerate() {
        write_entry(slot as u32, *index, name, name_off);
        name_off += name.len() as u32 + 1;
    }
    // Terminator entry: index 0, NULL name.
    let terminator: MutPtr<if_nameindex> = MutPtr::from_bits(base.to_bits() + 3 * entry_size);
    env.mem.write(
        terminator,
        if_nameindex {
            if_index: 0,
            if_name: ConstPtr::null(),
        },
    );

    log_dbg!("if_nameindex() => table with 3 fake interfaces (lo0, en0, en1)");
    base.cast()
}

/// `void if_freenameindex(struct if_nameindex *ptr)`
///
/// Frees the single guest allocation backing the array returned by
/// `if_nameindex`.
fn if_freenameindex(env: &mut Environment, ptr: MutPtr<if_nameindex>) {
    if !ptr.is_null() {
        let base: MutVoidPtr = ptr.cast();
        env.mem.free(base);
    }
}

// ---------------------------------------------------------------------------
// Export table
// ---------------------------------------------------------------------------

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getifaddrs(_)),
    export_c_func!(freeifaddrs(_)),
    export_c_func!(if_nametoindex(_)),
    export_c_func!(if_indextoname(_, _)),
    export_c_func!(if_nameindex()),
    export_c_func!(if_freenameindex(_)),
];
