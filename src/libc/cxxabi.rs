/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `cxxabi.h`
//!
//! Resources:
//! - [Itanium C++ ABI specification](https://itanium-cxx-abi.github.io/cxx-abi/abi.html#dso-dtor-runtime-api)

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr, Ptr};
use crate::Environment;
use std::collections::HashMap;
use std::time::Duration;

#[derive(Default)]
pub struct State {
    guards: HashMap<MutPtr<u8>, usize>,
    destructors: Vec<Destructor>,
}

enum Destructor {
    C(GuestFunction),
    Cxx {
        function: GuestFunction,
        argument: MutVoidPtr,
        dso: MutVoidPtr,
    },
}

pub(super) fn register_atexit(env: &mut Environment, function: GuestFunction) -> i32 {
    if function.to_ptr().is_null() {
        return -1;
    }
    if let Some(register) = env.dyld.guest_cxxabi_export("___cxa_atexit") {
        return register.call_from_host(env, (function, MutVoidPtr::null(), MutVoidPtr::null()));
    }
    env.libc_state
        .cxxabi
        .destructors
        .push(Destructor::C(function));
    0
}

fn __cxa_guard_acquire(env: &mut Environment, guard: MutPtr<u8>) -> i32 {
    if guard.is_null() {
        return 0;
    }
    loop {
        if env.mem.read(guard) & 1 != 0 {
            return 0;
        }
        match env.libc_state.cxxabi.guards.get(&guard) {
            None => {
                env.libc_state
                    .cxxabi
                    .guards
                    .insert(guard, env.current_thread);
                return 1;
            }
            Some(&owner) if owner == env.current_thread => {
                // Recursive initialization is undefined; do not deadlock.
                log_once!("Recursive C++ local static initialization");
                return 0;
            }
            Some(_) => env.sleep(Duration::from_millis(1)),
        }
    }
}

fn __cxa_guard_release(env: &mut Environment, guard: MutPtr<u8>) {
    if env.libc_state.cxxabi.guards.get(&guard) == Some(&env.current_thread) {
        let value = env.mem.read(guard);
        env.mem.write(guard, value | 1);
        env.libc_state.cxxabi.guards.remove(&guard);
    }
}

fn __cxa_guard_abort(env: &mut Environment, guard: MutPtr<u8>) {
    if env.libc_state.cxxabi.guards.get(&guard) == Some(&env.current_thread) {
        env.libc_state.cxxabi.guards.remove(&guard);
    }
}

fn __cxa_atexit(
    env: &mut Environment,
    func: GuestFunction, // void (*func)(void *)
    p: MutVoidPtr,
    d: MutVoidPtr,
) -> i32 {
    if func.to_ptr().is_null() {
        return -1;
    }
    env.libc_state.cxxabi.destructors.push(Destructor::Cxx {
        function: func,
        argument: p,
        dso: d,
    });
    0
}

pub(super) fn __cxa_finalize(env: &mut Environment, d: MutVoidPtr) {
    loop {
        let position = env
            .libc_state
            .cxxabi
            .destructors
            .iter()
            .rposition(|entry| match entry {
                Destructor::C(_) => d.is_null(),
                Destructor::Cxx { dso, .. } => d.is_null() || *dso == d,
            });
        let Some(position) = position else {
            break;
        };
        // Remove before invoking guest code: finalizers can reenter this API.
        let entry = env.libc_state.cxxabi.destructors.remove(position);
        match entry {
            Destructor::C(function) => {
                let (): () = function.call_from_host(env, ());
            }
            Destructor::Cxx {
                function, argument, ..
            } => {
                let (): () = function.call_from_host(env, (argument,));
            }
        }
    }
}

pub(super) fn finalize_for_exit(env: &mut Environment) {
    if let Some(finalize) = env.dyld.guest_cxxabi_export("___cxa_finalize") {
        let (): () = finalize.call_from_host(env, (MutVoidPtr::null(),));
    }
    __cxa_finalize(env, MutVoidPtr::null());
}

const MAX_RTTI_SUBOBJECTS: usize = 4096;
const MAX_RTTI_BASES: u32 = 512;
const MAX_RTTI_DEPTH: u8 = 64;

#[derive(Clone, Copy)]
struct RttiSubobject {
    type_info: u32,
    object: u32,
    parent: Option<usize>,
    public_from_parent: bool,
    public_from_root: bool,
    depth: u8,
}

fn read_guest_u32(env: &Environment, address: u32) -> Option<u32> {
    if address < env.mem.null_segment_size() || address & 3 != 0 {
        return None;
    }
    let bytes = env
        .mem
        .get_bytes_fallible(ConstVoidPtr::from_bits(address), 4)?;
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn guest_add_signed(base: u32, offset: i32) -> Option<u32> {
    let address = i64::from(base).checked_add(i64::from(offset))?;
    (0..=i64::from(u32::MAX))
        .contains(&address)
        .then_some(address as u32)
}

fn rtti_type_name(env: &Environment, type_info: u32) -> Option<&str> {
    let name_ptr = read_guest_u32(env, type_info.checked_add(4)?)?;
    if name_ptr < env.mem.null_segment_size() {
        return None;
    }
    let bytes = env
        .mem
        .get_bytes_fallible(ConstVoidPtr::from_bits(name_ptr), 256)?;
    let length = bytes.iter().position(|byte| *byte == 0)?;
    std::str::from_utf8(&bytes[..length]).ok()
}

fn rtti_types_equal(env: &Environment, left: u32, right: u32) -> bool {
    left == right
        || matches!(
            (rtti_type_name(env, left), rtti_type_name(env, right)),
            (Some(left), Some(right)) if left == right
        )
}

fn rtti_typeinfo_kind(
    env: &Environment,
    type_info: u32,
    trace: bool,
) -> Option<crate::dyld::CxxAbiTypeInfoKind> {
    let Some(vtable) = read_guest_u32(env, type_info) else {
        if trace {
            log!(
                "__dynamic_cast RTTI node has unreadable vtable: type_info={type_info:#010x} name={:?}",
                rtti_type_name(env, type_info)
            );
        }
        return None;
    };
    let kind = env.dyld.cxxabi_typeinfo_kind(vtable);
    if trace {
        log!(
            "RTTI node type_info={type_info:#010x} name={:?} vtable={vtable:#010x} kind={kind:?}",
            rtti_type_name(env, type_info)
        );
    }
    kind
}

fn rtti_base_object_address(env: &Environment, derived: u32, offset_flags: u32) -> Option<u32> {
    let flags = offset_flags & 0xff;
    let offset = (offset_flags as i32) >> 8;
    if flags & 1 == 0 {
        return guest_add_signed(derived, offset);
    }
    let vtable = read_guest_u32(env, derived)?;
    let virtual_offset_address = guest_add_signed(vtable, offset)?;
    let virtual_offset = read_guest_u32(env, virtual_offset_address)? as i32;
    guest_add_signed(derived, virtual_offset)
}

fn push_rtti_subobject(
    nodes: &mut Vec<RttiSubobject>,
    type_info: u32,
    object: u32,
    parent: usize,
    is_public: bool,
) -> Option<()> {
    if nodes.len() >= MAX_RTTI_SUBOBJECTS {
        return None;
    }
    let parent_node = nodes.get(parent)?;
    if parent_node.depth >= MAX_RTTI_DEPTH {
        return None;
    }
    let public_from_root = parent_node.public_from_root && is_public;
    let depth = parent_node.depth + 1;
    nodes.push(RttiSubobject {
        type_info,
        object,
        parent: Some(parent),
        public_from_parent: is_public,
        public_from_root,
        depth,
    });
    Some(())
}

fn rtti_subobjects(
    env: &Environment,
    dynamic_type: u32,
    dynamic_object: u32,
    trace: bool,
) -> Option<Vec<RttiSubobject>> {
    use crate::dyld::CxxAbiTypeInfoKind;

    let mut nodes = vec![RttiSubobject {
        type_info: dynamic_type,
        object: dynamic_object,
        parent: None,
        public_from_parent: true,
        public_from_root: true,
        depth: 0,
    }];
    let mut index = 0;
    while index < nodes.len() {
        let node = nodes[index];
        match rtti_typeinfo_kind(env, node.type_info, trace)? {
            CxxAbiTypeInfoKind::Class => {}
            CxxAbiTypeInfoKind::SingleInheritance => {
                let base_type = read_guest_u32(env, node.type_info.checked_add(8)?)?;
                push_rtti_subobject(&mut nodes, base_type, node.object, index, true)?;
            }
            CxxAbiTypeInfoKind::MultipleInheritance => {
                let base_count = read_guest_u32(env, node.type_info.checked_add(12)?)?;
                if base_count > MAX_RTTI_BASES {
                    return None;
                }
                for base_index in 0..base_count {
                    let entry = node
                        .type_info
                        .checked_add(16)?
                        .checked_add(base_index.checked_mul(8)?)?;
                    let base_type = read_guest_u32(env, entry)?;
                    let offset_flags = read_guest_u32(env, entry.checked_add(4)?)?;
                    let base_object = rtti_base_object_address(env, node.object, offset_flags)?;
                    push_rtti_subobject(
                        &mut nodes,
                        base_type,
                        base_object,
                        index,
                        offset_flags & 2 != 0,
                    )?;
                }
            }
        }
        index += 1;
    }
    Some(nodes)
}

fn rtti_path_is_public(nodes: &[RttiSubobject], source: usize, target: usize) -> bool {
    let mut current = target;
    while current != source {
        let Some(node) = nodes.get(current) else {
            return false;
        };
        if !node.public_from_parent {
            return false;
        }
        let Some(parent) = node.parent else {
            return false;
        };
        current = parent;
    }
    true
}

fn rtti_cast_from_subobjects<F>(
    nodes: &[RttiSubobject],
    src_type: u32,
    src_object: u32,
    dst_type: u32,
    same_type: F,
) -> Option<u32>
where
    F: Fn(u32, u32) -> bool,
{
    let sources: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            (node.object == src_object && same_type(node.type_info, src_type)).then_some(index)
        })
        .collect();
    if sources.is_empty() {
        return None;
    }
    let targets: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| same_type(node.type_info, dst_type).then_some(index))
        .collect();

    let mut downcast_targets = Vec::new();
    for &target in &targets {
        if sources
            .iter()
            .any(|&source| rtti_path_is_public(nodes, target, source))
        {
            let address = nodes[target].object;
            if !downcast_targets.contains(&address) {
                downcast_targets.push(address);
            }
        }
    }
    if !downcast_targets.is_empty() {
        return (downcast_targets.len() == 1).then_some(downcast_targets[0]);
    }

    let source_is_public = sources.iter().any(|&source| nodes[source].public_from_root);
    if !source_is_public {
        return None;
    }
    let mut public_targets = Vec::new();
    for target in targets {
        if nodes[target].public_from_root {
            let address = nodes[target].object;
            if !public_targets.contains(&address) {
                public_targets.push(address);
            }
        }
    }
    if public_targets.len() == 1 {
        Some(public_targets[0])
    } else {
        None
    }
}

fn exact_dynamic_type_cast(dynamic_object: u32, offset_to_top: i32, hint: i32) -> Option<u32> {
    if hint < 0 || offset_to_top != hint.checked_neg()? {
        return None;
    }
    Some(dynamic_object)
}

fn __dynamic_cast(
    env: &mut Environment,
    src: ConstVoidPtr,
    src_type: ConstVoidPtr,
    dst_type: ConstVoidPtr,
    src2dst_offset: i32,
) -> ConstVoidPtr {
    static TRACE_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let trace = std::env::var_os("TOUCHHLE_TRACE_DYNAMIC_CAST").is_some()
        && TRACE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 64;
    if trace {
        log!(
            "__dynamic_cast input: src={:#010x} src_type={:#010x} ({:?}) dst_type={:#010x} ({:?}) hint={}",
            src.to_bits(),
            src_type.to_bits(),
            rtti_type_name(env, src_type.to_bits()),
            dst_type.to_bits(),
            rtti_type_name(env, dst_type.to_bits()),
            src2dst_offset
        );
    }
    if src.is_null() {
        return Ptr::null();
    }
    let src_type = src_type.to_bits();
    let dst_type = dst_type.to_bits();
    if src_type == 0 || dst_type == 0 {
        return Ptr::null();
    }
    if rtti_types_equal(env, src_type, dst_type) {
        return src;
    }

    let Some(vtable) = read_guest_u32(env, src.to_bits()) else {
        return Ptr::null();
    };
    let Some(offset_to_top_address) = vtable.checked_sub(8) else {
        return Ptr::null();
    };
    let Some(type_info_address) = vtable.checked_sub(4) else {
        return Ptr::null();
    };
    let Some(offset_to_top) = read_guest_u32(env, offset_to_top_address).map(|x| x as i32) else {
        return Ptr::null();
    };
    let Some(dynamic_type) = read_guest_u32(env, type_info_address) else {
        return Ptr::null();
    };
    let Some(dynamic_object) = guest_add_signed(src.to_bits(), offset_to_top) else {
        return Ptr::null();
    };
    if trace {
        log!(
            "__dynamic_cast object: vtable={:#010x} dynamic_type={:#010x} ({:?}) dynamic_object={:#010x} offset_to_top={}",
            vtable,
            dynamic_type,
            rtti_type_name(env, dynamic_type),
            dynamic_object,
            offset_to_top
        );
    }

    if rtti_types_equal(env, dynamic_type, dst_type) && src2dst_offset >= 0 {
        return exact_dynamic_type_cast(dynamic_object, offset_to_top, src2dst_offset)
            .map_or(Ptr::null(), ConstVoidPtr::from_bits);
    }

    let Some(nodes) = rtti_subobjects(env, dynamic_type, dynamic_object, trace) else {
        if trace {
            log!(
                "__dynamic_cast hierarchy decode failed for {:#010x}",
                dynamic_type
            );
        }
        return Ptr::null();
    };
    let result =
        rtti_cast_from_subobjects(&nodes, src_type, src.to_bits(), dst_type, |left, right| {
            rtti_types_equal(env, left, right)
        });
    if trace {
        let hierarchy: Vec<_> = nodes
            .iter()
            .map(|node| {
                (
                    format!("{:#010x}", node.type_info),
                    rtti_type_name(env, node.type_info),
                    format!("{:#010x}", node.object),
                    node.parent,
                    node.public_from_parent,
                    node.public_from_root,
                )
            })
            .collect();
        log!("__dynamic_cast hierarchy={hierarchy:?} result={result:#?}");
    }
    result.map_or(Ptr::null(), ConstVoidPtr::from_bits)
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__cxa_atexit(_, _, _)),
    export_c_func!(__cxa_finalize(_)),
    export_c_func!(__dynamic_cast(_, _, _, _)),
    export_c_func!(__cxa_guard_acquire(_)),
    export_c_func!(__cxa_guard_release(_)),
    export_c_func!(__cxa_guard_abort(_)),
];

#[cfg(test)]
mod dynamic_cast_tests {
    use super::{rtti_cast_from_subobjects, RttiSubobject};

    fn node(
        type_info: u32,
        object: u32,
        parent: Option<usize>,
        public_from_parent: bool,
        public_from_root: bool,
        depth: u8,
    ) -> RttiSubobject {
        RttiSubobject {
            type_info,
            object,
            parent,
            public_from_parent,
            public_from_root,
            depth,
        }
    }

    #[test]
    fn public_base_downcasts_to_derived() {
        let nodes = vec![
            node(10, 0x1000, None, true, true, 0),
            node(20, 0x1010, Some(0), true, true, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 20, 0x1010, 10, |left, right| left == right),
            Some(0x1000)
        );
    }

    #[test]
    fn private_base_does_not_downcast() {
        let nodes = vec![
            node(10, 0x1000, None, true, true, 0),
            node(20, 0x1010, Some(0), false, false, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 20, 0x1010, 10, |left, right| left == right),
            None
        );
    }

    #[test]
    fn public_sibling_base_crosscasts() {
        let nodes = vec![
            node(30, 0x1000, None, true, true, 0),
            node(10, 0x1010, Some(0), true, true, 1),
            node(20, 0x1020, Some(0), true, true, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 10, 0x1010, 20, |left, right| left == right),
            Some(0x1020)
        );
    }

    #[test]
    fn ambiguous_public_sibling_targets_fail() {
        let nodes = vec![
            node(30, 0x1000, None, true, true, 0),
            node(20, 0x1010, Some(0), true, true, 1),
            node(10, 0x1020, Some(0), true, true, 1),
            node(10, 0x1030, Some(0), true, true, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 20, 0x1010, 10, |left, right| left == right),
            None
        );
    }
}
