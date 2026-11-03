/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Audio Unit graph topology. Audio processing is implemented separately.

use std::collections::{BTreeMap, HashMap};

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::mem::{ConstPtr, MutPtr, SafeRead};

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
    let old_len = state.connections.len();
    state.connections.retain(|connection| {
        connection.destination != destination || connection.destination_bus != destination_bus
    });
    if old_len == state.connections.len() {
        return paramErr;
    }
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(NewAUGraph(_)),
    export_c_func!(DisposeAUGraph(_)),
    export_c_func!(AUGraphAddNode(_, _, _)),
    export_c_func!(AUGraphRemoveNode(_, _)),
    export_c_func!(AUGraphCountNodes(_, _)),
    export_c_func!(AUGraphGetIndNode(_, _, _)),
    export_c_func!(AUGraphConnectNodeInput(_, _, _, _, _)),
    export_c_func!(AUGraphDisconnectNodeInput(_, _, _)),
];
