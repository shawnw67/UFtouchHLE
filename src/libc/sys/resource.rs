/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `sys/resource.h` (Darwin): `getrlimit` / `setrlimit`
//!
//! A previously-unimplemented pair of functions. Games that link C++
//! runtimes (e.g. Unreal/UE3 titles such as the iOS port of BioShock)
//! call `_getrlimit`/`_setrlimit` during startup — typically to raise
//! `RLIMIT_NOFILE` or query `RLIMIT_STACK` — and previously only received
//! return-0 stubs from dyld, which left the caller's output struct
//! uninitialised and could break the calling code path.
//!
//! We emulate a fixed, believable iOS-like resource environment. `setrlimit`
//! accepts any value (optionally clamping soft above hard) and remembers it
//! for subsequent `getrlimit` calls, matching the semantics of a privileged
//! process on a real device closely enough for guest code.

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::errno::{set_errno, EFAULT, EINVAL};
use crate::mem::{MutPtr, SafeRead};

pub struct State {
    /// The emulated per-resource limits, indexed by `which`.
    limits: [rlimit; RLIMIT_COUNT],
}

impl Default for State {
    fn default() -> Self {
        Self {
            limits: DEFAULT_LIMITS,
        }
    }
}

/// `rlim_t` is `uint64_t` on Darwin.
#[allow(non_camel_case_types)]
type rlim_t = u64;

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct rlimit {
    rlim_cur: rlim_t,
    rlim_max: rlim_t,
}
unsafe impl SafeRead for rlimit {}

// Darwin `which` values (sys/resource.h). Kept for reference; the limits
// array is indexed directly by the guest's `which` argument.
#[allow(dead_code)]
mod rlimit_which {
    pub(super) const RLIMIT_CPU: i32 = 0;
    pub(super) const RLIMIT_FSIZE: i32 = 1;
    pub(super) const RLIMIT_DATA: i32 = 2;
    pub(super) const RLIMIT_STACK: i32 = 3;
    pub(super) const RLIMIT_CORE: i32 = 4;
    pub(super) const RLIMIT_RSS: i32 = 5;
    pub(super) const RLIMIT_AS: i32 = 6;
    pub(super) const RLIMIT_MEMLOCK: i32 = 7;
    pub(super) const RLIMIT_NPROC: i32 = 8;
    pub(super) const RLIMIT_NOFILE: i32 = 9;
}

/// Darwin's `<sys/resource.h>` has 16 slots (`RLIM_NLIMITS`); values outside
/// it are rejected with `EINVAL` by the real kernel.
const RLIM_NLIMITS: i32 = 16;
const RLIMIT_COUNT: usize = RLIM_NLIMITS as usize;

/// xnu defines `RLIM_INFINITY` as `((rlim_t)((__uint64_t)1 << 63) - 1)`:
/// "no limit".
const RLIM_INFINITY: rlim_t = ((1u64) << 63) - 1;

const MB: rlim_t = 1024 * 1024;

/// A believable default resource environment for an iOS 6-era device.
const DEFAULT_LIMITS: [rlimit; RLIMIT_COUNT] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const INFINITY_LIM: rlimit = rlimit {
        rlim_cur: RLIM_INFINITY,
        rlim_max: RLIM_INFINITY,
    };
    [
        rlimit {
            rlim_cur: RLIM_INFINITY,
            rlim_max: RLIM_INFINITY,
        }, // RLIMIT_CPU
        rlimit {
            rlim_cur: RLIM_INFINITY,
            rlim_max: RLIM_INFINITY,
        }, // RLIMIT_FSIZE
        rlimit {
            rlim_cur: 512 * MB,
            rlim_max: 1024 * MB,
        }, // RLIMIT_DATA
        rlimit {
            rlim_cur: 8 * MB,
            rlim_max: 64 * MB,
        }, // RLIMIT_STACK
        rlimit {
            rlim_cur: 0,
            rlim_max: RLIM_INFINITY,
        }, // RLIMIT_CORE
        rlimit {
            rlim_cur: 512 * MB,
            rlim_max: 1024 * MB,
        }, // RLIMIT_RSS
        rlimit {
            rlim_cur: 512 * MB,
            rlim_max: 1024 * MB,
        }, // RLIMIT_AS
        INFINITY_LIM, // RLIMIT_MEMLOCK
        rlimit {
            rlim_cur: 128,
            rlim_max: 128,
        }, // RLIMIT_NPROC
        rlimit {
            rlim_cur: 256,
            rlim_max: 10240,
        }, // RLIMIT_NOFILE
        INFINITY_LIM, // RLIMIT_SBSIZE (unused slot)
        INFINITY_LIM, // RLIMIT_POSIXLOCKS (unused slot)
        INFINITY_LIM, // RLIMIT_NPTS (unused slot)
        INFINITY_LIM, // RLIMIT_KQUEUES (unused slot)
        INFINITY_LIM, // RLIMIT_UMTXP (unused slot)
        INFINITY_LIM, // RLIMIT_NVMEM (unused slot)
    ]
};

/// `int getrlimit(int which, struct rlimit *rlp)`
fn getrlimit(env: &mut Environment, which: i32, rlp: MutPtr<rlimit>) -> i32 {
    log_dbg!("getrlimit({}, {:?})", which, rlp);
    if !(0..RLIM_NLIMITS).contains(&which) {
        set_errno(env, EINVAL);
        return -1;
    }
    if rlp.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }
    set_errno(env, 0);
    env.mem
        .write(rlp, env.libc_state.resource.limits[which as usize]);
    0
}

/// `int setrlimit(int which, const struct rlimit *rlp)`
///
/// The emulated process always runs with root-like privileges, so any value
/// is accepted. As on Darwin, `rlim_cur > rlim_max` is rejected with
/// `EINVAL`.
fn setrlimit(env: &mut Environment, which: i32, rlp: MutPtr<rlimit>) -> i32 {
    log_dbg!("setrlimit({}, {:?})", which, rlp);
    if !(0..RLIM_NLIMITS).contains(&which) {
        set_errno(env, EINVAL);
        return -1;
    }
    if rlp.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }
    let new_limit = env.mem.read(rlp);
    if new_limit.rlim_cur > new_limit.rlim_max {
        set_errno(env, EINVAL);
        return -1;
    }
    set_errno(env, 0);
    env.libc_state.resource.limits[which as usize] = new_limit;
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getrlimit(_, _)),
    export_c_func!(setrlimit(_, _)),
];
