/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Audio Unit graph topology. Audio processing is implemented separately.

use std::collections::{BTreeMap, HashMap};

use super::{audio_components, audio_unit};
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::mem::{ConstPtr, MutPtr, SafeRead};
use audio_unit::AudioUnit;

#[repr(C, packed)]
pub struct OpaqueAUGraph {
    _pad: u8,
}
unsafe impl SafeRead for OpaqueAUGraph {}
type AUGraph = MutPtr<OpaqueAUGraph>;
type AUNode = i32;

#[derive(Clone, Copy)]
#[repr(C, packed)]
struct ComponentDescription {
    component_type: u32,
    component_sub_type: u32,
    component_manufacturer: u32,
    component_flags: u32,
    component_flags_mask: u32,
}
unsafe impl SafeRead for ComponentDescription {}

#[derive(Clone, Copy)]
struct Connection {
    source: AUNode,
    _source_bus: u32,
    destination: AUNode,
    destination_bus: u32,
}

#[derive(Default)]
struct Graph {
    nodes: BTreeMap<AUNode, ComponentDescription>,
    connections: Vec<Connection>,
    next_node: AUNode,
    unit: Option<AudioUnit>,
    initialized: bool,
    running: bool,
}

#[derive(Default)]
pub struct State {
    graphs: HashMap<AUGraph, Graph>,
}

fn NewAUGraph(env: &mut Environment, out_graph: MutPtr<AUGraph>) -> OSStatus {
    return_if_null!(out_graph);
    let graph = env.mem.alloc_and_write(OpaqueAUGraph { _pad: 0 });
    env.framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .insert(graph, Graph::default());
    env.mem.write(out_graph, graph);
    0
}

fn DisposeAUGraph(env: &mut Environment, graph: AUGraph) -> OSStatus {
    if !env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .contains_key(&graph)
    {
        return paramErr;
    }
    let result = AUGraphClose(env, graph);
    if result != 0 {
        return result;
    }
    if env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .remove(&graph)
        .is_none()
    {
        return paramErr;
    }
    env.mem.free(graph.cast());
    0
}

fn AUGraphAddNode(
    env: &mut Environment,
    graph: AUGraph,
    description: ConstPtr<ComponentDescription>,
    out_node: MutPtr<AUNode>,
) -> OSStatus {
    return_if_null!(description);
    return_if_null!(out_node);
    let description = env.mem.read(description);
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
    else {
        return paramErr;
    };
    if state.unit.is_some() {
        return paramErr;
    }
    let Some(node) = state.next_node.checked_add(1) else {
        return paramErr;
    };
    state.next_node = node;
    state.nodes.insert(node, description);
    env.mem.write(out_node, node);
    0
}

fn AUGraphRemoveNode(env: &mut Environment, graph: AUGraph, node: AUNode) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
    else {
        return paramErr;
    };
    if state.unit.is_some() {
        return paramErr;
    }
    if state.nodes.remove(&node).is_none() {
        return paramErr;
    }
    state
        .connections
        .retain(|connection| connection.source != node && connection.destination != node);
    0
}

fn AUGraphCountNodes(env: &mut Environment, graph: AUGraph, out_count: MutPtr<u32>) -> OSStatus {
    return_if_null!(out_count);
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    env.mem.write(out_count, state.nodes.len() as u32);
    0
}

fn AUGraphGetIndNode(
    env: &mut Environment,
    graph: AUGraph,
    index: u32,
    out_node: MutPtr<AUNode>,
) -> OSStatus {
    return_if_null!(out_node);
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    let Some(&node) = state.nodes.keys().nth(index as usize) else {
        return paramErr;
    };
    env.mem.write(out_node, node);
    0
}

fn AUGraphConnectNodeInput(
    env: &mut Environment,
    graph: AUGraph,
    source: AUNode,
    source_bus: u32,
    destination: AUNode,
    destination_bus: u32,
) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
    else {
        return paramErr;
    };
    if !state.nodes.contains_key(&source)
        || !state.nodes.contains_key(&destination)
        || state.unit.is_some()
        || state
            .connections
            .iter()
            .any(|c| c.destination == destination && c.destination_bus == destination_bus)
    {
        return paramErr;
    }
    state.connections.push(Connection {
        source,
        _source_bus: source_bus,
        destination,
        destination_bus,
    });
    0
}

fn AUGraphDisconnectNodeInput(
    env: &mut Environment,
    graph: AUGraph,
    destination: AUNode,
    destination_bus: u32,
) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
    else {
        return paramErr;
    };
    if !state.nodes.contains_key(&destination) {
        return paramErr;
    }
    if state.unit.is_some() {
        return paramErr;
    }
    let old_len = state.connections.len();
    state.connections.retain(|connection| {
        connection.destination != destination || connection.destination_bus != destination_bus
    });
    if old_len == state.connections.len() {
        return paramErr;
    }
    0
}

fn AUGraphNodeInfo(
    env: &mut Environment,
    graph: AUGraph,
    node: AUNode,
    out_desc: MutPtr<ComponentDescription>,
    out_unit: MutPtr<AudioUnit>,
) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    let Some(&description) = state.nodes.get(&node) else {
        return paramErr;
    };
    let unit = state.unit.unwrap_or_else(MutPtr::null);
    if !out_desc.is_null() {
        env.mem.write(out_desc, description);
    }
    if !out_unit.is_null() {
        env.mem.write(out_unit, unit);
    }
    0
}

fn AUGraphOpen(env: &mut Environment, graph: AUGraph) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    if state.unit.is_some() {
        return 0;
    }
    // The existing renderer supports one RemoteIO unit, not a connected mixer graph.
    if state.nodes.len() != 1 || !state.connections.is_empty() {
        return paramErr;
    }
    let desc = *state.nodes.values().next().unwrap();
    if desc.component_type != u32::from_be_bytes(*b"auou")
        || desc.component_sub_type != u32::from_be_bytes(*b"rioc")
        || desc.component_manufacturer != u32::from_be_bytes(*b"appl")
    {
        return paramErr;
    }
    let unit = audio_components::create_audio_unit_instance(env);
    env.framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
        .unwrap()
        .unit = Some(unit);
    0
}

fn AUGraphInitialize(env: &mut Environment, graph: AUGraph) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    let Some(unit) = state.unit else {
        return paramErr;
    };
    if state.initialized {
        return 0;
    }
    let result = audio_unit::AudioUnitInitialize(env, unit);
    if result == 0 {
        env.framework_state
            .audio_toolbox
            .au_graph
            .graphs
            .get_mut(&graph)
            .unwrap()
            .initialized = true;
    }
    result
}

fn AUGraphStart(env: &mut Environment, graph: AUGraph) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    let Some(unit) = state.unit else {
        return paramErr;
    };
    if !state.initialized {
        return paramErr;
    }
    if state.running {
        return 0;
    }
    let result = audio_unit::AudioOutputUnitStart(env, unit);
    if result == 0 {
        env.framework_state
            .audio_toolbox
            .au_graph
            .graphs
            .get_mut(&graph)
            .unwrap()
            .running = true;
    }
    result
}

fn AUGraphStop(env: &mut Environment, graph: AUGraph) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    if !state.running {
        return 0;
    }
    let unit = state.unit.unwrap();
    let result = audio_unit::AudioOutputUnitStop(env, unit);
    if result == 0 {
        env.framework_state
            .audio_toolbox
            .au_graph
            .graphs
            .get_mut(&graph)
            .unwrap()
            .running = false;
    }
    result
}

fn AUGraphUninitialize(env: &mut Environment, graph: AUGraph) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    if state.running {
        return paramErr;
    }
    if !state.initialized {
        return 0;
    }
    let unit = state.unit.unwrap();
    let result = audio_unit::AudioUnitUninitialize(env, unit);
    if result == 0 {
        env.framework_state
            .audio_toolbox
            .au_graph
            .graphs
            .get_mut(&graph)
            .unwrap()
            .initialized = false;
    }
    result
}

fn AUGraphClose(env: &mut Environment, graph: AUGraph) -> OSStatus {
    if !env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .contains_key(&graph)
    {
        return paramErr;
    }
    let result = AUGraphStop(env, graph);
    if result != 0 {
        return result;
    }
    let result = AUGraphUninitialize(env, graph);
    if result != 0 {
        return result;
    }
    let unit = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
        .unwrap()
        .unit
        .take();
    if let Some(unit) = unit {
        return audio_components::AudioComponentInstanceDispose(env, unit);
    }
    0
}

fn AUGraphIsOpen(env: &mut Environment, graph: AUGraph, out_value: MutPtr<u8>) -> OSStatus {
    return_if_null!(out_value);
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    env.mem.write(out_value, u8::from(state.unit.is_some()));
    0
}

fn AUGraphIsInitialized(env: &mut Environment, graph: AUGraph, out_value: MutPtr<u8>) -> OSStatus {
    return_if_null!(out_value);
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    env.mem.write(out_value, u8::from(state.initialized));
    0
}

fn AUGraphIsRunning(env: &mut Environment, graph: AUGraph, out_value: MutPtr<u8>) -> OSStatus {
    return_if_null!(out_value);
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    env.mem.write(out_value, u8::from(state.running));
    0
}

fn AUGraphSetNodeInputCallback(
    env: &mut Environment,
    graph: AUGraph,
    node: AUNode,
    bus: u32,
    callback: ConstPtr<audio_components::AURenderCallbackStruct>,
) -> OSStatus {
    return_if_null!(callback);
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    if bus != 0 || !state.nodes.contains_key(&node) {
        return paramErr;
    }
    let Some(unit) = state.unit else {
        return paramErr;
    };
    let callback = env.mem.read(callback);
    audio_components::State::get(&mut env.framework_state)
        .audio_component_instances
        .get_mut(&unit)
        .unwrap()
        .render_callback = Some(callback);
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(NewAUGraph(_)),
    export_c_func!(DisposeAUGraph(_)),
    export_c_func!(AUGraphAddNode(_, _, _)),
    export_c_func!(AUGraphRemoveNode(_, _)),
    export_c_func!(AUGraphCountNodes(_, _)),
    export_c_func!(AUGraphNodeInfo(_, _, _, _)),
    export_c_func!(AUGraphOpen(_)),
    export_c_func!(AUGraphClose(_)),
    export_c_func!(AUGraphInitialize(_)),
    export_c_func!(AUGraphUninitialize(_)),
    export_c_func!(AUGraphStart(_)),
    export_c_func!(AUGraphStop(_)),
    export_c_func!(AUGraphIsOpen(_, _)),
    export_c_func!(AUGraphIsInitialized(_, _)),
    export_c_func!(AUGraphIsRunning(_, _)),
    export_c_func!(AUGraphSetNodeInputCallback(_, _, _, _)),
    export_c_func!(AUGraphGetIndNode(_, _, _)),
    export_c_func!(AUGraphConnectNodeInput(_, _, _, _, _)),
    export_c_func!(AUGraphDisconnectNodeInput(_, _, _)),
];
